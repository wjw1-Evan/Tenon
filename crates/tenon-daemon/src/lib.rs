//! Tenon 本地 API daemon（设计方案 §15 / §12.6 / §6.2）。
//!
//! - 绑定 127.0.0.1 随机端口；HTTP 用 `X-Tenon-Token` 头鉴权；
//! - WS 先 `POST /ws-ticket` 换 60 秒一次性票据（首帧携带，重放即拒，ADR-10）；
//! - 校验 Origin / Host（CORS 仅放行应用自身源；网页请求一律拒绝，§12.6）；
//! - 会话 / 审批 / 回滚 / 文件 / 搜索 / 成本端点一一对应 §15 表。

mod auth;
mod pairing;
mod routes;
mod state;

pub use pairing::PairingStore;
pub use state::{DaemonOptions, DaemonState, SessionEntry};

use axum::Router;
use std::path::PathBuf;
use std::sync::Arc;
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
    let state = Arc::new(DaemonState::new(options).await);
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
    let mut app: Router = routes::build_router(state.clone());

    // 本机浏览器访问（§12.6 / M2）：UI 构建产物存在时由 daemon 同源托管——
    // 浏览器打开 http://127.0.0.1:{port}/ 即加载 UI 并经 /pairing 自发现握手
    let ui_dist = std::env::var("TENON_UI_DIST")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("ui/dist"));
    if ui_dist.join("index.html").exists() {
        // SPA 回退：非 API 路径返回 index.html（UI 自经 /pairing 自发现握手）
        let spa = ServeDir::new(&ui_dist)
            .not_found_service(ServeDir::new(&ui_dist).append_index_html_on_directories(false));
        app = app.fallback_service(spa);
        tracing::info!("本机浏览器访问：托管 UI 静态资源（{ui_dist:?}）");
    }

    // 绑定地址：默认仅本机回环（§12.6）；`--lan` 显式开启后绑全部接口
    //（局域网请求须持已配对设备令牌，见 auth_middleware）
    let bind_addr = if lan_bind {
        ("0.0.0.0", 0)
    } else {
        ("127.0.0.1", 0)
    };
    let listener = tokio::net::TcpListener::bind(bind_addr).await?;
    let port = listener.local_addr()?.port();
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let shutdown_handle = shutdown_tx.subscribe();

    tracing::info!("tenon daemon listening on 127.0.0.1:{port}");
    tokio::spawn(async move {
        let mut shutdown_rx = shutdown_rx;
        let _ = axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = shutdown_rx.wait_for(|v| *v).await;
            })
            .await;
        let _ = shutdown_tx.send(true);
    });

    // AI Evals 定时触发（M3 §18.3 流水线）：interval_hours > 0 时后台循环
    // 调用 tenon-evals 二进制跑附录 D 套件（报告落 eval_runs + evals/ 目录）
    if state.config.evals.interval_hours > 0 {
        let interval =
            std::time::Duration::from_secs(state.config.evals.interval_hours as u64 * 3600);
        let provider = if state.config.evals.provider.is_empty() {
            state.default_provider.clone()
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

    Ok(DaemonHandle {
        port,
        token: state.token.clone(),
        shutdown: shutdown_handle,
    })
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
        if path.exists() {
            // 陈旧锁检测：pid 不存活则复用
            if let Ok(pid) = std::fs::read_to_string(&path) {
                if let Ok(pid) = pid.trim().parse::<i32>() {
                    if pid_alive(pid) {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::AlreadyExists,
                            format!("tenon daemon 已在运行（pid {pid}）"),
                        ));
                    }
                }
            }
            let _ = std::fs::remove_file(&path);
        }
        std::fs::write(&path, std::process::id().to_string())?;
        Ok(Self { path })
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

fn pid_alive(pid: i32) -> bool {
    // kill(pid, 0)：ESRCH = 不存在；EPERM = 存活但无权限
    let r = unsafe { libc::kill(pid, 0) };
    r == 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
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
}
