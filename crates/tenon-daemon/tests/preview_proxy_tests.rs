//! 内嵌浏览器预览反代集成测试（§7.6 v2.77）：真 daemon + 真上游 dev server
//! （本地 axum 假服务），锁死去嵌入限制剥离、URL 重写、控制台桥注入、口令
//! 校验、回环目标约束与透传语义。

use std::sync::Arc;
use std::time::Duration;

use axum::extract::RawQuery;
use axum::http::HeaderValue;
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get};
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
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap()
}

/// 不跟随重定向的裸客户端（验 Location 改写必须看到 302 本身）。
fn plain_client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(15))
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

/// 假 dev server：带 X-Frame-Options / CSP 的 HTML、根绝对说明符的 JS、CSS、
/// 重定向、POST 回显与 query 回显——覆盖代理的各条改写与透传分支。
async fn start_fake_dev_server() -> u16 {
    async fn index() -> Response {
        (
            [
                ("content-type", "text/html; charset=utf-8"),
                ("x-frame-options", "DENY"),
                (
                    "content-security-policy",
                    "default-src 'self'; frame-ancestors 'none'",
                ),
            ],
            "<html><head><script type=\"module\" src=\"/src/main.tsx\"></script></head>\
             <body><link href=\"/style.css\"><img src=\"//cdn.example.com/a.png\"></body></html>",
        )
            .into_response()
    }
    async fn module_js() -> Response {
        (
            [("content-type", "text/javascript")],
            "import \"/@vite/client\";import {x} from \"/src/app.tsx\";",
        )
            .into_response()
    }
    async fn css() -> Response {
        (
            [("content-type", "text/css")],
            "body{background:url(/img/a.png)}",
        )
            .into_response()
    }
    async fn png() -> Response {
        (
            [("content-type", "image/png")],
            vec![0x89u8, 0x50, 0x4e, 0x47],
        )
            .into_response()
    }
    async fn echo(body: axum::body::Bytes) -> String {
        format!("echo:{}", String::from_utf8_lossy(&body))
    }
    async fn query(RawQuery(q): RawQuery) -> String {
        format!("q:{}", q.unwrap_or_default())
    }

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let port = listener.local_addr().unwrap().port();
    let redirect_port = port;
    let app = axum::Router::new()
        .route("/", get(index))
        .route("/src/main.tsx", get(module_js))
        .route("/style.css", get(css))
        .route("/img/a.png", get(png))
        .route("/api/echo", any(echo))
        .route("/api/query", any(query))
        .route(
            "/go",
            get(move || async move {
                (
                    axum::http::StatusCode::FOUND,
                    [(
                        "location",
                        HeaderValue::from_str(&format!("http://127.0.0.1:{redirect_port}/target"))
                            .unwrap(),
                    )],
                )
                    .into_response()
            }),
        );
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    port
}

