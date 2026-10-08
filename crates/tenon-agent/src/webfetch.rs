//! 出网拉取共享层（§9.2 v1.182）：SSRF 守卫 + 限长抓取 + DuckDuckGo Lite 搜索。
//!
//! `http_fetch` / `web_search` 的出网统一经 [`fetch_public_text`]：scheme 仅
//! http(s)、主机解析后全部地址须公网、连接钉扎首个已验证地址（防解析-连接
//! TOCTOU 的 DNS rebinding）、自动重定向关闭改手动 ≤3 跳逐跳复验、响应体限长。
//! 守卫在 daemon 工具面执行（C 级直执语义与审计不变，§12.2），不经命令沙箱。

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use futures::StreamExt;

/// 搜索后端（免密钥，§9.2 v1.182）：DuckDuckGo Lite HTML 版。
const SEARCH_ENDPOINT: &str = "https://lite.duckduckgo.com/lite/";

/// query 长度上限（字符）。
const QUERY_MAX_CHARS: usize = 400;
/// 结果条数上限与缺省值。
const RESULTS_MAX: usize = 10;
const RESULTS_DEFAULT: usize = 5;
/// web_search 下载帽与总超时（§9.2）。
const SEARCH_BODY_CAP: usize = 512 * 1024;
const SEARCH_TIMEOUT: Duration = Duration::from_secs(10);
/// http_fetch 下载帽（§9.2：模型上下文侧另有 20k 字符截断，executor 侧）。
pub const HTTP_FETCH_BODY_CAP: usize = 1024 * 1024;
/// 重定向跳数上限（自动跟随已关闭，手动逐跳复验）。
const MAX_REDIRECTS: usize = 3;
/// 结果条目截断（字符）。
const TITLE_MAX_CHARS: usize = 200;
const SNIPPET_MAX_CHARS: usize = 300;

/// SSRF 地址判定（§9.2 v1.182）：非公网一律拦截。
/// v4 侧含 loopback / 私网 / 链路本地 / CGNAT 100.64/10 / 基准 198.18/15 /
/// 测试网与 IETF 保留段 / 组播 / 未指定 / 广播；v6 侧 ULA 与链路本地，
/// v4 映射（::ffff:a.b.c.d）按 v4 语义递归判定。
pub fn is_blocked_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_blocked_v4(v4),
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => is_blocked_v4(v4),
            None => {
                v6.is_loopback()
                    || v6.is_unspecified()
                    || v6.is_multicast()
                    || (v6.segments()[0] & 0xfe00) == 0xfc00 // fc00::/7 ULA
                    || (v6.segments()[0] & 0xffc0) == 0xfe80 // fe80::/10 链路本地
            }
        },
    }
}

fn is_blocked_v4(v: Ipv4Addr) -> bool {
    let o = v.octets();
    v.is_loopback()
        || v.is_private()
        || v.is_link_local()
        || v.is_unspecified()
        || v.is_broadcast()
        || v.is_multicast()
        // 100.64/10 CGNAT（第二字节高两位 = 01）
        || (o[0] == 100 && (o[1] & 0b1100_0000) == 0b0100_0000)
        // 198.18/15 基准测试
        || (o[0] == 198 && (o[1] == 18 || o[1] == 19))
        // 测试网 192.0.2/24、198.51.100/24、203.0.113/24 与 IETF 协议段 192.0.0.0/24
        || (o[0] == 192 && o[1] == 0 && o[2] <= 2)
        || (o[0] == 198 && o[1] == 51 && o[2] == 100)
        || (o[0] == 203 && o[1] == 0 && o[2] == 113)
        // 保留段 240/4（含广播，幂等）
        || o[0] >= 240
}

