//! daemon 可执行入口：绑定 127.0.0.1 随机端口，握手信息以 JSON 行输出
//! （供 Tauri sidecar / 浏览器模式读取，§6.2）。
//!
//! 用法：`tenon-daemon [--db <path>] [--project <path>] [--provider <name>]`
//!       `[--port <n>] [--token <t>]`（后两项开发热重载专用：固定端口 / token）
//!       `[--web]`（Web 一键启动 v1.76：UI 产物预检 + URL 直出 + 自动开浏览器；
//!       已有实例存活时复用 endpoint 直接打开浏览器后退出）

use tenon_config::Config;
use tenon_daemon::{serve, DaemonOptions, InstanceLock};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 日志走 stderr：stdout 仅承载一行握手 JSON（sidecar 逐行解析契约，§6.2）
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "tenon_daemon=info".into()),
        )
        .with_writer(std::io::stderr)
        .init();

    let mut db_path = None;
    let mut project = None;
    let mut provider = String::new();
    let mut generate_keys = false;
    let mut no_lock = false;
    let mut config_path: Option<std::path::PathBuf> = None;
    let mut settings_path: Option<std::path::PathBuf> = None;
    let mut policy_path: Option<std::path::PathBuf> = None;
    let mut lan_bind = false;
    let mut bind_port: Option<u16> = None;
    let mut fixed_token: Option<String> = None;
    let mut web = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--db" => db_path = args.next().map(std::path::PathBuf::from),
            "--project" => project = args.next(),
            "--provider" => provider = args.next().unwrap_or_default(),
            "--config" => config_path = args.next().map(std::path::PathBuf::from),
            "--settings" => settings_path = args.next().map(std::path::PathBuf::from),
            "--policy" => policy_path = args.next().map(std::path::PathBuf::from),
            "--generate-keys" => generate_keys = true,
            "--no-lock" => no_lock = true,
            "--lan" => lan_bind = true,
            "--web" => web = true,
            // 开发热重载：UI DEV 回落约定 127.0.0.1:9876 + token "dev"
            "--port" => match args.next().and_then(|v| v.parse::<u16>().ok()) {
                Some(p) => bind_port = Some(p),
                None => {
                    eprintln!("--port 需要端口号");
                    std::process::exit(2);
                }
            },
            "--token" => match args.next() {
                Some(t) => fixed_token = Some(t),
                None => {
                    eprintln!("--token 需要 token 值");
                    std::process::exit(2);
                }
            },
            other => {
                eprintln!("未知参数: {other}");
                std::process::exit(2);
            }
        }
    }

    // 签名与 updater 密钥（§17 M0：生成 ed25519 密钥对，供插件 / 更新签名）
    if generate_keys {
        let dir = Config::data_dir().join("keys");
        let (sec_hex, pub_hex) = tenon_daemon::generate_signing_keypair()?;
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join("signing.sec"), sec_hex)?;
        std::fs::write(dir.join("signing.pub"), pub_hex)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(
                dir.join("signing.sec"),
                std::fs::Permissions::from_mode(0o600),
            );
        }
        println!("签名密钥已生成：{}", dir.join("signing.pub").display());
        return Ok(());
    }

    // --web 预检（v1.76）：UI 产物解析链必须命中，否则报错退出并给修复指引；
    // 普通模式（sidecar / 无头）不需要 UI，维持「未命中静默跳过」
    if web && tenon_daemon::resolve_ui_dist().is_none() {
        eprintln!("--web 需要 UI 构建产物：先 pnpm ui:build，或用 TENON_UI_DIST=<路径> 指定");
        std::process::exit(2);
    }

    // 单实例锁（§6.2；测试 / 特殊部署可 --no-lock）
    let endpoint_path = Config::data_dir().join("daemon.endpoint");
    let _lock = if no_lock {
        None
    } else {
        match InstanceLock::acquire(Config::data_dir().join("daemon.lock")) {
            Ok(l) => Some(l),
            Err(e) if web => {
                // 已有实例存活（v1.76）：endpoint 心跳新鲜则直接打开其 Web 后退出——
                // 重复执行 --web 等价于「打开」而不是报错
                if let Some((port, _)) = tenon_daemon::read_live_endpoint(&endpoint_path) {
                    let url = format!("http://127.0.0.1:{port}/");
                    eprintln!("daemon 已在运行；Tenon Web → {url}");
                    tenon_daemon::open_browser(&url);
                    return Ok(());
                }
                return Err(anyhow::anyhow!(
                    "--web 启动失败：{e}，且无存活的 daemon endpoint"
                ));
            }
            Err(e) => return Err(anyhow::anyhow!("无法获取单实例锁: {e}")),
        }
    };

    // 配置发现链（v1.140 附录 E）：显式 --config > ~/.tenon/config.toml >
    // ~/.tenon/config.local.toml（开发期回退）> 内置默认。解析失败告警后回退
    // 默认不静默吞——坏配置比空 provider（模型选择器呈「无」）更需要暴露。
    let config = match config_path {
        Some(p) => match Config::parse_toml_file(&p) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("--config {} 解析失败，回退内置默认：{e}", p.display());
                Config::default()
            }
        },
        None => match Config::discover_global() {
            Ok((c, Some(path))) => {
                tracing::info!("配置：{}", path.display());
                c
            }
            Ok((c, None)) => {
                tracing::warn!(
                    "未发现 ~/.tenon/config.toml 或 config.local.toml，使用内置默认（模型 provider 为空）"
                );
                c
            }
            Err(e) => {
                tracing::warn!("全局配置解析失败，回退内置默认：{e}");
                Config::default()
            }
        },
    };
    let options = DaemonOptions {
        // 默认持久库 ~/.tenon/db.sqlite（§14.1）；--db 显式覆盖；None 仅测试用
        db_path: db_path.or_else(|| Some(Config::data_dir().join("db.sqlite"))),
        lan_bind,
        project: project.clone(),
        config,
        providers: vec![],
        default_provider: provider,
        snapshots_root: None,
        worktrees_root: None,
        endpoint_path: None,
        settings_path,
        policy_path,
        updates_staging_dir: None,
        bind_port,
        fixed_token,
        laya_registry_url: None,
        laya_public_key: None,
        laya_models_dir: None,
        skills_dir: None,
        market_base: None,
        watch_poll_interval: None,
    };
    if let Some(proj) = project {
        let mut store = tenon_store::Store::open(
            &options
                .db_path
                .clone()
                .unwrap_or_else(|| Config::data_dir().join("db.sqlite")),
        )?;
        let p = store.upsert_project(&proj)?;
        eprintln!("project registered: {} ({})", p.id, p.path);
    }

    let handle = serve(options).await?;
    if web {
        // URL 直出 stderr（stdout 仅握手 JSON 行，sidecar 契约不变）并自动开浏览器
        let url = format!("http://127.0.0.1:{}/", handle.port);
        eprintln!("Tenon Web → {url}");
        tenon_daemon::open_browser(&url);
    }
    // 握手行：UI / sidecar 解析此行获取端口与 token
    println!(
        "{}",
        serde_json::json!({
            "tenon": 1,
            "port": handle.port,
            "token": handle.token,
        })
    );
    tokio::signal::ctrl_c().await?;
    Ok(())
}
