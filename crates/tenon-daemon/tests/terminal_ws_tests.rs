//! 终端 WS 全链回归（v2.42，§7.2 / §12.6 / ADR-10）：v1.197 落地时中间件
//! 豁免名单遗漏 `/projects/:id/terminal/ws`——升级请求被 HTTP 层 token 检查
//! 401 挡死，而浏览器 WebSocket 无法自定义请求头，终端永远打不开
//! （UI 报 terminal.open_failed）；且鉴权成功路径无 auth ok 帧、UI 握手
//! Promise 永不落定。三例锁死：HTTP 开终端、免头升级 + 首帧票据 + auth ok
//! + 输出回读、票据重放拒绝。

use std::sync::Arc;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
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

async fn start_daemon(script: Vec<ScriptedReply>) -> (tempfile::TempDir, u16, String) {
    let dir = tempfile::tempdir().unwrap();
    let mut options = DaemonOptions::in_memory();
    options.providers = vec![Arc::new(MockProvider::new("mock", "mock-1", script))];
    options.default_provider = "mock".into();
    options.snapshots_root = Some(dir.path().join("snapshots"));
    options.worktrees_root = Some(dir.path().join("worktrees"));
    options.endpoint_path = Some(dir.path().join("daemon.endpoint"));
    options.settings_path = Some(dir.path().join("settings.json"));
    options.updates_staging_dir = Some(dir.path().join("updates/staged"));
    options.laya_models_dir = Some(dir.path().join("models/laya"));
    let handle = serve(options).await.unwrap();
    (dir, handle.port, handle.token)
}

async fn open_project(port: u16, token: &str, dir: &tempfile::TempDir) -> String {
    let project = dir.path().join("ws-term-proj");
    std::fs::create_dir_all(&project).unwrap();
    let opened: serde_json::Value = client_with_token(token)
        .post(format!("{}/projects/open", base(port)))
        .json(&serde_json::json!({"path": project.to_string_lossy()}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    opened["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn terminal_open_http_returns_shell() {
    let (dir, port, token) = start_daemon(vec![]).await;
    let project_id = open_project(port, &token, &dir).await;
    let resp: serde_json::Value = client_with_token(&token)
        .post(format!(
            "{base}/projects/{project_id}/terminal",
            base = base(port)
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        resp["shell"].as_str().unwrap_or("").contains("/"),
        "开终端应返回 shell 路径: {resp}"
    );
}

#[tokio::test]
async fn terminal_ws_ticket_first_frame_auth_ok_and_output_roundtrip() {
    let (dir, port, token) = start_daemon(vec![]).await;
    let project_id = open_project(port, &token, &dir).await;

    let resp: serde_json::Value = client_with_token(&token)
        .post(format!("{}/ws-ticket", base(port)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let ticket = resp["ticket"].as_str().unwrap().to_string();

    // 浏览器同形态：升级请求不带任何自定义头（修复前此处 401）
    let (mut ws, _) = tokio_tungstenite::connect_async(format!(
        "ws://127.0.0.1:{port}/projects/{project_id}/terminal/ws"
    ))
    .await
    .unwrap();
    ws.send(tokio_tungstenite::tungstenite::Message::text(ticket))
        .await
        .unwrap();
    // v2.42：鉴权成功回发 auth ok（UI 握手 Promise 落定信号）
    let auth = ws.next().await.unwrap().unwrap();
    assert_eq!(auth.to_string(), "auth ok", "{auth}");

    // 输出回读：写入 echo，PTY 回显含标记（终端自回显语义）
    ws.send(tokio_tungstenite::tungstenite::Message::text(
        "echo TENON_TERM_V42\r\n",
    ))
    .await
    .unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut seen = false;
    while tokio::time::Instant::now() < deadline {
        let Ok(Some(Ok(msg))) = tokio::time::timeout(Duration::from_millis(500), ws.next()).await
        else {
            continue;
        };
        if msg.to_string().contains("TENON_TERM_V42") {
            seen = true;
            break;
        }
    }
    assert!(seen, "5s 内未读回终端回显");
    let _ = ws.close(None).await;
}

#[tokio::test]
async fn terminal_ws_rejects_replayed_ticket() {
    let (dir, port, token) = start_daemon(vec![]).await;
    let project_id = open_project(port, &token, &dir).await;

    let resp: serde_json::Value = client_with_token(&token)
        .post(format!("{}/ws-ticket", base(port)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let ticket = resp["ticket"].as_str().unwrap().to_string();

    // 先正常消费一次
    let (mut ws, _) = tokio_tungstenite::connect_async(format!(
        "ws://127.0.0.1:{port}/projects/{project_id}/terminal/ws"
    ))
    .await
    .unwrap();
    ws.send(tokio_tungstenite::tungstenite::Message::text(
        ticket.clone(),
    ))
    .await
    .unwrap();
    let auth = ws.next().await.unwrap().unwrap();
    assert_eq!(auth.to_string(), "auth ok", "{auth}");
    let _ = ws.close(None).await;

    // 重放同一票 → auth failed（ADR-10）
    let (mut ws2, _) = tokio_tungstenite::connect_async(format!(
        "ws://127.0.0.1:{port}/projects/{project_id}/terminal/ws"
    ))
    .await
    .unwrap();
    ws2.send(tokio_tungstenite::tungstenite::Message::text(ticket))
        .await
        .unwrap();
    let reply = ws2.next().await.unwrap().unwrap();
    assert_eq!(reply.to_string(), "auth failed", "{reply}");
}