/// 校验 URL 可出网性：scheme 限 http(s)、主机解析后全部地址须公网
/// （任一非公网即整体拒绝，不给部分通过）。返回规范 URL 与钉扎地址
/// （多地址取首个已验证者，连接失败直接报错、不静默回退未验证地址）。
pub async fn validate_url(raw: &str) -> Result<(reqwest::Url, SocketAddr), String> {
    let url = reqwest::Url::parse(raw).map_err(|e| format!("URL 解析失败: {e}"))?;
    match url.scheme() {
        "http" | "https" => {}
        other => return Err(format!("scheme \"{other}\" 不允许，仅 http(s)")),
    }
    let Some(host) = url.host_str() else {
        return Err("URL 缺少主机".into());
    };
    let port = url
        .port_or_known_default()
        .unwrap_or(if url.scheme() == "https" { 443 } else { 80 });
    // 字面 IP 直判不查 DNS；域名经系统解析器，全部结果须公网
    let addrs: Vec<SocketAddr> = if let Ok(ip) = host.parse::<IpAddr>() {
        vec![SocketAddr::new(ip, port)]
    } else {
        tokio::net::lookup_host(format!("{host}:{port}"))
            .await
            .map_err(|e| format!("主机解析失败: {e}"))?
            .collect()
    };
    for addr in &addrs {
        if is_blocked_ip(addr.ip()) {
            return Err(format!(
                "SSRF 守卫：目标解析到非公网地址 {}，拒绝",
                addr.ip()
            ));
        }
    }
    addrs
        .into_iter()
        .next()
        .map(|pin| (url, pin))
        .ok_or_else(|| "主机解析结果为空".into())
}

/// SSRF 守卫抓取：手动重定向 ≤[`MAX_REDIRECTS`] 跳（逐跳完整复验），总限时
/// `timeout`，响应体超 `body_cap` 即拒绝。返回 (状态码, UTF-8 文本)。
pub async fn fetch_public_text(
    raw: &str,
    body_cap: usize,
    timeout: Duration,
) -> Result<(u16, String), String> {
    let fut = async {
        let mut current = raw.to_string();
        for _hop in 0..=MAX_REDIRECTS {
            let (url, pin) = validate_url(&current).await?;
            let host = url.host_str().ok_or("URL 缺少主机")?.to_string();
            let client = reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                // 钉扎已验证地址：连接层不再查 DNS，解析-连接窗口关闭
                .resolve(host.as_str(), pin)
                .build()
                .map_err(|e| format!("HTTP client 构建失败: {e}"))?;
            let resp = client
                .get(url.clone())
                .send()
                .await
                .map_err(|e| format!("请求失败: {e}"))?;
            if resp.status().is_redirection() {
                let loc = resp
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .ok_or("重定向缺少 Location")?;
                let next = url
                    .join(loc)
                    .map_err(|e| format!("重定向 Location 非法: {e}"))?;
                current = next.to_string();
                continue;
            }
            let status = resp.status().as_u16();
            let bytes = read_capped(resp, body_cap).await?;
            return Ok((status, String::from_utf8_lossy(&bytes).into_owned()));
        }
        Err(format!("重定向超过 {MAX_REDIRECTS} 跳上限"))
    };
    tokio::time::timeout(timeout, fut)
        .await
        .map_err(|_| "出网请求超时".to_string())?
}

/// 流式读取响应体，超帽即拒绝（不信任 Content-Length，边收边记）。
async fn read_capped(resp: reqwest::Response, cap: usize) -> Result<Vec<u8>, String> {
    let mut buf = Vec::new();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("读取响应失败: {e}"))?;
        if buf.len() + chunk.len() > cap {
            return Err(format!("响应体超过 {cap} 字节上限"));
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}

/// web_search 参数校验（§9.2 v1.182）：query trim 非空 ≤400 字符、
/// max_results 1–10 缺省 5；失败返回错误提示、模型可重试不暂停。
pub fn validate_search_args(
    query: &str,
    max_results: Option<u64>,
) -> Result<(String, usize), String> {
    let q = query.trim();
    if q.is_empty() {
        return Err("query 不能为空".into());
    }
    if q.chars().count() > QUERY_MAX_CHARS {
        return Err(format!("query 超过 {QUERY_MAX_CHARS} 字符上限"));
    }
    let max = match max_results {
        None => RESULTS_DEFAULT,
        Some(n) if (1..=RESULTS_MAX as u64).contains(&n) => n as usize,
        Some(n) => return Err(format!("max_results 须在 1–{RESULTS_MAX}，收到 {n}")),
    };
    Ok((q.to_string(), max))
}

