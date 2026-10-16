//! 内嵌浏览器预览反代（§7.6 v2.77）：把项目本地 dev server 以「可嵌入 + 可观测」
//! 形态暴露给 UI 预览面板。
//!
//! ## 形态与口令传递
//!
//! 路径形如 `/preview/{secret}/proxy/{port}/{path...}`：`secret` 是 daemon 启动
//! 时生成的随机能力口令，经**须鉴权**的 `GET /preview/token` 下发。口令放路径而非
//! query / cookie 是刻意的——iframe 的**子资源**请求（JS / CSS / 图片）不会携带
//! query；而 UI 在 vite dev 下与 daemon 跨源、预览页又嵌在跨源 iframe 里，SameSite
//! cookie 不可靠。路径前缀对相对 URL 天然继承，对根绝对 URL 由本模块重写补齐。
//! 对应地，代理响应统一加 `Referrer-Policy: no-referrer`，防止口令随 Referer
//! 外泄给预览页引入的第三方资源。
//!
//! ## 安全边界
//!
//! - 目标**只允许回环**（`127.0.0.1` + 显式端口，端口 1-65535），否则反代就是
//!   一个任意 URL 代理（SSRF 面）；请求体 / 响应体均有上限；
//! - `/preview/*` 在鉴权中间件中豁免（子资源带不了 `X-Tenon-Token`），该路径
//!   的**唯一**鉴权即路径口令——口令 32 字节随机、比较走常时；
//! - 响应剥 `X-Frame-Options`、删 CSP 的 `frame-ancestors` 指令，让「拒绝被
//!   嵌入」的 dev server 也能预览；其余 CSP 指令原样保留。
//!
//! ## 为何要重写 HTML / JS
//!
//! Vite、Next dev 等默认 `base='/'`，产物里是根绝对 URL（`/src/main.tsx`）与
//! 根绝对模块说明符（`from "/src/App.tsx"`）。挂到子路径代理下若不重写，
//! 浏览器会向 daemon 根要 `/src/main.tsx` → 404。本模块对 HTML 属性
//! （href / src / action / poster）、JS 模块说明符（`from` / `import` / `import(`）、
//! CSS `url()` 做前缀补齐；HMR WebSocket 不代理（vite 客户端连不上会自行降级
//! 轮询 `/__vite_ping`，走代理可达），如实记为已知边界。

use std::sync::OnceLock;

use axum::body::Body;
use axum::extract::Request;
use axum::http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

/// 请求体上限（预览页表单 / 上传；超限 413）。
const MAX_BODY: usize = 32 * 1024 * 1024;
/// 上游超时（dev server 首编译可能较慢）。
const UPSTREAM_TIMEOUT_S: u64 = 60;
/// 预览代理路径前缀（鉴权中间件与 UI 共用）。
pub const PREVIEW_ROUTE: &str = "/preview/";

/// 进程级预览口令（懒初始化；daemon 存活期恒定）。
pub fn preview_secret() -> &'static str {
    static SECRET: OnceLock<String> = OnceLock::new();
    SECRET.get_or_init(crate::generate_token)
}

/// 共享上游客户端：不跟随重定向（Location 由我们重写）、不走系统代理
///（目标恒为回环，代理环境变量会把它带偏）。
fn upstream_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .timeout(std::time::Duration::from_secs(UPSTREAM_TIMEOUT_S))
            .build()
            .expect("preview 上游客户端")
    })
}

