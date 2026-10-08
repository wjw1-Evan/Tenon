//! Tenon 桌面壳（设计方案 §6.2）：WebView 承载 UI，本身无业务逻辑。
//!
//! daemon（Rust 内核）作为 sidecar 子进程管理：启动 → 读握手行 →
//! 将 WebView 导航至 daemon 同源托管的 UI（与浏览器访问同一条链路，
//! 握手经 URL 参数传递，无注入竞态）。壳退出时清理子进程。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::io::BufRead;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::path::BaseDirectory;
use tauri::{Emitter, Manager, Url};
use tauri_plugin_updater::{Update, UpdaterExt};

#[derive(Debug, Clone, Serialize)]
struct Handshake {
    port: u16,
    token: String,
    project: String,
}

/// v1.152/v1.154 更新载荷：{version, notes}（pending = 已装待展示，ready = 已下载待安装）。
#[derive(Debug, Clone, Serialize, Deserialize)]
struct UpdatePayload {
    version: String,
    notes: String,
}

/// 壳偏好（应用数据目录 shell-prefs.json，0600）：更新日志展示与手动升级识别。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ShellPrefs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pending_update: Option<UpdatePayload>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_seen_version: Option<String>,
}

/// UI 弹窗载荷：notes 为空 = 手动升级（无随包日志，仅版本号 + Release 外链）。
#[derive(Debug, Clone, Serialize)]
struct UpdateNotesPayload {
    version: String,
    notes: Option<String>,
}

fn prefs_path(app: &tauri::AppHandle) -> Option<PathBuf> {
    app.path()
        .app_data_dir()
        .ok()
        .map(|dir| dir.join("shell-prefs.json"))
}

fn load_prefs(path: &PathBuf) -> ShellPrefs {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn save_prefs(path: &PathBuf, prefs: &ShellPrefs) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("偏好目录创建失败: {e}"))?;
    }
    let body = serde_json::to_vec_pretty(prefs).map_err(|e| format!("偏好序列化失败: {e}"))?;
    // 与 daemon settings.json 同一安全姿态：0600
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
            .map_err(|e| format!("偏好写入失败: {e}"))?;
        file.write_all(&body)
            .map_err(|e| format!("偏好写入失败: {e}"))?;
    }
    #[cfg(not(unix))]
    std::fs::write(path, body).map_err(|e| format!("偏好写入失败: {e}"))?;
    Ok(())
}

/// 数字点分版本号逐段比较（发布 tag 受语义化版本门禁，宽松兜底即可）。
fn is_newer_version(candidate: &str, base: &str) -> bool {
    let parse = |v: &str| -> Vec<u64> { v.split('.').map(|s| s.parse().unwrap_or(0)).collect() };
    let (c, b) = (parse(candidate), parse(base));
    for i in 0..c.len().max(b.len()) {
        let (cv, bv) = (
            c.get(i).copied().unwrap_or(0),
            b.get(i).copied().unwrap_or(0),
        );
        if cv != bv {
            return cv > bv;
        }
    }
    false
}

/// 当前是否应展示更新日志：随包 pending 优先（notes 全量），
/// 其次 last_seen 与当前版本不一致（手动 dmg 升级，仅版本号）。
/// 全新安装（无 last_seen）不弹——首启不是「更新」。
fn resolve_update_notes(prefs: &ShellPrefs, current: &str) -> Option<UpdateNotesPayload> {
    if let Some(pending) = &prefs.pending_update {
        if pending.version == current {
            return Some(UpdateNotesPayload {
                version: current.to_string(),
                notes: Some(pending.notes.clone()),
            });
        }
    }
    match &prefs.last_seen_version {
        Some(last) if is_newer_version(current, last) => Some(UpdateNotesPayload {
            version: current.to_string(),
            notes: None,
        }),
        _ => None,
    }
}

struct AppState {
    handshake: Mutex<Option<Handshake>>,
    _child: Mutex<Option<Child>>,
    /// 序列化 shell-prefs.json 的读改写（更新监控线程与 IPC 命令并发）。
    prefs_io: Mutex<()>,
    /// v1.154 已下载待安装的更新与产物路径（进程内存态；下载物不跨重启保留）。
    ready_download: Mutex<Option<(Update, PathBuf)>>,
}

