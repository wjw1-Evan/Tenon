//! Tenon 桌面壳（设计方案 §6.2）：WebView 承载 UI，本身无业务逻辑。
//!
//! daemon（Rust 内核）作为 sidecar 子进程管理：启动 → 读握手行 →
//! 将 WebView 导航至 daemon 同源托管的 UI（与浏览器访问同一条链路，
//! 握手经 URL 参数传递，无注入竞态）。壳退出时清理子进程。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::io::BufRead;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;

use serde::Serialize;
use tauri::{Emitter, Manager, Url};

#[derive(Debug, Clone, Serialize)]
struct Handshake {
    port: u16,
    token: String,
    project: String,
}

struct AppState {
    handshake: Mutex<Option<Handshake>>,
    _child: Mutex<Option<Child>>,
}

/// daemon 托管 UI 的产物目录：env 指定 > 仓库根 ui/dist（开发态）。
fn ui_dist_dir() -> Option<std::path::PathBuf> {
    if let Ok(p) = std::env::var("TENON_UI_DIST") {
        let p = std::path::PathBuf::from(p);
        if p.join("index.html").exists() {
            return Some(p);
        }
    }
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ui/dist");
    p.join("index.html").exists().then_some(p)
}

/// 配置发现：TENON_CONFIG 显式指定 > 项目内 config.local.toml（开发期，不入库）
/// > `~/.tenon/config.local.toml`。
fn discover_config(project: &str) -> Option<std::path::PathBuf> {
    if let Ok(p) = std::env::var("TENON_CONFIG") {
        let p = std::path::PathBuf::from(p);
        if p.exists() {
            return Some(p);
        }
    }
    let in_project = std::path::Path::new(project).join("config.local.toml");
    if in_project.exists() {
        return Some(in_project);
    }
    let in_home = dirs::home_dir().map(|h| h.join(".tenon/config.local.toml"));
    in_home.filter(|p| p.exists())
}

/// 启动 daemon sidecar 并读取握手行（§6.2 动态端口 + 握手）。
fn spawn_daemon() -> Result<(Child, Handshake), String> {
    let project = std::env::var("TENON_PROJECT").unwrap_or_else(|_| ".".into());
    let target_triple = std::env::var("TARGET_TRIPLE")
        .unwrap_or_else(|_| format!("{}-apple-darwin", std::env::consts::ARCH));
    let candidates = [
        format!(
            "{}/binaries/tenon-daemon-{target_triple}",
            env!("CARGO_MANIFEST_DIR")
        ),
        // 开发态回退
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../target/release/tenon-daemon"
        )
        .to_string(),
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

    let mut cmd = Command::new(bin);
    cmd.arg("--project")
        .arg(&project)
        .arg("--no-lock")
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if let Some(cfg) = discover_config(&project) {
        cmd.arg("--config").arg(&cfg);
    }
    if let Some(ui) = ui_dist_dir() {
        cmd.env("TENON_UI_DIST", &ui);
    }

    let mut child = cmd.spawn().map_err(|e| format!("daemon 启动失败: {e}"))?;
    let stdout = child.stdout.take().expect("piped stdout");
    let reader = std::io::BufReader::new(stdout);
    for line in reader.lines() {
        let line = line.map_err(|e| e.to_string())?;
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line.trim()) {
            if v.get("tenon") == Some(&serde_json::json!(1)) {
                let port = v["port"].as_u64().ok_or("handshake 缺 port")? as u16;
                let token = v["token"].as_str().ok_or("handshake 缺 token")?.to_string();
                return Ok((
                    child,
                    Handshake {
                        port,
                        token,
                        project: project.clone(),
                    },
                ));
            }
        }
    }
    let _ = child.kill();
    Err("daemon 握手失败（未读到握手行）".into())
}

/// IPC 命令：UI 调用获取 daemon 握手（§6.2）。
#[tauri::command]
fn get_handshake(state: tauri::State<AppState>) -> Result<Handshake, String> {
    state
        .handshake
        .lock()
        .unwrap()
        .clone()
        .ok_or_else(|| "daemon 未就绪".to_string())
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .manage(AppState {
            handshake: Mutex::new(None),
            _child: Mutex::new(None),
        })
        .invoke_handler(tauri::generate_handler![get_handshake])
        .setup(|app| {
            let handle = app.handle().clone();
            // daemon 在后台线程启动：读握手行可能阻塞，不能卡住 setup
            std::thread::spawn(move || match spawn_daemon() {
                Ok((child, hs)) => {
                    let state = handle.state::<AppState>();
                    *state.handshake.lock().unwrap() = Some(hs.clone());
                    *state._child.lock().unwrap() = Some(child);
                    // 诊断：sidecar 握手落 stderr（端口 / token 前缀，便于联调定位）
                    eprintln!(
                        "[tenon-shell] daemon ready: port={} token_prefix={}",
                        hs.port,
                        &hs.token[..8.min(hs.token.len())]
                    );
                    let _ = handle.emit("tenon://handshake", &hs);
                    // 将 WebView 导航至 daemon 同源托管的 UI（浏览器同款链路：
                    // 握手经 URL 参数直传，UI 经 http://127.0.0.1 调 API，无跨域）
                    if let Some(win) = handle.get_webview_window("main") {
                        match Url::parse_with_params(
                            &format!("http://127.0.0.1:{}/", hs.port),
                            &[
                                ("port", hs.port.to_string()),
                                ("token", hs.token.clone()),
                                ("project", hs.project.clone()),
                            ],
                        ) {
                            Ok(url) => {
                                if let Err(e) = win.navigate(url) {
                                    eprintln!("WebView 导航失败: {e}");
                                }
                            }
                            Err(e) => eprintln!("URL 构造失败: {e}"),
                        }
                    }
                }
                Err(e) => {
                    eprintln!("daemon 启动失败: {e}");
                    // 失败也应让用户看到原因：初始页加载后 UI 会经
                    // get_handshake 轮询失败并渲染错误屏（main.tsx renderFatal）
                }
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            // 壳退出时清理 daemon 子进程
            if let tauri::WindowEvent::Destroyed = event {
                let child = {
                    let state = window.state::<AppState>();
                    let child = state._child.lock().unwrap().take();
                    child
                };
                if let Some(mut child) = child {
                    let _ = child.kill();
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("tenon app error");
}
