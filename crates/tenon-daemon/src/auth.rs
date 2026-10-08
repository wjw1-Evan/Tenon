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
        let mut tickets = self.tickets.lock().expect("tickets lock");
        // 发放时同步清理过期项：/pairing 免鉴权可无限刷 issue，
        // 只在 consume 里清会让票据集合无界增长
        let now = Instant::now();
        tickets.retain(|(_, exp)| *exp > now);
        tickets.insert((ticket.clone(), now + Duration::from_secs(60)));
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
    // IPv6 字面量带方括号（authority_host 保留），IpAddr 解析须先剥括号
    let req_ip = req_host.trim_start_matches('[').trim_end_matches(']');
    origin_host == req_host && req_ip.parse::<std::net::IpAddr>().is_ok()
}

/// 鉴权中间件共享状态：主 token + 局域网配对存储 + 测试钩子。
#[derive(Clone)]
pub struct AuthState {
    pub master: String,
    pub pairing: std::sync::Arc<PairingStore>,
    /// 测试钩子（v1.166）：回环判定改为真实连接对端地址后，集成测试经
    /// loopback socket 无法模拟非回环对端——置位后一律按局域网对端处理，
    /// LAN 门禁 / 配对流仍走完整真实链路（进程内显式注入，不读环境变量，
    /// 避免并行测试串扰）。
    pub force_lan_peer: bool,
}

/// 主 token 专属端点（§15 标注「主 token」）：已配对设备令牌不得调用——
/// /lan/status 会回当前配对码，设备令牌可自建新配对实现「吊销后仍存活」。
const MASTER_ONLY_ROUTES: [&str; 3] = ["/lan/enable", "/lan/status", "/lan/revoke"];

