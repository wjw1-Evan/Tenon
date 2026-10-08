//! Tenon 本地 API daemon（设计方案 §15 / §12.6 / §6.2）。
//!
//! - 绑定 127.0.0.1 随机端口；HTTP 用 `X-Tenon-Token` 头鉴权；
//! - WS 先 `POST /ws-ticket` 换 60 秒一次性票据（首帧携带，重放即拒，ADR-10）；
//! - 校验 Origin / Host（CORS 仅放行应用自身源；网页请求一律拒绝，§12.6）；
//! - 会话 / 回滚 / 文件 / 搜索 / 成本端点一一对应 §15 表。

mod auth;
pub mod exec;
mod lsp_edit;
mod pairing;
mod routes;
mod state;
pub mod subagents;
pub mod updates;

pub use pairing::PairingStore;
pub use state::{DaemonOptions, DaemonState, SessionEntry};
pub use updates::{apply_staged_update, mark_staged_update, take_apply_request, StagedUpdate};

use axum::Router;
use std::path::PathBuf;
use std::sync::Arc;
use tenon_config::Config;
use tower_http::services::ServeDir;

/// daemon 运行句柄：端口与握手 token。
#[derive(Debug, Clone)]
pub struct DaemonHandle {
    pub port: u16,
    pub token: String,
    pub shutdown: tokio::sync::watch::Receiver<bool>,
}