/// daemon 托管 UI 的产物目录：打包资源 > env 指定 > 仓库根 ui/dist（开发态）。
fn ui_dist_dir(app: &tauri::AppHandle) -> Option<PathBuf> {
    if let Ok(index) = app.path().resolve("ui/index.html", BaseDirectory::Resource) {
        if index.exists() {
            return index.parent().map(PathBuf::from);
        }
    }
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
fn spawn_daemon(app: &tauri::AppHandle) -> Result<(Child, Handshake), String> {
    let project = std::env::var("TENON_PROJECT").unwrap_or_else(|_| ".".into());
    let target_triple = std::env::var("TARGET_TRIPLE")
        .unwrap_or_else(|_| format!("{}-apple-darwin", std::env::consts::ARCH));
    let exe_suffix = std::env::consts::EXE_SUFFIX;
    let packaged_sidecar = format!("tenon-daemon{exe_suffix}");
    let source_sidecar = format!("tenon-daemon-{target_triple}{exe_suffix}");
    let mut candidates = Vec::new();
    // Tauri 安装后会把带 target 后缀的 externalBin 重命名为裸名，
    // 并与主程序同级（macOS Contents/MacOS、Linux/Windows bin 目录）。
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join(&packaged_sidecar));
            candidates.push(dir.join(&source_sidecar));
        }
    }
    let source_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    candidates.push(source_root.join("binaries").join(&source_sidecar));
    // 开发态回退。
    candidates.push(
        source_root
            .join("../../target/release")
            .join(format!("tenon-daemon{exe_suffix}")),
    );
    candidates.push(
        source_root
            .join("../../target/debug")
            .join(format!("tenon-daemon{exe_suffix}")),
    );
    let bin = candidates
        .iter()
        .find(|p| p.exists())
        .ok_or_else(|| "tenon-daemon sidecar 未找到".to_string())?;

    let mut cmd = Command::new(bin);
    cmd.arg("--project")
        .arg(&project)
        .arg("--no-lock")
        // v1.157（§12.6 用户裁定）：桌面启动即绑 0.0.0.0，局域网设备凭一次性
        // 配对码配对后可访问 Web 版（门禁与吊销见 daemon §12.6）
        .arg("--lan")
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if let Some(cfg) = discover_config(&project) {
        cmd.arg("--config").arg(&cfg);
    }
    if let Some(ui) = ui_dist_dir(app) {
        cmd.env("TENON_UI_DIST", &ui);
    }
    // v1.90：完整包由 Tauri Updater 负责；daemon 内置 auto 循环让位，避免双下载。
    cmd.env("TENON_UPDATE_SURFACE", "shell");

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
    // kill 后必须 wait 收尸，否则 daemon 成为壳进程生命周期内的僵尸
    let _ = child.wait();
    Err("daemon 握手失败（未读到握手行）".into())
}

/// 更新下载物落盘（cache 目录固定文件名，0600；覆盖写免清理）。
fn persist_download_artifact(app: &tauri::AppHandle, bytes: &[u8]) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_cache_dir()
        .map_err(|e| format!("cache 目录解析失败: {e}"))?
        .join("updates");
    std::fs::create_dir_all(&dir).map_err(|e| format!("更新目录创建失败: {e}"))?;
    let path = dir.join("update.pkg");
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&path)
            .map_err(|e| format!("更新产物写入失败: {e}"))?;
        file.write_all(bytes)
            .map_err(|e| format!("更新产物写入失败: {e}"))?;
    }
    #[cfg(not(unix))]
    std::fs::write(&path, bytes).map_err(|e| format!("更新产物写入失败: {e}"))?;
    Ok(path)
}

/// 桌面壳更新循环（v1.90；v1.154 收敛为恒自动）：启动即检查、每 6 小时至多一次。
/// 发现新版本只后台下载——安装与重启经 UI 通知卡由用户确认（§6.2），
/// 发布者签名由 Tauri Updater 强制校验。
fn spawn_update_monitor(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(6 * 3600));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // 启动即检查一次（下载完未装的残留也借此补提示），此后每 6 小时一轮
        let mut first = true;
        loop {
            if first {
                first = false;
            } else {
                ticker.tick().await;
            }
            match app.updater() {
                Ok(updater) => match updater.check().await {
                    Ok(Some(update)) => {
                        let version = update.version.clone();
                        let notes = update.body.clone().unwrap_or_default();
                        eprintln!("[tenon-shell] 发现桌面更新 v{version}，后台下载");
                        // 下载物落 cache 目录（固定文件名覆盖写，不跨重启保留），
                        // 避免整包常驻内存等待用户确认
                        let bytes = match update.download(|_, _| {}, || {}).await {
                            Ok(bytes) => bytes,
                            Err(e) => {
                                eprintln!("[tenon-shell] 桌面更新下载失败: {e}");
                                continue;
                            }
                        };
                        let artifact = match persist_download_artifact(&app, &bytes) {
                            Ok(path) => path,
                            Err(e) => {
                                eprintln!("[tenon-shell] 更新产物落盘失败: {e}");
                                continue;
                            }
                        };
                        *app.state::<AppState>().ready_download.lock().unwrap() =
                            Some((update, artifact));
                        let _ = app.emit(
                            "tenon://update-ready",
                            UpdatePayload {
                                version: version.clone(),
                                notes,
                            },
                        );
                        eprintln!("[tenon-shell] 桌面更新 v{version} 下载完成，等待用户确认安装");
                    }
                    Ok(None) => {
                        eprintln!("[tenon-shell] 桌面更新：已是最新版本");
                    }
                    Err(e) => eprintln!("[tenon-shell] 桌面更新检查失败: {e}"),
                },
                Err(e) => eprintln!("[tenon-shell] updater 初始化失败: {e}"),
            }
        }
    });
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

