//! v2.0 域过滤代理进程（design-v2.md §4.4，P1 安全债收口）。
//!
//! 背景：镜像代理态的 registry 白名单此前依赖「未来的代理进程」——Seatbelt
//! 路径 `(allow network*)` 放行全网（`NetworkState` 白名单结构未消费），
//! install_deps 实为「任意命令 + 网络全开」（A2）。本模块补齐缺失的一环：
//!
//! - **本地白名单代理**：daemon 进程内启动（127.0.0.1 随机端口），仅放行
//!   `NetworkState::mirror_default()` 预授权 registry 域（CONNECT 隧道 +
//!   绝对 URI 转发两形态），其余一律 403；
//! - **沙箱收紧**（seatbelt.rs 消费）：MirrorProxy 态从 `(allow network*)`
//!   改为 **deny network\* + 仅放行回环代理端口**——沙箱内命令的出网唯一
//!   通路是本代理，域名解析与直连全部被沙箱拒绝；
//! - **fail-closed**：代理不可用时 install_deps 在工具层直接拒绝（不静默
//!   回退全网放行）；`MirrorProxy { proxy: None }` 的 profile 同样 deny all
//!   （纵深防御，§3.3 DSH「无后端显式失败、禁止静默透传」口径）。
//!
//! TLS 不解密：CONNECT 隧道端到端（客户端 ↔ registry），代理只见主机名——
//! 白名单过滤与审计不需要正文，亦零证书面。Linux 侧沙箱（Landlock+seccomp）
//! 的镜像态回环收紧为后续项（当前 Linux 镜像态无网络限制，见 §12.3 标注）。

use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

/// 代理读写超时：包管理器慢速下载以 30s 无字节为断流判据（隧道保持靠数据流）。
const IO_TIMEOUT: Duration = Duration::from_secs(30);
/// 请求头读取上限与接收窗口（畸形请求不占内存）。
const HEAD_LIMIT: usize = 16 * 1024;

/// 本地 registry 白名单代理句柄。`Drop` 置停（接受循环 50ms 内退出；
/// 在途隧道随连接超时自然终结）。
pub struct RegistryProxy {
    addr: SocketAddr,
    shutdown: Arc<AtomicBool>,
}

impl RegistryProxy {
    /// 在 127.0.0.1 随机端口启动，仅放行 `allowlist` 中的主机（精确匹配）。
    pub fn start(allowlist: Vec<String>) -> std::io::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let addr = listener.local_addr()?;
        let shutdown = Arc::new(AtomicBool::new(false));
        let allow: Arc<BTreeSet<String>> = Arc::new(allowlist.into_iter().collect());
        let flag = shutdown.clone();
        // 非阻塞接受 + 50ms 轮询停机位（std 无跨平台 accept 超时）
        listener.set_nonblocking(true)?;
        std::thread::Builder::new()
            .name("tenon-registry-proxy".into())
            .spawn(move || loop {
                if flag.load(Ordering::Relaxed) {
                    break;
                }
                match listener.accept() {
                    Ok((sock, _)) => {
                        let allow = allow.clone();
                        let _ = std::thread::Builder::new()
                            .name("tenon-registry-proxy-conn".into())
                            .spawn(move || handle_conn(sock, &allow));
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(50));
                    }
                    Err(_) => break,
                }
            })?;
        Ok(Self { addr, shutdown })
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }
}

impl Drop for RegistryProxy {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
    }
}

/// 单条连接：解析请求头 → 白名单判定 → 隧道 / 转发 / 403。
fn handle_conn(mut client: TcpStream, allow: &BTreeSet<String>) {
    // accepted socket 可能继承监听器的非阻塞标志（macOS 实测）——显式复原阻塞，
    // 超时语义交由 read/write timeout 承载
    let _ = client.set_nonblocking(false);
    let _ = client.set_read_timeout(Some(IO_TIMEOUT));
    let _ = client.set_write_timeout(Some(IO_TIMEOUT));
    let Some((head, rest)) = read_head(&mut client) else {
        return;
    };
    let Some((method, target)) = parse_request_line(&head) else {
        let _ = client.write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n");
        return;
    };
    match method.as_str() {
        "CONNECT" => {
            // CONNECT host[:port]——HTTPS 隧道（npm/pip/cargo 等主通路）
            let (host, port) = split_host_port(&target, 443);
            if !allow.contains(&host) {
                deny(&mut client, &host);
                return;
            }
            let upstream = match TcpStream::connect((host.as_str(), port)) {
                Ok(s) => s,
                Err(_) => {
                    let _ = client.write_all(b"HTTP/1.1 502 Bad Gateway\r\n\r\n");
                    return;
                }
            };
            if client
                .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                .is_err()
            {
                return;
            }
            let _ = upstream.set_read_timeout(Some(IO_TIMEOUT));
            let _ = upstream.set_write_timeout(Some(IO_TIMEOUT));
            tunnel(client, upstream, rest);
        }
        // 绝对 URI 代理形态（METHOD http://host[:port]/path）——明文 HTTP 兜底
        _ if target.starts_with("http://") => {
            let Some((host, port, path, version)) = split_absolute_uri(&target, &head) else {
                let _ = client.write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n");
                return;
            };
            if !allow.contains(&host) {
                deny(&mut client, &host);
                return;
            }
            let mut upstream = match TcpStream::connect((host.as_str(), port)) {
                Ok(s) => s,
                Err(_) => {
                    let _ = client.write_all(b"HTTP/1.1 502 Bad Gateway\r\n\r\n");
                    return;
                }
            };
            let _ = upstream.set_read_timeout(Some(IO_TIMEOUT));
            let _ = upstream.set_write_timeout(Some(IO_TIMEOUT));
            // 重写请求行为 origin-form，其余头原样转发
            let _ = upstream.write_all(format!("{method} {path} {version}\r\n").as_bytes());
            let _ = upstream.write_all(&rest);
            relay(client, upstream);
        }
        // 相对 URI / 其他形态不是代理请求——拒绝（非白名单语义的绕行面）
        _ => {
            let _ = client.write_all(b"HTTP/1.1 403 Forbidden\r\n\r\n");
        }
    }
}

