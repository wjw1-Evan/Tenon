//! 目录浏览 API 集成测试（§15 v1.133：`GET /fs/dirs`——添加项目对话框浏览器模式
//! 目录选择器的数据源；真 HTTP 全链路）。

use std::sync::Arc;
use std::time::Duration;

use tenon_daemon::{serve, DaemonOptions};
use tenon_models::{MockProvider, ScriptedReply};

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

async fn start_daemon() -> (tempfile::TempDir, u16, String) {
    let dir = tempfile::tempdir().unwrap();
    let mut options = DaemonOptions::in_memory();
    options.providers = vec![Arc::new(MockProvider::new("mock", "mock-1", vec![]))];
    options.default_provider = "mock".into();
    options.endpoint_path = Some(dir.path().join("daemon.endpoint"));
    options.settings_path = Some(dir.path().join("settings.json"));
    let handle = serve(options).await.unwrap();
    (dir, handle.port, handle.token)
}

/// path 缺省 = 用户主目录（canonical 绝对路径，带 parent）。
#[tokio::test]
async fn fs_dirs_defaults_to_home() {
    let (_tmp, port, token) = start_daemon().await;
    let client = client_with_token(&token);
    let r: serde_json::Value = client
        .get(format!("{}/fs/dirs", base(port)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let home = std::fs::canonicalize(std::env::var("HOME").unwrap()).unwrap();
    assert_eq!(r["path"], home.to_string_lossy().as_ref());
    assert_eq!(
        r["parent"],
        home.parent().unwrap().to_string_lossy().as_ref()
    );
    assert!(r["entries"].is_array());
}

/// 只列直接子目录：文件不进、嵌套目录不进、符号链接跟随判定、路径 canonical 化。
#[tokio::test]
async fn fs_dirs_lists_direct_subdirs_only() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("sub-a")).unwrap();
    std::fs::create_dir_all(dir.path().join("sub-a/inner")).unwrap();
    std::fs::write(dir.path().join("f.txt"), "x").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(dir.path().join("sub-a"), dir.path().join("dir_link")).unwrap();

    let (_tmp, port, token) = start_daemon().await;
    let client = client_with_token(&token);
    let canonical = std::fs::canonicalize(dir.path()).unwrap();
    let r: serde_json::Value = client
        .get(format!("{}/fs/dirs", base(port)))
        .query(&[("path", dir.path().to_str().unwrap())])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(r["path"], canonical.to_string_lossy().as_ref());
    assert_eq!(
        r["parent"],
        canonical.parent().unwrap().to_string_lossy().as_ref()
    );
    let names: Vec<&str> = r["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"sub-a"));
    assert!(!names.contains(&"inner"), "嵌套目录不进一层列表");
    assert!(!names.contains(&"f.txt"), "文件不进目录选择器");
    #[cfg(unix)]
    assert!(names.contains(&"dir_link"), "指向目录的符号链接入选");
    // 子目录条目携带可继续浏览的绝对路径
    let sub = r["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == "sub-a")
        .unwrap();
    assert_eq!(
        sub["path"],
        canonical.join("sub-a").to_string_lossy().as_ref()
    );
}

/// 相对路径 / 不存在 / 文件路径均 400。
#[tokio::test]
async fn fs_dirs_rejects_relative_missing_and_file() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("f.txt"), "x").unwrap();

    let (_tmp, port, token) = start_daemon().await;
    let client = client_with_token(&token);
    for (label, path) in [
        ("relative", "some/relative/path"),
        ("missing", "/nonexistent-tenon-fs-dirs-test/xyz"),
        ("file", dir.path().join("f.txt").to_str().unwrap()),
    ] {
        let status = client
            .get(format!("{}/fs/dirs", base(port)))
            .query(&[("path", path)])
            .send()
            .await
            .unwrap()
            .status();
        assert_eq!(status, 400, "{label} 应 400");
    }
}

/// 未携带 token → 401（全局鉴权中间件；局域网未配对同门在 auth 中间件测试覆盖）。
#[tokio::test]
async fn fs_dirs_requires_token() {
    let (_tmp, port, _token) = start_daemon().await;
    let client = reqwest::Client::new();
    let status = client
        .get(format!("{}/fs/dirs", base(port)))
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(status, 401);
}
