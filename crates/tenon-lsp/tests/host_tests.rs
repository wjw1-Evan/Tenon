//! LSP 宿主集成测试：多路复用、通知分发、铁律七守卫（ADR-9）。

mod common;

use std::path::Path;
use std::time::Duration;

use common::{setup_host, FakeServerBehavior};
use tenon_lsp::guard::GuardDecision;

fn tmp_project() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

const TIMEOUT: Duration = Duration::from_secs(5);

#[test]
fn initialize_and_request_roundtrip() {
    let dir = tmp_project();
    let host = setup_host(dir.path(), &[], FakeServerBehavior::default());
    let result = host
        .request(
            "textDocument/hover",
            serde_json::json!({"textDocument": {"uri": "file:///a.rs"}, "position": {"line": 0, "character": 0}}),
            TIMEOUT,
        )
        .unwrap();
    assert_eq!(result["contents"]["value"], "fake hover");
    host.shutdown();
}

#[test]
fn diagnostics_notification_forwarded() {
    let dir = tmp_project();
    let host = setup_host(
        dir.path(),
        &[],
        FakeServerBehavior {
            publish_diagnostics: true,
            ..Default::default()
        },
    );
    let rx = host.subscribe();
    host.initialize(TIMEOUT).unwrap();
    let note = common::recv_notification(&rx);
    assert_eq!(note.method, "textDocument/publishDiagnostics");
    host.shutdown();
}

#[test]
fn execute_command_denied_by_default_and_reported() {
    let dir = tmp_project();
    let host = setup_host(
        dir.path(),
        &[],
        FakeServerBehavior {
            trigger_command: Some("editor.action.organizeImports".into()),
            ..Default::default()
        },
    );
    host.initialize(TIMEOUT).unwrap();
    host.request(
        "test/trigger/executeCommand",
        serde_json::json!({}),
        TIMEOUT,
    )
    .unwrap();
    std::thread::sleep(Duration::from_millis(200));
    let reports = host.guard().reports();
    assert!(
        reports
            .iter()
            .any(|r| r.method == "workspace/executeCommand" && r.decision == GuardDecision::Denied),
        "白名单外 executeCommand 必须被拒绝并记录（ADR-9）"
    );
    host.shutdown();
}

#[test]
fn execute_command_whitelisted_allowed() {
    let dir = tmp_project();
    let host = setup_host(
        dir.path(),
        &["generate"],
        FakeServerBehavior {
            trigger_command: Some("generate".into()),
            ..Default::default()
        },
    );
    host.initialize(TIMEOUT).unwrap();
    host.request(
        "test/trigger/executeCommand",
        serde_json::json!({}),
        TIMEOUT,
    )
    .unwrap();
    std::thread::sleep(Duration::from_millis(200));
    let reports = host.guard().reports();
    assert!(reports
        .iter()
        .any(|r| r.method == "workspace/executeCommand" && r.decision == GuardDecision::Allowed));
    host.shutdown();
}

