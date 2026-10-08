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

/// v1.187 §13.3 按 id 分发：慢响应与快响应并发互不吞；服务器发起的请求必答
/// METHOD_NOT_FOUND；resources / prompts 往返。
#[test]
fn per_id_dispatch_resources_and_prompts() {
    let server_js = r#"
import * as readline from 'node:readline';
const rl = readline.createInterface({ input: process.stdin });
const reply = (id, result) => process.stdout.write(JSON.stringify({jsonrpc:'2.0',id,result}) + '\n');
rl.on('line', (line) => {
  let msg; try { msg = JSON.parse(line); } catch { return; }
  if (msg.method === 'initialize') { reply(msg.id, {capabilities:{}}); return; }
  if (msg.method === 'notifications/initialized') return;
  switch (msg.method) {
    case 'tools/call': {
      if (msg.params.name === 'slow') {
        setTimeout(() => reply(msg.id, {content:[{type:'text',text:'SLOW-DONE'}]}), 400);
      } else if (msg.params.name === 'fast') {
        reply(msg.id, {content:[{type:'text',text:'FAST-DONE'}]});
      } else if (msg.params.name === 'probe_server_request') {
        // 主动发一个服务器→客户端请求，读取客户端应答后把错误码带回
        process.stdout.write(JSON.stringify({jsonrpc:'2.0',id:'srv-1',method:'sampling/createMessage',params:{}}) + '\n');
        setTimeout(() => {}, 0);
        let buf = '';
        const onLine = (l) => {
          buf += l;
          try {
            const r = JSON.parse(buf);
            if (r.id === 'srv-1') {
              rl.removeListener('line', onLine);
              reply(msg.id, {content:[{type:'text',text:'SRV-REQ-CODE:' + (r.error ? r.error.code : 'no-error')}]});
            }
          } catch { buf = buf; }
        };
        // 监听客户端对 srv-1 的应答（同一 stdin 流）
        rl.on('line', onLine);
      } else {
        reply(msg.id, {content:[{type:'text',text:'UNKNOWN'}]});
      }
      break;
    }
    case 'resources/list':
      reply(msg.id, {resources:[{uri:'file:///x.txt', name:'X 文档', mimeType:'text/plain'}]});
      break;
    case 'resources/read':
      reply(msg.id, {contents:[{uri:msg.params.uri, text:'RESOURCE-BODY'}]});
      break;
    case 'prompts/list':
      reply(msg.id, {prompts:[{name:'review', description:'代码审查提示'}]});
      break;
    case 'prompts/get':
      reply(msg.id, {messages:[{role:'user', content:{type:'text', text:'PROMPT-BODY'}}]});
      break;
    default:
      if (msg.id !== undefined) reply(msg.id, {content:[]});
  }
});
"#;
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("fake_meta.mjs");
    std::fs::write(&script, server_js).unwrap();
    let conn = std::sync::Arc::new(
        McpConnection::spawn(&which_node(), &[script.to_str().unwrap()], Path::new("."))
            .expect("spawn"),
    );
    conn.initialize("tenon-test").expect("initialize");

    // 并发：慢请求不吞快请求（串行化旧实现下 fast 会等 slow 400ms+）
    let (slow_tx, slow_rx) = std::sync::mpsc::channel();
    let (fast_tx, fast_rx) = std::sync::mpsc::channel();
    let c1 = conn.clone();
    let c2 = conn.clone();
    std::thread::spawn(move || {
        let _ = slow_tx.send(c1.call_tool("slow", serde_json::json!({})));
    });
    std::thread::spawn(move || {
        let _ = fast_tx.send(c2.call_tool("fast", serde_json::json!({})));
    });
    let fast = fast_rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .unwrap()
        .unwrap();
    assert!(fast.contains("FAST-DONE"), "{fast}");
    let slow = slow_rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .unwrap()
        .unwrap();
    assert!(slow.contains("SLOW-DONE"), "{slow}");

    // 服务器发起的请求 → 客户端必答 METHOD_NOT_FOUND（-32601）
    let probe = conn
        .call_tool("probe_server_request", serde_json::json!({}))
        .expect("probe");
    assert!(probe.contains("SRV-REQ-CODE:-32601"), "{probe}");

    // resources / prompts 往返
    let resources = conn.list_resources().expect("resources/list");
    assert_eq!(resources["resources"][0]["uri"], "file:///x.txt");
    let body = conn.read_resource("file:///x.txt").expect("resources/read");
    assert_eq!(body, "RESOURCE-BODY");
    let prompts = conn.list_prompts().expect("prompts/list");
    assert_eq!(prompts["prompts"][0]["name"], "review");
    let rendered = conn
        .get_prompt("review", serde_json::json!({}))
        .expect("prompts/get");
    assert_eq!(rendered, "PROMPT-BODY");

    conn.shutdown();
}