/// 读取到 `\r\n\r\n`（保留其后的已读字节，隧道期原样转发）。
fn read_head(stream: &mut TcpStream) -> Option<(String, Vec<u8>)> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    loop {
        if let Some(pos) = find_head_end(&buf) {
            let head = String::from_utf8_lossy(&buf[..pos]).into_owned();
            let rest = buf[pos + 4..].to_vec();
            return Some((head, rest));
        }
        if buf.len() > HEAD_LIMIT {
            return None;
        }
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
}

fn find_head_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

/// 首行 → (METHOD, target)。
fn parse_request_line(head: &str) -> Option<(String, String)> {
    let line = head.lines().next()?;
    let mut parts = line.split_whitespace();
    let method = parts.next()?.to_string();
    let target = parts.next()?.to_string();
    if method.is_empty() || target.is_empty() {
        return None;
    }
    Some((method, target))
}

fn split_host_port(target: &str, default_port: u16) -> (String, u16) {
    match target.rsplit_once(':') {
        Some((host, port)) => match port.parse::<u16>() {
            Ok(p) => (host.to_string(), p),
            Err(_) => (target.to_string(), default_port),
        },
        None => (target.to_string(), default_port),
    }
}

/// `http://host[:port]/path?… VER` → (host, port, origin-path, version)。
/// version 从首行尾段取（与 target 同行）。
fn split_absolute_uri(target: &str, head: &str) -> Option<(String, u16, String, String)> {
    let rest = target.strip_prefix("http://")?;
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let (host, port) = split_host_port(authority, 80);
    let version = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(2))
        .unwrap_or("HTTP/1.1")
        .to_string();
    Some((host, port, path.to_string(), version))
}

fn deny(client: &mut TcpStream, host: &str) {
    // 主机名回显仅入本地命令输出（审计可见拒绝原因），不含任何凭据面
    let body = format!("tenon-registry-proxy: host not in allowlist: {host}\n");
    let _ = client.write_all(
        format!(
            "HTTP/1.1 403 Forbidden\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
        .as_bytes(),
    );
}

/// CONNECT 隧道：双向转发（`rest` 为握手期已读的客户端首段）。
fn tunnel(client: TcpStream, upstream: TcpStream, rest: Vec<u8>) {
    let mut up = upstream;
    let _ = up.write_all(&rest);
    relay(client, up);
}

/// 双向中继：一个方向的复制放独立线程，本线程做另一方向；任一方向断流
/// （EOF / 超时 / 错误）即结束——连接 Drop 促使对端线程在下个超时窗退出。
fn relay(client: TcpStream, upstream: TcpStream) {
    let up_clone = match upstream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    };
    let client_clone = match client.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    };
    let _ = std::thread::spawn(move || {
        let _ = std::io::copy(&mut &client_clone, &mut &up_clone);
    });
    let _ = std::io::copy(&mut &upstream, &mut &client);
}

/// 全局单例：daemon 进程生命周期内常驻（首次 install_deps 惰性启动），
/// 白名单取 `NetworkState::mirror_default()`（单一事实源，§12.3）。
static GLOBAL: OnceLock<RegistryProxy> = OnceLock::new();
static GLOBAL_ERR: Mutex<Option<String>> = Mutex::new(None);

