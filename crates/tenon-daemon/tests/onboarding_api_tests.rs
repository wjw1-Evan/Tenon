//! 免费模型引导 API 集成测试（§15 v1.163 / v1.165）：provider `api_key` 直存
//! settings.json 的持久化 / 回显剥离 / 缺席保留语义、`POST /models/verify`
//! 对本地 mock 上游的试连闭环（含 GLM thinking 翻译在真实 HTTP 链路上的
//! 端到端断言）。

use std::sync::Arc;
use std::time::Duration;

use axum::response::IntoResponse;
use axum::routing::post;
use axum::Json;
use serde_json::{json, Value};
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

async fn start_daemon() -> (tempfile::TempDir, u16, String) {
    let dir = tempfile::tempdir().unwrap();
    let mut options = DaemonOptions::in_memory();
    options.providers = vec![Arc::new(MockProvider::new("mock", "mock-1", vec![]))];
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

/// OpenAI 兼容 mock 上游：校验 Bearer 与 GLM thinking 翻译后按给定状态码回包。
async fn start_mock_upstream(status: u16, captured: Arc<std::sync::Mutex<Option<Value>>>) -> u16 {
    let app = axum::Router::new().route(
        "/chat/completions",
        post(
            move |headers: axum::http::HeaderMap, Json(body): Json<Value>| async move {
                let auth = headers
                    .get(axum::http::header::AUTHORIZATION)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or_default()
                    .to_string();
                *captured.lock().unwrap() = Some(json!({"auth": auth, "body": body}));
                if status == 200 {
                    axum::Json(json!({
                        "choices": [{
                            "finish_reason": "stop",
                            "index": 0,
                            "message": {"content": "pong", "role": "assistant"}
                        }],
                        "model": "glm-4.7-flash",
                        "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
                    }))
                    .into_response()
                } else {
                    (
                        axum::http::StatusCode::from_u16(status).unwrap(),
                        axum::Json(json!({"error": {"message": "invalid api key"}})),
                    )
                        .into_response()
                }
            },
        ),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    port
}

/// v1.165 密钥直存（§11 双轨）：PUT /settings 接受 provider `api_key` 明文 →
/// 持久化 settings.json（0600）→ GET 永不回显；覆盖表整体替换时载荷缺席
/// `api_key` 即保留既有值（UI 无法重发不可回显字段）；整条缺席（删除
/// provider）即随条目消失。
#[tokio::test]
async fn settings_api_key_persists_redacted_and_survives_absent_field() {
    let (tmp, port, token) = start_daemon().await;
    let client = client_with_token(&token);
    let settings_path = tmp.path().join("settings.json");

    let glm = json!({
        "kind": "openai",
        "base_url": "https://open.bigmodel.cn/api/paas/v4",
        "model": "glm-4.7-flash",
        "api_key": "sk-live-secret-xyz"
    });
    let resp = client
        .put(format!("{}/settings", base(port)))
        .json(&json!({"models": {"default": "glm", "providers": {"glm": glm}}}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // 持久化：settings.json 含明文（0600 文件 = 用户本机权威存储）
    let persisted = std::fs::read_to_string(&settings_path).unwrap();
    assert!(
        persisted.contains("sk-live-secret-xyz"),
        "明文应落 settings.json"
    );

    // GET 永不回显密钥
    let view: Value = client
        .get(format!("{}/settings", base(port)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let glm_view = &view["models"]["providers"]["glm"];
    assert!(glm_view.get("api_key").is_none(), "GET 不得回显 api_key");
    assert_eq!(glm_view["overridden"], true);

    // 缺席保留：重发不含 api_key 的全量表（模拟设置面板保存），密钥仍在盘上
    let resp = client
        .put(format!("{}/settings", base(port)))
        .json(&json!({"models": {"default": "glm", "providers": {"glm": {
            "kind": "openai",
            "base_url": "https://open.bigmodel.cn/api/paas/v4",
            "model": "glm-4.7-flash"
        }}}}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let persisted = std::fs::read_to_string(&settings_path).unwrap();
    assert!(
        persisted.contains("sk-live-secret-xyz"),
        "载荷缺席 api_key 应保留既有值"
    );

    // 整条缺席 = 删除 provider，密钥随之消失
    let resp = client
        .put(format!("{}/settings", base(port)))
        .json(&json!({"models": {"default": "", "providers": {}}}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let persisted = std::fs::read_to_string(&settings_path).unwrap();
    assert!(
        !persisted.contains("sk-live-secret-xyz"),
        "删除 provider 应连带清除密钥"
    );
}

/// 试连闭环：GLM 模型名触发 thinking disabled 翻译、Bearer 透传、成功回包。
#[tokio::test]
async fn verify_model_roundtrip_against_mock_upstream() {
    let (_tmp, port, token) = start_daemon().await;
    let client = client_with_token(&token);
    let captured = Arc::new(std::sync::Mutex::new(None));
    let upstream = start_mock_upstream(200, captured.clone()).await;

    let resp = client
        .post(format!("{}/models/verify", base(port)))
        .json(&json!({
            "kind": "openai",
            "base_url": format!("http://127.0.0.1:{upstream}"),
            "model": "glm-4.7-flash",
            "api_key": "sk-test-abc"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["ok"], true);
    assert_eq!(body["model"], "glm-4.7-flash");
    assert!(body["latency_ms"].is_u64());

    // 端到端断言：Bearer 透传 + GLM thinking 翻译生效（reasoning_effort 不透传）
    let captured = captured.lock().unwrap().clone().unwrap();
    assert_eq!(captured["auth"], "Bearer sk-test-abc");
    assert_eq!(captured["body"]["thinking"]["type"], "disabled");
    assert!(captured["body"].get("reasoning_effort").is_none());
    assert_eq!(captured["body"]["max_tokens"], 16);
    // v1.174 回归：verify 用 0.7f32 刻意走 f32→f64 拓宽路径，
    // 原始 body 序列化必须 ≤2 位小数（智谱 1210）
    let raw = serde_json::to_string(&captured["body"]).unwrap();
    assert!(
        raw.contains(r#""temperature":0.7"#),
        "temperature 序列化异常: {raw}"
    );
    assert!(!raw.contains("999999"), "f32→f64 长尾泄漏: {raw}");
}

/// 试连参数校验与上游失败路径：非法 base_url / 空 key → 400；
/// 上游 401 → 400 `{ok:false, error}`，错误正文不吞。
#[tokio::test]
async fn verify_model_rejects_bad_input_and_reports_upstream_error() {
    let (_tmp, port, token) = start_daemon().await;
    let client = client_with_token(&token);

    for body in [
        json!({"base_url": "ftp://x", "model": "m", "api_key": "k"}),
        json!({"base_url": "http://127.0.0.1:1", "model": "", "api_key": "k"}),
        json!({"base_url": "http://127.0.0.1:1", "model": "m", "api_key": ""}),
    ] {
        let resp = client
            .post(format!("{}/models/verify", base(port)))
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 400);
    }

    let upstream = start_mock_upstream(401, Arc::new(std::sync::Mutex::new(None))).await;
    let resp = client
        .post(format!("{}/models/verify", base(port)))
        .json(&json!({
            "base_url": format!("http://127.0.0.1:{upstream}"),
            "model": "glm-4.7-flash",
            "api_key": "wrong"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["ok"], false);
    let error = body["error"].as_str().unwrap();
    assert!(error.contains("401"), "错误应含上游状态码: {error}");
    assert!(
        error.contains("invalid api key"),
        "错误应含上游正文: {error}"
    );
}