/// 常时口令比较。
fn secret_matches(candidate: &str) -> bool {
    let expected = preview_secret().as_bytes();
    let got = candidate.as_bytes();
    if got.len() != expected.len() {
        return false;
    }
    got.iter()
        .zip(expected)
        .fold(0u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}

/// `GET /preview/token`（须主 token / 设备令牌）：下发预览口令与代理前缀。
pub async fn preview_token() -> Response {
    let secret = preview_secret();
    Json(json!({
        "secret": secret,
        "prefix": format!("{PREVIEW_ROUTE}{secret}/proxy/"),
    }))
    .into_response()
}

/// 判断路径是否预览代理路径（鉴权中间件豁免用）：`/preview/<secret>/proxy...`。
pub fn is_proxy_path(path: &str) -> bool {
    path.starts_with(PREVIEW_ROUTE) && path.contains("/proxy")
}

fn is_hop_by_hop(name: &HeaderName) -> bool {
    matches!(
        name.as_str(),
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
    )
}

/// 代理入口：解析路径 → 校验口令与目标 → 转发 → 出口侧去嵌入限制与 URL 重写。
pub async fn preview_proxy(req: Request) -> Response {
    let uri = req.uri().clone();
    let path = uri.path().to_string();
    let Some(rest) = path.strip_prefix(PREVIEW_ROUTE) else {
        return proxy_err(StatusCode::NOT_FOUND, "预览路径不存在");
    };
    let Some((secret, tail)) = rest.split_once("/proxy") else {
        return proxy_err(StatusCode::NOT_FOUND, "预览路径不存在");
    };
    if !(tail.is_empty() || tail.starts_with('/')) {
        return proxy_err(StatusCode::NOT_FOUND, "预览路径不存在");
    }
    if !secret_matches(secret) {
        return proxy_err(StatusCode::UNAUTHORIZED, "预览口令无效");
    }
    let tail = tail.trim_start_matches('/');
    let (port_s, upstream_path) = match tail.split_once('/') {
        Some((p, q)) => (p, q.to_string()),
        None => (tail, String::new()),
    };
    let Ok(port) = port_s.parse::<u16>() else {
        return proxy_err(StatusCode::BAD_REQUEST, "预览端口非法");
    };
    if port == 0 {
        return proxy_err(StatusCode::BAD_REQUEST, "预览端口非法");
    }

    let query = uri.query().map(|q| format!("?{q}")).unwrap_or_default();
    // 目标恒为回环字面量：不接受任何调用方提供的 host（SSRF 面收口）。
    let upstream = format!("http://127.0.0.1:{port}/{upstream_path}{query}");

    let mut fwd = HeaderMap::new();
    for (name, value) in req.headers() {
        if is_hop_by_hop(name) || name == header::HOST || name == header::ACCEPT_ENCODING {
            continue;
        }
        fwd.append(name.clone(), value.clone());
    }
    // 上游不压缩：响应体要就地改写（注入桥脚本 / 重写 URL），压缩流改不了。
    fwd.insert(
        header::ACCEPT_ENCODING,
        HeaderValue::from_static("identity"),
    );

    let method = reqwest::Method::from_bytes(req.method().as_str().as_bytes())
        .unwrap_or(reqwest::Method::GET);
    let body = match axum::body::to_bytes(req.into_body(), MAX_BODY).await {
        Ok(b) => b,
        Err(_) => return proxy_err(StatusCode::PAYLOAD_TOO_LARGE, "预览请求体过大"),
    };

    let sent = upstream_client()
        .request(method, &upstream)
        .headers(fwd)
        .body(body.to_vec())
        .send()
        .await;
    let resp = match sent {
        Ok(r) => r,
        Err(e) => {
            return proxy_err(
                StatusCode::BAD_GATEWAY,
                &format!("无法连接 127.0.0.1:{port}（dev server 未启动？）：{e}"),
            )
        }
    };

    let status = resp.status();
    let up_headers = resp.headers().clone();
    let content_type = up_headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let raw = match resp.bytes().await {
        Ok(b) => b,
        Err(e) => {
            return proxy_err(
                StatusCode::BAD_GATEWAY,
                &format!("读取 dev server 响应失败：{e}"),
            )
        }
    };

    let prefix = format!("{PREVIEW_ROUTE}{secret}/proxy/{port}/");
    let transformed = transform_body(&content_type, &raw, &prefix);

    let mut out = HeaderMap::new();
    for (name, value) in &up_headers {
        if is_hop_by_hop(name)
            || name == header::CONTENT_LENGTH
            || name == header::CONTENT_ENCODING
            // 去嵌入限制：这是预览能工作的前提
            || name == header::X_FRAME_OPTIONS
            || name == header::CONTENT_SECURITY_POLICY
            || name == "x-content-security-policy"
            || name == header::STRICT_TRANSPORT_SECURITY
        {
            continue;
        }
        if name == header::LOCATION {
            if let Ok(v) = value.to_str() {
                out.insert(
                    header::LOCATION,
                    HeaderValue::from_str(&rewrite_location(v, port, &prefix))
                        .unwrap_or_else(|_| value.clone()),
                );
                continue;
            }
        }
        out.append(name.clone(), value.clone());
    }
    if let Some(csp) = up_headers
        .get(header::CONTENT_SECURITY_POLICY)
        .and_then(|v| v.to_str().ok())
    {
        let stripped = strip_frame_ancestors(csp);
        if !stripped.trim().is_empty() {
            if let Ok(v) = HeaderValue::from_str(&stripped) {
                out.insert(header::CONTENT_SECURITY_POLICY, v);
            }
        }
    }
    if transformed.is_some() {
        // 响应体已改，验证器必然失配，去掉避免 304 回空体。
        out.remove(header::ETAG);
        out.remove(header::LAST_MODIFIED);
    }
    out.insert(
        HeaderName::from_static("referrer-policy"),
        HeaderValue::from_static("no-referrer"),
    );

    let mut response = Response::new(Body::from(transformed.unwrap_or_else(|| raw.to_vec())));
    *response.status_mut() = status;
    *response.headers_mut() = out;
    response
}

fn proxy_err(status: StatusCode, msg: &str) -> Response {
    (status, Json(json!({"error": msg}))).into_response()
}

/// 去掉 CSP 中的 `frame-ancestors` 指令，其余指令原样保留。
fn strip_frame_ancestors(csp: &str) -> String {
    csp.split(';')
        .map(str::trim)
        .filter(|d| !d.is_empty() && !d.to_ascii_lowercase().starts_with("frame-ancestors"))
        .collect::<Vec<_>>()
        .join("; ")
}

/// `Location` 指向本机 dev server 时改写成代理前缀（否则跨源跳转会绕开代理）。
fn rewrite_location(value: &str, port: u16, prefix: &str) -> String {
    let port_str = port.to_string();
    for host in ["http://127.0.0.1:", "http://localhost:", "http://[::1]:"] {
        if let Some(rest) = value.strip_prefix(host) {
            if let Some((p, path)) = rest.split_once('/') {
                if p == port_str {
                    return format!("{prefix}{path}");
                }
            }
        }
    }
    value.to_string()
}

/// 按响应类型改写响应体；不可改写（二进制 / 非 UTF-8 / 无关类型）返回 None。
fn transform_body(content_type: &str, body: &[u8], prefix: &str) -> Option<Vec<u8>> {
    let lower = content_type.to_ascii_lowercase();
    let is_markup = lower.contains("text/html") || lower.contains("application/xhtml");
    let is_js = lower.contains("javascript") || lower.contains("ecmascript");
    let is_css = lower.contains("text/css");
    if !(is_markup || is_js || is_css) {
        return None;
    }
    let text = std::str::from_utf8(body).ok()?;
    let base = prefix.trim_end_matches('/');
    if is_markup {
        let rewritten = rewrite_markup(text, base);
        Some(inject_bridge(&rewritten).into_bytes())
    } else if is_js {
        Some(rewrite_js(text, base).into_bytes())
    } else {
        Some(rewrite_css(text, base).into_bytes())
    }
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$'
}

/// 根绝对 URL 才需补前缀：`/x` 补，`//cdn/x`、`http://…`、`#x`、`data:…` 不补。
fn needs_prefix(value: &str) -> bool {
    value.starts_with('/') && !value.starts_with("//") && !value.starts_with(PREVIEW_ROUTE)
}

/// HTML 属性重写（href / src / action / poster）——Vite / Next 的入口脚本、
/// 样式表、图片都是根绝对 URL。
fn rewrite_markup(body: &str, prefix: &str) -> String {
    const PATTERNS: [&str; 4] = ["href=", "src=", "action=", "poster="];
    let mut out = String::with_capacity(body.len() + 256);
    let mut last = 0usize;
    let mut i = 0usize;
    while i < body.len() {
        let mut hit: Option<usize> = None;
        for p in PATTERNS {
            if let Some(rel) = body[i..].find(p) {
                let at = i + rel;
                if hit.map(|h| at < h).unwrap_or(true) {
                    hit = Some(at + p.len());
                }
            }
        }
        let Some(val_start) = hit else { break };
        let Some(quote) = body[val_start..].chars().next() else {
            break;
        };
        if quote != '"' && quote != '\'' {
            i = val_start;
            continue;
        }
        let vstart = val_start + 1;
        let Some(rel_end) = body[vstart..].find(quote) else {
            break;
        };
        let vend = vstart + rel_end;
        if needs_prefix(&body[vstart..vend]) {
            out.push_str(&body[last..vstart]);
            out.push_str(prefix);
            last = vstart;
        }
        i = vend + 1;
    }
    out.push_str(&body[last..]);
    out
}

/// JS 模块说明符重写：`from "…"` / `import "…"` / `import("…")` 三类。
fn rewrite_js(body: &str, prefix: &str) -> String {
    let bytes = body.as_bytes();
    let mut out = String::with_capacity(body.len() + 256);
    let mut last = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'"' || b == b'\'' {
            let mut j = i;
            while j > 0 && (bytes[j - 1] == b' ' || bytes[j - 1] == b'\t') {
                j -= 1;
            }
            let mut keyword = false;
            for kw in ["from", "import", "export"] {
                if j >= kw.len()
                    && &body[j - kw.len()..j] == kw
                    && (j == kw.len() || !is_ident_byte(bytes[j - kw.len() - 1]))
                {
                    keyword = true;
                }
            }
            // `import("/x")`：引号前是 `(`，再往前找 import
            if !keyword && j > 0 && bytes[j - 1] == b'(' {
                let mut k = j - 1;
                while k > 0 && (bytes[k - 1] == b' ' || bytes[k - 1] == b'\t') {
                    k -= 1;
                }
                if k >= 6 && &body[k - 6..k] == "import" && (k == 6 || !is_ident_byte(bytes[k - 7]))
                {
                    keyword = true;
                }
            }
            let value_starts = i + 1 < bytes.len() && bytes[i + 1] == b'/';
            let not_protocol_relative = i + 2 >= bytes.len() || bytes[i + 2] != b'/';
            let already = body[i + 1..].starts_with(PREVIEW_ROUTE);
            if keyword && value_starts && not_protocol_relative && !already {
                out.push_str(&body[last..i + 1]);
                out.push_str(prefix);
                last = i + 1;
            }
        }
        i += 1;
    }
    out.push_str(&body[last..]);
    out
}

