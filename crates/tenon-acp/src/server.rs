//! ACP 服务端（design-v2.md §8② / design.md §13.3 v2.26 首期）：Tenon harness
//! 作为 ACP agent 外供 Zed / JetBrains 等编辑器——agent 侧协议循环。
//!
//! `AcpServer::run(backend)` 从 stdin 读 JSON-RPC（LSP 帧复用 codec）：
//! initialize（protocolVersion 1）→ session/new（cwd）→ session/prompt（backend
//! 经 ChunkSink 回调流式外发 agent_message_chunk，收尾回 stopReason）→
//! session/cancel。`AcpBackend` trait 隔离实现（测试 MockBackend / 生产
//! DaemonBackend——见 `tenon-acp-agent` 二进制，经 HTTP 连 Tenon daemon）。

use std::io::{BufRead, Write};

/// prompt 期间流式外发文本块的回调（server 组 update 通知并写 stdout）。
pub type ChunkSink<'a> = &'a mut dyn FnMut(&str);

/// ACP agent 后端：会话与执行语义的实现（协议循环不关心）。
pub trait AcpBackend {
    /// agent 能力声明（initialize 响应的 agentCapabilities）。
    fn capabilities(&self) -> serde_json::Value {
        serde_json::json!({"loadSession": false})
    }
    /// 建会话（cwd 作用域）；返回 sessionId。
    fn new_session(&mut self, cwd: &str) -> Result<String, String>;
    /// 执行一条 prompt：文本经 chunk 回调流式外发；返回 stopReason。
    fn prompt(&mut self, session_id: &str, text: &str, chunk: ChunkSink) -> Result<String, String>;
}

/// 协议错误码（JSON-RPC）。
fn rpc_error(id: Option<u64>, code: i64, message: &str) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {"code": code, "message": message}
    })
}

fn rpc_ok(id: u64, result: serde_json::Value) -> serde_json::Value {
    serde_json::json!({"jsonrpc": "2.0", "id": id, "result": result})
}

/// ACP agent 侧协议循环。EOF / 未知方法（-32601）/ 后端错误（-32000）显式。
pub struct AcpServer;

impl AcpServer {
    /// 跑协议循环直到 stdin EOF；写响应 / 通知到 `out`。
    pub fn run<R: BufRead, W: Write, B: AcpBackend>(
        mut input: R,
        mut out: W,
        backend: &mut B,
    ) -> Result<(), String> {
        loop {
            let frame = match tenon_lsp::codec::read_message(&mut input) {
                Ok(f) => f,
                Err(_) => return Ok(()), // EOF：编辑器关闭连接
            };
            let msg: serde_json::Value =
                serde_json::from_str(&frame).map_err(|e| format!("非法 JSON: {e}"))?;
            let id = msg.get("id").and_then(|v| v.as_u64());
            let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
            match method {
                "initialize" => {
                    let id: u64 = id.ok_or("initialize 缺 id")?;
                    let resp = rpc_ok(
                        id,
                        serde_json::json!({
                            "protocolVersion": crate::PROTOCOL_VERSION,
                            "agentCapabilities": backend.capabilities(),
                        }),
                    );
                    write_frame(&mut out, &resp)?;
                }
                "session/new" => {
                    let id = id.ok_or("session/new 缺 id")?;
                    let cwd = msg
                        .pointer("/params/cwd")
                        .and_then(|c| c.as_str())
                        .unwrap_or(".");
                    match backend.new_session(cwd) {
                        Ok(sid) => {
                            write_frame(
                                &mut out,
                                &rpc_ok(id, serde_json::json!({"sessionId": sid})),
                            )?;
                        }
                        Err(e) => write_frame(&mut out, &rpc_error(Some(id), -32000, &e))?,
                    }
                }
                "session/prompt" => {
                    let id = id.ok_or("session/prompt 缺 id")?;
                    let sid = msg
                        .pointer("/params/sessionId")
                        .and_then(|s| s.as_str())
                        .unwrap_or("");
                    let text = msg
                        .pointer("/params/prompt/0/text")
                        .and_then(|t| t.as_str())
                        .unwrap_or("");
                    let out_ref = &mut out;
                    let sid_owned = sid.to_string();
                    let mut sink = |chunk: &str| {
                        let update = serde_json::json!({
                            "jsonrpc": "2.0",
                            "method": "session/update",
                            "params": {
                                "sessionId": sid_owned,
                                "update": {
                                    "type": "agent_message_chunk",
                                    "content": {"type": "text", "text": chunk}
                                }
                            }
                        });
                        // 单条通知写失败：放弃该 chunk（连接已断由外层 EOF 收口）
                        let _ = write_frame(out_ref, &update);
                    };
                    match backend.prompt(sid, text, &mut sink) {
                        Ok(stop_reason) => {
                            write_frame(
                                &mut out,
                                &rpc_ok(id, serde_json::json!({"stopReason": stop_reason})),
                            )?;
                        }
                        Err(e) => write_frame(&mut out, &rpc_error(Some(id), -32000, &e))?,
                    }
                }
                "session/cancel" => {
                    if let Some(id) = id {
                        write_frame(&mut out, &rpc_ok(id, serde_json::json!({})))?;
                    }
                }
                other => {
                    if let Some(id) = id {
                        write_frame(
                            &mut out,
                            &rpc_error(Some(id), -32601, &format!("未知方法: {other}")),
                        )?;
                    }
                    // 通知形态的未知方法：丢弃（LSP/ACP agent 惯例）
                }
            }
        }
    }
}

