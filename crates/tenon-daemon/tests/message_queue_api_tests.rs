//! v1.147 发送消息队列 API 集成测试（§9.1）：运行态入队 / 回合自然完成后 drain /
//! 停止冻结队列 / 队满 409 / 删除排队消息。慢速 provider 撑出确定的「运行中」窗口，
//! 入队判定在 sessions 锁内置 busy，早于首条消息 202 返回，无需竞态重试。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tenon_daemon::{serve, DaemonOptions};
use tenon_models::{
    ChatRequest, ChatResponse, MockProvider, ModelProvider, ProviderResult, ScriptedReply,
    MEMORY_MARKER, TITLE_MARKER,
};

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

/// 首个任务调用延迟返回（撑出「运行中」窗口）；标题 / 记忆提取标记调用与后续调用即时
/// 委托内部脚本化 Mock（loop_last 使后续回合直接得到最终回答）。
struct SlowOnceProvider {
    inner: MockProvider,
    delayed: AtomicBool,
    delay: Duration,
}

#[async_trait::async_trait]
impl ModelProvider for SlowOnceProvider {
    fn name(&self) -> &str {
        "slow-once"
    }

    fn default_model(&self) -> String {
        self.inner.default_model()
    }

    async fn chat(&self, req: &ChatRequest) -> ProviderResult<ChatResponse> {
        let is_meta = req
            .messages
            .iter()
            .any(|m| m.content.contains(TITLE_MARKER) || m.content.contains(MEMORY_MARKER));
        if !is_meta && !self.delayed.swap(true, Ordering::SeqCst) {
            tokio::time::sleep(self.delay).await;
        }
        self.inner.chat(req).await
    }
}

