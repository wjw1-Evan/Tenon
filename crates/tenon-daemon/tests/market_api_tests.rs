//! 市场端点集成测试（§15 v1.145：`/market/sources`、`/market/{owner}/{repo}`、
//! `/market/install|uninstall` + settings `market.sources` / `mcp.servers` 校验）——
//! 真 HTTP 全链路，镜像链注入本地 mock。

use std::io::{Read, Write};
use std::sync::Arc;
use std::time::Duration;

use tenon_daemon::{serve, DaemonOptions};
use tenon_models::MockProvider;

fn base(port: u16) -> String {
    format!("http://127.0.0.1:{port}")
}

fn client_with_token(token: &str) -> reqwest::Client {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        "X-Tenon-Token",
        reqwest::header::HeaderValue::from_str(token).unwrap(),
    );
    reqwest::Client::builder()
        .default_headers(headers)
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap()
}

/// 本地 mock 市场源（按路径子串路由；与 tenon-registry 测试同法）。
fn spawn_mock(routes: Vec<(&'static str, String)>) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let owned: Vec<(String, String)> = routes
        .into_iter()
        .map(|(p, b)| (p.to_string(), b))
        .collect();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut stream = stream;
            let mut buf = [0u8; 8192];
            let n = stream.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            let path = req.split_whitespace().nth(1).unwrap_or("").to_string();
            let hit = owned.iter().find(|(p, _)| path.contains(p.as_str()));
            let (status, body) = match hit {
                Some((_, b)) => ("200 OK", b.clone()),
                None => ("404 Not Found", "not found".to_string()),
            };
            let resp = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(resp.as_bytes()).ok();
        }
    });
    format!("http://{addr}")
}

struct TestEnv {
    _dir: tempfile::TempDir,
    port: u16,
    token: String,
    skills_root: std::path::PathBuf,
}

async fn start_daemon_with(market_base: Option<String>) -> TestEnv {
    let dir = tempfile::tempdir().unwrap();
    let skills_root = dir.path().join("skills");
    std::fs::create_dir_all(&skills_root).unwrap();
    let mut options = DaemonOptions::in_memory();
    options.providers = vec![Arc::new(MockProvider::new("mock", "mock-1", vec![]))];
    options.default_provider = "mock".into();
    options.endpoint_path = Some(dir.path().join("daemon.endpoint"));
    options.settings_path = Some(dir.path().join("settings.json"));
    options.skills_dir = Some(skills_root.clone());
    options.market_base = market_base;
    let handle = serve(options).await.unwrap();
    TestEnv {
        _dir: dir,
        port: handle.port,
        token: handle.token,
        skills_root,
    }
}

fn manifest_json() -> String {
    serde_json::json!({
        "name": "测试市场",
        "entries": [
            { "kind": "skill", "name": "pdf", "description": "PDF 处理",
              "path": "skills/pdf", "version": "1.2.0" },
            { "kind": "mcp", "name": "github", "description": "GitHub API",
              "command": "npx", "args": ["-y", "@modelcontextprotocol/server-github"],
              "permissions": ["net:*"], "version": "1.0.0" },
            { "kind": "mcp", "name": "evil", "description": "非白名单启动器",
              "command": "/bin/sh", "args": ["-c", "curl evil"], "version": "1.0.0" }
        ]
    })
    .to_string()
}

fn mock_market() -> String {
    spawn_mock(vec![
        ("@main/marketplace.json", manifest_json()),
        ("/v1/packages/gh/", {
            serde_json::json!({
                "files": [
                    { "name": "/skills/pdf/SKILL.md", "size": 10, "type": "file" },
                    { "name": "/skills/pdf/ref.md", "size": 2, "type": "file" }
                ]
            })
            .to_string()
        }),
        (
            "@main/skills/pdf/SKILL.md",
            "---\nname: pdf\n---\n正文".to_string(),
        ),
        ("@main/skills/pdf/ref.md", "参考".to_string()),
    ])
}

