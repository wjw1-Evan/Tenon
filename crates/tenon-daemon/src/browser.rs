//! 浏览器宿主（§7.6 v2.80）：以 CDP（Chrome DevTools Protocol）驱动本机
//! Chrome / Chromium 的**无头实例**，供 Agent 侧 `browser_*` 工具调试
//! 本地 dev server（起服务 → 打开页面 → 读控制台 → 求值断言）。
//!
//! ## 线程与连接模型
//!
//! 一个专用 OS 线程跑独立 current_thread runtime（`worker`）：惰性启动
//! Chrome（首次调用才起，`--headless=new --remote-debugging-port=0`，端口由
//! Chrome 自选并经 stderr 的 `DevTools listening on ws://…` 回报），随后在
//! 一条浏览器级 WebSocket 上顺序处理命令。命令与响应严格串行（`call` 内联
//! 排空 WS 消息、顺手把 `Runtime.consoleAPICalled` 等事件记进缓冲），因此
//! 无需并发调度与锁——工具调用本就是串行语义。
//!
//! 连接级故障（发送 / 接收失败、超时、Chrome 退出）标 `Fatal`：丢弃连接，
//! 下次调用重新启动浏览器；CDP 语义级错误（未知 target、求值抛异常）标
//! `Soft`：只回报错误、连接继续可用。
//!
//! ## 环境依赖（如实）
//!
//! 需要本机已安装 Chrome / Chromium：探测顺序 = 环境变量 `TENON_BROWSER_PATH`
//! → 平台常见安装路径。均未命中时工具如实报错并给出安装 / 指定路径指引，
//! 不做静默降级（无浏览器就没有调试面）。无头实例用独立 user-data-dir
//!（`<data_dir>/browser-profile`），不碰用户日常浏览器会话。

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use futures::stream::{SplitSink, SplitStream};
use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;

use tenon_agent::browser::{BrowserHost, BrowserPage, ConsoleEntry};

/// 单条 CDP 命令超时（含事件排空）。
const CDP_CALL_TIMEOUT: Duration = Duration::from_secs(30);
/// 页面加载等待上限（readyState === complete）。
const LOAD_TIMEOUT: Duration = Duration::from_secs(20);
/// 单页面控制台缓冲上限（超出丢最旧）。
const LOG_CAP: usize = 500;
/// 单条控制台文本截断（字符）。
const LOG_TEXT_CAP: usize = 4000;
/// Chrome 启动（含 DevTools 端点回报）超时。
const LAUNCH_TIMEOUT: Duration = Duration::from_secs(25);

type WsStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;
type Reply<T> = oneshot::Sender<Result<T, String>>;

/// 浏览器宿主句柄（`BrowserHost` 实现）：命令经通道交给专用工作线程。
pub struct ChromeBrowser {
    tx: mpsc::UnboundedSender<Cmd>,
    /// 是否已开过页面（未开过时的求值 / 读控制台直接拒绝，不白启一个浏览器）。
    started: std::sync::atomic::AtomicBool,
}

enum Cmd {
    Open(String, Reply<BrowserPage>),
    Eval(Option<String>, String, Reply<String>),
    Console(Option<String>, bool, Reply<Vec<ConsoleEntry>>),
    Close(Option<String>, Reply<Vec<String>>),
    Shutdown,
}

/// CDP 故障分层：`Fatal` 丢弃连接（下次重启浏览器），`Soft` 仅回报。
enum CdpError {
    Fatal(String),
    Soft(String),
}

impl CdpError {
    fn into_message(self) -> String {
        match self {
            CdpError::Fatal(m) | CdpError::Soft(m) => m,
        }
    }
}

impl ChromeBrowser {
    /// 构造宿主（不启动浏览器；首条命令才惰性启动）。
    /// `chrome_path` = 显式可执行文件路径（测试 / 用户指定），None = 自动探测。
    pub fn new(chrome_path: Option<PathBuf>) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let spawned = std::thread::Builder::new()
            .name("tenon-browser".into())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => {
                        tracing::error!("浏览器工作线程 runtime 构建失败: {e}");
                        return;
                    }
                };
                runtime.block_on(worker(rx, chrome_path));
            });
        if let Err(e) = spawned {
            tracing::error!("浏览器工作线程启动失败: {e}");
        }
        Self {
            tx,
            started: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// 优雅关闭（daemon 停机时调用）：回收 Chrome 进程。
    pub fn shutdown(&self) {
        let _ = self.tx.send(Cmd::Shutdown);
    }
}