/// CSS `url(/x)` 重写。
fn rewrite_css(body: &str, prefix: &str) -> String {
    let bytes = body.as_bytes();
    let mut out = String::with_capacity(body.len() + 128);
    let mut last = 0usize;
    let mut i = 0usize;
    while let Some(rel) = body[i..].find("url(") {
        let mut vs = i + rel + 4;
        if vs < bytes.len() && (bytes[vs] == b'"' || bytes[vs] == b'\'') {
            vs += 1;
        }
        let plain = vs < bytes.len()
            && bytes[vs] == b'/'
            && (vs + 1 >= bytes.len() || bytes[vs + 1] != b'/')
            && !body[vs..].starts_with(PREVIEW_ROUTE);
        if plain {
            out.push_str(&body[last..vs]);
            out.push_str(prefix);
            last = vs;
        }
        i = if vs > i + rel { vs } else { i + rel + 4 };
    }
    out.push_str(&body[last..]);
    out
}

/// 控制台桥：console / onerror / unhandledrejection → `postMessage` 给宿主 UI。
/// 注入到 `<head>` 之后（先于应用脚本），保证早期日志也能捕获。
fn inject_bridge(html: &str) -> String {
    let script = BRIDGE_SCRIPT;
    let lower = html.to_ascii_lowercase();
    for anchor in ["<head>", "<head ", "<html>", "<html ", "<body>", "<body "] {
        if let Some(at) = lower.find(anchor) {
            if let Some(close_rel) = html[at..].find('>') {
                let insert_at = at + close_rel + 1;
                let mut out = String::with_capacity(html.len() + script.len());
                out.push_str(&html[..insert_at]);
                out.push_str(script);
                out.push_str(&html[insert_at..]);
                return out;
            }
        }
    }
    format!("{script}{html}")
}