/// web_search 生产入口（§9.2 v1.182）：守卫抓取 Lite 版结果页并解析为
/// `[{title, url, snippet}]` JSON；零结果返回提示（模型可改写关键词重试）。
pub async fn web_search(query: String, max_results: usize) -> Result<String, String> {
    let url = format!("{SEARCH_ENDPOINT}?q={}", urlencode(&query));
    let (status, html) = fetch_public_text(&url, SEARCH_BODY_CAP, SEARCH_TIMEOUT).await?;
    if status != 200 {
        return Err(format!("搜索后端返回 HTTP {status}"));
    }
    let items: Vec<serde_json::Value> = parse_lite(&html, max_results)
        .into_iter()
        .map(|r| serde_json::json!({"title": r.title, "url": r.url, "snippet": r.snippet}))
        .collect();
    if items.is_empty() {
        return Ok("无搜索结果，可改写关键词重试".into());
    }
    serde_json::to_string(&items).map_err(|e| format!("结果序列化失败: {e}"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiteResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

/// 解析 DuckDuckGo Lite 结果页（best-effort）：`result-link` 锚提取标题与
/// 链接（`/l/?uddg=` 跳转还原），`result-snippet` 单元提取摘要；按出现顺序
/// 配对，最多 `max` 条。
pub fn parse_lite(html: &str, max: usize) -> Vec<LiteResult> {
    let links = extract_links(html);
    let snippets = extract_snippets(html);
    let mut out = Vec::new();
    for (idx, (href, title)) in links.into_iter().enumerate() {
        if out.len() >= max {
            break;
        }
        let snippet = snippets.get(idx).map(String::as_str).unwrap_or("");
        out.push(LiteResult {
            title: clip(
                &normalize_ws(&decode_entities(&strip_tags(&title))),
                TITLE_MAX_CHARS,
            ),
            url: resolve_result_url(&href),
            snippet: clip(
                &normalize_ws(&decode_entities(&strip_tags(snippet))),
                SNIPPET_MAX_CHARS,
            ),
        });
    }
    out
}

/// 顺序提取 (href, 标题文本) 对。
fn extract_links(html: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut from = 0usize;
    while let Some(rel) = html[from..].find("result-link") {
        let marker = from + rel;
        let Some(tag_start) = html[..marker].rfind("<a") else {
            break;
        };
        let Some(gt_rel) = html[marker..].find('>') else {
            break;
        };
        let gt = marker + gt_rel;
        let tag = &html[tag_start..=gt];
        let Some(href) = tag_attr(tag, "href") else {
            from = gt + 1;
            continue;
        };
        let Some(close_rel) = html[gt..].find("</a>") else {
            break;
        };
        let title = &html[gt + 1..gt + close_rel];
        out.push((href, title.to_string()));
        from = gt + close_rel + 4;
    }
    out
}

/// 顺序提取摘要单元格文本。
fn extract_snippets(html: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut from = 0usize;
    while let Some(rel) = html[from..].find("result-snippet") {
        let marker = from + rel;
        let Some(gt_rel) = html[marker..].find('>') else {
            break;
        };
        let gt = marker + gt_rel;
        let Some(end_rel) = html[gt..].find("</td>") else {
            break;
        };
        out.push(html[gt + 1..gt + end_rel].to_string());
        from = gt + end_rel + 5;
    }
    out
}

/// 从标签文本提取属性值（单双引号或无引号）。
fn tag_attr(tag: &str, attr: &str) -> Option<String> {
    let pat = format!("{attr}=");
    let p = tag.to_ascii_lowercase().find(&pat)?;
    let rest = tag[p + pat.len()..].trim_start();
    match rest.chars().next()? {
        q @ ('"' | '\'') => {
            let end = rest[1..].find(q)?;
            Some(rest[1..1 + end].to_string())
        }
        _ => {
            let end = rest
                .find(|c: char| c.is_whitespace() || c == '>')
                .unwrap_or(rest.len());
            Some(rest[..end].to_string())
        }
    }
}

/// 还原结果链接：Lite 的 `/l/?uddg=<percent 编码>` 跳转取回真实 URL，
/// 协议相对链接补 https。
fn resolve_result_url(href: &str) -> String {
    let h = href.trim();
    if let Some(q) = h.find("uddg=") {
        let enc = &h[q + 5..];
        let end = enc.find('&').unwrap_or(enc.len());
        return percent_decode(&enc[..end]);
    }
    if h.starts_with("//") {
        format!("https:{h}")
    } else {
        h.to_string()
    }
}

/// 剥离残留标签（best-effort：成对 `<...>` 移除）。
fn strip_tags(s: &str) -> String {
    if !s.contains('<') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(p) = rest.find('<') {
        out.push_str(&rest[..p]);
        match rest[p..].find('>') {
            Some(e) => rest = &rest[p + e + 1..],
            None => break,
        }
    }
    out.push_str(rest);
    out
}

/// HTML 实体解码（命名常用集 + 数字 / 十六进制；扫描式解码天然防二次解码）。
fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(p) = rest.find('&') {
        out.push_str(&rest[..p]);
        let tail = &rest[p..];
        let end = tail.find(';').unwrap_or(tail.len());
        let ent = &tail[1..end];
        let decoded = match ent {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            _ => ent.strip_prefix('#').and_then(|num| {
                let hex = num.strip_prefix('x').or_else(|| num.strip_prefix('X'));
                match hex {
                    Some(h) => u32::from_str_radix(h, 16).ok(),
                    None => num.parse::<u32>().ok(),
                }
                .and_then(char::from_u32)
            }),
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &tail[end + 1..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// 空白折叠为单空格并去首尾。
fn normalize_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 字符数截断（超限补省略号）。
fn clip(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max_chars).collect();
    out.push('…');
    out
}

/// query 的最小 percent 编码（unreserved 之外的字节全编码）。
fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// percent 解码（uddg 还原用；非法序列原样保留）。
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Some(v) = std::str::from_utf8(&bytes[i + 1..i + 3])
                .ok()
                .and_then(|h| u8::from_str_radix(h, 16).ok())
            {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv6Addr};

    #[test]
    fn blocked_ip_table() {
        let blocked = [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "172.31.255.255",
            "192.168.1.1",
            "169.254.1.1",
            "0.0.0.0",
            "100.64.0.1",
            "100.127.255.255",
            "198.18.0.1",
            "198.19.255.255",
            "192.0.2.1",
            "198.51.100.7",
            "203.0.113.9",
            "240.0.0.1",
            "255.255.255.255",
            "224.0.0.1",
            "::1",
            "::",
            "fe80::1",
            "fc00::1",
            "fd12::1",
            "ff02::1",
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.1",
            "::ffff:192.168.0.1",
        ];
        for s in blocked {
            let ip: IpAddr = s.parse().unwrap();
            assert!(is_blocked_ip(ip), "{s} 应拦截");
        }
        let public = ["8.8.8.8", "1.1.1.1", "172.32.0.1", "100.128.0.1", "9.9.9.9"];
        for s in public {
            let ip: IpAddr = s.parse().unwrap();
            assert!(!is_blocked_ip(ip), "{s} 应放行");
        }
        let public_v6: IpAddr = "2606:4700:4700::1111".parse::<Ipv6Addr>().unwrap().into();
        assert!(!is_blocked_ip(public_v6));
    }

    #[tokio::test]
    async fn validate_url_rejects_bad_scheme_and_private_targets() {
        assert!(validate_url("ftp://example.com/x").await.is_err());
        assert!(validate_url("file:///etc/passwd").await.is_err());
        for host in [
            "127.0.0.1",
            "10.0.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "[::1]",
        ] {
            let url = format!("http://{host}/x");
            let err = validate_url(&url).await.unwrap_err();
            assert!(err.contains("SSRF"), "{url} 应被 SSRF 守卫拒绝: {err}");
        }
        let https_err = validate_url("https://localhost/x").await.unwrap_err();
        assert!(https_err.contains("SSRF"), "localhost 应拒绝: {https_err}");
        // 公网字面 IP 通过（不发起连接，仅校验）
        let (url, pin) = validate_url("https://8.8.8.8/dns-query").await.unwrap();
        assert_eq!(pin.ip(), IpAddr::from([8, 8, 8, 8]));
        assert_eq!(pin.port(), 443);
        assert_eq!(url.host_str(), Some("8.8.8.8"));
    }

    #[tokio::test]
    async fn fetch_public_text_rejects_loopback_listener() {
        // 守卫在连接前拒绝——监听套接字存在即可，无需真正服务
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let err = fetch_public_text(
            &format!("http://127.0.0.1:{port}/secret"),
            HTTP_FETCH_BODY_CAP,
            Duration::from_secs(5),
        )
        .await
        .unwrap_err();
        assert!(err.contains("SSRF"), "loopback 抓取应被拒绝: {err}");
    }

    #[test]
    fn validate_search_args_bounds() {
        assert!(validate_search_args("   ", None).is_err());
        let long = "a".repeat(401);
        assert!(validate_search_args(&long, None).is_err());
        let ok = validate_search_args("  tenon ide  ", Some(3)).unwrap();
        assert_eq!(ok, ("tenon ide".to_string(), 3));
        assert_eq!(validate_search_args("x", None).unwrap().1, 5);
        assert!(validate_search_args("x", Some(0)).is_err());
        assert!(validate_search_args("x", Some(11)).is_err());
        assert_eq!(validate_search_args("x", Some(10)).unwrap().1, 10);
    }

    const LITE_FIXTURE: &str = r#"<html><body>
<table>
<tr><td>1.</td><td><a rel="nofollow" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fguide%3Fa%3D1%26b%3D2&rut=abc" class='result-link'>Tenon &amp; IDE <b>guide</b></a></td></tr>
<tr><td class='result-snippet'>A &quot;great&quot; intro&#x27;s page&#8230; with   spacing</td></tr>
<tr><td>2.</td><td><a rel="nofollow" href="https://docs.example.org/api" class='result-link'>API 参考</a></td></tr>
<tr><td class='result-snippet'>接口说明文档</td></tr>
</table></body></html>"#;

    #[test]
    fn parse_lite_extracts_results_with_uddg_and_entities() {
        let r = parse_lite(LITE_FIXTURE, 10);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].url, "https://example.com/guide?a=1&b=2");
        assert_eq!(r[0].title, "Tenon & IDE guide");
        assert_eq!(r[0].snippet, "A \"great\" intro's page… with spacing");
        assert_eq!(r[1].url, "https://docs.example.org/api");
        assert_eq!(r[1].title, "API 参考");
        assert_eq!(r[1].snippet, "接口说明文档");
        // max 截断
        assert_eq!(parse_lite(LITE_FIXTURE, 1).len(), 1);
        assert!(parse_lite("<html></html>", 5).is_empty());
    }

    #[test]
    fn clip_and_encoding_helpers() {
        assert_eq!(clip("abcdef", 3), "abc…");
        assert_eq!(clip("abc", 5), "abc");
        assert_eq!(urlencode("a b&c=d"), "a%20b%26c%3Dd");
        assert_eq!(
            percent_decode("https%3A%2F%2Fa.b%2Fc%20d"),
            "https://a.b/c d"
        );
        assert_eq!(percent_decode("plain"), "plain");
        // 非法序列原样保留
        assert_eq!(percent_decode("100%zz"), "100%zz");
    }
}
