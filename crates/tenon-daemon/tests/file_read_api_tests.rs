//! v2.41 目录读取友好错误——`GET /project/:id/file` 路由面回归（§8.1）：
//! 目录路径此前以裸「Is a directory (os error 21)」400 直达 UI，现返回
//! 「路径是目录」专用错误；文件读取与缺失路径行为不回归。

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

async fn start_daemon() -> (u16, String) {
    let dir = tempfile::tempdir().unwrap();
    let mut options = DaemonOptions::in_memory();
    options.providers = vec![Arc::new(MockProvider::new("mock", "mock-1", vec![]))];
    options.default_provider = "mock".into();
    options.endpoint_path = Some(dir.path().join("daemon.endpoint"));
    options.settings_path = Some(dir.path().join("settings.json"));
    let handle = serve(options).await.unwrap();
    (handle.port, handle.token)
}

/// 注册临时项目（内含一个文件与一个子目录），返回 (tempdir, project_id)。
async fn setup_project(client: &reqwest::Client, port: u16) -> (tempfile::TempDir, String) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "hello\n").unwrap();
    std::fs::create_dir(dir.path().join("sub")).unwrap();
    let registered: serde_json::Value = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({"path": dir.path().to_string_lossy()}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    (dir, registered["id"].as_str().unwrap().to_string())
}

/// 普通文件照常 200 全量读取（回归保护：判目录不得误伤普通文件）。
#[tokio::test]
async fn file_api_reads_file() {
    let (port, token) = start_daemon().await;
    let client = client_with_token(&token);
    let (_dir, project_id) = setup_project(&client, port).await;
    let r: serde_json::Value = client
        .get(format!("{}/project/{project_id}/file", base(port)))
        .query(&[("path", "a.txt")])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(r["content"], "hello\n");
    assert_eq!(r["total_bytes"], serde_json::json!(6));
}

/// 目录 → 400 且文案友好：「路径是目录」，不冒裸 os error。
#[tokio::test]
async fn file_api_directory_returns_friendly_error() {
    let (port, token) = start_daemon().await;
    let client = client_with_token(&token);
    let (_dir, project_id) = setup_project(&client, port).await;
    let resp = client
        .get(format!("{}/project/{project_id}/file", base(port)))
        .query(&[("path", "sub")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let msg = resp.json::<serde_json::Value>().await.unwrap()["error"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(msg.contains("路径是目录"), "{msg}");
    assert!(!msg.contains("os error"), "{msg}");
}

/// 缺失路径维持 400（NotFound 语义不回归，文案不在本次改动范围）。
#[tokio::test]
async fn file_api_missing_path_400() {
    let (port, token) = start_daemon().await;
    let client = client_with_token(&token);
    let (_dir, project_id) = setup_project(&client, port).await;
    let status = client
        .get(format!("{}/project/{project_id}/file", base(port)))
        .query(&[("path", "nope.txt")])
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(status, 400);
}