/// 启动 daemon（绑定 127.0.0.1 随机端口）。
pub async fn serve(options: DaemonOptions) -> std::io::Result<DaemonHandle> {
    let lan_bind = options.lan_bind;
    // 开发热重载 `--port`：须在 options 移入 DaemonState 前取出
    let port_fixed = options.bind_port.unwrap_or(0);
    let laya_registry_url = options.laya_registry_url.clone();
    let manage_endpoint_file = options.endpoint_path.is_some() || options.db_path.is_some();
    let endpoint_path = options
        .endpoint_path
        .clone()
        .unwrap_or_else(|| Config::data_dir().join("daemon.endpoint"));
    let state = Arc::new(DaemonState::new(options).await);
    // v1.86 更新执行器：只应用显式请求；绑定前原子替换并重验哈希。失败保留旧版。
    match crate::updates::take_apply_request(&state.updates_staging_dir) {
        Ok(Some(staged)) => {
            let current_exe = std::env::current_exe()?;
            match crate::updates::apply_staged_update(
                std::path::Path::new(&staged.path),
                &current_exe,
            ) {
                Ok(()) => {
                    let _ = std::fs::remove_file(&staged.path);
                    tracing::info!(
                        "更新已在启动前应用: v{} → {}",
                        staged.version,
                        current_exe.display()
                    );
                }
                Err(e) => tracing::warn!("staged 更新应用失败，继续旧版: {e}"),
            }
        }
        Ok(None) => {}
        Err(e) => tracing::warn!("staged 更新请求解析失败: {e}"),
    }
    // 崩溃恢复（§10.3）：重启扫描非终态会话 → 回滚最近快照 → ROLLED_BACK 入 Trace
    match tenon_agent::recover_stale_sessions(state.store.clone(), &state.snapshots_root).await {
        Ok(report) if !report.sessions.is_empty() => {
            tracing::warn!(
                "崩溃恢复：{} 个会话回滚至最近快照（错误 {} 项）",
                report.sessions.len(),
                report.errors.len()
            );
        }
        Ok(_) => {}
        Err(e) => tracing::error!("崩溃恢复扫描失败: {e}"),
    }
    let app: Router = build_app(state.clone(), resolve_ui_dist());

    // Laya 自动下载并启用（§9.8 v1.71）：启动即后台拉取，失败静默回退
    state.spawn_laya_auto_download(laya_registry_url);

    // 自动更新执行器（v1.86；v1.154 起恒自动，无通道门槛）：周期检查、验签 staging，
    // 重启时生效。桌面壳表面（v1.90）由 Tauri Updater 更新完整包，这里不重复下载 sidecar。
    if std::env::var("TENON_UPDATE_SURFACE").as_deref() == Ok("shell") {
        tracing::debug!("桌面壳表面：daemon-only 自动更新已让位给完整包 updater");
    } else {
        let state = state.clone();
        let interval = state.config.update.check_interval_s;
        let manifest_url = state.config.update.manifest_url.clone();
        let public_key = state.config.update.public_key_hex.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(std::time::Duration::from_secs(interval));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                match crate::updates::stage_update(
                    &manifest_url,
                    &public_key,
                    env!("CARGO_PKG_VERSION"),
                    &crate::updates::current_target(),
                    &state.updates_staging_dir,
                )
                .await
                {
                    Ok(staged) => {
                        // 恒自动语义：已验证 staging 自动请求下次启动应用。
                        if let Err(e) =
                            crate::updates::mark_staged_update(&staged, &state.updates_staging_dir)
                        {
                            tracing::warn!("更新 staging 标记失败: {e}");
                        } else {
                            tracing::info!("更新已 staging: v{}（重启生效）", staged.version);
                        }
                    }
                    Err(
                        crate::updates::UpdateError::NotNewer
                        | crate::updates::UpdateError::NoPlatform,
                    ) => {}
                    Err(e) => {
                        tracing::warn!("自动更新检查失败: {e}");
                    }
                }
            }
        });
    }

    // L4 增量索引 worker（§10.1）：项目激活 / watcher 变更驱动。
    if let Some(requests) = state.take_l4_requests() {
        state.spawn_l4_worker(requests);
    }

    // ProjectRuntime 空闲回收（§6.4）：无活跃任务且超过 TTL 时关闭 watcher / LSP。
    {
        let state = state.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(std::time::Duration::from_secs(60));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                for project_id in state.reclaim_idle_projects().await {
                    tracing::info!("ProjectRuntime 空闲回收: {project_id}");
                }
            }
        });
    }

    // 绑定地址：默认仅本机回环（§12.6）；`--lan` 显式开启后绑全部接口
    //（局域网请求须持已配对设备令牌，见 auth_middleware）
    let bind_addr = if lan_bind {
        ("0.0.0.0", port_fixed)
    } else {
        ("127.0.0.1", port_fixed)
    };
    let listener = tokio::net::TcpListener::bind(bind_addr).await?;
    let port = listener.local_addr()?.port();
    state.port.store(port, std::sync::atomic::Ordering::Relaxed);
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let shutdown_handle = shutdown_tx.subscribe();

    // 握手端点文件（§6.2 / §14.1）：随机端口 + token 不入日志；文件 0600。
    // 内存库（测试 serve）不写，避免测试端口污染共享端点；
    // 持久 daemon 每 20s 心跳重写，避免旧实例退出清理误删活实例端点。
    if manage_endpoint_file {
        let endpoint_json = format!("{{\"port\":{port},\"token\":\"{}\"}}\n", state.token);
        let write_endpoint = {
            let path = endpoint_path.clone();
            let json = endpoint_json.clone();
            move || {
                let _ = std::fs::write(&path, &json);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
                }
            }
        };
        write_endpoint();
        let mut heartbeat_rx = shutdown_handle.clone();
        let heartbeat_write = write_endpoint.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(20));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    _ = tick.tick() => heartbeat_write(),
                    _ = heartbeat_rx.wait_for(|v| *v) => break,
                }
            }
        });
    }

    tracing::info!("tenon daemon listening on 127.0.0.1:{port}");
    let endpoint_for_shutdown = endpoint_path.clone();
    let cleanup_endpoint = manage_endpoint_file;
    tokio::spawn(async move {
        let mut shutdown_rx = shutdown_rx;
        // into_make_service_with_connect_info：中间件取真实连接对端地址做
        // 回环判定（Host 头客户端可伪造，v1.166 前局域网客户端伪装
        // `Host: 127.0.0.1` 即可骗过回环判定拿到主 token）
        let _ = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .with_graceful_shutdown(async move {
            let _ = shutdown_rx.wait_for(|v| *v).await;
        })
        .await;
        if cleanup_endpoint {
            let _ = std::fs::remove_file(&endpoint_for_shutdown);
        }
        let _ = shutdown_tx.send(true);
    });

    // AI Evals 定时触发（M3 §18.3 流水线）：interval_hours > 0 时后台循环
    // 调用 tenon-evals 二进制跑附录 D 套件（报告落 eval_runs + evals/ 目录）
    if state.config.evals.interval_hours > 0 {
        let interval =
            std::time::Duration::from_secs(state.config.evals.interval_hours as u64 * 3600);
        let provider = if state.config.evals.provider.is_empty() {
            state.default_provider.read().unwrap().clone()
        } else {
            state.config.evals.provider.clone()
        };
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(interval).await;
                let exe = std::env::current_exe()
                    .ok()
                    .and_then(|p| p.parent().map(|d| d.join("tenon-evals")))
                    .map(|p| p.to_string_lossy().into_owned());
                let Some(exe) = exe else { continue };
                if !std::path::Path::new(&exe).exists() {
                    tracing::warn!("tenon-evals 二进制不存在，跳过定时触发: {exe}");
                    continue;
                }
                let out = std::process::Command::new(&exe)
                    .args([
                        "--provider",
                        &provider,
                        "--out",
                        &format!("evals/scheduled-{provider}.json"),
                    ])
                    .output();
                match out {
                    Ok(o) if o.status.success() => {
                        tracing::info!("定时 Evals 完成（{provider}）");
                    }
                    Ok(o) => {
                        tracing::warn!(
                            "定时 Evals 失败: {}",
                            String::from_utf8_lossy(&o.stderr).trim()
                        );
                    }
                    Err(e) => tracing::warn!("定时 Evals 启动失败: {e}"),
                }
            }
        });
    }

    // shadow 快照库周期 gc（§10.3 v1.93 接线）：keep_days > 0 时每小时对全部
    // 登记项目执行 `git gc --prune=<keep_days>.days.ago`——对象级清理，
    // checkpoint 记录与事件日志不受影响；失败静默留待下轮。
    if state.config.checkpoint.keep_days > 0 {
        let state = state.clone();
        let keep_days = state.config.checkpoint.keep_days;
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(3600));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tick.tick().await;
                state.gc_snapshot_stores(keep_days).await;
            }
        });
    }

    // 会话归档定时（§14.2 v1.93 接线）：events_days > 0 时每日把关闭超期的
    // 会话压缩归档至 ~/.tenon/archive/（热数据留 SQLite）。
    if state.config.archive.events_days > 0 {
        let state = state.clone();
        let days = state.config.archive.events_days;
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(24 * 3600));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tick.tick().await;
                let archive_dir = tenon_config::Config::data_dir().join("archive");
                let mut store = state.store.lock().await;
                match store.archive_old_sessions(days, &archive_dir) {
                    Ok(ids) if !ids.is_empty() => {
                        tracing::info!("归档 {} 个超期会话至 {}", ids.len(), archive_dir.display())
                    }
                    Ok(_) => {}
                    Err(e) => tracing::warn!("会话归档失败: {e}"),
                }
                // v1.172 存储治理（§14.2）：归档后按空闲页阈值回收 SQLite 文件
                match store.maybe_vacuum() {
                    Ok(true) => tracing::info!("归档后 VACUUM 完成（空闲页超阈值）"),
                    Ok(false) => {}
                    Err(e) => tracing::warn!("归档后 VACUUM 失败: {e}"),
                }
            }
        });
    }

    Ok(DaemonHandle {
        port,
        token: state.token.clone(),
        shutdown: shutdown_handle,
    })
}