/// 桥脚本（纯字符串常量：注入到预览页，把控制台事件转给宿主 UI）。
const BRIDGE_SCRIPT: &str = r#"<script data-tenon-preview="1">(function(){
if(window.__tenonPreviewBridge)return;window.__tenonPreviewBridge=true;
var m=/[?&]__tenon=([^&#]*)/.exec(location.search);
var CHANNEL=m?decodeURIComponent(m[1]):"";
try{
  if(CHANNEL)sessionStorage.setItem("__tenon_preview_channel",CHANNEL);
  else CHANNEL=sessionStorage.getItem("__tenon_preview_channel")||"";
}catch(e){}
// 频道号随 query 传递，但页内整页导航（根绝对链接）不带 query——落
// sessionStorage，导航后的新文档据此找回归属，nav / console 桥不静默断流。
var post=function(p){try{p.__tenon_preview=true;p.channel=CHANNEL;window.parent.postMessage(p,"*");}catch(e){}};
var fmt=function(v){try{if(typeof v==="string")return v;if(v instanceof Error)return v.stack||String(v);return JSON.stringify(v);}catch(e){return String(v);}};
var emit=function(level,args){
  var parts=[];for(var i=0;i<args.length;i++)parts.push(fmt(args[i]));
  var text=parts.join(" ");if(text.length>4000)text=text.slice(0,4000)+"…";
  post({kind:"console",level:level,text:text,ts:Date.now()});
};
["log","info","warn","error","debug"].forEach(function(level){
  var orig=console[level]?console[level].bind(console):function(){};
  console[level]=function(){emit(level,arguments);orig.apply(null,arguments);};
});
window.addEventListener("error",function(e){
  post({kind:"console",level:"error",text:(e.message||"error")+(e.filename?" @"+e.filename+":"+e.lineno:""),ts:Date.now()});
});
window.addEventListener("unhandledrejection",function(e){
  post({kind:"console",level:"error",text:"unhandledrejection: "+fmt(e.reason),ts:Date.now()});
});
post({kind:"nav",url:location.href});
window.addEventListener("load",function(){post({kind:"nav",url:location.href});});
})();</script>"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markup_rewrites_root_absolute_only() {
        let html = r#"<link href="/style.css" rel="x"><script src='/src/main.tsx'></script>
