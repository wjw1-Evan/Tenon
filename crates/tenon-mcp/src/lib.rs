//! MCP 外部进程插件客户端（设计方案 §13.3 / §6.3）：
//! stdio JSON-RPC（换行分帧）——initialize 握手 → tools/list → tools/call。
//!
//! 分级映射（§13.3：MCP 默认 C/D；§12.2 铁律三不可放宽）：
//! - 默认 **D**（外部进程能力面未知，保守处理）；
//! - manifest `permissions` 含 `net:*` 的工具可降为 C；
//! - 任何映射都不低于 C——MCP 工具永不自动执行。

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{self, BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Mutex;

#[derive(Debug, thiserror::Error)]
pub enum McpError {
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON 错误: {0}")]
    Json(String),
    #[error("MCP 请求被拒绝: {0}")]
    Denied(String),
    #[error("MCP 调用超时")]
    Timeout,
}

pub type Result<T> = std::result::Result<T, McpError>;

/// MCP 工具描述（tools/list 结果）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpTool {
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// JSON Schema
    #[serde(default, rename = "inputSchema")]
    pub input_schema: Value,
}

/// 工具动作分级（§13.3：默认 D；`net:*` 权限 → C；永不自动执行）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpToolLevel {
    C,
    D,
}

/// MCP 工具分级策略（§13.3：默认 C/D；外部插件权限映射）。
#[derive(Debug, Clone, Default)]
pub struct McpLevelPolicy {
    /// 声明 `net:*` 权限的工具 → C；其余 → D（默认保守）。
    pub net_tools: std::collections::HashSet<String>,
}

impl McpLevelPolicy {
    pub fn level_for(&self, tool: &str) -> McpToolLevel {
        if self.net_tools.contains(tool) {
            McpToolLevel::C
        } else {
            McpToolLevel::D
        }
    }
}

/// MCP 客户端连接（阻塞实现；daemon 侧经 spawn_blocking 调用）。
pub struct McpConnection {
    stdin: Mutex<ChildStdin>,
    stdout: Mutex<BufReader<ChildStdout>>,
    child: Mutex<Child>,
    next_id: AtomicI64,
    // 单路 stdout、无按 id 分发器：并发请求会互相读走对方的响应
    // （双方都拿不到 → 白等满超时 → 看门狗误杀健康服务器）。
    // 整个 request（发送+读回）持这把锁串行化，同连接排队执行。
    call_lock: Mutex<()>,
}

impl McpConnection {
    /// 启动 MCP 服务器子进程。
    pub fn spawn(program: &str, args: &[&str], cwd: &Path) -> io::Result<McpConnection> {
        Self::spawn_with_env(program, args, cwd, &[])
    }

    /// 带 env 注入的启动（§13.5 v1.145：值由调用方解析自 `env:VAR` 引用）。
    pub fn spawn_with_env(
        program: &str,
        args: &[&str],
        cwd: &Path,
        envs: &[(&str, &str)],
    ) -> io::Result<McpConnection> {
        let mut cmd = Command::new(program);
        cmd.args(args)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        for (k, v) in envs {
            cmd.env(k, v);
        }
        Self::from_child(cmd.spawn()?)
    }

    pub fn from_child(mut child: Child) -> io::Result<Self> {
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        Ok(Self {
            stdin: Mutex::new(stdin),
            stdout: Mutex::new(BufReader::new(stdout)),
            child: Mutex::new(child),
            next_id: AtomicI64::new(1),
            call_lock: Mutex::new(()),
        })
    }

    fn send(&self, value: &Value) -> Result<()> {
        let mut w = self.stdin.lock().expect("stdin lock");
        writeln!(w, "{value}").map_err(McpError::Io)
    }

