//! MCP 外部进程插件客户端（设计方案 §13.3 / §6.3）：
//! stdio JSON-RPC（换行分帧）——initialize 握手 → tools/list → tools/call。
//!
//! 分级映射（§13.3：MCP 默认 C/D；§12.2 铁律三不可放宽）：
//! - 默认 **D**（恒审批：外部进程能力面未知，保守处理）；
//! - manifest `permissions` 含 `net:*` 的工具可降为 C（仍恒审批）；
//! - 任何映射都不低于 C——MCP 工具永不自动执行。

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{self, BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Mutex;

#[derive(Debug, thiserror::Error)]
pub enum McpError {
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON 错误: {0}")]
    Json(String),
    #[error("MCP 请求被拒绝: {0}")]
    Denied(String),
    #[error("MCP 调用超时")]
    Timeout,
}

pub type Result<T> = std::result::Result<T, McpError>;

/// MCP 工具描述（tools/list 结果）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpTool {
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// JSON Schema
    #[serde(default, rename = "inputSchema")]
    pub input_schema: Value,
}

/// 工具动作分级（§13.3：默认 D；`net:*` 权限 → C；永不自动执行）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpToolLevel {
    C,
    D,
}

/// MCP 工具分级策略（§13.3：默认 C/D；外部插件权限映射）。
#[derive(Debug, Clone, Default)]
pub struct McpLevelPolicy {
    /// 声明 `net:*` 权限的工具 → C；其余 → D（默认保守）。
    pub net_tools: std::collections::HashSet<String>,
}

impl McpLevelPolicy {
    pub fn level_for(&self, tool: &str) -> McpToolLevel {
        if self.net_tools.contains(tool) {
            McpToolLevel::C
        } else {
            McpToolLevel::D
        }
    }
}

/// MCP 客户端连接（阻塞实现；daemon 侧经 spawn_blocking 调用）。
pub struct McpConnection {
    stdin: Mutex<ChildStdin>,
    stdout: Mutex<BufReader<ChildStdout>>,
    child: Mutex<Child>,
    next_id: AtomicI64,
}

impl McpConnection {
    /// 启动 MCP 服务器子进程。
    pub fn spawn(program: &str, args: &[&str], cwd: &Path) -> io::Result<McpConnection> {
        Self::from_child(
            Command::new(program)
                .args(args)
                .current_dir(cwd)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()?,
        )
    }

    pub fn from_child(mut child: Child) -> io::Result<Self> {
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        Ok(Self {
            stdin: Mutex::new(stdin),
            stdout: Mutex::new(BufReader::new(stdout)),
            child: Mutex::new(child),
            next_id: AtomicI64::new(1),
        })
    }

    fn send(&self, value: &Value) -> Result<()> {
        let mut w = self.stdin.lock().expect("stdin lock");
        writeln!(w, "{value}").map_err(McpError::Io)
    }

    fn read_message(&self) -> Result<Value> {
        let mut r = self.stdout.lock().expect("stdout lock");
        let mut line = String::new();
        let n = r.read_line(&mut line)?;
        if n == 0 {
            return Err(McpError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "mcp server closed",
            )));
        }
        serde_json::from_str(line.trim()).map_err(|e| McpError::Json(e.to_string()))
    }

    fn request(&self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        self.send(&msg)?;
        loop {
            let incoming = self.read_message()?;
            // 跳过通知（无 id）
            if incoming.get("id").is_none() {
                continue;
            }
            if incoming["id"] == serde_json::json!(id) {
                if let Some(err) = incoming.get("error") {
                    return Err(McpError::Denied(err.to_string()));
                }
                return Ok(incoming.get("result").cloned().unwrap_or(Value::Null));
            }
        }
    }

    fn notify(&self, method: &str, params: Value) -> Result<()> {
        self.send(&serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        }))
    }

    /// initialize 握手 + initialized 通知（MCP 规范）。
    pub fn initialize(&self, client_name: &str) -> Result<Value> {
        let result = self.request(
            "initialize",
            serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": client_name, "version": env!("CARGO_PKG_VERSION") },
            }),
        )?;
        self.notify("notifications/initialized", serde_json::json!({}))?;
        Ok(result)
    }

    /// tools/list：可用工具（供映射为分级工具目录）。
    pub fn list_tools(&self) -> Result<Vec<McpTool>> {
        let result = self.request("tools/list", serde_json::json!({}))?;
        let tools = result
            .get("tools")
            .cloned()
            .unwrap_or_else(|| serde_json::json!([]));
        serde_json::from_value(tools).map_err(|e| McpError::Json(e.to_string()))
    }

    /// tools/call：执行外部工具（调用方须已完成 D/C 级审批）。
    pub fn call_tool(&self, name: &str, arguments: Value) -> Result<String> {
        let result = self.request(
            "tools/call",
            serde_json::json!({ "name": name, "arguments": arguments }),
        )?;
        let is_error = result
            .get("isError")
            .and_then(|e| e.as_bool())
            .unwrap_or(false);
        let text: String = result
            .get("content")
            .and_then(|c| c.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        if is_error {
            return Err(McpError::Denied(text));
        }
        Ok(text)
    }

    pub fn shutdown(&self) {
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for McpConnection {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_policy_defaults_to_d_and_net_downgrades_to_c() {
        let policy = McpLevelPolicy {
            net_tools: ["mcp:db/query".to_string()].into_iter().collect(),
        };
        assert_eq!(policy.level_for("mcp:fs/read"), McpToolLevel::D, "默认 D");
        assert_eq!(policy.level_for("mcp:db/query"), McpToolLevel::C);
    }

    #[test]
    fn mcp_tools_serialize() {
        let t: McpTool = serde_json::from_value(serde_json::json!({
            "name": "query",
            "description": "查询数据库",
            "inputSchema": {"type": "object"}
        }))
        .unwrap();
        assert_eq!(t.name, "query");
    }
}
