use std::time::Duration;
use tenon_mcp::{McpConnection, McpLevelPolicy, McpToolLevel};

fn node() -> String {
    which_node()
}

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
    let conn = McpConnection::spawn(
        "node",
        &["-e", &format!("import('{server}')")],
        Path::new("."),
    )
    .or_else(|_| McpConnection::spawn(&node(), &[server], Path::new(".")))
    .expect("spawn fake mcp server");

    let init = conn.initialize("tenon-test").expect("initialize");
    assert!(init.get("serverInfo").is_some(), "{init}");

    let tools = conn.list_tools().expect("tools/list");
    assert_eq!(tools.len(), 2);
    assert!(tools.iter().any(|t| t.name == "query"));

    // net 工具（query）→ C 级；写文件 → 默认 D
    let policy = McpLevelPolicy {
        net_tools: ["query".to_string()].into_iter().collect(),
    };
    assert_eq!(policy.level_for("query"), McpToolLevel::C);
    assert_eq!(policy.level_for("write_file"), McpToolLevel::D);

    // tools/call：query 成功
    let rows = conn
        .call_tool("query", serde_json::json!({"sql": "SELECT 1"}))
        .expect("call");
    assert!(rows.contains("ROWS for SELECT 1"), "{rows}");
    // write_file 服务器返回 isError → Denied
    let err = conn
        .call_tool("write_file", serde_json::json!({"path": "x"}))
        .unwrap_err();
    assert!(err.to_string().contains("denied"));

    conn.shutdown();
    let _ = Duration::from_secs(1);
}

use std::path::Path;
