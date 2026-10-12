//! ACP 客户端（design-v2.md §8① P4 先行 / design.md §13.3 v2.23 首期）：
//! 外部 agent（Claude Code / Gemini CLI 的 ACP 适配等）经 ACP 协议接入——
//! JSON-RPC 2.0 over stdio、LSP 式 `Content-Length` 帧（复用 tenon-lsp codec）。
//!
//! 首期范围（纯协议层，不触碰执行面）：`spawn` → `initialize` 握手 →
//! `session/new` → `session/prompt`（流式收 `session/update` 通知，文本块
//! 拼接为收尾回答）→ `session/stop` / 进程回收；超时 / 进程退出 / 协议错误
//! 显式失败。外部 agent 作为子代理提供方与审查面全链列后续单元——接线时
//! 工具执行必经 Tenon 策略总线与沙箱（design-v2 §11 安全不变式）。

pub mod server;

pub use server::{AcpBackend, AcpServer};

use std::io::{BufReader, Write};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::time::Duration;

/// ACP 协议版本（agentclientprotocol.com 规范 v1）。
pub const PROTOCOL_VERSION: u64 = 1;

/// ACP 客户端错误——全部显式（不静默半结果）。
#[derive(Debug, thiserror::Error)]
pub enum AcpError {
    #[error("IO: {0}")]
    Io(#[from] std::io::Error),
    #[error("帧解码: {0}")]
    Codec(String),
    #[error("agent 进程退出（code={0:?}）")]
    AgentExited(Option<i32>),
    #[error("JSON-RPC 错误响应 ({code}): {message}")]
    Rpc { code: i64, message: String },
    #[error("协议违规: {0}")]
    Protocol(String),
    #[error("超时（{0:?}）")]
    Timeout(Duration),
}

/// 一次 prompt 的运行结果。
#[derive(Debug, Clone, serde::Serialize)]
pub struct AcpRunResult {
    /// session/update 中 agent_message_chunk 文本块的拼接（收尾回答）。
    pub answer: String,
    /// 收到的 update 载荷种类计数（审计面：tool_call / plan / 等各多少）。
    pub updates: serde_json::Value,
    /// stopReason（prompt 响应载荷；缺省空串）。
    pub stop_reason: String,
}

/// 拉起的外部 ACP agent 进程连接。
pub struct AcpAgent {
    child: Child,
    /// 持久 BufReader——每次 recv 新建会丢内部缓冲的多读帧（实测挂死）。
    reader: Option<BufReader<ChildStdout>>,
    next_id: u64,
}

impl AcpAgent {
    /// 拉起外部 agent 进程（argv 直启无 shell 面——与 MCP 启动器同约束）。
    pub fn spawn(command: &str, args: &[String]) -> Result<Self, AcpError> {
        let mut cmd = Command::new(command);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let child = cmd.spawn()?;
        Self::attach(child)
    }

    /// 接管已启动的 piped 子进程（v2.24 子代理轨：进程可先经
    /// tenon-sandbox external 沙箱包装，再交由本连接跑 ACP 协议）。
    pub fn attach(mut child: Child) -> Result<Self, AcpError> {
        let reader = child
            .stdout
            .take()
            .map(BufReader::new)
            .ok_or_else(|| AcpError::Protocol("stdout 不可用".into()))?;
        Ok(Self {
            child,
            reader: Some(reader),
            next_id: 1,
        })
    }

    fn alloc_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    fn send(&mut self, body: &serde_json::Value) -> Result<(), AcpError> {
        let stdin = self
            .child
            .stdin
            .as_mut()
            .ok_or_else(|| AcpError::Protocol("stdin 不可用".into()))?;
        stdin.write_all(&tenon_lsp::codec::encode(&body.to_string()))?;
        stdin.flush()?;
        Ok(())
    }

    /// 读一帧（阻塞）；EOF → AgentExited。
    fn recv(&mut self) -> Result<serde_json::Value, AcpError> {
        let reader = self
            .reader
            .as_mut()
            .ok_or_else(|| AcpError::Protocol("stdout 已回收".into()))?;
        let frame = tenon_lsp::codec::read_message(reader).map_err(|e| {
            if matches!(e, tenon_lsp::codec::CodecError::Io(ref io) if io.kind() == std::io::ErrorKind::UnexpectedEof)
            {
                AcpError::AgentExited(self.child.wait().ok().and_then(|s| s.code()))
            } else {
                AcpError::Codec(e.to_string())
            }
        })?;
        serde_json::from_str(&frame).map_err(|e| AcpError::Codec(e.to_string()))
    }