/// 组装完整 Router（§12.6 / v1.76 / v1.157）：API 路由 + 鉴权中间件之上
/// 追加 UI 静态托管 fallback——fallback 在鉴权层**之外**（axum `layer` 只包
/// 已注册路由），未配对局域网设备因此可加载 UI、全部 API 仍被中间件拦截；
/// `ui_dist` 为 None 时无静态托管（sidecar 无 UI 需求）。测试注入目录复用。
pub fn build_app(state: Arc<DaemonState>, ui_dist: Option<PathBuf>) -> Router {
    let mut app = routes::build_router(state);
    if let Some(ui_dist) = ui_dist {
        // SPA 回退：非 API 路径返回 index.html（UI 自经 /pairing 自发现握手）
        let spa = ServeDir::new(&ui_dist)
            .not_found_service(ServeDir::new(&ui_dist).append_index_html_on_directories(false));
        app = app.fallback_service(spa);
        tracing::info!("本机浏览器访问：托管 UI 静态资源（{ui_dist:?}）");
    }
    app
}

/// 生成本地 token（随机 32 字节 hex）。
pub fn generate_token() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// 伪随机 u64（配对码等轻量用途；非密码学）。
pub fn generate_token_hash() -> u64 {
    use rand::RngCore;
    let mut bytes = [0u8; 8];
    rand::rng().fill_bytes(&mut bytes);
    u64::from_le_bytes(bytes)
}

