//! Tenon 桌面壳（设计方案 §6.2）：WebView 承载 UI，本身无业务逻辑。
//!
//! daemon（Rust 内核）作为 sidecar 子进程管理：启动 → 读握手行
//! （`{"tenon":1,"port":…,"token":…}`）→ 注入 WebView；壳退出时清理子进程。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::io::BufRead;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;

use tauri::Manager;

struct DaemonChild(Mutex<Option<Child>>);

/// 启动 daemon sidecar 并读取握手行（§6.2 动态端口 + 握手）。
/// 打包态使用 `binaries/tenon-daemon-<target-triple>`；开发态回退 target/debug。
fn spawn_daemon() -> Result<(Child, u16, String), String> {
    let target_triple = std::env::var("TARGET_TRIPLE")
        .unwrap_or_else(|_| format!("{}-apple-darwin", std::env::consts::ARCH));
    let candidates = [
        format!(
            "{}/binaries/tenon-daemon-{target_triple}",
            env!("CARGO_MANIFEST_DIR")
        ),
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../target/debug/tenon-daemon"
        )
        .to_string(),
    ];
    let bin = candidates
        .iter()
        .find(|p| std::path::Path::new(p).exists())
        .ok_or_else(|| "tenon-daemon sidecar 未找到".to_string())?;

    let mut child = Command::new(bin)
        .arg("--project")
        .arg(std::env::var("TENON_PROJECT").unwrap_or_else(|_| ".".into()))
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("daemon 启动失败: {e}"))?;
    let stdout = child.stdout.take().expect("piped stdout");
    let reader = std::io::BufReader::new(stdout);
    for line in reader.lines() {
        let line = line.map_err(|e| e.to_string())?;
        // 握手行是 stdout 中唯一的裸 JSON 行
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line.trim()) {
            if v.get("tenon") == Some(&serde_json::json!(1)) {
                let port = v["port"].as_u64().ok_or("handshake 缺 port")? as u16;
                let token = v["token"].as_str().ok_or("handshake 缺 token")?.to_string();
                return Ok((child, port, token));
            }
        }
    }
    let _ = child.kill();
    Err("daemon 握手失败（未读到握手行）".into())
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .manage(DaemonChild(Mutex::new(None)))
        .setup(|app| {
            match spawn_daemon() {
                Ok((child, port, token)) => {
                    // 子进程归壳管理：退出时清理（§6.2 生命周期规则）
                    let state = app.state::<DaemonChild>();
                    *state.0.lock().unwrap() = Some(child);
                    let project = std::env::var("TENON_PROJECT").unwrap_or_else(|_| ".".into());
                    let init_js = format!(
                        "window.__TENON_HANDSHAKE__={};window.__TENON_PROJECT__={};",
                        serde_json::json!({ "port": port, "token": token }),
                        serde_json::json!(project),
                    );
                    let win = app
                        .get_webview_window("main")
                        .ok_or_else(|| tauri::Error::WindowNotFound)?;
                    win.eval(&init_js)?;
                }
                Err(e) => {
                    // 降级：UI 仍可加载（面板显示连接失败），错误进日志
                    eprintln!("daemon 启动失败: {e}");
                }
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