<img src="//cdn.example.com/a.png"><a href="https://x.dev/y">z</a><img src="/p.png">"#;
        let out = rewrite_markup(html, "/preview/S/proxy/3000");
        assert!(out.contains(r#"href="/preview/S/proxy/3000/style.css""#));
        assert!(out.contains(r#"src='/preview/S/proxy/3000/src/main.tsx'"#));
        assert!(out.contains(r#"src="//cdn.example.com/a.png""#));
        assert!(out.contains(r#"href="https://x.dev/y""#));
        assert!(out.contains(r#"src="/preview/S/proxy/3000/p.png""#));
    }

    #[test]
    fn markup_rewrite_is_idempotent() {
        let html = r#"<script src="/preview/S/proxy/3000/a.js"></script>"#;
        assert_eq!(rewrite_markup(html, "/preview/S/proxy/3000"), html);
    }

    #[test]
    fn js_rewrites_module_specifiers() {
        let js = r#"import "/@vite/client";import {a} from "/src/a.tsx";
const b = await import("/src/b.tsx");const s = split("/");const u = "//cdn/x";"#;
        let out = rewrite_js(js, "/preview/S/proxy/3000");
        assert!(out.contains(r#"import "/preview/S/proxy/3000/@vite/client""#));
        assert!(out.contains(r#"from "/preview/S/proxy/3000/src/a.tsx""#));
        assert!(out.contains(r#"import("/preview/S/proxy/3000/src/b.tsx")"#));
        // 普通字符串字面量与协议相对 URL 不动
        assert!(out.contains(r#"split("/")"#));
        assert!(out.contains(r#""//cdn/x""#));
    }

    #[test]
    fn css_rewrites_url() {
        let css = r#"a{background:url(/img/a.png)}b{background:url("https://x/y.png")}"#;
        let out = rewrite_css(css, "/preview/S/proxy/3000");
        assert!(out.contains("url(/preview/S/proxy/3000/img/a.png)"));
        assert!(out.contains(r#"url("https://x/y.png")"#));
    }

    #[test]
    fn csp_drops_frame_ancestors_keeps_rest() {
        let csp = "default-src 'self'; frame-ancestors 'none'; script-src 'unsafe-inline'";
        assert_eq!(
            strip_frame_ancestors(csp),
            "default-src 'self'; script-src 'unsafe-inline'"
        );
    }

    #[test]
    fn location_rewritten_for_same_port_only() {
        let p = "/preview/S/proxy/3000/";
        assert_eq!(
            rewrite_location("http://127.0.0.1:3000/login", 3000, p),
            "/preview/S/proxy/3000/login"
        );
        assert_eq!(
            rewrite_location("http://localhost:4000/x", 3000, p),
            "http://localhost:4000/x"
        );
        assert_eq!(rewrite_location("/already", 3000, p), "/already");
    }

    #[test]
    fn bridge_injected_after_head() {
        let html = "<html><head><title>t</title></head><body></body></html>";
        let out = inject_bridge(html);
        let head_at = out.find("<head>").unwrap();
        let bridge_at = out.find("data-tenon-preview").unwrap();
        assert!(bridge_at > head_at && bridge_at < out.find("</head>").unwrap());
    }

    #[test]
    fn transform_skips_binary_and_unrelated() {
        assert!(transform_body("image/png", &[0x89, 0x50], "/preview/S/proxy/1/").is_none());
        assert!(transform_body("application/json", b"{}", "/preview/S/proxy/1/").is_none());
        assert!(transform_body("text/html", b"<html></html>", "/preview/S/proxy/1/").is_some());
    }

    #[test]
    fn proxy_path_detection() {
        assert!(is_proxy_path("/preview/abc/proxy/3000/src/main.tsx"));
        assert!(is_proxy_path("/preview/abc/proxy/3000"));
        assert!(!is_proxy_path("/preview/token"));
        assert!(!is_proxy_path("/projects/x/files"));
    }
}