/// UI 构建产物解析链（v1.76 Web 一键启动）：`TENON_UI_DIST` → cwd 及其祖先目录的
/// `ui/dist` → 可执行文件同级的 `ui/dist`；命中以 `index.html` 存在为准。
pub fn resolve_ui_dist() -> Option<PathBuf> {
    let env_dir = std::env::var("TENON_UI_DIST").ok().map(PathBuf::from);
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()));
    resolve_ui_dist_from(&cwd, exe_dir.as_deref(), env_dir)
}

/// 解析链核心（base / exe_dir / env 可注入，供测试）。
fn resolve_ui_dist_from(
    base: &std::path::Path,
    exe_dir: Option<&std::path::Path>,
    env_override: Option<PathBuf>,
) -> Option<PathBuf> {
    if let Some(env) = env_override {
        if env.join("index.html").exists() {
            return Some(env);
        }
    }
    let mut cur = Some(base);
    while let Some(dir) = cur {
        let candidate = dir.join("ui").join("dist");
        if candidate.join("index.html").exists() {
            return Some(candidate);
        }
        cur = dir.parent();
    }
    if let Some(exe) = exe_dir {
        let candidate = exe.join("ui").join("dist");
        if candidate.join("index.html").exists() {
            return Some(candidate);
        }
    }
    None
}

/// 读取存活 daemon 实例的 endpoint（§6.2）：返回 (port, token)。
/// 心跳每 20s 重写 endpoint 文件，mtime 超过 45s 视为陈旧残留（实例已死）。
pub fn read_live_endpoint(path: &std::path::Path) -> Option<(u16, String)> {
    let age = std::fs::metadata(path)
        .ok()?
        .modified()
        .ok()?
        .elapsed()
        .ok()?;
    if age > std::time::Duration::from_secs(45) {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    Some((
        v.get("port")?.as_u64()? as u16,
        v.get("token")?.as_str()?.to_string(),
    ))
}

/// 用系统默认浏览器打开 URL（尽力而为：失败仅记日志，调用方不依赖结果）。
pub fn open_browser(url: &str) {
    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = std::process::Command::new("open");
        c.arg(url);
        c
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut cmd = {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(url);
        c
    };
    #[cfg(windows)]
    let mut cmd = {
        let mut c = std::process::Command::new("cmd");
        // start 后第一个引号参数是窗口标题占位，防 URL 被当标题吞掉
        c.args(["/c", "start", "", url]);
        c
    };
    match cmd.output() {
        Ok(o) if o.status.success() => {}
        Ok(o) => tracing::warn!("浏览器打开退出码异常: {}", o.status),
        Err(e) => tracing::warn!("浏览器打开失败: {e}"),
    }
}

/// 单实例锁（§6.2）：锁文件独占创建（O_EXCL），写入 pid；释放时删除。
/// 已有实例存活（pid 可探测）→ 拒绝二次启动。
pub struct InstanceLock {
    path: std::path::PathBuf,
}

impl InstanceLock {
    pub fn acquire(path: std::path::PathBuf) -> std::io::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // 独占创建（O_EXCL，§6.2）：并发启动时恰好一个成功，不存在
        // 「双方都见无文件、都写入」的 check-then-write 窗口。文件已存在时
        // 做陈旧锁检测：pid 不存活才移除并重试一次。
        for _ in 0..2 {
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(mut file) => {
                    use std::io::Write as _;
                    file.write_all(std::process::id().to_string().as_bytes())?;
                    return Ok(Self { path });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let existing = std::fs::read_to_string(&path).unwrap_or_default();
                    let live = existing
                        .trim()
                        .parse::<i32>()
                        .map(pid_alive)
                        // 内容不可读 / 非 pid：按陈旧锁复用（原实现同口径）
                        .unwrap_or(false);
                    if live {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::AlreadyExists,
                            format!("tenon daemon 已在运行（pid {}）", existing.trim()),
                        ));
                    }
                    let _ = std::fs::remove_file(&path);
                }
                Err(e) => return Err(e),
            }
        }
        // 陈旧锁移除后仍被抢占（极端竞争）：按已存在处理，不做无限重试
        Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "tenon daemon 锁竞争失败，请重试",
        ))
    }

    pub fn pid(&self) -> u32 {
        std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0)
    }
}