fn write_frame<W: Write>(out: &mut W, value: &serde_json::Value) -> Result<(), String> {
    let body = value.to_string();
    out.write_all(&tenon_lsp::codec::encode(&body))
        .map_err(|e| format!("写出失败: {e}"))?;
    out.flush().map_err(|e| format!("flush 失败: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    struct MockBackend {
        sessions: usize,
    }

    impl AcpBackend for MockBackend {
        fn new_session(&mut self, _cwd: &str) -> Result<String, String> {
            self.sessions += 1;
            Ok(format!("srv-{}", self.sessions))
        }
        fn prompt(
            &mut self,
            session_id: &str,
            _text: &str,
            chunk: ChunkSink,
        ) -> Result<String, String> {
            chunk("部分一 ");
            chunk("部分二");
            Ok(format!("end_turn:{session_id}"))
        }
    }

    fn frame(body: &str) -> String {
        format!("Content-Length: {}\r\n\r\n{}", body.len(), body)
    }

    /// v2.26：协议循环全链——initialize → session/new → prompt（双 chunk 通知 +
    /// stopReason 响应）→ 未知方法 -32601。
    #[test]
    fn server_protocol_loop_full_chain() {
        let input = Cursor::new(
            frame(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#)
                + &frame(
                    r#"{"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":"/tmp"}}"#,
                )
                + &frame(
                    r#"{"jsonrpc":"2.0","id":3,"method":"session/prompt","params":{"sessionId":"srv-1","prompt":[{"type":"text","text":"干活"}]}}"#,
                )
                + &frame(r#"{"jsonrpc":"2.0","id":4,"method":"mystery"}"#),
        );
        let mut out = Vec::new();
        let mut backend = MockBackend { sessions: 0 };
        AcpServer::run(input, &mut out, &mut backend).unwrap();
        let text = String::from_utf8(out).unwrap();
        // initialize：protocolVersion 1
        assert!(text.contains(r#""protocolVersion":1"#), "{text}");
        // session/new
        assert!(text.contains(r#""sessionId":"srv-1""#), "{text}");
        // prompt：两条 chunk 通知 + stopReason 响应
        assert_eq!(text.matches("agent_message_chunk").count(), 2, "{text}");
        assert!(text.contains("部分一 "), "{text}");
        assert!(text.contains(r#""stopReason":"end_turn:srv-1""#), "{text}");
        // 未知方法
        assert!(text.contains("-32601"), "{text}");
    }
}