/// IPC 命令：待展示的更新日志（peek 不消费，关闭弹窗走 dismiss_update_notes）。
#[tauri::command]
fn get_update_notes(app: tauri::AppHandle) -> Result<Option<UpdateNotesPayload>, String> {
    let Some(path) = prefs_path(&app) else {
        return Ok(None);
    };
    let state = app.state::<AppState>();
    let _guard = state.prefs_io.lock().unwrap();
    let prefs = load_prefs(&path);
    let current = app.package_info().version.to_string();
    Ok(resolve_update_notes(&prefs, &current))
}

/// IPC 命令：已下载待安装的更新（v1.154，peek 不消费）。
/// 只在下载物就绪时返回 Some——UI 挂载即查 + 轮询发现（get_handshake 同风格）。
#[tauri::command]
fn get_update_ready(app: tauri::AppHandle) -> Result<Option<UpdateNotesPayload>, String> {
    let download = app
        .state::<AppState>()
        .ready_download
        .lock()
        .unwrap()
        .clone();
    Ok(download.map(|(update, _)| UpdateNotesPayload {
        version: update.version.clone(),
        notes: update.body,
    }))
}

/// IPC 命令：用户确认安装（v1.154）——先把更新日志转 pending_update
/// （新版首启 v1.152 弹窗接管；Windows 的 install 会直接退出进程，必须先落盘），
/// 再 install + 重启到最新版本。
#[tauri::command]
fn install_update(app: tauri::AppHandle) -> Result<(), String> {
    let download = app
        .state::<AppState>()
        .ready_download
        .lock()
        .unwrap()
        .clone();
    let Some((update, artifact)) = download else {
        return Err("没有已下载的更新".into());
    };
    let version = update.version.clone();
    if let Some(path) = prefs_path(&app) {
        let state = app.state::<AppState>();
        let _guard = state.prefs_io.lock().unwrap();
        let mut prefs = load_prefs(&path);
        prefs.pending_update = Some(UpdatePayload {
            version: version.clone(),
            notes: update.body.clone().unwrap_or_default(),
        });
        if let Err(e) = save_prefs(&path, &prefs) {
            eprintln!("[tenon-shell] 更新日志持久化失败: {e}");
        }
    }
    let bytes = std::fs::read(&artifact).map_err(|e| format!("更新产物读取失败: {e}"))?;
    update
        .install(&bytes)
        .map_err(|e| format!("更新安装失败: {e}"))?;
    let _ = std::fs::remove_file(&artifact);
    eprintln!("[tenon-shell] 桌面更新 v{version} 安装完成，重启");
    app.restart();
}

/// IPC 命令：用户确认看过更新日志——清 pending 并记 last_seen_version。
#[tauri::command]
fn dismiss_update_notes(app: tauri::AppHandle) -> Result<(), String> {
    let Some(path) = prefs_path(&app) else {
        return Ok(());
    };
    let state = app.state::<AppState>();
    let _guard = state.prefs_io.lock().unwrap();
    let mut prefs = load_prefs(&path);
    prefs.pending_update = None;
    prefs.last_seen_version = Some(app.package_info().version.to_string());
    save_prefs(&path, &prefs)
}

