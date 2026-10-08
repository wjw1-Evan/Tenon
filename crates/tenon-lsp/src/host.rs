//! LSP 宿主核心：JSON-RPC 多路复用、通知分发、铁律七守卫（ADR-9）。

use crate::guard::{GuardDecision, LspGuard, LspGuardConfig};
use crate::transport::{FrameRead, FrameWrite};
use crate::{RpcError, RpcMessage, METHOD_NOT_FOUND, REQUEST_DENIED};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum LspHostError {
    #[error("LSP 请求超时: {method}")]
    Timeout { method: String },
    #[error("LSP 连接已关闭")]
    Closed,
    #[error("LSP 请求被拒绝: {0}")]
    Denied(String),
    #[error("JSON 错误: {0}")]
    Json(String),
}

/// 服务器 → 宿主的通知（如 publishDiagnostics）。
#[derive(Debug, Clone)]
pub struct Notification {
    pub method: String,
    pub params: serde_json::Value,
}

/// 宿主配置。
#[derive(Debug, Clone)]
pub struct LspHostConfig {
    /// 语言标识（language pack key，如 "typescript"）。
    pub language: String,
    /// 项目根（workspace root）。
    pub root_path: PathBuf,
    /// 铁律七守卫配置（allowed_commands 默认空 = 全拒）。
    pub guard: LspGuardConfig,
    /// initialize.initializationOptions（如 tsserver.path）。
    pub initialization_options: Option<serde_json::Value>,
}

pub struct LspHost {
    root: PathBuf,
    initialization_options: Option<serde_json::Value>,
    next_id: AtomicI64,
    writer: Mutex<Box<dyn FrameWrite>>,
    pending: Arc<Mutex<HashMap<i64, std::sync::mpsc::SyncSender<RpcMessage>>>>,
    subscribers: Arc<Mutex<Vec<std::sync::mpsc::Sender<Notification>>>>,
    guard: Arc<LspGuard>,
    alive: Arc<AtomicBool>,
    reader_handle: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl LspHost {
    /// 接管既有连接的读写半（调用方负责进程或通道建立）。
    pub fn connect(
        config: LspHostConfig,
        reader: Box<dyn FrameRead>,
        writer: Box<dyn FrameWrite>,
    ) -> Arc<Self> {
        let guard = Arc::new(LspGuard::new(config.guard));
        let host = Arc::new(Self {
            root: config.root_path.clone(),
            initialization_options: config.initialization_options.clone(),
            next_id: AtomicI64::new(1),
            writer: Mutex::new(writer),
            pending: Arc::new(Mutex::new(HashMap::new())),
            subscribers: Arc::new(Mutex::new(Vec::new())),
            guard,
            alive: Arc::new(AtomicBool::new(true)),
            reader_handle: Mutex::new(None),
        });
        let reader_thread_host = Arc::clone(&host);
        let handle = std::thread::Builder::new()
            .name(format!("lsp-host-{}", config.language))
            .spawn(move || reader_loop(reader, reader_thread_host))
            .expect("spawn lsp reader");
        *host.reader_handle.lock().expect("handle lock") = Some(handle);
        host
    }

    pub fn guard(&self) -> &LspGuard {
        &self.guard
    }

    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    /// 订阅服务器通知（诊断等）。
    pub fn subscribe(&self) -> std::sync::mpsc::Receiver<Notification> {
        let (tx, rx) = std::sync::mpsc::channel();
        self.subscribers.lock().expect("subs lock").push(tx);
        rx
    }

    /// 客户端 → 服务器请求（多路复用：按 id 路由回响应）。
    pub fn request(
        &self,
        method: &str,
        params: serde_json::Value,
        timeout: Duration,
    ) -> Result<serde_json::Value, LspHostError> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = std::sync::mpsc::sync_channel::<RpcMessage>(1);
        self.pending.lock().expect("pending lock").insert(id, tx);

        let msg = RpcMessage::request(id, method, params);
        let body = serde_json::to_string(&msg).map_err(|e| LspHostError::Json(e.to_string()))?;
        if std::env::var("TENON_LSP_DEBUG").is_ok() {
            eprintln!("[lsp-out-req] id={id} {method}");
        }
        {
            let mut w = self.writer.lock().expect("writer lock");
            if w.write_frame(&body).is_err() {
                self.pending.lock().expect("pending lock").remove(&id);
                return Err(LspHostError::Closed);
            }
        }
        match rx.recv_timeout(timeout) {
            Ok(resp) => {
                if let Some(err) = resp.error {
                    return Err(LspHostError::Denied(err.message));
                }
                Ok(resp.result.unwrap_or(serde_json::Value::Null))
            }
            Err(_) => {
                self.pending.lock().expect("pending lock").remove(&id);
                if !self.is_alive() {
                    Err(LspHostError::Closed)
                } else {
                    Err(LspHostError::Timeout {
                        method: method.to_string(),
                    })
                }
            }
        }
    }

