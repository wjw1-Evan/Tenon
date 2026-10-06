//! 测试用假 MCP 服务器（stdio JSON-RPC 换行分帧）：
//! initialize → tools/list（echo 工具）→ tools/call（回显 name + arguments）。
//! 仅供 tenon-mcp 集成测试与 daemon E2E 使用，不进发布面。

use serde_json::Value;
use std::io::{BufRead, Write};

fn main() {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let Ok(msg) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        // 通知（无 id）直接忽略。
        let Some(id) = msg.get("id").cloned() else {
            continue;
        };
        let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let result = match method {
            "initialize" => serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "fake-mcp", "version": "0.0.0" }
            }),
            "tools/list" => serde_json::json!({
                "tools": [{
                    "name": "echo",
                    "description": "回显参数",
                    "inputSchema": { "type": "object", "properties": { "text": { "type": "string" } } }
                }]
            }),
            "tools/call" => {
                let name = msg["params"]["name"].as_str().unwrap_or("");
                let args = &msg["params"]["arguments"];
                serde_json::json!({
                    "content": [{ "type": "text", "text": format!("echo:{name}:{args}") }]
                })
            }
            _ => serde_json::json!({}),
        };
        let resp = serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result });
        writeln!(stdout, "{resp}").ok();
        stdout.flush().ok();
    }
}
