//! 局域网 Web 访问端到端测试（v1.157 §12.6）：静态托管在鉴权层外 + 配对全链 +
//! 管理端点 + `--lan` 绑定语义。

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use tower::ServiceExt as _;

use tenon_daemon::{build_app, DaemonOptions, DaemonState};

async fn lan_state(lan_bind: bool) -> Arc<DaemonState> {
    let mut options = DaemonOptions::in_memory();
    options.lan_bind = lan_bind;
    Arc::new(DaemonState::new(options).await)
}

/// 复刻 serve 的组装（UI 静态托管 fallback + 鉴权中间件），Host 头按需注入。
fn lan_app(state: Arc<DaemonState>) -> (axum::Router, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("index.html"), "<html>tenon</html>").unwrap();
    (build_app(state, Some(dir.path().to_path_buf())), dir)
}

fn get(path: &str, host: &str) -> Request<Body> {
    Request::builder()
        .uri(format!("http://{host}{path}"))
        .header(header::HOST, host)
        .body(Body::empty())
        .unwrap()
}

async fn json_body(res: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(res.into_body(), 1_000_000)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn static_assets_free_but_api_gated_on_lan_host() {
    // v1.157：UI 静态资产挂鉴权层之外——未配对局域网设备可加载 UI（公开前端
    // 代码）并据此渲染配对屏；API 同 Host 无令牌仍 403
    let state = lan_state(true).await;
    let (app, _dir) = lan_app(state);

    let res = app
        .clone()
        .oneshot(get("/index.html", "192.168.1.5:41234"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK, "静态资产免令牌");

    let res = app
        .oneshot(get("/models", "192.168.1.5:41234"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN, "API 仍拦");
}

#[tokio::test]
async fn lan_pairing_flow_end_to_end() {
    let state = lan_state(true).await;
    let master = state.token.clone();
    let (app, _dir) = lan_app(state.clone());
    // lan_bind 时 DaemonState::new 已自动 enable——此处重取码对齐「刷新」语义
    let code = state
        .lan_pairing
        .current_code()
        .expect("--lan 自动启用配对")
        .0;

    // 未配对：/pairing 403（主 token 不泄漏给未配对设备）
    let res = app
        .clone()
        .oneshot(get("/pairing", "192.168.1.5:41234"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);

    // 配对：一次性码换设备令牌（/lan/pair 免令牌）
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("http://192.168.1.5:41234/lan/pair")
                .header(header::HOST, "192.168.1.5:41234")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(format!(
                    r#"{{"device":"phone","code":"{code}"}}"#
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = json_body(res).await;
    let paired = body["paired_token"].as_str().expect("配对返回设备令牌");

    // 已配对：/pairing 200，返回主 token 与 lan_url 字段
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("http://192.168.1.5:41234/pairing")
                .header(header::HOST, "192.168.1.5:41234")
                .header("X-Tenon-Paired", paired)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = json_body(res).await;
    assert_eq!(body["token"], master.as_str(), "配对设备获得主握手");
    assert!(body.get("lan_url").is_some(), "lan_url 字段存在");

    // 配对码一次性：同码重放 403
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("http://192.168.1.5:41234/lan/pair")
                .header(header::HOST, "192.168.1.5:41234")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(format!(
                    r#"{{"device":"tablet","code":"{code}"}}"#
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN, "配对码一次性");

    // /lan/status：主 token 可读，含 code / devices；令牌本体不出 daemon
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("http://127.0.0.1:41234/lan/status")
                .header(header::HOST, "127.0.0.1:41234")
                .header("X-Tenon-Token", &master)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = json_body(res).await;
    assert_eq!(body["devices"][0]["device"], "phone");
    assert!(body["devices"][0].get("token").is_none(), "令牌不外泄");

    // 吊销：设备令牌立即失效
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("http://127.0.0.1:41234/lan/revoke")
                .header(header::HOST, "127.0.0.1:41234")
                .header("X-Tenon-Token", &master)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"device":"phone"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let res = app
        .oneshot(
            Request::builder()
                .uri("http://192.168.1.5:41234/pairing")
                .header(header::HOST, "192.168.1.5:41234")
                .header("X-Tenon-Paired", paired)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN, "吊销即失效");
}

#[tokio::test]
async fn lan_bind_accepts_loopback_connections() {
    // `--lan` 绑 0.0.0.0：回环同端口可达（本机桌面壳 / 浏览器不受影响）
    let mut options = DaemonOptions::in_memory();
    options.lan_bind = true;
    let handle = tenon_daemon::serve(options).await.unwrap();
    let r = reqwest::get(format!("http://127.0.0.1:{}/health", handle.port))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
}