/// 开发热重载（v1.65）：debug 构建探测 Vite dev server（127.0.0.1:5173），
/// 在线则导航 dev server——UI 编辑即时 HMR，握手仍经 URL 参数直传；
/// 未运行（或 release 构建）返回 None，回落 daemon 托管 UI（生产行为不变）。
fn dev_server_url(port: u16, token: &str, project: &str) -> Option<Url> {
    if !cfg!(debug_assertions) {
        return None;
    }
    let addr: std::net::SocketAddr = "127.0.0.1:5173".parse().ok()?;
    let timeout = std::time::Duration::from_millis(200);
    std::net::TcpStream::connect_timeout(&addr, timeout).ok()?;
    Url::parse_with_params(
        "http://localhost:5173/",
        &[
            ("port", port.to_string()),
            ("token", token.to_string()),
            ("project", project.to_string()),
        ],
    )
    .ok()
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(AppState {
            handshake: Mutex::new(None),
            _child: Mutex::new(None),
            prefs_io: Mutex::new(()),
            ready_download: Mutex::new(None),
        })
        .invoke_handler(tauri::generate_handler![
            get_handshake,
            get_update_notes,
            dismiss_update_notes,
            get_update_ready,
            install_update
        ])
        .setup(|app| {
            let handle = app.handle().clone();
            // daemon 在后台线程启动：读握手行可能阻塞，不能卡住 setup
            std::thread::spawn(move || match spawn_daemon(&handle) {
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
                    spawn_update_monitor(handle.clone());
                    // 将 WebView 导航至 UI：debug 且 Vite dev server 在跑时导航
                    // dev server（HMR 热重载），否则 daemon 同源托管的 UI（浏览器
                    // 同款链路：握手经 URL 参数直传，UI 经 http://127.0.0.1 调 API，
                    // 无跨域）
                    if let Some(win) = handle.get_webview_window("main") {
                        let url = dev_server_url(hs.port, &hs.token, &hs.project).or_else(|| {
                            Url::parse_with_params(
                                &format!("http://127.0.0.1:{}/", hs.port),
                                &[
                                    ("port", hs.port.to_string()),
                                    ("token", hs.token.clone()),
                                    ("project", hs.project.clone()),
                                ],
                            )
                            .ok()
                        });
                        match url {
                            Some(url) => {
                                if let Err(e) = win.navigate(url) {
                                    eprintln!("WebView 导航失败: {e}");
                                }
                            }
                            None => eprintln!("导航 URL 构造失败"),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_compare_numeric_segments() {
        assert!(is_newer_version("0.1.2", "0.1.1"));
        assert!(is_newer_version("0.2.0", "0.1.99"));
        assert!(is_newer_version("1.0.0", "0.9.9"));
        assert!(!is_newer_version("0.1.1", "0.1.1"));
        assert!(!is_newer_version("0.1.0", "0.1.1"));
        // 降级（手动回滚装旧包）不弹
        assert!(!is_newer_version("0.1.0", "0.1.1"));
    }

    #[test]
    fn pending_notes_shown_when_version_matches() {
        let prefs = ShellPrefs {
            pending_update: Some(UpdatePayload {
                version: "0.1.2".into(),
                notes: "- 修复若干问题".into(),
            }),
            last_seen_version: Some("0.1.1".into()),
        };
        let payload = resolve_update_notes(&prefs, "0.1.2").expect("应展示随包日志");
        assert_eq!(payload.version, "0.1.2");
        assert_eq!(payload.notes.as_deref(), Some("- 修复若干问题"));
    }

    #[test]
    fn stale_pending_falls_back_to_manual_detection() {
        // pending 是更旧一次更新的残留（如用户连跳两版）：按手动升级识别
        let prefs = ShellPrefs {
            pending_update: Some(UpdatePayload {
                version: "0.1.2".into(),
                notes: "旧日志".into(),
            }),
            last_seen_version: Some("0.1.1".into()),
        };
        let payload = resolve_update_notes(&prefs, "0.1.3").expect("应按手动升级展示");
        assert_eq!(payload.version, "0.1.3");
        assert!(payload.notes.is_none());
    }

    #[test]
    fn manual_upgrade_without_pending_shows_version_only() {
        let prefs = ShellPrefs {
            pending_update: None,
            last_seen_version: Some("0.1.1".into()),
        };
        let payload = resolve_update_notes(&prefs, "0.1.2").expect("手动升级应展示");
        assert!(payload.notes.is_none());
    }

    #[test]
    fn fresh_install_and_repeat_launch_stay_silent() {
        // 全新安装：无 last_seen，首启不是「更新」
        let fresh = ShellPrefs::default();
        assert!(resolve_update_notes(&fresh, "0.1.2").is_none());
        // 已确认过的版本重复启动不弹
        let seen = ShellPrefs {
            pending_update: None,
            last_seen_version: Some("0.1.2".into()),
        };
        assert!(resolve_update_notes(&seen, "0.1.2").is_none());
    }
}