    /// 收一帧并校验为对应 id 的响应（中途的 notification 按调用方语义处理——
    /// prompt 流式下 notification 是主体，由 `run_prompt` 内循环消费）。
    fn recv_response(&mut self, id: u64) -> Result<serde_json::Value, AcpError> {
        loop {
            let msg = self.recv()?;
            if let Some(result) = msg.get("result") {
                if msg.get("id").and_then(|v| v.as_u64()) == Some(id) {
                    return Ok(result.clone());
                }
            }
            if let Some(err) = msg.get("error") {
                if msg.get("id").and_then(|v| v.as_u64()) == Some(id) {
                    return Err(AcpError::Rpc {
                        code: err.get("code").and_then(|c| c.as_i64()).unwrap_or(-1),
                        message: err
                            .get("message")
                            .and_then(|m| m.as_str())
                            .unwrap_or_default()
                            .into(),
                    });
                }
            }
            // 其他 id 的响应 / 服务器请求：首期协议层不预期——跳过但防御
        }
    }

    /// `initialize` 握手：protocolVersion 1 + 客户端能力（静态声明——method
    /// 调用暂不支持，如实声明能力避免 agent 走不通路径）。
    pub fn initialize(&mut self) -> Result<serde_json::Value, AcpError> {
        let id = self.alloc_id();
        self.send(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "initialize",
            "params": {
                "protocolVersion": PROTOCOL_VERSION,
                "clientCapabilities": {
                    "fs": { "readTextFile": false, "writeTextFile": false },
                    "terminal": false
                }
            }
        }))?;
        self.recv_response(id)
    }

    /// `session/new`：cwd 作用域；返回 sessionId。
    pub fn new_session(&mut self, cwd: &std::path::Path) -> Result<String, AcpError> {
        let id = self.alloc_id();
        self.send(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "session/new",
            "params": { "cwd": cwd }
        }))?;
        let result = self.recv_response(id)?;
        result
            .get("sessionId")
            .and_then(|s| s.as_str())
            .map(String::from)
            .ok_or_else(|| AcpError::Protocol("session/new 未返回 sessionId".into()))
    }

    /// `session/prompt`：发送任务文本，流式消费 `session/update` 通知直到
    /// prompt 响应（stopReason）到达。update 载荷类型化解析：
    /// agent_message_chunk（文本拼接）/ tool_call / plan（计数入审计面）。
    pub fn prompt(&mut self, session_id: &str, text: &str) -> Result<AcpRunResult, AcpError> {
        let id = self.alloc_id();
        self.send(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "session/prompt",
            "params": { "sessionId": session_id, "prompt": [{ "type": "text", "text": text }] }
        }))?;
        let mut answer = String::new();
        let mut counts = std::collections::BTreeMap::<String, u64>::new();
        loop {
            let msg = self.recv()?;
            let is_target_response = msg.get("id").and_then(|v| v.as_u64()) == Some(id);
            if let Some(err) = msg.get("error") {
                if is_target_response {
                    return Err(AcpError::Rpc {
                        code: err.get("code").and_then(|c| c.as_i64()).unwrap_or(-1),
                        message: err
                            .get("message")
                            .and_then(|m| m.as_str())
                            .unwrap_or_default()
                            .into(),
                    });
                }
            }
            if is_target_response {
                if let Some(result) = msg.get("result") {
                    let stop_reason = result
                        .get("stopReason")
                        .and_then(|s| s.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let updates = serde_json::to_value(&counts).unwrap_or_default();
                    return Ok(AcpRunResult {
                        answer,
                        updates,
                        stop_reason,
                    });
                }
            }
            // notification（method 存在、无 id）：session/update 主体
            if msg.get("method").and_then(|m| m.as_str()) == Some("session/update") {
                let update = msg.get("params").cloned().unwrap_or_default();
                let session_id_upd = update.get("sessionId").and_then(|s| s.as_str());
                let update_type = update
                    .get("update")
                    .and_then(|u| u.get("type"))
                    .and_then(|t| t.as_str())
                    .unwrap_or("unknown");
                *counts.entry(update_type.to_string()).or_insert(0) += 1;
                // 仅本会话的 update 计入回答（防御其他会话串流）
                if session_id_upd == Some(session_id) && update_type == "agent_message_chunk" {
                    if let Some(content) = update
                        .get("update")
                        .and_then(|u| u.get("content"))
                        .and_then(|c| c.get("text"))
                        .and_then(|t| t.as_str())
                    {
                        answer.push_str(content);
                    }
                }
            }
        }
    }

    /// `session/cancel`：请求取消当前 prompt（尽力——agent 可能已完成）。
    pub fn cancel(&mut self, session_id: &str, reason: &str) -> Result<(), AcpError> {
        let id = self.alloc_id();
        self.send(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "session/cancel",
            "params": { "sessionId": session_id, "reason": reason }
        }))?;
        let _ = self.recv_response(id);
        Ok(())
    }

    /// 进程回收（kill + wait；幂等）。
    pub fn shutdown(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for AcpAgent {
    fn drop(&mut self) {
        self.shutdown();
    }
}