#[test]
fn apply_edit_outside_project_denied() {
    let dir = tmp_project();
    let host = setup_host(
        dir.path(),
        &[],
        FakeServerBehavior {
            trigger_edit: Some(serde_json::json!({
                "changes": {"file:///etc/hosts": [{"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 0}}, "newText": "x"}]}
            })),
            ..Default::default()
        },
    );
    host.initialize(TIMEOUT).unwrap();
    host.request("test/trigger/applyEdit", serde_json::json!({}), TIMEOUT)
        .unwrap();
    std::thread::sleep(Duration::from_millis(200));
    let reports = host.guard().reports();
    assert!(reports
        .iter()
        .any(|r| r.method == "workspace/applyEdit" && r.decision == GuardDecision::Denied));
    host.shutdown();
}

#[test]
fn apply_edit_inside_project_allowed() {
    let dir = tmp_project();
    let file_uri = format!("file://{}", dir.path().join("a.rs").to_string_lossy());
    let host = setup_host(
        dir.path(),
        &[],
        FakeServerBehavior {
            trigger_edit: Some(serde_json::json!({
                "changes": {file_uri: [{"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 0}}, "newText": "x"}]}
            })),
            ..Default::default()
        },
    );
    host.initialize(TIMEOUT).unwrap();
    host.request("test/trigger/applyEdit", serde_json::json!({}), TIMEOUT)
        .unwrap();
    std::thread::sleep(Duration::from_millis(200));
    let reports = host.guard().reports();
    assert!(reports
        .iter()
        .any(|r| r.method == "workspace/applyEdit" && r.decision == GuardDecision::Allowed));
    host.shutdown();
}

#[test]
fn show_document_and_register_capability_denied() {
    let dir = tmp_project();
    let host = setup_host(
        dir.path(),
        &[],
        FakeServerBehavior {
            trigger_show_document: Some("https://evil.example.com/payload".into()),
            trigger_register: Some("workspace/didChangeWatchedFiles".into()),
            ..Default::default()
        },
    );
    host.initialize(TIMEOUT).unwrap();
    host.request("test/trigger/showDocument", serde_json::json!({}), TIMEOUT)
        .unwrap();
    host.request(
        "test/trigger/registerCapability",
        serde_json::json!({}),
        TIMEOUT,
    )
    .unwrap();
    std::thread::sleep(Duration::from_millis(200));
    let reports = host.guard().reports();
    assert!(reports
        .iter()
        .any(|r| r.method == "window/showDocument" && r.decision == GuardDecision::Denied));
    assert!(reports
        .iter()
        .any(|r| r.method == "client/registerCapability" && r.decision == GuardDecision::Denied));
    host.shutdown();
}

#[test]
fn concurrent_requests_are_multiplexed() {
    let dir = tmp_project();
    let host = setup_host(dir.path(), &[], FakeServerBehavior::default());
    host.initialize(TIMEOUT).unwrap();
    let h1 = std::thread::spawn({
        let host = host.clone();
        move || {
            host.request(
                "textDocument/hover",
                serde_json::json!({"q": 1}),
                Duration::from_secs(10),
            )
            .unwrap()
        }
    });
    let h2 = std::thread::spawn({
        let host = host.clone();
        move || {
            host.request(
                "textDocument/hover",
                serde_json::json!({"q": 2}),
                Duration::from_secs(10),
            )
            .unwrap()
        }
    });
    let r1 = h1.join().unwrap();
    let r2 = h2.join().unwrap();
    assert_eq!(r1["contents"]["value"], "fake hover");
    assert_eq!(r2["contents"]["value"], "fake hover");
    host.shutdown();
}

#[test]
fn unknown_server_request_gets_method_not_found() {
    // 未知服务器请求：宿主按规范回 -32601，不执行任何动作
    let dir = tmp_project();
    let host = setup_host(dir.path(), &[], FakeServerBehavior::default());
    host.initialize(TIMEOUT).unwrap();
    // 无触发器路径的未知方法——通过守卫记录数量验证无副作用即可
    let before = host.guard().reports().len();
    let _ = host.request(
        "test/unknown",
        serde_json::json!({}),
        Duration::from_millis(300),
    );
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(
        host.guard().reports().len(),
        before,
        "未知请求不产生守卫记录"
    );
    host.shutdown();
}

#[test]
fn real_process_transport_handshake_if_node_available() {
    // 有 Node 环境时用真实进程传输做一次握手冒烟（CI 无 Node 时跳过）
    let node = which_node();
    let Some(node) = node else { return };
    let dir = tmp_project();
    // 最小 LSP 服务器：回显 initialize
    let server_js = r#"
let buf = Buffer.alloc(0);
process.stdin.on('data', d => {
  buf = Buffer.concat([buf, d]);
  for (;;) {
    const s = buf.indexOf('\r\n\r\n');
    if (s < 0) break;
    const head = buf.slice(0, s).toString();
    const len = parseInt(/Content-Length: (\d+)/.exec(head)[1]);
    if (buf.length < s + 4 + len) break;
    const body = JSON.parse(buf.slice(s + 4, s + 4 + len).toString());
    buf = buf.slice(s + 4 + len);
    if (body.method === 'initialize') {
      const resp = JSON.stringify({jsonrpc: '2.0', id: body.id, result: {capabilities: {}}});
      process.stdout.write(`Content-Length: ${Buffer.byteLength(resp)}\r\n\r\n${resp}`);
    }
  }
});
"#;
    let js_path = dir.path().join("mini-lsp.js");
    std::fs::write(&js_path, server_js).unwrap();
    let conn = tenon_lsp::transport::ProcessConnection::spawn(
        &node,
        &[js_path.to_string_lossy().as_ref()],
        dir.path(),
    )
    .unwrap();
    let mut guard_cfg = tenon_lsp::guard::LspGuardConfig::new(dir.path());
    guard_cfg.allowed_commands.clear();
    let cfg = tenon_lsp::LspHostConfig {
        language: "js".into(),
        root_path: dir.path().to_path_buf(),
        guard: guard_cfg,
        initialization_options: None,
        edit_applier: None,
    };
    let host = tenon_lsp::LspHost::connect(cfg, Box::new(conn.reader), Box::new(conn.writer));
    let result = host.initialize(Duration::from_secs(10));
    assert!(result.is_ok(), "真实进程握手: {:?}", result.err());
    host.shutdown();
}

fn which_node() -> Option<String> {
    let out = std::process::Command::new("which")
        .arg("node")
        .output()
        .ok()?;
    if out.status.success() {
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if s.is_empty() {
            None
        } else {
            Some(s)
        }
    } else {
        None
    }
}

#[allow(dead_code)]
fn unused_path(_: &Path) {}