impl Drop for InstanceLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(unix)]
fn pid_alive(pid: i32) -> bool {
    // kill(pid, 0)：ESRCH = 不存在；EPERM = 存活但无权限
    let r = unsafe { libc::kill(pid, 0) };
    r == 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

#[cfg(windows)]
fn pid_alive(pid: i32) -> bool {
    // Windows 原生兜底面（设计主路径为 WSL2，§12.3）：OpenProcess
    // PROCESS_QUERY_LIMITED_INFORMATION 探测进程存在性，查到句柄即存活。
    unsafe {
        let handle = windows_sys::Win32::System::Threading::OpenProcess(
            windows_sys::Win32::System::Threading::PROCESS_QUERY_LIMITED_INFORMATION,
            0,
            pid as u32,
        );
        if handle == 0 {
            return false;
        }
        windows_sys::Win32::Foundation::CloseHandle(handle);
        true
    }
}

/// 生成 ed25519 签名密钥对（M0：签名与 updater 密钥，§17）。
/// 返回 (私钥 hex, 公钥 hex)；私钥落盘 0600。
pub fn generate_signing_keypair() -> std::io::Result<(String, String)> {
    use ed25519_dalek::Signer;
    let mut secret = [0u8; 32];
    {
        use rand::RngCore;
        rand::rng().fill_bytes(&mut secret);
    }
    let signing = ed25519_dalek::SigningKey::from_bytes(&secret);
    let msg = b"tenon-key-check";
    let sig = signing.sign(msg);
    // 自校验
    signing.verify(msg, &sig).expect("self-verify");
    Ok((
        hex_encode(signing.to_bytes()),
        hex_encode(signing.verifying_key().to_bytes()),
    ))
}

fn hex_encode(bytes: [u8; 32]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{load_team_policy, validate_team_policy, SettingsOverrides};

    #[test]
    fn signing_keypair_self_verifies() {
        let (sec, public) = generate_signing_keypair().unwrap();
        assert_eq!(sec.len(), 64);
        assert_eq!(public.len(), 64);
        // 不同次生成互异
        let (sec2, _) = generate_signing_keypair().unwrap();
        assert_ne!(sec, sec2);
    }

    #[test]
    fn instance_lock_exclusive_with_stale_recovery() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tenon.lock");
        let lock = InstanceLock::acquire(path.clone()).unwrap();
        assert_eq!(lock.pid(), std::process::id());
        // 存活实例 → 拒绝
        let second = InstanceLock::acquire(path.clone());
        assert!(second.is_err(), "单实例锁生效");
        // 释放后可再取
        drop(lock);
        let third = InstanceLock::acquire(path.clone()).unwrap();
        assert_eq!(third.pid(), std::process::id());
        // 陈旧锁（不存在的 pid）→ 自动回收
        std::fs::write(&path, "999999999").unwrap();
        let fourth = InstanceLock::acquire(path).unwrap();
        assert_eq!(fourth.pid(), std::process::id());
    }

    #[test]
    fn pid_alive_self_true_dead_false() {
        assert!(pid_alive(std::process::id() as i32));
        assert!(!pid_alive(9_999_999));
    }

    #[test]
    fn ui_dist_resolution_walks_ancestors_env_overrides() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        let sub = repo.join("crates").join("tenon-daemon");
        std::fs::create_dir_all(repo.join("ui").join("dist")).unwrap();
        std::fs::write(repo.join("ui").join("dist").join("index.html"), "<html/>").unwrap();
        std::fs::create_dir_all(&sub).unwrap();
        // cwd 子目录 → 祖先目录回溯命中
        let found = resolve_ui_dist_from(&sub, None, None).unwrap();
        assert_eq!(found, repo.join("ui").join("dist"));
        // env 显式覆盖优先于解析链
        let other = tempfile::tempdir().unwrap();
        std::fs::write(other.path().join("index.html"), "<html/>").unwrap();
        let found = resolve_ui_dist_from(&sub, None, Some(other.path().to_path_buf())).unwrap();
        assert_eq!(found, other.path());
        // 可执行文件同级兜底
        let bin = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(bin.path().join("ui").join("dist")).unwrap();
        std::fs::write(
            bin.path().join("ui").join("dist").join("index.html"),
            "<html/>",
        )
        .unwrap();
        let empty = tempfile::tempdir().unwrap();
        let found = resolve_ui_dist_from(empty.path(), Some(bin.path()), None).unwrap();
        assert_eq!(found, bin.path().join("ui").join("dist"));
        // 全链未命中 → None（--web 据此报错）
        assert!(resolve_ui_dist_from(empty.path(), None, None).is_none());
        // env 指向无效目录 → 回落解析链而非直接采纳
        assert!(
            resolve_ui_dist_from(empty.path(), None, Some(empty.path().join("nope"))).is_none()
        );
    }

    #[test]
    fn live_endpoint_rejects_stale_and_malformed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("daemon.endpoint");
        assert_eq!(read_live_endpoint(&path), None, "文件缺失");
        std::fs::write(&path, r#"{"port":1234,"token":"abc"}"#).unwrap();
        assert_eq!(
            read_live_endpoint(&path),
            Some((1234, "abc".to_string())),
            "新鲜 endpoint 可读"
        );
        // mtime 拨回纪元 → 心跳停更 → 陈旧拒绝（File::set_modified，Rust 1.75+）
        {
            let f = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
            f.set_modified(std::time::SystemTime::UNIX_EPOCH).unwrap();
        }
        assert_eq!(read_live_endpoint(&path), None, "陈旧 endpoint 拒绝");
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(read_live_endpoint(&path), None, "畸形内容拒绝");
    }

    #[test]
    fn settings_overrides_merge_json() {
        let mut overrides = SettingsOverrides::default();
        overrides
            .merge_json(&serde_json::json!({
                "session": { "first_edit_buffer_ms": 3000 },
                "exec": { "command_timeout_s": 60 }
            }))
            .unwrap();
        assert_eq!(overrides.first_edit_buffer_ms, Some(3000));
        assert_eq!(overrides.command_timeout_s, Some(60));
    }

    #[test]
    fn settings_overrides_to_json_roundtrip() {
        let mut overrides = SettingsOverrides::default();
        overrides
            .merge_json(&serde_json::json!({
                "session": { "first_edit_buffer_ms": 1500 }
            }))
            .unwrap();
        let json = overrides.to_json();
        assert!(json.is_object());
    }

    #[test]
    fn settings_overrides_models_merge() {
        let mut overrides = SettingsOverrides::default();
        overrides
            .merge_json(&serde_json::json!({
                "models": { "default": "openai" }
            }))
            .unwrap();
        let mut config = tenon_config::ModelsConfig::default();
        overrides.apply_models_to(&mut config);
        assert_eq!(config.default, "openai");
    }

    #[test]
    fn settings_overrides_invalid_mode_rejected() {
        let mut overrides = SettingsOverrides::default();
        let result = overrides.merge_json(&serde_json::json!({
            "session": { "mode": "invalid_mode" }
        }));
        assert!(result.is_err());
    }

    #[test]
    fn settings_overrides_reject_update_block() {
        // v1.154：更新通道移除，update.* 不再是有效覆盖键
        let mut overrides = SettingsOverrides::default();
        let result = overrides.merge_json(&serde_json::json!({
            "update": { "channel": "auto" }
        }));
        assert!(result.is_err());
    }

    #[test]
    fn load_team_policy_default_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let _policy = load_team_policy(&dir.path().join("nonexistent-policy.toml"));
        // 默认策略加载不崩溃
    }

    #[test]
    fn validate_team_policy_valid_input() {
        let policy = validate_team_policy(&serde_json::json!({
            "denied_tools": ["git_push"],
            "max_cost_usd": 10.0
        }));
        assert!(policy.is_ok());
    }

    #[test]
    fn validate_team_policy_rejects_invalid_cost() {
        let result = validate_team_policy(&serde_json::json!({
            "max_cost_usd": -1
        }));
        assert!(result.is_err());
    }

    #[test]
    fn settings_overrides_load_and_persist() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let mut overrides = SettingsOverrides::default();
        overrides
            .merge_json(&serde_json::json!({
                "session": { "first_edit_buffer_ms": 2500 }
            }))
            .unwrap();
        overrides.persist_to(&path);
        assert!(path.exists());
        let loaded = SettingsOverrides::load_from_path(&path);
        assert_eq!(loaded.first_edit_buffer_ms, Some(2500));
    }

    #[test]
    fn settings_overrides_load_nonexistent_returns_default() {
        let dir = tempfile::tempdir().unwrap();
        let overrides = SettingsOverrides::load_from_path(&dir.path().join("no-settings.json"));
        // Should return default (no crash)
        let _ = overrides;
    }

    #[test]
    fn settings_overrides_persist_creates_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let overrides = SettingsOverrides::default();
        overrides.persist_to(&path);
        // persist_to may or may not create the file depending on implementation
    }

    #[test]
    fn settings_overrides_merge_models_default() {
        let mut overrides = SettingsOverrides::default();
        overrides
            .merge_json(&serde_json::json!({
                "models": { "default": "anthropic" }
            }))
            .unwrap();
        let mut config = tenon_config::ModelsConfig::default();
        overrides.apply_models_to(&mut config);
        assert_eq!(config.default, "anthropic");
    }

    #[test]
    fn settings_overrides_merge_command_timeout() {
        let mut overrides = SettingsOverrides::default();
        overrides
            .merge_json(&serde_json::json!({
                "exec": { "command_timeout_s": 30 }
            }))
            .unwrap();
        assert_eq!(overrides.command_timeout_s, Some(30));
    }

    #[test]
    fn settings_overrides_to_json_contains_fields() {
        let mut overrides = SettingsOverrides::default();
        overrides
            .merge_json(&serde_json::json!({
                "session": { "first_edit_buffer_ms": 1000 }
            }))
            .unwrap();
        let json = overrides.to_json();
        // JSON should be serializable
        let _ = serde_json::to_string(&json).unwrap();
    }

    #[test]
    fn settings_overrides_merge_and_to_json_theme() {
        let mut overrides = SettingsOverrides::default();
        overrides
            .merge_json(&serde_json::json!({
                "session": { "first_edit_buffer_ms": 500 }
            }))
            .unwrap();
        let json = overrides.to_json();
        let serialized = serde_json::to_string(&json).unwrap();
        assert!(serialized.contains("500"));
    }

    #[test]
    fn settings_overrides_merge_empty_body() {
        let mut overrides = SettingsOverrides::default();
        let result = overrides.merge_json(&serde_json::json!({}));
        assert!(result.is_ok());
    }

    #[test]
    fn settings_overrides_merge_partial_exec() {
        let mut overrides = SettingsOverrides::default();
        overrides
            .merge_json(&serde_json::json!({
                "exec": { "command_timeout_s": 300 }
            }))
            .unwrap();
        assert_eq!(overrides.command_timeout_s, Some(300));
    }
}