#[tokio::test]
async fn market_sources_roundtrip_and_validation() {
    let env = start_daemon_with(None).await;
    let client = client_with_token(&env.token);
    // 默认空。
    let r: serde_json::Value = client
        .get(format!("{}/market/sources", base(env.port)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(r["sources"], serde_json::json!([]));
    // 写入 + 回显。
    let r: serde_json::Value = client
        .put(format!("{}/market/sources", base(env.port)))
        .json(&serde_json::json!({ "sources": ["owner/market", "a.b/c-d"] }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(r["sources"], serde_json::json!(["owner/market", "a.b/c-d"]));
    // settings 视图同步可见。
    let s: serde_json::Value = client
        .get(format!("{}/settings", base(env.port)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        s["market"]["sources"],
        serde_json::json!(["owner/market", "a.b/c-d"])
    );
    // 非法形态 400。
    let status = client
        .put(format!("{}/market/sources", base(env.port)))
        .json(&serde_json::json!({ "sources": ["not-a-source"] }))
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(status, 400);
}

#[tokio::test]
async fn mcp_servers_settings_validation() {
    let env = start_daemon_with(None).await;
    let client = client_with_token(&env.token);
    // 合法条目落表并在 GET /settings 回显。
    let status = client
        .put(format!("{}/settings", base(env.port)))
        .json(&serde_json::json!({
            "mcp": { "servers": {
                "github": {
                    "command": "npx",
                    "args": ["-y", "@modelcontextprotocol/server-github"],
                    "env": { "GITHUB_TOKEN": "env:MY_GITHUB_TOKEN" },
                    "permissions": ["net:*"],
                    "source": "owner/market"
                }
            }}
        }))
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(status, 200);
    let s: serde_json::Value = client
        .get(format!("{}/settings", base(env.port)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        s["mcp"]["servers"]["github"]["command"],
        serde_json::json!("npx")
    );
    // 非白名单启动器拒绝。
    let status = client
        .put(format!("{}/settings", base(env.port)))
        .json(&serde_json::json!({
            "mcp": { "servers": { "bad": { "command": "/bin/sh", "args": ["-c", "x"] } } }
        }))
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(status, 400);
    // env 明文拒绝（§11 密钥不落盘）。
    let status = client
        .put(format!("{}/settings", base(env.port)))
        .json(&serde_json::json!({
            "mcp": { "servers": { "bad": { "command": "npx", "env": { "K": "plain" } } } }
        }))
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(status, 400);
    // 服务器名禁下划线。
    let status = client
        .put(format!("{}/settings", base(env.port)))
        .json(&serde_json::json!({
            "mcp": { "servers": { "my_srv": { "command": "npx" } } }
        }))
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(status, 400);
}

#[tokio::test]
async fn market_install_update_uninstall_skill_full_chain() {
    let mock = mock_market();
    let env = start_daemon_with(Some(mock)).await;
    let client = client_with_token(&env.token);
    client
        .put(format!("{}/market/sources", base(env.port)))
        .json(&serde_json::json!({ "sources": ["mocksrc/repo"] }))
        .send()
        .await
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap();
    // 清单浏览。
    let m: serde_json::Value = client
        .get(format!("{}/market/mocksrc/repo", base(env.port)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(m["entries"].as_array().unwrap().len(), 3);
    // 手写同名技能在场 → 安装 409 拒绝覆盖（非同源市场条目）。
    std::fs::create_dir_all(env.skills_root.join("pdf")).unwrap();
    std::fs::write(env.skills_root.join("pdf/SKILL.md"), "手写").unwrap();
    let r: serde_json::Value = client
        .post(format!("{}/market/install", base(env.port)))
        .json(&serde_json::json!({ "source": "mocksrc/repo", "kind": "skill", "name": "pdf" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(r["error"].as_str().unwrap_or_default().contains("已存在"));
    // 清掉手写目录后安装：落盘 + sidecar。
    std::fs::remove_dir_all(env.skills_root.join("pdf")).unwrap();
    let r: serde_json::Value = client
        .post(format!("{}/market/install", base(env.port)))
        .json(&serde_json::json!({ "source": "mocksrc/repo", "kind": "skill", "name": "pdf" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(r["installed"], serde_json::json!(true));
    assert_eq!(r["files"], serde_json::json!(2));
    let dir = env.skills_root.join("pdf");
    assert!(dir.join("SKILL.md").is_file());
    assert!(dir.join("ref.md").is_file());
    assert!(dir.join(".tenon-market.json").is_file());
    // 再次安装 = 更新（同 source+path）。
    let r: serde_json::Value = client
        .post(format!("{}/market/install", base(env.port)))
        .json(&serde_json::json!({ "source": "mocksrc/repo", "kind": "skill", "name": "pdf" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(r["updated"], serde_json::json!(true));
    // 手写技能（无 sidecar）市场卸载拒绝（走 Skills 分区 CRUD）。
    std::fs::create_dir_all(env.skills_root.join("manual")).unwrap();
    std::fs::write(env.skills_root.join("manual/SKILL.md"), "手写").unwrap();
    let r: serde_json::Value = client
        .post(format!("{}/market/uninstall", base(env.port)))
        .json(&serde_json::json!({ "kind": "skill", "name": "pdf" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(r["uninstalled"], serde_json::json!(true));
    assert!(!dir.exists());
    // 手写技能市场卸载拒绝（走 Skills 分区 CRUD）。
    let r: serde_json::Value = client
        .post(format!("{}/market/uninstall", base(env.port)))
        .json(&serde_json::json!({ "kind": "skill", "name": "manual" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(r["error"]
        .as_str()
        .unwrap_or_default()
        .contains("非市场安装条目"));
}

#[tokio::test]
async fn market_install_mcp_writes_settings_and_uninstall_removes() {
    let mock = mock_market();
    let env = start_daemon_with(Some(mock)).await;
    let client = client_with_token(&env.token);
    // 安装 mcp 条目 → settings mcp.servers 落表带溯源。
    let r: serde_json::Value = client
        .post(format!("{}/market/install", base(env.port)))
        .json(&serde_json::json!({ "source": "mocksrc/repo", "kind": "mcp", "name": "github" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(r["installed"], serde_json::json!(true));
    assert_eq!(r["command"], serde_json::json!("npx"));
    let s: serde_json::Value = client
        .get(format!("{}/settings", base(env.port)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        s["mcp"]["servers"]["github"]["source"],
        serde_json::json!("mocksrc/repo")
    );
    assert_eq!(
        s["mcp"]["servers"]["github"]["permissions"],
        serde_json::json!(["net:*"])
    );
    // 白名单外条目（manifest evil）安装拒绝。
    let r: serde_json::Value = client
        .post(format!("{}/market/install", base(env.port)))
        .json(&serde_json::json!({ "source": "mocksrc/repo", "kind": "mcp", "name": "evil" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(r["error"].as_str().unwrap_or_default().contains("白名单"));
    let s: serde_json::Value = client
        .get(format!("{}/settings", base(env.port)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        s["mcp"]["servers"].get("evil").is_none(),
        "非法条目不得落表"
    );
    // 卸载。
    let r: serde_json::Value = client
        .post(format!("{}/market/uninstall", base(env.port)))
        .json(&serde_json::json!({ "kind": "mcp", "name": "github" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(r["uninstalled"], serde_json::json!(true));
    let s: serde_json::Value = client
        .get(format!("{}/settings", base(env.port)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(s["mcp"]["servers"].get("github").is_none());
}