    /// 客户端 → 服务器通知（didOpen / didChange 等）。
    pub fn notify(&self, method: &str, params: serde_json::Value) -> Result<(), LspHostError> {
        let msg = RpcMessage::notification(method, params);
        let body = serde_json::to_string(&msg).map_err(|e| LspHostError::Json(e.to_string()))?;
        let mut w = self.writer.lock().expect("writer lock");
        w.write_frame(&body).map_err(|_| LspHostError::Closed)
    }

    /// LSP initialize 握手。
    pub fn initialize(&self, timeout: Duration) -> Result<serde_json::Value, LspHostError> {
        // 宿主标准能力声明：接收 publishDiagnostics（诊断推送的前提）、
        // 拉取诊断、hover/补全等由请求驱动无需声明
        let mut params = serde_json::json!({
            "processId": std::process::id(),
            // 完整 percent-encode（路径含空格 / 非 ASCII 时裸拼是非法 URI，
            // 严格的服务器（pyright）会工作区匹配失败）；root 同步 canonicalize，
            // 与 didOpen 等文档 URI 同基——符号链接根（/tmp → /private/tmp）不同
            // 前缀会被服务器工作区前缀匹配判成项目外
            "rootUri": format!(
                "file://{}",
                crate::pack::percent_encode_path(
                    &self
                        .root
                        .canonicalize()
                        .unwrap_or_else(|_| self.root.clone())
                        .to_string_lossy()
                )
            ),
            "capabilities": {
                "textDocument": {
                    "publishDiagnostics": {
                        "relatedInformation": true,
                        "versionSupport": true,
                        "tagSupport": { "valueSet": [1, 2] }
                    },
                    "diagnostic": {},
                    "synchronization": { "dynamicRegistration": false, "didSave": true },
                },
                "workspace": { "configuration": true },
            },
        });
        if let Some(opts) = &self.initialization_options {
            params["initializationOptions"] = opts.clone();
        }
        let result = self.request("initialize", params, timeout)?;
        let _ = self.notify("initialized", serde_json::json!({}));
        Ok(result)
    }

    pub fn root_path(&self) -> &PathBuf {
        &self.root
    }

    /// 关闭宿主（终止读线程与传输）。
    pub fn shutdown(&self) {
        self.alive.store(false, Ordering::SeqCst);
        let _ = self.request("shutdown", serde_json::json!(null), Duration::from_secs(2));
        let _ = self.notify("exit", serde_json::json!(null));
        let mut w = self.writer.lock().expect("writer lock");
        w.shutdown();
        if let Ok(mut h) = self.reader_handle.lock() {
            if let Some(handle) = h.take() {
                let _ = handle.join();
            }
        }
    }
}

fn reader_loop(mut reader: Box<dyn FrameRead>, host: Arc<LspHost>) {
    loop {
        if !host.is_alive() {
            break;
        }
        match reader.read_frame() {
            Ok(body) => {
                if std::env::var("TENON_LSP_DEBUG").is_ok() {
                    eprintln!(
                        "[lsp-raw-in] {}",
                        body.chars().take(260).collect::<String>()
                    );
                }
                let msg: RpcMessage = match serde_json::from_str(&body) {
                    Ok(m) => m,
                    Err(_) => continue,
                };
                if msg.is_response() {
                    if let Some(serde_json::Value::Number(n)) = &msg.id {
                        let id = n.as_i64().unwrap_or(-1);
                        if let Some(tx) = host.pending.lock().expect("pending lock").remove(&id) {
                            let _ = tx.send(msg);
                        }
                    }
                } else if msg.is_server_request() {
                    handle_server_request(&host, msg);
                } else if let Some(method) = &msg.method {
                    // 通知：分发订阅者
                    let note = Notification {
                        method: method.clone(),
                        params: msg.params.unwrap_or(serde_json::Value::Null),
                    };
                    let mut subs = host.subscribers.lock().expect("subs lock");
                    subs.retain(|tx| tx.send(note.clone()).is_ok());
                }
            }
            Err(_) => {
                // 连接断开：唤醒所有等待者并退出
                host.alive.store(false, Ordering::SeqCst);
                let mut pending = host.pending.lock().expect("pending lock");
                pending.clear(); // SyncSender drop → 等待方收到 Disconnected → Timeout/Closed
                break;
            }
        }
    }
}