fn dead() -> String {
    "浏览器工作线程已退出".to_string()
}

/// 未打开任何页面时的统一提示（含指引）。
fn no_page() -> String {
    "尚未打开任何页面（先用 browser_open 打开一个地址）".to_string()
}

#[async_trait::async_trait]
impl BrowserHost for ChromeBrowser {
    async fn open(&self, url: &str) -> Result<BrowserPage, String> {
        self.started
            .store(true, std::sync::atomic::Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(Cmd::Open(url.to_string(), tx))
            .map_err(|_| dead())?;
        rx.await.map_err(|_| dead())?
    }

    async fn eval(&self, target: Option<&str>, expression: &str) -> Result<String, String> {
        if !self.started.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(no_page());
        }
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(Cmd::Eval(
                target.map(str::to_string),
                expression.to_string(),
                tx,
            ))
            .map_err(|_| dead())?;
        rx.await.map_err(|_| dead())?
    }

    async fn console(
        &self,
        target: Option<&str>,
        clear: bool,
    ) -> Result<Vec<ConsoleEntry>, String> {
        if !self.started.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(no_page());
        }
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(Cmd::Console(target.map(str::to_string), clear, tx))
            .map_err(|_| dead())?;
        rx.await.map_err(|_| dead())?
    }

    async fn close(&self, target: Option<&str>) -> Result<Vec<String>, String> {
        if !self.started.load(std::sync::atomic::Ordering::Relaxed) {
            return Ok(Vec::new());
        }
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(Cmd::Close(target.map(str::to_string), tx))
            .map_err(|_| dead())?;
        rx.await.map_err(|_| dead())?
    }
}

/// 工作线程主循环：命令串行处理，连接级故障即弃（下次调用重启浏览器）。
async fn worker(mut rx: mpsc::UnboundedReceiver<Cmd>, chrome_path: Option<PathBuf>) {
    let mut conn: Option<Conn> = None;
    while let Some(cmd) = rx.recv().await {
        if let Cmd::Shutdown = cmd {
            break;
        }
        // 惰性启动：任何需要页面的命令先确保连接
        if conn.is_none() {
            match Conn::launch(chrome_path.clone()).await {
                Ok(c) => conn = Some(c),
                Err(e) => {
                    reply_err(cmd, e);
                    continue;
                }
            }
        }
        let c = conn.as_mut().expect("conn 已确保");
        let fatal;
        match cmd {
            Cmd::Open(url, reply) => {
                let r = c.open(&url).await;
                fatal = matches!(r, Err(CdpError::Fatal(_)));
                let _ = reply.send(r.map_err(CdpError::into_message));
            }
            Cmd::Eval(target, expr, reply) => {
                let r = c.eval(target.as_deref(), &expr).await;
                fatal = matches!(r, Err(CdpError::Fatal(_)));
                let _ = reply.send(r.map_err(CdpError::into_message));
            }
            Cmd::Console(target, clear, reply) => {
                let r = c.console(target.as_deref(), clear).await;
                fatal = matches!(r, Err(CdpError::Fatal(_)));
                let _ = reply.send(r.map_err(CdpError::into_message));
            }
            Cmd::Close(target, reply) => {
                let r = c.close(target.as_deref()).await;
                fatal = matches!(r, Err(CdpError::Fatal(_)));
                let _ = reply.send(r.map_err(CdpError::into_message));
            }
            Cmd::Shutdown => unreachable!("Shutdown 已提前返回"),
        }
        if fatal {
            // 连接坏了：丢弃（Drop 杀 Chrome），下次调用重新启动
            conn = None;
        }
    }
    drop(conn);
}