/// 取预览口令（须主 token）。
async fn preview_secret(port: u16, token: &str) -> String {
    let r: serde_json::Value = client_with_token(token)
        .get(format!("{}/preview/token", base(port)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    r["secret"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn preview_token_endpoint_requires_auth() {
    let (_tmp, port, token) = start_daemon().await;
    let bare = reqwest::Client::new()
        .get(format!("{}/preview/token", base(port)))
        .send()
        .await
        .unwrap();
    assert_eq!(bare.status(), 401, "口令端点须 token 鉴权");

    let ok: serde_json::Value = client_with_token(&token)
        .get(format!("{}/preview/token", base(port)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let secret = ok["secret"].as_str().unwrap();
    assert_eq!(secret.len(), 64, "口令为 32 字节 hex");
    assert_eq!(
        ok["prefix"].as_str().unwrap(),
        format!("/preview/{secret}/proxy/")
    );
}

#[tokio::test]
async fn preview_html_strips_frame_headers_injects_bridge_and_rewrites_urls() {
    let (_tmp, port, token) = start_daemon().await;
    let up = start_fake_dev_server().await;
    let secret = preview_secret(port, &token).await;
    let prefix = format!("/preview/{secret}/proxy/{up}");

    // 裸请求：子资源带不了 token，凭路径口令放行
    let resp = plain_client()
        .get(format!("{}{prefix}/", base(port)))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "子资源豁免令牌、凭口令放行");
    assert!(
        resp.headers().get("x-frame-options").is_none(),
        "X-Frame-Options 必须剥离"
    );
    let csp = resp
        .headers()
        .get("content-security-policy")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    assert!(
        !csp.contains("frame-ancestors"),
        "CSP 去 frame-ancestors: {csp}"
    );
    assert!(csp.contains("default-src"), "其余 CSP 指令保留: {csp}");
    assert_eq!(
        resp.headers()
            .get("referrer-policy")
            .and_then(|v| v.to_str().ok()),
        Some("no-referrer"),
        "防口令随 Referer 外泄"
    );

    let body = resp.text().await.unwrap();
    assert!(
        body.contains(&format!("src=\"{prefix}/src/main.tsx\"")),
        "入口脚本须补代理前缀: {body}"
    );
    assert!(
        body.contains(&format!("href=\"{prefix}/style.css\"")),
        "样式表须补代理前缀"
    );
    assert!(
        body.contains("src=\"//cdn.example.com/a.png\""),
        "协议相对 URL 不得改写"
    );
    assert!(body.contains("data-tenon-preview"), "控制台桥须注入");
    assert_eq!(
        body.matches("data-tenon-preview").count(),
        1,
        "桥只注入一次"
    );
}

#[tokio::test]
async fn preview_rewrites_js_and_css_and_passes_binary_through() {
    let (_tmp, port, token) = start_daemon().await;
    let up = start_fake_dev_server().await;
    let secret = preview_secret(port, &token).await;
    let prefix = format!("/preview/{secret}/proxy/{up}");

    let js = plain_client()
        .get(format!("{}{prefix}/src/main.tsx", base(port)))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        js.contains(&format!("import \"{prefix}/@vite/client\"")),
        "模块说明符须补代理前缀: {js}"
    );
    assert!(js.contains(&format!("from \"{prefix}/src/app.tsx\"")));

    let css = plain_client()
        .get(format!("{}{prefix}/style.css", base(port)))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        css.contains(&format!("url({prefix}/img/a.png)")),
        "CSS url 重写: {css}"
    );

    let png = plain_client()
        .get(format!("{}{prefix}/img/a.png", base(port)))
        .send()
        .await
        .unwrap();
    assert_eq!(png.headers()["content-type"], "image/png");
    assert_eq!(
        png.bytes().await.unwrap().to_vec(),
        vec![0x89, 0x50, 0x4e, 0x47],
        "二进制响应须原样透传"
    );
}

#[tokio::test]
async fn preview_forwards_method_body_query_and_rewrites_location() {
    let (_tmp, port, token) = start_daemon().await;
    let up = start_fake_dev_server().await;
    let secret = preview_secret(port, &token).await;
    let prefix = format!("/preview/{secret}/proxy/{up}");

    let echoed = plain_client()
        .post(format!("{}{prefix}/api/echo", base(port)))
        .body("hello")
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(echoed, "echo:hello", "请求体与方法须透传");

    let queried = plain_client()
        .get(format!("{}{prefix}/api/query?a=1&b=2", base(port)))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(queried, "q:a=1&b=2", "query 须透传");

    let resp = plain_client()
        .get(format!("{}{prefix}/go", base(port)))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 302);
    assert_eq!(
        resp.headers()["location"].to_str().unwrap(),
        format!("{prefix}/target"),
        "本机 dev server 的 Location 须改写成代理前缀"
    );
}

#[tokio::test]
async fn preview_rejects_bad_secret_and_bad_port() {
    let (_tmp, port, token) = start_daemon().await;
    let up = start_fake_dev_server().await;
    let secret = preview_secret(port, &token).await;

    let wrong = plain_client()
        .get(format!("{}/preview/deadbeef/proxy/{up}/", base(port)))
        .send()
        .await
        .unwrap();
    assert_eq!(wrong.status(), 401, "口令不符须 401");

    for bad in ["0", "99999", "abc"] {
        let resp = plain_client()
            .get(format!("{}/preview/{secret}/proxy/{bad}/", base(port)))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 400, "端口 {bad} 须 400");
    }
}

#[tokio::test]
async fn preview_unreachable_upstream_is_502() {
    let (_tmp, port, token) = start_daemon().await;
    let secret = preview_secret(port, &token).await;
    // 该端口上无服务：代理须如实 502（UI 据此提示「dev server 未启动」）
    let resp = plain_client()
        .get(format!("{}/preview/{secret}/proxy/1/", base(port)))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 502);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["error"].as_str().unwrap_or("").contains("127.0.0.1:1"));
}
