//! 免费模型引导 API 集成测试（§15 v1.163）：`PUT /secrets/{name}` 校验与
//! 钥匙串写入回读、`POST /models/verify` 对本地 mock 上游的试连闭环
//! （含 GLM thinking 翻译在真实 HTTP 链路上的端到端断言）。

use std::sync::Arc;
use std::time::Duration;

use axum::response::IntoResponse;
use axum::routing::post;
use axum::Json;
use serde_json::{json, Value};
use tenon_daemon::{serve, DaemonOptions};
use tenon_models::{KeyStore, MockProvider};

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

/// 密钥名域校验（§15 `^[A-Z][A-Z0-9_]{0,63}$`）与载荷校验，全部 400。
#[tokio::test]
async fn put_secret_rejects_invalid_name_and_payload() {
    let (_tmp, port, token) = start_daemon().await;
    let client = client_with_token(&token);

    // 空名在路由层即不可达（/secrets/ 不匹配 {name} 段），不进用例
    for name in ["abc", "1ABC", "A-B", &"A".repeat(65)] {
        let resp = client
            .put(format!("{}/secrets/{}", base(port), name))
            .json(&json!({"value": "x"}))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 400, "密钥名 {name:?} 应被拒绝");
    }
    // 缺 value / 空 value
    for body in [json!({}), json!({"value": ""})] {
        let resp = client
            .put(format!("{}/secrets/TENON_V163_TEST", base(port)))
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 400);
    }
    // 未带 token 直接 401（凭据写入是高特权操作，必须在鉴权层内）
    let anon = reqwest::Client::new();
    let resp = anon
        .put(format!("{}/secrets/TENON_V163_TEST", base(port)))
        .json(&json!({"value": "x"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

/// 钥匙串写入回读（真实 OS 凭据库；不可用环境跳过，不假失败）。
#[tokio::test]
async fn put_secret_writes_and_reads_back_via_keychain() {
    let (_tmp, port, token) = start_daemon().await;
    let client = client_with_token(&token);
    let name = "TENON_V163_TEST_SECRET";
    let value = "test-key-value-xyz";

    // 环境门：先直连 KeychainStore 往返确认凭据库真的可用——
    // 不能拿 HTTP 500 当「环境不可用」跳过（v1.164：那会掩盖真实回归）
    let gate = tenon_models::KeychainStore::new();
    gate.set("TENON_V163_TEST_GATE", "gate-value");
    let keychain_ok = gate.get("TENON_V163_TEST_GATE").as_deref() == Some("gate-value");
    gate.delete("TENON_V163_TEST_GATE");
    if !keychain_ok {
        eprintln!("跳过：系统凭据库不可用");
        return;
    }

    let resp = client
        .put(format!("{}/secrets/{name}", base(port)))
        .json(&json!({"value": value}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "凭据库可用时 /secrets 必须成功");
    // 回读确认（HTTP 层无 GET，防旁路读取——直接用 KeychainStore 验证）
    let keys = tenon_models::KeychainStore::new();
    assert_eq!(keys.get(name).as_deref(), Some(value));
    keys.delete(name);
    assert!(keys.get(name).is_none());
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