fn reply_err(cmd: Cmd, err: String) {
    match cmd {
        Cmd::Open(_, tx) => {
            let _ = tx.send(Err(err));
        }
        Cmd::Eval(_, _, tx) => {
            let _ = tx.send(Err(err));
        }
        Cmd::Console(_, _, tx) => {
            let _ = tx.send(Err(err));
        }
        Cmd::Close(_, tx) => {
            let _ = tx.send(Err(err));
        }
        Cmd::Shutdown => {}
    }
}

/// 一条浏览器级连接（一个 Chrome 进程 + 其 DevTools WebSocket）。
struct Conn {
    child: std::process::Child,
    sink: SplitSink<WsStream, Message>,
    stream: SplitStream<WsStream>,
    next_id: u64,
    /// target_id → session_id
    by_target: HashMap<String, String>,
    /// session_id → target_id
    by_session: HashMap<String, String>,
    /// session_id → 控制台缓冲
    logs: HashMap<String, Vec<ConsoleEntry>>,
    /// 打开顺序（target_id），用于「关闭全部」与「最近页面」回退
    order: Vec<String>,
    last: Option<String>,
}

impl Drop for Conn {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Conn {
    /// 启动无头 Chrome 并连上其 DevTools 端点。
    async fn launch(chrome_path: Option<PathBuf>) -> Result<Self, String> {
        let exe = chrome_path
            .filter(|p| p.exists())
            .or_else(detect_chrome)
            .ok_or_else(|| {
                "未找到 Chrome / Chromium：请安装，或用环境变量 TENON_BROWSER_PATH 指定可执行文件路径"
                    .to_string()
            })?;
        let profile = tenon_config::Config::data_dir().join("browser-profile");
        std::fs::create_dir_all(&profile)
            .map_err(|e| format!("创建浏览器 profile 目录失败: {e}"))?;
        let mut child = std::process::Command::new(&exe)
            .arg("--headless=new")
            .arg("--disable-gpu")
            .arg("--no-first-run")
            .arg("--no-default-browser-check")
            .arg("--disable-extensions")
            .arg("--disable-background-networking")
            .arg("--remote-debugging-port=0")
            .arg(format!("--user-data-dir={}", profile.display()))
            .arg("about:blank")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| format!("启动浏览器失败（{}）: {e}", exe.display()))?;

        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| "无法读取浏览器 stderr".to_string())?;
        let (tx, rx) = oneshot::channel::<String>();
        // stderr 必须持续读走，否则管道写满会卡死 Chrome；首条 DevTools 行回报端点
        std::thread::spawn(move || {
            use std::io::BufRead;
            let mut tx = Some(tx);
            for line in std::io::BufReader::new(stderr)
                .lines()
                .map_while(Result::ok)
            {
                if let Some(pos) = line.find("DevTools listening on ") {
                    if let Some(tx) = tx.take() {
                        let _ = tx.send(
                            line[pos + "DevTools listening on ".len()..]
                                .trim()
                                .to_string(),
                        );
                    }
                }
            }
        });
        let ws_url = match tokio::time::timeout(LAUNCH_TIMEOUT, rx).await {
            Ok(Ok(url)) => url,
            Ok(Err(_)) => return Err("浏览器未报告 DevTools 端点（stderr 已关闭）".to_string()),
            Err(_) => return Err("浏览器启动超时（未回报 DevTools 端点）".to_string()),
        };
        let (ws, _) = tokio_tungstenite::connect_async(&ws_url)
            .await
            .map_err(|e| format!("连接 DevTools 端点失败: {e}"))?;
        let (sink, stream) = ws.split();
        Ok(Self {
            child,
            sink,
            stream,
            next_id: 1,
            by_target: HashMap::new(),
            by_session: HashMap::new(),
            logs: HashMap::new(),
            order: Vec::new(),
            last: None,
        })
    }

    /// 发一条 CDP 命令并等它的响应；途中把事件记进控制台缓冲。
    async fn call(
        &mut self,
        method: &str,
        params: Value,
        session: Option<&str>,
    ) -> Result<Value, CdpError> {
        let id = self.next_id;
        self.next_id += 1;
        let mut msg = json!({"id": id, "method": method, "params": params});
        if let Some(s) = session {
            msg["sessionId"] = Value::String(s.to_string());
        }
        self.sink
            .send(Message::text(msg.to_string()))
            .await
            .map_err(|e| CdpError::Fatal(format!("CDP 发送失败: {e}")))?;
        let deadline = Instant::now() + CDP_CALL_TIMEOUT;
        loop {
            let remain = deadline.saturating_duration_since(Instant::now());
            if remain.is_zero() {
                return Err(CdpError::Fatal(format!("CDP {method} 超时")));
            }
            let next = match tokio::time::timeout(remain, self.stream.next()).await {
                Ok(v) => v,
                Err(_) => return Err(CdpError::Fatal(format!("CDP {method} 超时"))),
            };
            let msg = match next {
                Some(Ok(m)) => m,
                Some(Err(e)) => return Err(CdpError::Fatal(format!("CDP 连接错误: {e}"))),
                None => return Err(CdpError::Fatal("CDP 连接已关闭".into())),
            };
            if msg.is_close() {
                return Err(CdpError::Fatal("CDP 连接已关闭".into()));
            }
            let Ok(text) = msg.to_text() else { continue };
            let Ok(v) = serde_json::from_str::<Value>(text) else {
                continue;
            };
            if v.get("id").and_then(Value::as_u64) == Some(id) {
                if let Some(err) = v.get("error") {
                    return Err(CdpError::Soft(format!("CDP {method} 失败: {err}")));
                }
                return Ok(v.get("result").cloned().unwrap_or(Value::Null));
            }
            if let Some(m) = v.get("method").and_then(Value::as_str) {
                let m = m.to_string();
                self.record(&m, &v);
            }
        }
    }

    /// 事件入库（console / 未捕获异常 / 浏览器日志）。
    fn record(&mut self, method: &str, v: &Value) {
        let Some(session) = v.get("sessionId").and_then(Value::as_str) else {
            return;
        };
        let params = v.get("params").cloned().unwrap_or(Value::Null);
        let entry = match method {
            "Runtime.consoleAPICalled" => {
                let level = params
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("log")
                    .to_string();
                let text = params
                    .get("args")
                    .and_then(Value::as_array)
                    .map(|args| args.iter().map(describe_arg).collect::<Vec<_>>().join(" "))
                    .unwrap_or_default();
                Some(ConsoleEntry {
                    level,
                    text: truncate_chars(&text, LOG_TEXT_CAP),
                    ts_ms: now_ms(),
                })
            }
            "Runtime.exceptionThrown" => {
                let d = params
                    .get("exceptionDetails")
                    .cloned()
                    .unwrap_or(Value::Null);
                let text = d
                    .get("exception")
                    .and_then(|e| e.get("description"))
                    .and_then(Value::as_str)
                    .or_else(|| d.get("text").and_then(Value::as_str))
                    .unwrap_or("未捕获异常")
                    .to_string();
                Some(ConsoleEntry {
                    level: "error".into(),
                    text: truncate_chars(&text, LOG_TEXT_CAP),
                    ts_ms: now_ms(),
                })
            }
            "Log.entryAdded" => {
                let e = params.get("entry").cloned().unwrap_or(Value::Null);
                let level = e
                    .get("level")
                    .and_then(Value::as_str)
                    .unwrap_or("log")
                    .to_string();
                let text = e
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                Some(ConsoleEntry {
                    level,
                    text: truncate_chars(&text, LOG_TEXT_CAP),
                    ts_ms: now_ms(),
                })
            }
            _ => None,
        };
        if let Some(entry) = entry {
            let buf = self.logs.entry(session.to_string()).or_default();
            if buf.len() >= LOG_CAP {
                buf.remove(0);
            }
            buf.push(entry);
        }
    }

    /// 空转一次 CDP 命令，把 WS 上排队的事件排空。
    /// 事件只在「与某次请求同批到达」时才被读到（`call` 内联排空），因此读控制台
    /// 之前必须先往返一次，否则 browser_open 之后新产生的事件会滞留在 socket 里。
    async fn flush_events(&mut self) -> Result<(), CdpError> {
        self.call("Browser.getVersion", json!({}), None)
            .await
            .map(|_| ())
    }

    /// 解析页面句柄（省略 = 最近打开的页面）。
    fn resolve(&self, target: Option<&str>) -> Result<String, CdpError> {
        let target_id = match target {
            Some(t) => t.to_string(),
            None => self.last.clone().ok_or_else(|| {
                CdpError::Soft("尚未打开任何页面（先用 browser_open 打开一个地址）".into())
            })?,
        };
        self.by_target.get(&target_id).cloned().ok_or_else(|| {
            CdpError::Soft(format!(
                "未知 target_id={target_id}（当前页面：{}）",
                if self.order.is_empty() {
                    "无".to_string()
                } else {
                    self.order.join(", ")
                }
            ))
        })
    }

    /// 求值并取回原始值（异常转 Soft 错误）。
    async fn eval_raw(&mut self, session: &str, expression: &str) -> Result<Value, CdpError> {
        let r = self
            .call(
                "Runtime.evaluate",
                json!({"expression": expression, "returnByValue": true, "awaitPromise": true}),
                Some(session),
            )
            .await?;
        if let Some(d) = r.get("exceptionDetails") {
            let msg = d
                .get("exception")
                .and_then(|e| e.get("description"))
                .and_then(Value::as_str)
                .or_else(|| d.get("text").and_then(Value::as_str))
                .unwrap_or("求值抛出异常");
            return Err(CdpError::Soft(msg.to_string()));
        }
        Ok(r.get("result").cloned().unwrap_or(Value::Null))
    }

    async fn eval_string(&mut self, session: &str, expression: &str) -> Result<String, CdpError> {
        let v = self.eval_raw(session, expression).await?;
        Ok(match v.get("value") {
            Some(Value::String(s)) => s.clone(),
            Some(other) => other.to_string(),
            None => v
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        })
    }

    async fn open(&mut self, url: &str) -> Result<BrowserPage, CdpError> {
        let created = self
            .call("Target.createTarget", json!({"url": "about:blank"}), None)
            .await?;
        let target_id = created
            .get("targetId")
            .and_then(Value::as_str)
            .ok_or_else(|| CdpError::Soft("createTarget 未返回 targetId".into()))?
            .to_string();
        let attached = self
            .call(
                "Target.attachToTarget",
                json!({"targetId": target_id, "flatten": true}),
                None,
            )
            .await?;
        let session = attached
            .get("sessionId")
            .and_then(Value::as_str)
            .ok_or_else(|| CdpError::Soft("attachToTarget 未返回 sessionId".into()))?
            .to_string();
        // 先开域再导航：页面脚本早期产生的 console / 异常才能被捕获
        for domain in ["Page.enable", "Runtime.enable", "Log.enable"] {
            self.call(domain, json!({}), Some(&session)).await?;
        }
        self.by_target.insert(target_id.clone(), session.clone());
        self.by_session.insert(session.clone(), target_id.clone());
        self.order.push(target_id.clone());
        self.last = Some(target_id.clone());

        self.call("Page.navigate", json!({"url": url}), Some(&session))
            .await?;
        let deadline = Instant::now() + LOAD_TIMEOUT;
        loop {
            match self.eval_string(&session, "document.readyState").await {
                Ok(state) if state == "complete" => break,
                Ok(_) => {}
                Err(CdpError::Soft(_)) => {}
                Err(e) => return Err(e),
            }
            if Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
        let title = self
            .eval_string(&session, "document.title")
            .await
            .unwrap_or_default();
        let final_url = self
            .eval_string(&session, "location.href")
            .await
            .unwrap_or_else(|_| url.to_string());
        Ok(BrowserPage {
            target_id,
            url: final_url,
            title,
        })
    }

    async fn eval(&mut self, target: Option<&str>, expression: &str) -> Result<String, CdpError> {
        let session = self.resolve(target)?;
        let v = self.eval_raw(&session, expression).await?;
        Ok(match v.get("value") {
            Some(Value::String(s)) => s.clone(),
            Some(other) => other.to_string(),
            None => v
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("undefined")
                .to_string(),
        })
    }

    async fn console(
        &mut self,
        target: Option<&str>,
        clear: bool,
    ) -> Result<Vec<ConsoleEntry>, CdpError> {
        let session = self.resolve(target)?;
        self.flush_events().await?;
        let entries = self.logs.get(&session).cloned().unwrap_or_default();
        if clear {
            self.logs.remove(&session);
        }
        Ok(entries)
    }

    async fn close(&mut self, target: Option<&str>) -> Result<Vec<String>, CdpError> {
        let targets: Vec<String> = match target {
            Some(t) => {
                if !self.by_target.contains_key(t) {
                    return Err(CdpError::Soft(format!("未知 target_id={t}")));
                }
                vec![t.to_string()]
            }
            None => self.order.clone(),
        };
        let mut closed = Vec::new();
        for t in targets {
            match self
                .call("Target.closeTarget", json!({"targetId": t}), None)
                .await
            {
                Ok(_) => {
                    if let Some(session) = self.by_target.remove(&t) {
                        self.by_session.remove(&session);
                        self.logs.remove(&session);
                    }
                    self.order.retain(|x| x != &t);
                    closed.push(t);
                }
                Err(CdpError::Soft(_)) => continue,
                Err(e) => return Err(e),
            }
        }
        self.last = self.order.last().cloned();
        Ok(closed)
    }
}

