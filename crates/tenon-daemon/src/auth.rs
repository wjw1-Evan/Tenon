//! 鉴权与来源校验（设计方案 §12.6 / §15 / ADR-10）。

use crate::PairingStore;
use axum::body::Body;
use axum::http::{header, HeaderMap, Method, Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use std::collections::HashSet;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// 一次性 WS 票据库（60 秒有效、用后即焚、重放即拒）。
#[derive(Default)]
pub struct TicketStore {
    tickets: Mutex<HashSet<(String, Instant)>>,
}

impl TicketStore {
    pub fn issue(&self) -> String {
        let ticket = crate::generate_token();
        self.tickets
            .lock()
            .expect("tickets lock")
            .insert((ticket.clone(), Instant::now() + Duration::from_secs(60)));
        ticket
    }

    /// 消费票据：有效返回 true（一次性）；过期 / 重放返回 false。
    pub fn consume(&self, ticket: &str) -> bool {
        let mut tickets = self.tickets.lock().expect("tickets lock");
        let now = Instant::now();
        tickets.retain(|(_, exp)| *exp > now);
        if let Some(found) = tickets.iter().find(|(t, _)| t == ticket).cloned() {
            tickets.remove(&found);
            true
        } else {
            false
        }
    }
}

/// CORS 白名单（§12.6）：应用自身源（Tauri WebView）+ 本机 dev server +
/// 本机浏览器访问。局域网同源请求走 `same_host_origin`（v1.157）。
pub const CORS_WHITELIST: [&str; 3] = ["tauri://localhost", "http://localhost", "http://127.0.0.1"];

pub fn origin_allowed(origin: &str) -> bool {
    CORS_WHITELIST.iter().any(|a| {
        origin == *a
            || origin.starts_with("http://127.0.0.1:")
            || origin.starts_with("http://localhost:")
    })
}

/// authority（host[:port]）取 host 段：IPv6 保留方括号内整段，其余按末个
/// `:` 后全数字为端口截断；无端口原样返回。
fn authority_host(authority: &str) -> &str {
    if authority.starts_with('[') {
        if let Some(close) = authority.find(']') {
            return &authority[..=close];
        }
    }
    match authority.rfind(':') {
        Some(idx) if authority[idx + 1..].chars().all(|c| c.is_ascii_digit()) => &authority[..idx],
        _ => authority,
    }
}

/// 同源放行判定（v1.157 §12.6）：Origin host 与 Host 一致**且 Host 为 IP
/// 字面量**。局域网浏览器经 IP 直访是唯一合法形态；Host 为域名的请求一律
/// 不放行——DNS rebinding 攻击的 Host 必为域名（否则无法解析），防线保持。
fn same_host_origin(origin: &str, host_header: Option<&str>) -> bool {
    let Some(host) = host_header else {
        return false;
    };
    let origin_authority = origin
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(origin);
    let origin_host = authority_host(origin_authority);
    let req_host = authority_host(host);
    origin_host == req_host && req_host.parse::<std::net::IpAddr>().is_ok()
}

/// HTTP 鉴权 + Origin/Host 校验 + CORS 响应头中间件。
/// state = (主 token, 局域网配对存储)：配对设备令牌经 PairingStore 校验。
pub async fn auth_middleware(
    state: axum::extract::State<(String, std::sync::Arc<PairingStore>)>,
    headers: HeaderMap,
    request: Request<Body>,
    next: Next,
) -> Response {
    let token = &state.0;
    let pairing = &state.1;
    // Host 校验（§12.6）：回环 = 本机访问；局域网地址 = 显式开启 + 已配对令牌
    //（/lan/pair 以一次性配对码自证免令牌；/ws 靠首帧一次性票据——浏览器
    // WebSocket 无法自定义请求头，配对令牌带不上）
    let path = request.uri().path();
    let host_header = headers.get(header::HOST).and_then(|h| h.to_str().ok());
    let mut lan_request = false;
    if let Some(host) = host_header {
        let host_part = authority_host(host);
        if host_part != "127.0.0.1" && host_part != "localhost" && host_part != "[::1]" {
            lan_request = true;
        }
    }
    if lan_request && path != "/lan/pair" && path != "/ws" {
        // 局域网访问（--lan 绑定后）：须持已配对设备令牌（X-Tenon-Paired，
        // 经 PairingStore 校验；吊销即失效，§12.6 可吊销）
        let paired = headers
            .get("X-Tenon-Paired")
            .and_then(|t| t.to_str().ok())
            .map(|t| pairing.verify_token(t))
            .unwrap_or(false);
        if !paired {
            return (
                StatusCode::FORBIDDEN,
                "lan access requires paired device token",
            )
                .into_response();
        }
    }
    // CORS 白名单细化：放行源附加响应头；其余源拒绝（§12.6；局域网同源
    // IP 直访经 same_host_origin 放行，域名 Host 不在放行之列）
    let mut cors_origin: Option<String> = None;
    if let Some(origin) = headers.get(header::ORIGIN).and_then(|o| o.to_str().ok()) {
        if !origin_allowed(origin) && !same_host_origin(origin, host_header) {
            return (StatusCode::FORBIDDEN, "origin rejected").into_response();
        }
        cors_origin = Some(origin.to_string());
    }
    // CORS 预检（跨源 dev server 必经）：浏览器发起的 OPTIONS 不携带自定义
    // 头（无 token），须在中间件直接答复 2xx + CORS 头；否则 axum 405 会让
    // 预检失败、后续所有跨源请求报 TypeError: Failed to fetch（§12.6 白名单
    // 已在上一步收口，非白名单源到不了这里）。
    if request.method() == Method::OPTIONS {
        if let Some(origin) = cors_origin {
            let mut preflight = StatusCode::OK.into_response();
            preflight.headers_mut().insert(
                header::ACCESS_CONTROL_ALLOW_ORIGIN,
                header::HeaderValue::from_str(&origin).expect("origin header"),
            );
            preflight.headers_mut().insert(
                header::ACCESS_CONTROL_ALLOW_HEADERS,
                header::HeaderValue::from_static("Content-Type, X-Tenon-Token"),
            );
            preflight.headers_mut().insert(
                header::ACCESS_CONTROL_ALLOW_METHODS,
                header::HeaderValue::from_static("GET, POST, PUT, DELETE, OPTIONS"),
            );
            preflight.headers_mut().insert(
                header::ACCESS_CONTROL_MAX_AGE,
                header::HeaderValue::from_static("600"),
            );
            return preflight;
        }
        return StatusCode::OK.into_response();
    }
    // Token 校验（/health、/pairing、/lan/pair 免鉴权；/ws 用一次性票据首帧鉴权 ADR-10）。
    // 主 token（X-Tenon-Token）或已配对设备令牌（X-Tenon-Paired，PairingStore 校验）均可。
    let auth_exempt =
        path == "/health" || path == "/ws" || path == "/pairing" || path == "/lan/pair";
    if !auth_exempt {
        let master = headers.get("X-Tenon-Token").and_then(|t| t.to_str().ok());
        let paired = headers
            .get("X-Tenon-Paired")
            .and_then(|t| t.to_str().ok())
            .map(|t| pairing.verify_token(t))
            .unwrap_or(false);
        let ok = master == Some(token.0.as_str()) || paired;
        if !ok {
            return (StatusCode::UNAUTHORIZED, "missing or invalid token").into_response();
        }
    }
    let mut response = next.run(request).await;
    if let Some(origin) = cors_origin {
        response.headers_mut().insert(
            header::ACCESS_CONTROL_ALLOW_ORIGIN,
            header::HeaderValue::from_str(&origin).expect("origin header"),
        );
        response.headers_mut().insert(
            header::ACCESS_CONTROL_ALLOW_HEADERS,
            header::HeaderValue::from_static("Content-Type, X-Tenon-Token"),
        );
        response.headers_mut().insert(
            header::ACCESS_CONTROL_ALLOW_METHODS,
            header::HeaderValue::from_static("GET, POST, PUT, DELETE"),
        );
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower::ServiceExt as _;

    #[test]
    fn ticket_single_use_and_expiry() {
        let store = TicketStore::default();
        let t = store.issue();
        assert!(store.consume(&t));
        assert!(!store.consume(&t), "重放即拒（ADR-10）");
        assert!(!store.consume("nonexistent"));
    }

    #[test]
    fn cors_whitelist_exact_and_port_variants() {
        assert!(origin_allowed("tauri://localhost"));
        assert!(origin_allowed("http://localhost:5173"));
        assert!(origin_allowed("http://127.0.0.1:8080"));
        assert!(!origin_allowed("https://evil.example.com"));
        assert!(
            !origin_allowed("http://192.168.1.5:5173"),
            "局域网源未配对不放行"
        );
    }

    /// 回归（v1.30）：跨源 dev server 的 OPTIONS 预检须 200 + CORS 头，
    /// 且不要求 token（浏览器预检不携带自定义头）；非白名单源仍 403。
    #[tokio::test]
    async fn preflight_options_allowed_without_token_and_rejects_foreign_origin() {
        let state = (
            String::from("tok"),
            std::sync::Arc::new(PairingStore::default()),
        );
        let app = axum::Router::new()
            .route("/projects/open", axum::routing::post(|| async { "ok" }))
            .layer(axum::middleware::from_fn_with_state(
                state.clone(),
                auth_middleware,
            ));

        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::OPTIONS)
                    .uri("http://127.0.0.1/projects/open")
                    .header(header::ORIGIN, "http://localhost:5199")
                    .header(header::HOST, "127.0.0.1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(
            res.headers().get(header::ACCESS_CONTROL_ALLOW_ORIGIN),
            Some(&header::HeaderValue::from_static("http://localhost:5199"))
        );

        let res = app
            .oneshot(
                Request::builder()
                    .method(Method::OPTIONS)
                    .uri("http://127.0.0.1/projects/open")
                    .header(header::ORIGIN, "https://evil.example.com")
                    .header(header::HOST, "127.0.0.1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
    }

    #[test]
    fn authority_host_strips_port_and_keeps_ipv6() {
        assert_eq!(authority_host("192.168.1.5:41234"), "192.168.1.5");
        assert_eq!(authority_host("192.168.1.5"), "192.168.1.5");
        assert_eq!(authority_host("127.0.0.1:80"), "127.0.0.1");
        assert_eq!(authority_host("[::1]:8080"), "[::1]");
        assert_eq!(authority_host("evil.example.com:80"), "evil.example.com");
    }

    #[test]
    fn same_host_origin_requires_ip_literal_host() {
        // 局域网 IP 直访同源 → 放行
        assert!(same_host_origin(
            "http://192.168.1.5:41234",
            Some("192.168.1.5:41234")
        ));
        // 无端口 Host 与带端口 Origin host 一致 → 放行
        assert!(same_host_origin("http://10.0.0.2:8080", Some("10.0.0.2")));
        // 域名 Host（DNS rebinding 形态）→ 不放行
        assert!(!same_host_origin(
            "http://evil.example.com:8080",
            Some("evil.example.com:8080")
        ));
        // 源与 Host 不同（跨源伪造）→ 不放行
        assert!(!same_host_origin(
            "http://192.168.1.66:9999",
            Some("192.168.1.5:41234")
        ));
        // 缺 Host / null origin → 不放行
        assert!(!same_host_origin("http://192.168.1.5", None));
        assert!(!same_host_origin("null", Some("192.168.1.5:1")));
    }

    /// v1.157：局域网同源 IP 直访——已配对设备带同源 Origin 的 POST 放行；
    /// 域名 Host（rebinding）即使「同源」也 403；/ws 升级免配对头（首帧票据
    /// 鉴权兜底）；未配对且无令牌的 API 请求仍 403。
    #[tokio::test]
    async fn lan_same_origin_and_ws_exempt() {
        let pairing = std::sync::Arc::new(PairingStore::new());
        let code = pairing.enable();
        let device_token = pairing.pair("phone", &code).expect("配对成功");
        let state = (String::from("tok"), pairing);
        let app = axum::Router::new()
            .route("/projects/open", axum::routing::post(|| async { "ok" }))
            .route("/ws", axum::routing::get(|| async { "ws" }))
            .layer(axum::middleware::from_fn_with_state(
                state.clone(),
                auth_middleware,
            ));

        // 已配对 + 局域网同源 Origin → 200
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("http://192.168.1.5/projects/open")
                    .header(header::HOST, "192.168.1.5:41234")
                    .header(header::ORIGIN, "http://192.168.1.5:41234")
                    .header("X-Tenon-Paired", &device_token)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK, "已配对设备同源放行");

        // rebinding：域名 Host + 同域名 Origin → 403
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("http://evil.example.com/projects/open")
                    .header(header::HOST, "evil.example.com:41234")
                    .header(header::ORIGIN, "http://evil.example.com:41234")
                    .header("X-Tenon-Paired", &device_token)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN, "域名 Host 不放行");

        // /ws 升级请求：浏览器 WS 带不了 X-Tenon-Paired → 免配对头放行（首帧票据兜底）
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("http://192.168.1.5/ws")
                    .header(header::HOST, "192.168.1.5:41234")
                    .header(header::ORIGIN, "http://192.168.1.5:41234")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK, "/ws 免配对头");

        // 未配对：局域网同源 Origin 但无令牌 → 403
        let res = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("http://192.168.1.5/projects/open")
                    .header(header::HOST, "192.168.1.5:41234")
                    .header(header::ORIGIN, "http://192.168.1.5:41234")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN, "未配对仍拦截");
    }
}