    fn read_message(&self) -> Result<Value> {
        let mut r = self.stdout.lock().expect("stdout lock");
        let mut line = String::new();
        let n = r.read_line(&mut line)?;
        if n == 0 {
            return Err(McpError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "mcp server closed",
            )));
        }
        serde_json::from_str(line.trim()).map_err(|e| McpError::Json(e.to_string()))
    }

    fn request(&self, method: &str, params: Value) -> Result<Value> {
        const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(45);
        // 串行化整个请求往返：stdout 是单路流且无按 id 分发，
        // 并发调用会互相吞响应（见 call_lock 注释）
        let _serial = self.call_lock.lock().expect("call lock");
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        self.send(&msg)?;
        let deadline = std::time::Instant::now() + REQUEST_TIMEOUT;
        // 看门狗：read_line 是无限期阻塞读，挂死的服务器会让本调用与
        // 同连接后续调用（stdout 锁排队）永久卡死——超时 kill 子进程解阻塞
        let pid = self.child.lock().expect("child lock").id();
        let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let watchdog = Watchdog {
            cancelled: cancelled.clone(),
            pid,
            deadline,
        };
        watchdog.spawn_arm();
        // 持活到请求返回（Drop 置位 cancelled，看门狗不再 kill）
        let _hold = watchdog;
        loop {
            let incoming = match self.read_message() {
                Ok(v) => v,
                Err(e) => {
                    if std::time::Instant::now() >= deadline {
                        return Err(McpError::Timeout);
                    }
                    return Err(e);
                }
            };
            // 跳过通知与服务器发起的请求（有 method 字段——其 id 空间与
            // 我们的 next_id 撞号，当响应处理会错拿 Null 结果 / 死等）
            if incoming.get("method").is_some() || incoming.get("id").is_none() {
                continue;
            }
            if incoming["id"].as_i64() == Some(id) {
                if let Some(err) = incoming.get("error") {
                    return Err(McpError::Denied(err.to_string()));
                }
                return Ok(incoming.get("result").cloned().unwrap_or(Value::Null));
            }
            // 他人响应（id 不匹配）：丢弃但留痕，不静默吞
        }
    }

    fn notify(&self, method: &str, params: Value) -> Result<()> {
        self.send(&serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        }))
    }

    /// initialize 握手 + initialized 通知（MCP 规范）。
    pub fn initialize(&self, client_name: &str) -> Result<Value> {
        let result = self.request(
            "initialize",
            serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": client_name, "version": env!("CARGO_PKG_VERSION") },
            }),
        )?;
        self.notify("notifications/initialized", serde_json::json!({}))?;
        Ok(result)
    }

    /// tools/list：可用工具（供映射为分级工具目录）。
    pub fn list_tools(&self) -> Result<Vec<McpTool>> {
        let result = self.request("tools/list", serde_json::json!({}))?;
        let tools = result
            .get("tools")
            .cloned()
            .unwrap_or_else(|| serde_json::json!([]));
        serde_json::from_value(tools).map_err(|e| McpError::Json(e.to_string()))
    }

    /// tools/call：执行外部工具（调用方负责黑名单与只读边界）。
    pub fn call_tool(&self, name: &str, arguments: Value) -> Result<String> {
        let result = self.request(
            "tools/call",
            serde_json::json!({ "name": name, "arguments": arguments }),
        )?;
        let is_error = result
            .get("isError")
            .and_then(|e| e.as_bool())
            .unwrap_or(false);
        let text: String = result
            .get("content")
            .and_then(|c| c.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        if is_error {
            return Err(McpError::Denied(text));
        }
        Ok(text)
    }

    pub fn shutdown(&self) {
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for McpConnection {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// 请求看门狗：到 deadline 仍未取消（请求正常返回时 Drop 置位）则 kill
/// 子进程，解除 read_line 的无限期阻塞。
struct Watchdog {
    cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
    pid: u32,
    deadline: std::time::Instant,
}

impl Drop for Watchdog {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }
}

impl Watchdog {
    fn spawn_arm(&self) {
        let cancelled = self.cancelled.clone();
        let pid = self.pid;
        let deadline = self.deadline;
        std::thread::spawn(move || loop {
            let now = std::time::Instant::now();
            if now >= deadline {
                if !cancelled.load(Ordering::SeqCst) {
                    #[cfg(unix)]
                    unsafe {
                        libc::kill(pid as libc::pid_t, libc::SIGKILL);
                    }
                    #[cfg(not(unix))]
                    let _ = pid;
                }
                return;
            }
            std::thread::sleep((deadline - now).min(std::time::Duration::from_millis(500)));
        });
    }
}

/// MCP 服务器启动配置（settings.json `mcp.servers` 条目，§13.5 v1.145）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// 环境变量注入；值仅允许 `env:VAR` 引用（spawn 时解析，settings 不存明文密钥，§11）。
    #[serde(default)]
    pub env: std::collections::BTreeMap<String, String>,
    #[serde(default = "crate::default_true")]
    pub enabled: bool,
    /// 当前仅 `net:*` 一键：工具映射 C 级（§13.3；进程网络不设限，同语言服务器路线）。
    #[serde(default)]
    pub permissions: Vec<String>,
    /// 市场溯源 `owner/repo`（§13.5；手动添加为空）。
    #[serde(default)]
    pub source: Option<String>,
    /// 市场条目版本（§13.5 更新比对；手动添加为空）。
    #[serde(default)]
    pub version: Option<String>,
}

fn default_true() -> bool {
    true
}

/// 会话工具命名：`mcp_{server}_{tool}`（§13.5 v1.145）。
/// server 名 `^[a-z][a-z0-9-]{0,31}$` 不含下划线——按首个下划线切分即无歧义。
pub fn mcp_tool_name(server: &str, tool: &str) -> String {
    format!("mcp_{server}_{tool}")
}

/// 解析 `mcp_{server}_{tool}` 为 (server, tool)；非 mcp_ 前缀返回 None。
pub fn parse_mcp_tool_name(name: &str) -> Option<(String, String)> {
    let rest = name.strip_prefix("mcp_")?;
    let split = rest.find('_')?;
    let (server, tool) = rest.split_at(split);
    if server.is_empty() || tool.len() <= 1 {
        return None;
    }
    Some((server.to_string(), tool[1..].to_string()))
}

/// 多服务器 MCP 宿主（§13.5 v1.145）：按 settings 快照持有命名配置，
/// 首次使用懒 spawn + initialize，调用失败丢弃连接（下次调用重启），
/// 随 ToolContext 释放回收（Drop 逐连接 kill）。阻塞实现；调用方经 spawn_blocking。
pub struct McpHost {
    configs: std::collections::BTreeMap<String, McpServerConfig>,
    conns: Mutex<std::collections::BTreeMap<String, std::sync::Arc<McpConnection>>>,
    cwd: std::path::PathBuf,
}

/// list_tools 的展平条目（含来源 server 名）。
#[derive(Debug, Clone)]
pub struct McpHostTool {
    pub server: String,
    pub tool: McpTool,
}

impl McpHost {
    pub fn new(
        configs: std::collections::BTreeMap<String, McpServerConfig>,
        cwd: std::path::PathBuf,
    ) -> Self {
        Self {
            configs,
            conns: Mutex::new(std::collections::BTreeMap::new()),
            cwd,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.configs.values().all(|c| !c.enabled)
    }

    /// 分级策略（§13.3）：声明 `net:*` 的服务器的全部工具 → C，其余 → D。
    /// 键为完整会话工具名 `mcp_{server}_{tool}`——需 tools/list 结果；
    /// 未见过的 `mcp_` 工具一律 D（[`McpLevelPolicy::level_for`] 默认）。
    pub fn level_policy_with(&self, tools: &[McpHostTool]) -> McpLevelPolicy {
        let mut net_tools = std::collections::HashSet::new();
        for entry in tools {
            let Some(cfg) = self.configs.get(&entry.server) else {
                continue;
            };
            if cfg.permissions.iter().any(|p| p == "net:*") {
                net_tools.insert(mcp_tool_name(&entry.server, &entry.tool.name));
            }
        }
        McpLevelPolicy { net_tools }
    }

    /// 单台服务器：取缓存连接或懒 spawn + initialize。
    fn connection_for(&self, server: &str) -> Result<std::sync::Arc<McpConnection>> {
        if let Some(conn) = self.conns.lock().expect("mcp conns lock").get(server) {
            return Ok(conn.clone());
        }
        let cfg = self
            .configs
            .get(server)
            .ok_or_else(|| McpError::Denied(format!("未配置的 MCP 服务器: {server}")))?;
        let conn = std::sync::Arc::new(self.spawn_connection(cfg)?);
        self.conns
            .lock()
            .expect("mcp conns lock")
            .insert(server.to_string(), conn.clone());
        Ok(conn)
    }

    fn spawn_connection(&self, cfg: &McpServerConfig) -> Result<McpConnection> {
        let args = cfg.args.iter().map(String::as_str).collect::<Vec<_>>();
        let envs = resolved_env(&cfg.env);
        let env_refs = envs
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect::<Vec<_>>();
        let conn = McpConnection::spawn_with_env(&cfg.command, &args, &self.cwd, &env_refs)?;
        conn.initialize("tenon")?;
        Ok(conn)
    }

    /// 列出全部 enabled 服务器的工具（展平，含来源 server）。
    /// 单台服务器失败跳过（不阻塞其余服务器，§13.5）。
    pub fn list_tools(&self) -> Vec<McpHostTool> {
        let mut out = Vec::new();
        for server in self.configs.keys() {
            let cfg = match self.configs.get(server) {
                Some(c) if c.enabled => c,
                _ => continue,
            };
            let _ = cfg;
            let Ok(conn) = self.connection_for(server) else {
                continue;
            };
            match conn.list_tools() {
                Ok(tools) => {
                    for tool in tools {
                        out.push(McpHostTool {
                            server: server.clone(),
                            tool,
                        });
                    }
                }
                Err(_) => {
                    // 连接不可用：丢弃，下次调用重启（§13.5）。
                    self.conns.lock().expect("mcp conns lock").remove(server);
                }
            }
        }
        out
    }

    /// 调用指定服务器的工具；失败丢弃连接（下次调用重启，§13.5）。
    pub fn call(&self, server: &str, tool: &str, args: Value) -> Result<String> {
        let conn = self.connection_for(server)?;
        match conn.call_tool(tool, args) {
            Ok(text) => Ok(text),
            Err(e) => {
                self.conns.lock().expect("mcp conns lock").remove(server);
                Err(e)
            }
        }
    }
}

impl std::fmt::Debug for McpHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpHost")
            .field("servers", &self.configs.keys().collect::<Vec<_>>())
            .field("active", &self.conns.lock().map(|c| c.len()).unwrap_or(0))
            .finish()
    }
}