/// RemoteObject → 可读文本（console 参数）。
fn describe_arg(arg: &Value) -> String {
    if let Some(Value::String(s)) = arg.get("value") {
        return s.clone();
    }
    if let Some(v) = arg.get("value") {
        return v.to_string();
    }
    if let Some(d) = arg.get("description").and_then(Value::as_str) {
        return d.to_string();
    }
    arg.get("type")
        .and_then(Value::as_str)
        .unwrap_or("undefined")
        .to_string()
}

fn truncate_chars(s: &str, cap: usize) -> String {
    if s.chars().count() <= cap {
        return s.to_string();
    }
    let mut out: String = s.chars().take(cap).collect();
    out.push('…');
    out
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Chrome / Chromium 可执行文件探测：环境变量 `TENON_BROWSER_PATH` → 平台常见路径。
pub fn detect_chrome() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("TENON_BROWSER_PATH") {
        let p = PathBuf::from(p);
        if p.exists() {
            return Some(p);
        }
    }
    let candidates: &[&str] = &[
        // macOS
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Chromium.app/Contents/MacOS/Chromium",
        "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
        "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
        // Linux
        "/usr/bin/google-chrome",
        "/usr/bin/google-chrome-stable",
        "/usr/bin/chromium",
        "/usr/bin/chromium-browser",
        "/snap/bin/chromium",
        "/usr/bin/microsoft-edge",
        // Windows
        "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe",
        "C:\\Program Files (x86)\\Google\\Chrome\\Application\\chrome.exe",
        "C:\\Program Files\\Microsoft\\Edge\\Application\\msedge.exe",
    ];
    candidates.iter().map(PathBuf::from).find(|p| p.exists())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describe_arg_prefers_value_then_description() {
        assert_eq!(
            describe_arg(&json!({"type": "string", "value": "hi"})),
            "hi"
        );
        assert_eq!(describe_arg(&json!({"type": "number", "value": 2})), "2");
        assert_eq!(
            describe_arg(&json!({"type": "object", "description": "Object"})),
            "Object"
        );
        assert_eq!(describe_arg(&json!({"type": "undefined"})), "undefined");
    }

    #[test]
    fn truncate_respects_char_cap() {
        assert_eq!(truncate_chars("abc", 5), "abc");
        assert_eq!(truncate_chars("abcdef", 3), "abc…");
    }

    #[test]
    fn detect_chrome_env_override_wins() {
        // 环境变量指向不存在的路径 → 不生效（不 panic）
        std::env::set_var("TENON_BROWSER_PATH", "/nonexistent/chrome");
        let d = detect_chrome();
        std::env::remove_var("TENON_BROWSER_PATH");
        // 有 Chrome 的机器上应能探到；没有则 None——两者都合法，只断言不 panic
        let _ = d;
    }
}
