//! L5 对话记忆 API 集成测试（§15 v1.104：GET / POST `/project/:id/memories`
//! + DELETE `/memories/:id`；真 HTTP 全链路）。

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
        // 高负载（并行构建）下 /projects/open 触发 L4 初始索引可能偏慢
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap()
}

async fn start_daemon(script: Vec<ScriptedReply>) -> (tempfile::TempDir, u16, String) {
    let dir = tempfile::tempdir().unwrap();
    let mut options = DaemonOptions::in_memory();
    options.providers = vec![Arc::new(MockProvider::new("mock", "mock-1", script))];
    options.default_provider = "mock".into();
    options.snapshots_root = Some(dir.path().join("snapshots"));
    options.worktrees_root = Some(dir.path().join("worktrees"));
    options.endpoint_path = Some(dir.path().join("daemon.endpoint"));
    options.settings_path = Some(dir.path().join("settings.json"));
    options.policy_path = Some(dir.path().join("policy.toml"));
    options.updates_staging_dir = Some(dir.path().join("updates/staged"));
    options.laya_models_dir = Some(dir.path().join("models/laya"));
    let handle = serve(options).await.unwrap();
    (dir, handle.port, handle.token)
}

async fn open_project(client: &reqwest::Client, port: u16, path: &str) -> String {
    let opened: serde_json::Value = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({"path": path}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    opened["id"].as_str().unwrap().to_string()
}

/// v1.104：手动写入 → 列表（项目层 + global preference 跨项目可见）→
/// q 过滤 → global 非 preference 400 → 删除 → 404。
#[tokio::test]
async fn memories_crud_and_scope_visibility() {
    let dir = tempfile::tempdir().unwrap();
    let project_a = dir.path().join("proj-a");
    let project_b = dir.path().join("proj-b");
    std::fs::create_dir_all(&project_a).unwrap();
    std::fs::create_dir_all(&project_b).unwrap();

    let (_tmp, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);
    let pid_a = open_project(&client, port, &project_a.to_string_lossy()).await;
    let pid_b = open_project(&client, port, &project_b.to_string_lossy()).await;

    // 项目层写入
    let created: serde_json::Value = client
        .post(format!("{}/project/{pid_a}/memories", base(port)))
        .json(&serde_json::json!({"content": "本项目跑 cargo test 验证", "kind": "workflow"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(created["merged"], false);
    assert_eq!(created["memory"]["kind"], "workflow");
    let mem_id = created["memory"]["id"].as_str().unwrap().to_string();

    // global preference 写入
    let global: serde_json::Value = client
        .post(format!("{}/project/{pid_a}/memories", base(port)))
        .json(
            &serde_json::json!({"content": "回复用中文", "kind": "preference", "scope": "global"}),
        )
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(global["memory"]["scope"], "global");

    // global + 非 preference 组合 → 400
    let status = client
        .post(format!("{}/project/{pid_a}/memories", base(port)))
        .json(&serde_json::json!({"content": "非法组合", "kind": "fact", "scope": "global"}))
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(status, 400);

    // A 项目列表 = 项目层 + global preference
    let list_a: serde_json::Value = client
        .get(format!("{}/project/{pid_a}/memories", base(port)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(list_a["memories"].as_array().unwrap().len(), 2);

    // B 项目只见 global preference
    let list_b: serde_json::Value = client
        .get(format!("{}/project/{pid_b}/memories", base(port)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let b_items = list_b["memories"].as_array().unwrap();
    assert_eq!(b_items.len(), 1);
    assert_eq!(b_items[0]["scope"], "global");

    // q 过滤
    let filtered: serde_json::Value = client
        .get(format!("{}/project/{pid_a}/memories?q=cargo", base(port)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(filtered["memories"].as_array().unwrap().len(), 1);

    // 未知项目 404
    let status = client
        .get(format!("{}/project/nonexistent/memories", base(port)))
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(status, 404);

    // 删除 → 再删 404
    let status = client
        .delete(format!("{}/memories/{mem_id}", base(port)))
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(status, 200);
    let status = client
        .delete(format!("{}/memories/{mem_id}", base(port)))
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(status, 404);
}

/// v1.104：同义记忆经 API 写入命中去重合并（merged=true，不新增行）。
#[tokio::test]
async fn memories_api_dedupes_similar_content() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("proj-dedupe");
    std::fs::create_dir_all(&project).unwrap();

    let (_tmp, port, token) = start_daemon(vec![]).await;
    let client = client_with_token(&token);
    let pid = open_project(&client, port, &project.to_string_lossy()).await;

    let first: serde_json::Value = client
        .post(format!("{}/project/{pid}/memories", base(port)))
        .json(&serde_json::json!({"content": "代码注释用中文书写", "kind": "preference"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(first["merged"], false);

    let second: serde_json::Value = client
        .post(format!("{}/project/{pid}/memories", base(port)))
        .json(&serde_json::json!({"content": "代码注释用中文书写 永远", "kind": "preference"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        second["merged"], true,
        "同义内容合并到既有条目（本地 embedding 余弦去重）"
    );
    assert_eq!(second["memory"]["id"], first["memory"]["id"]);

    let list: serde_json::Value = client
        .get(format!("{}/project/{pid}/memories", base(port)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(list["memories"].as_array().unwrap().len(), 1);
}