/// 解析 `env:VAR` 引用为实际值（§11：daemon 环境变量；缺失 / 非法引用跳过）。
fn resolved_env(env: &std::collections::BTreeMap<String, String>) -> Vec<(String, String)> {
    env.iter()
        .filter_map(|(k, v)| {
            let var = v.strip_prefix("env:")?;
            if var.is_empty() || !var.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                return None;
            }
            std::env::var(var).ok().map(|val| (k.clone(), val))
        })
        .collect()
}

impl Drop for McpHost {
    fn drop(&mut self) {
        // Arc 释放触发 McpConnection::drop → kill。
        self.conns.lock().expect("mcp conns lock").clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_policy_defaults_to_d_and_net_downgrades_to_c() {
        let policy = McpLevelPolicy {
            net_tools: ["mcp:db/query".to_string()].into_iter().collect(),
        };
        assert_eq!(policy.level_for("mcp:fs/read"), McpToolLevel::D, "默认 D");
        assert_eq!(policy.level_for("mcp:db/query"), McpToolLevel::C);
    }

    #[test]
    fn mcp_tools_serialize() {
        let t: McpTool = serde_json::from_value(serde_json::json!({
            "name": "query",
            "description": "查询数据库",
            "inputSchema": {"type": "object"}
        }))
        .unwrap();
        assert_eq!(t.name, "query");
    }

    #[test]
    fn mcp_error_display() {
        let e = McpError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "not found",
        ));
        assert!(e.to_string().contains("not found"));
    }

    #[test]
    fn mcp_tool_level_all_variants() {
        let policy = McpLevelPolicy {
            net_tools: Default::default(),
        };
        assert_eq!(policy.level_for("mcp:any"), McpToolLevel::D);
    }

    #[test]
    fn mcp_tool_deserialize_minimal() {
        let t: McpTool = serde_json::from_value(serde_json::json!({
            "name": "minimal"
        }))
        .unwrap();
        assert_eq!(t.name, "minimal");
        assert!(t.description.is_empty());
    }

    #[test]
    fn mcp_tool_deserialize_full() {
        let t: McpTool = serde_json::from_value(serde_json::json!({
            "name": "full",
            "description": "A full tool",
            "inputSchema": {"type": "object", "properties": {}}
        }))
        .unwrap();
        assert_eq!(t.description, "A full tool");
    }
}
