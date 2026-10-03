//! 鉴权与来源校验（设计方案 §12.6 / §15 / ADR-10）。

use crate::PairingStore;
use axum::body::Body;
use axum::http::{header, HeaderMap, Request, StatusCode};
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
/// 本机浏览器访问。局域网源仅在显式开启并配对后放行（M3）。
pub const CORS_WHITELIST: [&str; 3] = ["tauri://localhost", "http://localhost", "http://127.0.0.1"];

pub fn origin_allowed(origin: &str) -> bool {
    CORS_WHITELIST.iter().any(|a| {
        origin == *a
            || origin.starts_with("http://127.0.0.1:")
            || origin.starts_with("http://localhost:")
    })
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
    //（/lan/pair 以一次性配对码自证，免令牌）
    let path = request.uri().path();
    let mut lan_request = false;
    if let Some(host) = headers.get(header::HOST).and_then(|h| h.to_str().ok()) {
        let host_part = host.split(':').next().unwrap_or("");
        if host_part != "127.0.0.1" && host_part != "localhost" {
            lan_request = true;
        }
    }
    if lan_request && path != "/lan/pair" {
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
    // CORS 白名单细化：放行源附加响应头；其余源拒绝（§12.6）
    let mut cors_origin: Option<String> = None;
    if let Some(origin) = headers.get(header::ORIGIN).and_then(|o| o.to_str().ok()) {
        if !origin_allowed(origin) {
            return (StatusCode::FORBIDDEN, "origin rejected").into_response();
        }
        cors_origin = Some(origin.to_string());
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
}
