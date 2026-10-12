//! v2.35 恢复双语义 API 集成测试（§9.1）：终态暂停 / 空闲态的 resume 此前是死命令
//! ——ControlCommand 入队后任务循环已退出、无人消费，UI「恢复」钮点击毫无效果。
//! 现约定：运行态挂起（is_running）→ 控制命令唤醒（v1.92 语义不变）；否则 →
//! 转为「续跑」，复用发送链路以 text（缺省「继续」）开新回合。

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
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap()
}

async fn start_daemon() -> (tempfile::TempDir, u16, String) {
    let dir = tempfile::tempdir().unwrap();
    let mut options = DaemonOptions::in_memory();
    options.providers = vec![Arc::new(MockProvider::new(
        "mock",
        "mock-1",
        vec![ScriptedReply::Text("任务完成".into())],
    ))];
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

/// 注册项目 → 信任 → 建会话，返回 session_id（daemon_tests 全链路样板）。
async fn setup_session(client: &reqwest::Client, port: u16, project: &std::path::Path) -> String {
    let registered: serde_json::Value = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({"path": project.to_string_lossy()}))
        .send()
        .await
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap();
    let project_id = registered["id"].as_str().unwrap().to_string();
    client
        .put(format!("{}/project/trust", base(port)))
        .json(&serde_json::json!({"project_id": project_id, "trusted": true}))
        .send()
        .await
        .unwrap();
    client
        .post(format!("{}/session", base(port)))
        .json(&serde_json::json!({
            "project_path": project.to_string_lossy(),
            "provider": "mock",
            "mode": "auto",
        }))
        .send()
        .await
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap()["session_id"]
        .as_str()
        .unwrap()
        .to_string()
}

async fn control_resume(
    client: &reqwest::Client,
    port: u16,
    sid: &str,
    text: Option<&str>,
) -> (u16, serde_json::Value) {
    let mut body = serde_json::json!({"action": "resume"});
    if let Some(t) = text {
        body["text"] = serde_json::json!(t);
    }
    let r = client
        .post(format!("{base}/session/{sid}/control", base = base(port)))
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = r.status().as_u16();
    (status, r.json::<serde_json::Value>().await.unwrap())
}

async fn wait_done(client: &reqwest::Client, port: u16, sid: &str) {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        assert!(std::time::Instant::now() < deadline, "等待会话 done 超时");
        let s = client
            .get(format!("{base}/session/{sid}", base = base(port)))
            .send()
            .await
            .unwrap()
            .json::<serde_json::Value>()
            .await
            .unwrap();
        if s["status"] == "done" {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn user_input_texts(client: &reqwest::Client, port: u16, sid: &str) -> Vec<String> {
    let trace = client
        .get(format!("{base}/session/{sid}/trace", base = base(port)))
        .send()
        .await
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap();
    trace["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["type"] == "user_input")
        .map(|e| {
            e["payload"]["text"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// 空闲态 resume（此前死命令）→ 转续跑：以附带 text 开新回合并跑完。
#[tokio::test]
async fn resume_on_idle_session_starts_continuation_turn_with_text() {
    let (_dir, port, token) = start_daemon().await;
    let client = client_with_token(&token);
    let project = tempfile::tempdir().unwrap();
    let sid = setup_session(&client, port, project.path()).await;

    let (status, body) = control_resume(&client, port, &sid, Some("续跑继续语")).await;
    assert!(
        (200..300).contains(&status),
        "空闲态 resume 应转续跑成功: {status} {body}"
    );
    wait_done(&client, port, &sid).await;
    let texts = user_input_texts(&client, port, &sid).await;
    assert!(
        texts.iter().any(|t| t == "续跑继续语"),
        "续跑回合应以附带 text 作为用户消息入上下文: {texts:?}"
    );
}

/// 不带 text 的 resume → 缺省「继续」继续语（旧客户端兼容路径同样可续跑）。
#[tokio::test]
async fn resume_without_text_falls_back_to_default_continuation() {
    let (_dir, port, token) = start_daemon().await;
    let client = client_with_token(&token);
    let project = tempfile::tempdir().unwrap();
    let sid = setup_session(&client, port, project.path()).await;

    let (status, body) = control_resume(&client, port, &sid, None).await;
    assert!(
        (200..300).contains(&status),
        "空闲态 resume（无 text）应转续跑成功: {status} {body}"
    );
    wait_done(&client, port, &sid).await;
    let texts = user_input_texts(&client, port, &sid).await;
    assert!(
        texts.iter().any(|t| t == "继续"),
        "缺省继续语应为「继续」: {texts:?}"
    );
}