/// HTTP 鉴权 + Origin/Host 校验 + CORS 响应头中间件。
pub async fn auth_middleware(
    state: axum::extract::State<AuthState>,
    headers: HeaderMap,
    request: Request<Body>,
    next: Next,
) -> Response {
    let token = &state.master;
    let pairing = &state.pairing;
    // Host 校验（§12.6）：回环 = 本机访问；局域网地址 = 显式开启 + 已配对令牌
    //（/lan/pair 以一次性配对码自证免令牌；/ws 靠首帧一次性票据——浏览器
    // WebSocket 无法自定义请求头，配对令牌带不上）
    //
    // 域名形态 Host 一律拒绝（DNS rebinding 面 §12.6）：合法访问形态只有
    // 回环名（本机）与 IP 直访（局域网已配对设备）；域名 Host 必为伪造——
    // 浏览器经攻击域名解析到 127.0.0.1 时对端恰是回环，Host 是唯一破绽。
    let path = request.uri().path();
    let host_header = headers.get(header::HOST).and_then(|h| h.to_str().ok());
    if let Some(host) = host_header {
        let host_part = authority_host(host);
        let host_is_ip = host_part
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<std::net::IpAddr>()
            .is_ok();
        if host_part != "localhost" && !host_is_ip {
            return (StatusCode::FORBIDDEN, "host rejected").into_response();
        }
    }
    // 回环判定以真实连接对端地址为准（v1.166）：Host 头由客户端完全控制，
    // 局域网客户端伪造 `Host: 127.0.0.1`（或不带 Host）即可伪装成本机请求，
    // 把免令牌的 /pairing 主 token 披露向全网开放（未配对设备即可读取）。
    // 对端地址判局域网后，/pairing 与其他 API 同受下方配对门禁约束——
    // 未配对 403；已配对设备按 v1.157 流程领主 token + lan_url（设计语义）。
    let lan_request = state.force_lan_peer
        || match request
            .extensions()
            .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
            .map(|c| c.0)
        {
            Some(peer) => !peer.ip().is_loopback(),
            None => {
                let host_is_loopback = host_header
                    .map(|host| {
                        let host_part = authority_host(host);
                        host_part == "127.0.0.1" || host_part == "localhost" || host_part == "[::1]"
                    })
                    // HTTP/1.0 可无 Host：按本机请求处理（与既有行为一致）
                    .unwrap_or(true);
                !host_is_loopback
            }
        };
    if lan_request && path != "/lan/pair" && path != "/ws" {
        // 局域网访问（--lan 绑定后）：须持已配对设备令牌（X-Tenon-Paired，
        // 经 PairingStore 校验；吊销即失效，§12.6 可吊销）。主 token 同样
        // 放行——持有者本就是全权，不因来源是局域网而收窄；/pairing 的
        // 主 token 披露仍限回环（见上）。
        let master = headers.get("X-Tenon-Token").and_then(|t| t.to_str().ok());
        let paired = headers
            .get("X-Tenon-Paired")
            .and_then(|t| t.to_str().ok())
            .map(|t| pairing.verify_token(t))
            .unwrap_or(false);
        if !paired && master != Some(token.as_str()) {
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
    // 主 token（X-Tenon-Token）或已配对设备令牌（X-Tenon-Paired，PairingStore 校验）均可；
    // 主 token 专属端点（§15）只认主 token——设备令牌不得借道提升权限。
    let auth_exempt =
        path == "/health" || path == "/ws" || path == "/pairing" || path == "/lan/pair";
    if !auth_exempt {
        let master = headers.get("X-Tenon-Token").and_then(|t| t.to_str().ok());
        let paired = headers
            .get("X-Tenon-Paired")
            .and_then(|t| t.to_str().ok())
            .map(|t| pairing.verify_token(t))
            .unwrap_or(false);
        let master_ok = master == Some(token.as_str());
        if !master_ok && !paired {
            return (StatusCode::UNAUTHORIZED, "missing or invalid token").into_response();
        }
        if !master_ok && MASTER_ONLY_ROUTES.contains(&path) {
            return (
                StatusCode::FORBIDDEN,
                "this endpoint requires the master token",
            )
                .into_response();
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
        let state = AuthState {
            master: String::from("tok"),
            pairing: std::sync::Arc::new(PairingStore::default()),
            force_lan_peer: false,
        };
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
    fn same_host_origin_accepts_ipv6_literal() {
        // v1.166：方括号形式须剥括号后按 IpAddr 解析，此前永远 403
        assert!(same_host_origin("http://[::1]:41234", Some("[::1]:41234")));
    }

    /// v1.166 回归：回环判定不得依赖客户端可控的 Host 头——局域网对端
    /// （注入非回环 ConnectInfo）伪造 `Host: 127.0.0.1` 不得经免鉴权
    /// /pairing 拿到主 token；真实回环连接不受影响。
    #[tokio::test]
    async fn lan_peer_cannot_spoof_loopback_via_host_header() {
        let pairing = std::sync::Arc::new(PairingStore::default());
        let state = AuthState {
            master: String::from("master-tok"),
            pairing,
            force_lan_peer: false,
        };
        let app = axum::Router::new()
            .route(
                "/pairing",
                axum::routing::get(|| async {
                    axum::Json(serde_json::json!({ "token": "master-tok" }))
                }),
            )
            .layer(axum::middleware::from_fn_with_state(
                state.clone(),
                auth_middleware,
            ));
        let lan = axum::extract::ConnectInfo(std::net::SocketAddr::new(
            std::net::IpAddr::from([192, 168, 1, 9]),
            51000,
        ));
        let local = axum::extract::ConnectInfo(std::net::SocketAddr::new(
            std::net::IpAddr::from([127, 0, 0, 1]),
            51001,
        ));

        let mut req = Request::builder()
            .uri("http://127.0.0.1/pairing")
            .header(header::HOST, "127.0.0.1")
            .body(Body::empty())
            .unwrap();
        req.extensions_mut().insert(lan);
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(
            res.status(),
            StatusCode::FORBIDDEN,
            "伪造 Host 的局域网请求不得读主 token"
        );

        let mut req = Request::builder()
            .uri("http://127.0.0.1/pairing")
            .header(header::HOST, "127.0.0.1")
            .body(Body::empty())
            .unwrap();
        req.extensions_mut().insert(local);
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK, "真实回环连接仍放行");
    }

    /// v1.166 回归：主 token 专属端点（/lan/status 等）不接受设备令牌——
    /// 否则被吊销设备可借 /lan/status 回吐的配对码自建新配对续命。
    #[tokio::test]
    async fn paired_device_token_cannot_call_master_only_routes() {
        let pairing = std::sync::Arc::new(PairingStore::new());
        let code = pairing.enable();
        let device_token = pairing.pair("phone", &code).expect("配对成功");
        let state = AuthState {
            master: String::from("tok"),
            pairing,
            force_lan_peer: false,
        };
        let app = axum::Router::new()
            .route("/lan/status", axum::routing::get(|| async { "ok" }))
            .layer(axum::middleware::from_fn_with_state(
                state.clone(),
                auth_middleware,
            ));

        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("http://127.0.0.1/lan/status")
                    .header(header::HOST, "127.0.0.1")
                    .header("X-Tenon-Token", "tok")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK, "主 token 放行");

        let res = app
            .oneshot(
                Request::builder()
                    .uri("http://127.0.0.1/lan/status")
                    .header(header::HOST, "127.0.0.1")
                    .header("X-Tenon-Paired", &device_token)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            res.status(),
            StatusCode::FORBIDDEN,
            "设备令牌不得调主 token 专属端点"
        );
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
        let state = AuthState {
            master: String::from("tok"),
            pairing,
            force_lan_peer: false,
        };
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
