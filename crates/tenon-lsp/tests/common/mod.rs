//! 假语言服务器（测试用）：走通道传输，实现握手回显与守卫探测行为。

use std::path::Path;
use std::time::Duration;

use tenon_lsp::guard::LspGuardConfig;
use tenon_lsp::transport::{channel_pair, FrameRead, FrameWrite};
use tenon_lsp::{LspHost, LspHostConfig, Notification};

/// 假服务器行为配置。
#[derive(Default, Clone)]
pub struct FakeServerBehavior {
    /// 收到 `test/trigger/executeCommand` 时向客户端发起该请求。
    pub trigger_command: Option<String>,
    /// 收到 `test/trigger/applyEdit` 时发起（params: {"edit": …}）。
    pub trigger_edit: Option<serde_json::Value>,
    /// 收到 `test/trigger/showDocument` 时发起（params: {"uri": …}）。
    pub trigger_show_document: Option<String>,
    /// 收到 `test/trigger/registerCapability` 时发起。
    pub trigger_register: Option<String>,
    /// initialize 后是否推送诊断通知。
    pub publish_diagnostics: bool,
}

/// 假服务器主循环：在独立线程运行，直到读端关闭。
pub fn spawn_fake_server(
    mut reader: Box<dyn FrameRead>,
    mut writer: Box<dyn FrameWrite>,
    behavior: FakeServerBehavior,
) {
    std::thread::spawn(move || {
        fn send(writer: &mut Box<dyn FrameWrite>, v: &serde_json::Value) {
            if let Ok(s) = serde_json::to_string(v) {
                let _ = writer.write_frame(&s);
            }
        }
        let mut server_req_id: i64 = 900_000;
        while let Ok(body) = reader.read_frame() {
            let msg: serde_json::Value = match serde_json::from_str(&body) {
                Ok(m) => m,
                Err(_) => continue,
            };
            let id = msg.get("id").cloned();
            let method = msg
                .get("method")
                .and_then(|m| m.as_str())
                .unwrap_or("")
                .to_string();

            let is_request = id.is_some() && !method.is_empty();
            let is_notification = id.is_none() && !method.is_empty();

            if is_request {
                let response = match method.as_str() {
                    "initialize" => serde_json::json!({
                        "jsonrpc": "2.0", "id": id,
                        "result": {"capabilities": {"textDocumentSync": 1}}
                    }),
                    "shutdown" => serde_json::json!({"jsonrpc": "2.0", "id": id, "result": null}),
                    "textDocument/hover" => serde_json::json!({
                        "jsonrpc": "2.0", "id": id,
                        "result": {"contents": {"kind": "markdown", "value": "fake hover"}}
                    }),
                    // 触发器：先用 null 响应触发请求，再以独立 id 发起服务器请求
                    "test/trigger/executeCommand" => {
                        server_req_id += 1;
                        let rid = server_req_id;
                        let cmd = behavior.trigger_command.clone().unwrap_or_default();
                        let outgoing = serde_json::json!({"jsonrpc": "2.0", "id": rid, "method": "workspace/executeCommand",
                            "params": {"command": cmd, "arguments": []}});
                        send(&mut writer, &outgoing);
                        serde_json::json!({"jsonrpc": "2.0", "id": id, "result": {"triggered": true}})
                    }
                    "test/trigger/applyEdit" => {
                        server_req_id += 1;
                        let rid = server_req_id;
                        let outgoing = serde_json::json!({"jsonrpc": "2.0", "id": rid, "method": "workspace/applyEdit",
                            "params": {"label": "t", "edit": behavior.trigger_edit.clone().unwrap_or(serde_json::json!({}))}});
                        send(&mut writer, &outgoing);
                        serde_json::json!({"jsonrpc": "2.0", "id": id, "result": {"triggered": true}})
                    }
                    "test/trigger/showDocument" => {
                        server_req_id += 1;
                        let rid = server_req_id;
                        let outgoing = serde_json::json!({"jsonrpc": "2.0", "id": rid, "method": "window/showDocument",
                            "params": {"uri": behavior.trigger_show_document.clone().unwrap_or_default()}});
                        send(&mut writer, &outgoing);
                        serde_json::json!({"jsonrpc": "2.0", "id": id, "result": {"triggered": true}})
                    }
                    "test/trigger/registerCapability" => {
                        server_req_id += 1;
                        let rid = server_req_id;
                        let outgoing = serde_json::json!({"jsonrpc": "2.0", "id": rid, "method": "client/registerCapability",
                            "params": {"registrations": [{"id": "r1", "method": behavior.trigger_register.clone().unwrap_or_default()}]}});
                        send(&mut writer, &outgoing);
                        serde_json::json!({"jsonrpc": "2.0", "id": id, "result": {"triggered": true}})
                    }
                    _ => serde_json::json!({"jsonrpc": "2.0", "id": id,
                        "error": {"code": -32601, "message": "not implemented"}}),
                };
                send(&mut writer, &response);
            } else if is_notification {
                match method.as_str() {
                    "initialized" if behavior.publish_diagnostics => {
                        let note = serde_json::json!({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics",
                            "params": {"uri": "file:///a.rs", "diagnostics": []}});
                        send(&mut writer, &note);
                    }
                    _ => {}
                }
            }
            // 服务器发起请求后客户端的响应（id 有值、method 为空）：无需处理
        }
    });
}

/// 建立宿主 + 假服务器。
pub fn setup_host(
    project_root: &Path,
    allowed: &[&str],
    behavior: FakeServerBehavior,
) -> Arc<LspHost> {
    let ((server_reader, server_writer), (host_reader, host_writer)) = channel_pair();
    spawn_fake_server(Box::new(server_reader), Box::new(server_writer), behavior);
    let mut guard_cfg = LspGuardConfig::new(project_root);
    for a in allowed {
        guard_cfg.allowed_commands.insert(a.to_string());
    }
    let cfg = LspHostConfig {
        language: "fake".into(),
        root_path: project_root.to_path_buf(),
        guard: guard_cfg,
        initialization_options: None,
    };
    LspHost::connect(cfg, Box::new(host_reader), Box::new(host_writer))
}

use std::sync::Arc;

/// 等待通知到达（测试辅助）。
pub fn recv_notification(rx: &std::sync::mpsc::Receiver<Notification>) -> Notification {
    rx.recv_timeout(Duration::from_secs(5))
        .expect("notification")
}