pub fn registry_proxy() -> Result<SocketAddr, String> {
    if let Some(p) = GLOBAL.get() {
        return Ok(p.addr());
    }
    let allowlist = match crate::network::NetworkState::mirror_default() {
        crate::network::NetworkState::MirrorProxy { registries, .. } => {
            registries.into_iter().collect()
        }
        _ => unreachable!("mirror_default 恒为 MirrorProxy"),
    };
    match RegistryProxy::start(allowlist) {
        Ok(p) => {
            let addr = p.addr();
            match GLOBAL.set(p) {
                Ok(()) => Ok(addr),
                // 并发竞态：另一线程已启动——用既有实例，本实例 Drop 自停
                Err(_) => Ok(GLOBAL.get().map(|p| p.addr()).unwrap_or(addr)),
            }
        }
        Err(e) => {
            *GLOBAL_ERR.lock().unwrap() = Some(format!("registry proxy 启动失败: {e}"));
            Err("registry proxy 不可用（启动失败）——install_deps fail-closed".to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试用 echo 原点服务器：收到即回（模拟 registry 端点）。
    fn echo_server() -> SocketAddr {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for sock in listener.incoming() {
                let mut s = sock.unwrap();
                let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
                let mut buf = [0u8; 1024];
                match s.read(&mut buf) {
                    Ok(n) if n > 0 => {
                        let _ = s.write_all(&buf[..n]);
                    }
                    _ => {}
                }
            }
        });
        addr
    }

    fn proxy_for(allow: &[&str]) -> RegistryProxy {
        RegistryProxy::start(allow.iter().map(|s| s.to_string()).collect()).unwrap()
    }

    #[test]
    fn connect_tunnel_allows_allowlisted_host() {
        let origin = echo_server();
        let proxy = proxy_for(&["127.0.0.1"]);
        let mut c = TcpStream::connect(proxy.addr()).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        c.write_all(
            format!(
                "CONNECT 127.0.0.1:{} HTTP/1.1\r\nHost: x\r\n\r\n",
                origin.port()
            )
            .as_bytes(),
        )
        .unwrap();
        let mut head = vec![0u8; 64];
        let n = c.read(&mut head).unwrap();
        let resp = String::from_utf8_lossy(&head[..n]).into_owned();
        assert!(resp.starts_with("HTTP/1.1 200"), "应建隧道: {resp}");
        // 隧道期数据透传（echo 回环）
        c.write_all(b"ping-through-tunnel").unwrap();
        let mut back = vec![0u8; 64];
        let n = c.read(&mut back).unwrap();
        assert_eq!(&back[..n], b"ping-through-tunnel");
    }

    #[test]
    fn connect_tunnel_denies_non_allowlisted_host() {
        let proxy = proxy_for(&["registry.npmjs.org"]);
        let mut c = TcpStream::connect(proxy.addr()).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        c.write_all(b"CONNECT evil.example:443 HTTP/1.1\r\nHost: x\r\n\r\n")
            .unwrap();
        let mut head = vec![0u8; 512];
        let n = c.read(&mut head).unwrap();
        let resp = String::from_utf8_lossy(&head[..n]).into_owned();
        assert!(resp.starts_with("HTTP/1.1 403"), "应拒绝: {resp}");
        assert!(resp.contains("evil.example"), "拒绝理由应含主机名: {resp}");
    }

    #[test]
    fn absolute_uri_forwarding_allow_and_deny() {
        let origin = echo_server();
        let proxy = proxy_for(&["127.0.0.1"]);
        // 放行：绝对 URI 请求被转发为 origin-form（分段到达只断言首行——
        // 首行重写即证明代理转发语义，echo 原点单次读可能截到分段）
        let mut c = TcpStream::connect(proxy.addr()).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        c.write_all(
            format!(
                "GET http://127.0.0.1:{}/pkg HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n",
                origin.port()
            )
            .as_bytes(),
        )
        .unwrap();
        let mut back = vec![0u8; 512];
        let n = c.read(&mut back).unwrap();
        assert!(
            back[..n].starts_with(b"GET /pkg HTTP/1.1\r\n"),
            "应为 origin-form: {:?}",
            String::from_utf8_lossy(&back[..n])
        );
        // 拒绝：非白名单域
        let mut c2 = TcpStream::connect(proxy.addr()).unwrap();
        c2.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        c2.write_all(b"GET http://evil.example/x HTTP/1.1\r\nHost: evil.example\r\n\r\n")
            .unwrap();
        let mut back2 = vec![0u8; 512];
        let n2 = c2.read(&mut back2).unwrap();
        assert!(String::from_utf8_lossy(&back2[..n2]).starts_with("HTTP/1.1 403"));
    }

    #[test]
    fn relative_uri_request_is_rejected() {
        let proxy = proxy_for(&["registry.npmjs.org"]);
        let mut c = TcpStream::connect(proxy.addr()).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        c.write_all(b"GET /pkg HTTP/1.1\r\nHost: registry.npmjs.org\r\n\r\n")
            .unwrap();
        let mut back = vec![0u8; 128];
        let n = c.read(&mut back).unwrap();
        assert!(String::from_utf8_lossy(&back[..n]).starts_with("HTTP/1.1 403"));
    }

    #[test]
    fn singleton_returns_stable_addr() {
        let a = registry_proxy().unwrap();
        let b = registry_proxy().unwrap();
        assert_eq!(a, b);
    }
}
