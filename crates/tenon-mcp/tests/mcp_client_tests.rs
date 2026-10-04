//! MCP 客户端集成测试：完整流程 + 错误路径。

use std::path::Path;

use tenon_mcp::{McpConnection, McpLevelPolicy, McpToolLevel};

fn which_node() -> String {
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|d| d.join("node"))
        .find(|p| p.is_file())
        .map(|p| p.to_string_lossy().into_owned())
        .expect("node 应存在")
}

#[test]
fn full_client_flow_with_fake_server() {
    let server = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fake_server.mjs");
    let conn = McpConnection::spawn(&which_node(), &[server], Path::new(".")).expect("spawn");

    let init = conn.initialize("tenon-test").expect("initialize");
    assert!(init.get("serverInfo").is_some(), "{init}");

    let tools = conn.list_tools().expect("tools/list");
    assert_eq!(tools.len(), 2);
    assert!(tools.iter().any(|t| t.name == "query"));

    let policy = McpLevelPolicy {
        net_tools: ["query".to_string()].into_iter().collect(),
    };
    assert_eq!(policy.level_for("query"), McpToolLevel::C);
    assert_eq!(policy.level_for("write_file"), McpToolLevel::D);

    let rows = conn
        .call_tool("query", serde_json::json!({"sql": "SELECT 1"}))
        .expect("call");
    assert!(rows.contains("ROWS for SELECT 1"), "{rows}");
    let err = conn
        .call_tool("write_file", serde_json::json!({"path": "x"}))
        .unwrap_err();
    assert!(err.to_string().contains("denied"));

    conn.shutdown();
}

#[test]
fn error_response_maps_to_denied() {
    let dir = tempfile::tempdir().unwrap();
    let server_js = r#"
import * as readline from 'node:readline';
const rl = readline.createInterface({ input: process.stdin });
rl.on('line', (line) => {
  try {
    const msg = JSON.parse(line);
    if (msg.id !== undefined) {
      if (msg.method === 'initialize') {
        process.stdout.write(JSON.stringify({jsonrpc:'2.0',id:msg.id,result:{capabilities:{}}}) + '\n');
      } else {
        process.stdout.write(JSON.stringify({jsonrpc:'2.0',id:msg.id,error:{code:-32603,message:'internal error'}}) + '\n');
      }
    }
  } catch(e) {}
});
"#;
    let script = dir.path().join("err_server.mjs");
    std::fs::write(&script, server_js).unwrap();

    let conn = McpConnection::spawn(
        &which_node(),
        &[script.to_string_lossy().as_ref()],
        dir.path(),
    )
    .unwrap();
    conn.initialize("tenon-test").unwrap();
    let result = conn.list_tools();
    assert!(result.is_err(), "error 响应应映射为 McpError::Denied");
    conn.shutdown();
}

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
    let t: tenon_mcp::McpTool = serde_json::from_value(serde_json::json!({
        "name": "query",
        "description": "查询数据库",
        "inputSchema": {"type": "object"}
    }))
    .unwrap();
    assert_eq!(t.name, "query");
}
