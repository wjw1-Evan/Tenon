//! daemon 可执行入口：绑定 127.0.0.1 随机端口，握手信息以 JSON 行输出
//! （供 Tauri sidecar / 浏览器模式读取，§6.2）。
//!
//! 用法：`tenon-daemon [--db <path>] [--project <path>] [--provider <name>]`

use tenon_config::Config;
use tenon_daemon::{serve, DaemonOptions, InstanceLock};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "tenon_daemon=info".into()),
        )
        .init();

    let mut db_path = None;
    let mut project = None;
    let mut provider = String::new();
    let mut generate_keys = false;
    let mut no_lock = false;
    let mut config_path: Option<std::path::PathBuf> = None;
    let mut lan_bind = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--db" => db_path = args.next().map(std::path::PathBuf::from),
            "--project" => project = args.next(),
            "--provider" => provider = args.next().unwrap_or_default(),
            "--config" => config_path = args.next().map(std::path::PathBuf::from),
            "--generate-keys" => generate_keys = true,
            "--no-lock" => no_lock = true,
            "--lan" => lan_bind = true,
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

    // 单实例锁（§6.2；测试 / 特殊部署可 --no-lock）
    let _lock = if no_lock {
        None
    } else {
        Some(
            InstanceLock::acquire(Config::data_dir().join("daemon.lock"))
                .map_err(|e| anyhow::anyhow!("无法获取单实例锁: {e}"))?,
        )
    };

    let config = match config_path {
        Some(p) => Config::parse_toml_file(&p).unwrap_or_default(),
        None => Config::load_global().unwrap_or_default(),
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