fn handle_server_request(host: &Arc<LspHost>, msg: RpcMessage) {
    // 数字 id 走正常分发；字符串等其它类型 id 无法经数字路由表回程，
    // 但规范要求必须应答——统一回 METHOD_NOT_FOUND，服务器才不会挂起干等
    let foreign_id_response = match msg.id.clone() {
        Some(serde_json::Value::Number(_)) | None => None,
        Some(other) => Some(RpcMessage {
            jsonrpc: Some("2.0".into()),
            id: Some(other),
            method: None,
            params: None,
            result: None,
            error: Some(RpcError {
                code: METHOD_NOT_FOUND,
                message: "宿主不支持该服务器请求（非数字 id）".into(),
            }),
        }),
    };
    if let Some(err) = foreign_id_response {
        send_response(host, &err);
        return;
    }
    let id = match msg.id {
        Some(serde_json::Value::Number(n)) => n.as_i64().unwrap_or(-1),
        _ => return,
    };
    let method = msg.method.unwrap_or_default();
    let params = msg.params.unwrap_or(serde_json::Value::Null);

    let response = match method.as_str() {
        "workspace/executeCommand" => {
            let command = params.get("command").and_then(|c| c.as_str()).unwrap_or("");
            match host.guard.check_execute_command(command) {
                GuardDecision::Allowed => RpcMessage::response(id, serde_json::json!(null)),
                GuardDecision::Denied => RpcMessage::error_response(
                    id,
                    REQUEST_DENIED,
                    &format!("铁律七：executeCommand 白名单外拒绝（{command}）"),
                ),
            }
        }
        "workspace/applyEdit" => match host
            .guard
            .check_workspace_edit(&params.get("edit").cloned().unwrap_or_default())
        {
            // 宿主不代写服务器发起的编辑（写盘统一走 daemon 写守卫链路）：
            // 如实回 applied:false 交服务器走用户确认/重试，不得谎报已落盘
            GuardDecision::Allowed => {
                RpcMessage::response(id, serde_json::json!({"applied": false}))
            }
            GuardDecision::Denied => RpcMessage::error_response(
                id,
                REQUEST_DENIED,
                "铁律七：workspace edit 越出项目边界拒绝",
            ),
        },
        "window/showDocument" => {
            let uri = params.get("uri").and_then(|u| u.as_str()).unwrap_or("");
            match host.guard.check_show_document(uri) {
                // 同 applyEdit：宿主不执行展示动作，如实回 success:false
                GuardDecision::Allowed => {
                    RpcMessage::response(id, serde_json::json!({"success": false}))
                }
                GuardDecision::Denied => {
                    RpcMessage::error_response(id, REQUEST_DENIED, "铁律七：showDocument 越界拒绝")
                }
            }
        }
        "workspace/configuration" => {
            // 声明了 configuration 能力就必须应答：未知节返回 null（LSP 规范）
            let count = params
                .get("items")
                .and_then(|i| i.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            let result = vec![serde_json::Value::Null; count];
            RpcMessage::response(id, serde_json::json!(result))
        }
        "client/registerCapability" => {
            let caps = params
                .get("registrations")
                .and_then(|r| r.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            let _ = host
                .guard
                .check_register_capability(&format!("{} registrations", caps));
            RpcMessage::error_response(id, REQUEST_DENIED, "铁律七：动态注册 capability 默认拒绝")
        }
        _ => RpcMessage::error_response(id, METHOD_NOT_FOUND, "宿主不支持该服务器请求"),
    };

    send_response(host, &response);
}

fn send_response(host: &Arc<LspHost>, response: &RpcMessage) {
    if std::env::var("TENON_LSP_DEBUG").is_ok() {
        eprintln!(
            "[lsp-out-resp] {}",
            serde_json::to_string(response)
                .unwrap_or_default()
                .chars()
                .take(200)
                .collect::<String>()
        );
    }
    if let Ok(body) = serde_json::to_string(response) {
        let mut w = host.writer.lock().expect("writer lock");
        let _ = w.write_frame(&body);
    }
}