async fn start_daemon_with(provider: Arc<dyn ModelProvider>) -> (tempfile::TempDir, u16, String) {
    let dir = tempfile::tempdir().unwrap();
    let mut options = DaemonOptions::in_memory();
    options.providers = vec![provider];
    options.default_provider = "slow-once".into();
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

/// 注册项目 → 信任 → 建会话，返回 session_id（同 daemon_tests 全链路样板）。
async fn setup_session(client: &reqwest::Client, port: u16, project: &std::path::Path) -> String {
    let registered: serde_json::Value = client
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({"path": project.to_string_lossy()}))
        .send()
        .await
        .unwrap()
        .json()
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
            "provider": "slow-once",
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

async fn send_message(
    client: &reqwest::Client,
    port: u16,
    sid: &str,
    text: &str,
) -> (u16, serde_json::Value) {
    let r = client
        .post(format!("{base}/session/{sid}/message", base = base(port)))
        .json(&serde_json::json!({ "text": text }))
        .send()
        .await
        .unwrap();
    let status = r.status().as_u16();
    (status, r.json::<serde_json::Value>().await.unwrap())
}

async fn get_status(client: &reqwest::Client, port: u16, sid: &str) -> serde_json::Value {
    client
        .get(format!("{base}/session/{sid}", base = base(port)))
        .send()
        .await
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap()
}

/// 轮询直至会话 done 且队列清空（drain 完成）。
async fn wait_done_and_drained(
    client: &reqwest::Client,
    port: u16,
    sid: &str,
) -> serde_json::Value {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let s = get_status(client, port, sid).await;
        if s["status"] == "done" && s["queue"].as_array().is_none_or(|q| q.is_empty()) {
            return s;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "会话未在时限内完成 drain：{s}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn user_input_texts(client: &reqwest::Client, port: u16, sid: &str) -> Vec<String> {
    client
        .get(format!(
            "{base}/session/{sid}/trace?after=0",
            base = base(port)
        ))
        .send()
        .await
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap()["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["type"] == "user_input")
        .map(|e| e["payload"]["text"].as_str().unwrap_or("").to_string())
        .collect()
}

#[tokio::test]
async fn running_message_enqueues_then_drains_after_done() {
    let provider = Arc::new(SlowOnceProvider {
        inner: MockProvider::new(
            "slow-once",
            "mock-1",
            vec![ScriptedReply::Text("第一回合完成".into())],
        ),
        delayed: AtomicBool::new(false),
        delay: Duration::from_millis(1200),
    });
    let (_dir, port, token) = start_daemon_with(provider).await;
    let client = client_with_token(&token);
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("proj");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join("README.md"), "# demo\n").unwrap();
    let sid = setup_session(&client, port, &project).await;

    // 直发首条：busy 在 202 前已置位
    let (code, body) = send_message(&client, port, &sid, "任务一").await;
    assert_eq!(code, 202);
    assert_eq!(body["queued"], false);

    // 运行态发送 → 入队（202 + queued/position）
    let (code, body) = send_message(&client, port, &sid, "任务二").await;
    assert_eq!(code, 202, "运行态发送应入队而非拒绝：{body}");
    assert_eq!(body["queued"], true);
    assert_eq!(body["position"], 1);

    // 状态响应携带队列快照（多窗口一致）
    let s = get_status(&client, port, &sid).await;
    assert_eq!(
        s["queue"],
        serde_json::json!([{ "id": "q1", "text": "任务二" }])
    );

    // 删除排队消息（编辑 = 移除后重发）；再入队补位
    let r = client
        .delete(format!("{base}/session/{sid}/queue/q1", base = base(port)))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(
        get_status(&client, port, &sid).await["queue"],
        serde_json::json!([])
    );
    // 移除不存在的条目 → 404
    let r = client
        .delete(format!("{base}/session/{sid}/queue/q1", base = base(port)))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404);

    let (code, body) = send_message(&client, port, &sid, "任务三").await;
    assert_eq!(code, 202);
    assert_eq!(body["queued"], true, "仍在运行态（任务一未完成）：{body}");
    assert_eq!(body["position"], 1);

    // 回合自然完成 → 自动出队「任务三」续跑，完成后队列清空
    wait_done_and_drained(&client, port, &sid).await;
    assert_eq!(
        user_input_texts(&client, port, &sid).await,
        vec!["任务一".to_string(), "任务三".to_string()],
        "被删除的「任务二」不得入线程，出队顺序 FIFO"
    );
}

#[tokio::test]
async fn queue_full_returns_409_and_delete_frees_slot() {
    let provider = Arc::new(SlowOnceProvider {
        inner: MockProvider::new(
            "slow-once",
            "mock-1",
            vec![ScriptedReply::Text("完成".into())],
        ),
        delayed: AtomicBool::new(false),
        delay: Duration::from_millis(800),
    });
    let (_dir, port, token) = start_daemon_with(provider).await;
    let client = client_with_token(&token);
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("proj");
    std::fs::create_dir_all(&project).unwrap();
    let sid = setup_session(&client, port, &project).await;

    let (code, _) = send_message(&client, port, &sid, "任务〇").await;
    assert_eq!(code, 202);

    // 排满 10 条；第 11 条 409
    for i in 1..=10 {
        let (code, body) = send_message(&client, port, &sid, &format!("任务{i}")).await;
        assert_eq!(code, 202, "第 {i} 条应入队：{body}");
        assert_eq!(body["position"], i);
    }
    let (code, _) = send_message(&client, port, &sid, "任务11").await;
    assert_eq!(code, 409, "超出上限应 409");

    // 删除一条释放位次；新消息补位
    let r = client
        .delete(format!("{base}/session/{sid}/queue/q1", base = base(port)))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let (code, body) = send_message(&client, port, &sid, "任务11").await;
    assert_eq!(code, 202, "删除后应可入队：{body}");
    assert_eq!(body["position"], 10);

    // 11 个回合逐条 drain（首回合延迟 + 10 个即时回合）
    wait_done_and_drained(&client, port, &sid).await;
    let texts = user_input_texts(&client, port, &sid).await;
    assert_eq!(texts.len(), 11);
    assert_eq!(texts[0], "任务〇");
    assert_eq!(texts[1], "任务2", "q1（任务1）被删除，FIFO 顺延");
    assert_eq!(texts[10], "任务11");
}

#[tokio::test]
async fn stop_freezes_queue_then_manual_send_resumes_drain() {
    // 首回合带工具调用：停止在工具检查点生效（v1.93 真挂起 / 退出路径）
    let provider = Arc::new(SlowOnceProvider {
        inner: MockProvider::new(
            "slow-once",
            "mock-1",
            vec![
                ScriptedReply::Tool {
                    name: "read_file".into(),
                    args: serde_json::json!({"path": "README.md"}),
                },
                ScriptedReply::Text("第一回合完成".into()),
            ],
        ),
        delayed: AtomicBool::new(false),
        delay: Duration::from_millis(1200),
    });
    let (_dir, port, token) = start_daemon_with(provider).await;
    let client = client_with_token(&token);
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("proj");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join("README.md"), "# demo\n").unwrap();
    let sid = setup_session(&client, port, &project).await;

    let (code, _) = send_message(&client, port, &sid, "任务一").await;
    assert_eq!(code, 202);
    let (code, body) = send_message(&client, port, &sid, "任务二").await;
    assert_eq!(code, 202);
    assert_eq!(body["queued"], true);

    // 停止：任务在工具检查点退出（Paused），队列冻结不清空
    client
        .post(format!("{base}/session/{sid}/control", base = base(port)))
        .json(&serde_json::json!({ "action": "stop" }))
        .send()
        .await
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    loop {
        let s = get_status(&client, port, &sid).await;
        if s["status"] == "paused" {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "停止未在时限内生效：{s}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    // drain 收尾（busy 清除）与 paused 状态可见存在微小窗口，留出余量后断言队列仍在
    tokio::time::sleep(Duration::from_millis(300)).await;
    let s = get_status(&client, port, &sid).await;
    assert_eq!(
        s["queue"],
        serde_json::json!([{ "id": "q1", "text": "任务二" }]),
        "停止后队列必须冻结保留（手动续发语义）"
    );

    // 手动续发新消息（直发，不入队）→ 完成后队列自动出队「任务二」
    let (code, body) = send_message(&client, port, &sid, "任务三").await;
    assert_eq!(code, 202, "空闲态发送应直发：{body}");
    assert_eq!(body["queued"], false);
    wait_done_and_drained(&client, port, &sid).await;
    assert_eq!(
        user_input_texts(&client, port, &sid).await,
        vec![
            "任务一".to_string(),
            "任务三".to_string(),
            "任务二".to_string()
        ],
        "冻结的「任务二」在手动续发回合完成后按 FIFO 出队"
    );
}
