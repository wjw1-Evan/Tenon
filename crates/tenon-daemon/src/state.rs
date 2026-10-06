//! daemon 状态：store / 会话表 / provider 注册表 / 配置。

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::{Mutex, Semaphore};

use tenon_store::L4ChunkRecord;

use crate::pairing::PairingStore;
use tenon_agent::session::{AgentSession, ProjectWriteLock, TaskOutcome};
use tenon_config::Config;
use tenon_laya::LayaRuntime;
use tenon_lsp::LspManager;
use tenon_models::ModelProvider;
use tenon_snapshot::SnapshotStore;
use tenon_store::Store;

use crate::auth::TicketStore;

/// provider 名约束（v1.40 §15）：小写标识符，用于 /session/:id/model 切换与展示。
fn is_valid_provider_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase())
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

fn parse_provider_kind(s: &str) -> Option<tenon_config::ProviderKind> {
    match s {
        "openai" => Some(tenon_config::ProviderKind::Openai),
        "anthropic" => Some(tenon_config::ProviderKind::Anthropic),
        "openai_responses" => Some(tenon_config::ProviderKind::OpenaiResponses),
        _ => None,
    }
}

/// provider 覆盖条目（v1.40 §15）：密钥仅存环境变量引用名，永不落盘明文。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProviderOverride {
    /// openai | anthropic | openai_responses
    pub kind: Option<String>,
    pub base_url: Option<String>,
    pub wire_api: Option<String>,
    pub model: Option<String>,
    pub api_key_env: Option<String>,
}

impl ProviderOverride {
    fn merge_json(&mut self, v: &serde_json::Value) -> Result<(), String> {
        if v.get("api_key").is_some() {
            return Err(
                "models.providers.api_key 明文禁止经设置链路写入——密钥仅存 api_key_env 引用（§11）"
                    .into(),
            );
        }
        if let Some(k) = v.get("kind") {
            let k = k.as_str().ok_or("models.providers.kind 须为字符串")?;
            if !matches!(k, "openai" | "anthropic" | "openai_responses") {
                return Err(
                    "models.providers.kind 仅支持 openai | anthropic | openai_responses".into(),
                );
            }
            self.kind = Some(k.to_string());
        }
        if let Some(u) = v.get("base_url") {
            let u = u.as_str().ok_or("models.providers.base_url 须为字符串")?;
            if !(u.starts_with("http://") || u.starts_with("https://")) {
                return Err("models.providers.base_url 须以 http(s):// 开头".into());
            }
            self.base_url = Some(u.to_string());
        }
        if let Some(w) = v.get("wire_api") {
            let w = w.as_str().ok_or("models.providers.wire_api 须为字符串")?;
            if w != "chat" && w != "responses" {
                return Err("models.providers.wire_api 仅支持 chat | responses".into());
            }
            self.wire_api = Some(w.to_string());
        }
        if let Some(m) = v.get("model") {
            let m = m.as_str().ok_or("models.providers.model 须为字符串")?;
            if m.is_empty() {
                return Err("models.providers.model 不可为空".into());
            }
            self.model = Some(m.to_string());
        }
        if let Some(e) = v.get("api_key_env") {
            let e = e
                .as_str()
                .ok_or("models.providers.api_key_env 须为字符串")?;
            let valid = !e.is_empty()
                && e.chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_uppercase() || c == '_')
                && e.chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
            if !valid {
                return Err("models.providers.api_key_env 须为合法环境变量名".into());
            }
            self.api_key_env = Some(e.to_string());
        }
        Ok(())
    }

    fn to_json(&self) -> serde_json::Value {
        let mut o = serde_json::Map::new();
        if let Some(k) = &self.kind {
            o.insert("kind".into(), serde_json::Value::String(k.clone()));
        }
        if let Some(u) = &self.base_url {
            o.insert("base_url".into(), serde_json::Value::String(u.clone()));
        }
        if let Some(w) = &self.wire_api {
            o.insert("wire_api".into(), serde_json::Value::String(w.clone()));
        }
        if let Some(m) = &self.model {
            o.insert("model".into(), serde_json::Value::String(m.clone()));
        }
        if let Some(e) = &self.api_key_env {
            o.insert("api_key_env".into(), serde_json::Value::String(e.clone()));
        }
        serde_json::Value::Object(o)
    }
}

/// 设置面板运行时覆盖（§7.2 / §15）：known-keys 子集。
/// 持久化 `~/.tenon/settings.json`，**新会话**生效（既有会话保持各自配置）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SettingsOverrides {
    pub first_edit_buffer_ms: Option<u64>,
    pub command_timeout_s: Option<u64>,
    /// v1.83 更新分区：manual | auto（auto 仍受更新器实现限制，仅记录偏好）。
    pub update_channel: Option<String>,
    /// v1.40 模型分区：默认 provider 名与 provider 覆盖表（整体替换语义）。
    pub models_default: Option<String>,
    pub models_providers: std::collections::BTreeMap<String, ProviderOverride>,
    /// v1.130 技能分区：停用名单（§13.4，整体替换；新会话生效）。
    pub skills_disabled: Option<Vec<String>>,
    /// v1.145 市场分区：市场源 owner/repo 数组（§13.5，整体替换）。
    pub market_sources: Option<Vec<String>>,
    /// v1.145 MCP 分区：服务器表（§13.5，整体替换；新会话生效）。
    pub mcp_servers: Option<std::collections::BTreeMap<String, tenon_mcp::McpServerConfig>>,
}

/// mcp.servers 单条校验（§13.5 安装期硬约束；settings PUT 与市场安装共用）。
pub fn validate_mcp_server(name: &str, cfg: &tenon_mcp::McpServerConfig) -> Result<(), String> {
    if !tenon_registry::market::is_valid_mcp_name(name) {
        return Err(format!(
            "mcp.servers.{name} 名须匹配 ^[a-z][a-z0-9-]{{0,31}}$"
        ));
    }
    if !tenon_registry::market::LAUNCHER_WHITELIST.contains(&cfg.command.as_str()) {
        return Err(format!(
            "mcp.servers.{name} 启动器不在白名单 {:?}: {}",
            tenon_registry::market::LAUNCHER_WHITELIST,
            cfg.command
        ));
    }
    for arg in &cfg.args {
        if arg.chars().any(|c| c.is_control()) {
            return Err(format!("mcp.servers.{name} args 含控制字符"));
        }
    }
    for (k, v) in &cfg.env {
        let Some(var) = v.strip_prefix("env:") else {
            return Err(format!(
                "mcp.servers.{name} env 值仅允许 env:VAR 引用（§11 密钥不落盘）: {k}"
            ));
        };
        if var.is_empty()
            || !var.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            || !k
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        {
            return Err(format!("mcp.servers.{name} env 键值非法: {k}={v}"));
        }
    }
    for p in &cfg.permissions {
        if p != "net:*" {
            return Err(format!(
                "mcp.servers.{name} permissions 当前仅支持 net:*: {p}"
            ));
        }
    }
    if let Some(src) = &cfg.source {
        if !tenon_registry::market::is_valid_source(src) {
            return Err(format!("mcp.servers.{name} source 形态非法: {src}"));
        }
    }
    Ok(())
}

impl SettingsOverrides {
    /// 校验并合并 PUT body；非法值返回错误文案。
    pub fn merge_json(&mut self, body: &serde_json::Value) -> Result<(), String> {
        if let Some(session) = body.get("session") {
            if session.get("mode").is_some() {
                return Err("session.mode 已移除（v1.92：审批移除后档位无行为差异）".into());
            }
            if let Some(v) = session.get("first_edit_buffer_ms") {
                let v = v
                    .as_u64()
                    .ok_or("session.first_edit_buffer_ms 须为非负整数")?;
                if v > 10_000 {
                    return Err("session.first_edit_buffer_ms 上限 10000ms".into());
                }
                self.first_edit_buffer_ms = Some(v);
            }
        }
        if let Some(exec) = body.get("exec") {
            if let Some(v) = exec.get("command_timeout_s") {
                let v = v.as_u64().ok_or("exec.command_timeout_s 须为非负整数")?;
                if !(1..=3600).contains(&v) {
                    return Err("exec.command_timeout_s 取值 1-3600s".into());
                }
                self.command_timeout_s = Some(v);
            }
        }
        if let Some(privacy) = body.get("privacy") {
            // v1.92：遥测 / 崩溃报告无采集端，键整体拒绝（不设无效开关）。
            if privacy.get("telemetry").is_some() || privacy.get("crash_reports").is_some() {
                return Err("privacy.* 已移除（v1.92：无采集端，不设无效开关）".into());
            }
        }
        if let Some(update) = body.get("update") {
            if let Some(v) = update.get("channel") {
                let v = v.as_str().ok_or("update.channel 须为字符串")?;
                if v != "manual" && v != "auto" {
                    return Err("update.channel 仅支持 manual | auto".into());
                }
                self.update_channel = Some(v.to_string());
            }
        }
        if let Some(models) = body.get("models") {
            // models 块原子提交：校验全部通过才落（避免 400 时部分覆盖生效）
            let mut next_default = self.models_default.clone();
            let mut next_providers = self.models_providers.clone();
            if let Some(d) = models.get("default") {
                let d = d.as_str().ok_or("models.default 须为字符串")?;
                if d.is_empty() {
                    // 空串 = 清除默认 provider 覆盖（回退配置文件值）
                    next_default = None;
                } else if !is_valid_provider_name(d) {
                    return Err("models.default 须为合法 provider 名".into());
                } else {
                    next_default = Some(d.to_string());
                }
            }
            if let Some(p) = models.get("providers") {
                let p = p.as_object().ok_or("models.providers 须为对象")?;
                let mut built = std::collections::BTreeMap::new();
                for (name, entry) in p {
                    if !is_valid_provider_name(name) {
                        return Err(format!(
                            "models.providers.{name} 名须匹配 ^[a-z][a-z0-9_-]{{0,63}}$"
                        ));
                    }
                    let mut ov = ProviderOverride::default();
                    ov.merge_json(entry).map_err(|e| format!("{name}: {e}"))?;
                    built.insert(name.clone(), ov);
                }
                // 整体替换覆盖表（UI 每次保存发全量，支持删除）；基础 config 条目不受影响
                next_providers = built;
            }
            self.models_default = next_default;
            self.models_providers = next_providers;
        }
        if let Some(skills) = body.get("skills") {
            if let Some(v) = skills.get("disabled") {
                // 整体替换（§13.4 v1.130）：名称数组，元素须为合法技能 id
                let arr = v.as_array().ok_or("skills.disabled 须为字符串数组")?;
                let mut names = Vec::with_capacity(arr.len());
                for item in arr {
                    let s = item.as_str().ok_or("skills.disabled 须为字符串数组")?;
                    if !tenon_core::skills::is_valid_skill_name(s) {
                        return Err(format!("skills.disabled 含非法技能名: {s}"));
                    }
                    if !names.contains(&s.to_string()) {
                        names.push(s.to_string());
                    }
                }
                self.skills_disabled = Some(names);
            }
        }
        if let Some(market) = body.get("market") {
            if let Some(v) = market.get("sources") {
                // 整体替换（§13.5 v1.145）：owner/repo 数组
                let arr = v.as_array().ok_or("market.sources 须为字符串数组")?;
                let mut sources = Vec::with_capacity(arr.len());
                for item in arr {
                    let s = item.as_str().ok_or("market.sources 须为字符串数组")?;
                    if !tenon_registry::market::is_valid_source(s) {
                        return Err(format!("market.sources 含非法源（owner/repo 形态）: {s}"));
                    }
                    if !sources.contains(&s.to_string()) {
                        sources.push(s.to_string());
                    }
                }
                self.market_sources = Some(sources);
            }
        }
        if let Some(mcp) = body.get("mcp") {
            if let Some(v) = mcp.get("servers") {
                // 整体替换（§13.5 v1.145）：逐条校验后整体落表（新会话生效）
                let obj = v.as_object().ok_or("mcp.servers 须为对象")?;
                let mut built = std::collections::BTreeMap::new();
                for (name, entry) in obj {
                    let cfg: tenon_mcp::McpServerConfig = serde_json::from_value(entry.clone())
                        .map_err(|e| format!("mcp.servers.{name}: {e}"))?;
                    validate_mcp_server(name, &cfg)?;
                    built.insert(name.clone(), cfg);
                }
                self.mcp_servers = Some(built);
            }
        }
        Ok(())
    }

    pub fn to_json(&self) -> serde_json::Value {
        let mut session = serde_json::Map::new();
        if let Some(v) = self.first_edit_buffer_ms {
            session.insert("first_edit_buffer_ms".into(), serde_json::json!(v));
        }
        let mut exec = serde_json::Map::new();
        if let Some(v) = self.command_timeout_s {
            exec.insert("command_timeout_s".into(), serde_json::json!(v));
        }
        let update = serde_json::json!({
            "channel": self.update_channel.clone().unwrap_or_else(|| "manual".into()),
        });
        let mut models = serde_json::Map::new();
        if let Some(d) = &self.models_default {
            models.insert("default".into(), serde_json::Value::String(d.clone()));
        }
        if !self.models_providers.is_empty() {
            let providers: serde_json::Map<String, serde_json::Value> = self
                .models_providers
                .iter()
                .map(|(k, v)| (k.clone(), v.to_json()))
                .collect();
            models.insert("providers".into(), serde_json::Value::Object(providers));
        }
        let skills = serde_json::json!({
            "disabled": self.skills_disabled.clone().unwrap_or_default(),
        });
        let market = serde_json::json!({
            "sources": self.market_sources.clone().unwrap_or_default(),
        });
        let mcp = serde_json::json!({
            "servers": self.mcp_servers.clone().unwrap_or_default(),
        });
        serde_json::json!({
            "session": session,
            "exec": exec,
            "update": update,
            "models": models,
            "skills": skills,
            "market": market,
            "mcp": mcp,
        })
    }

    /// 叠加模型覆盖到基础配置（v1.40）：同名单条目按字段合并（保留基础条目的
    /// api_key 等未覆盖字段），`models.default` 覆盖默认 provider。
    pub fn apply_models_to(&self, models: &mut tenon_config::ModelsConfig) {
        if let Some(d) = &self.models_default {
            models.default = d.clone();
        }
        for (name, ov) in &self.models_providers {
            let entry = models.providers.entry(name.clone()).or_default();
            if let Some(k) = &ov.kind {
                entry.kind = parse_provider_kind(k);
            }
            if let Some(u) = &ov.base_url {
                entry.base_url = u.clone();
            }
            if let Some(w) = &ov.wire_api {
                entry.wire_api = Some(w.clone());
            }
            if let Some(m) = &ov.model {
                entry.model = Some(m.clone());
            }
            if let Some(e) = &ov.api_key_env {
                entry.api_key_env = Some(e.clone());
            }
        }
    }

    /// 从指定 settings 文件读取（损坏 / 非法条目逐项忽略）。
    pub fn load_from_path(path: &std::path::Path) -> Self {
        let mut out = Self::default();
        let Ok(text) = std::fs::read_to_string(path) else {
            return out;
        };
        let Ok(mut v) = serde_json::from_str::<serde_json::Value>(&text) else {
            return out;
        };
        // v1.92 兼容：静默剔除已移除键（session.mode / privacy.*），旧文件其余覆盖照常生效。
        if let Some(session) = v.get_mut("session").and_then(|s| s.as_object_mut()) {
            session.remove("mode");
        }
        if let Some(obj) = v.as_object_mut() {
            obj.remove("privacy");
        }
        let _ = out.merge_json(&v);
        out
    }

    pub fn persist_to(&self, path: &std::path::Path) {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let json = self.to_json().to_string();
        if std::fs::write(path, json).is_ok() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
            }
        }
    }
}

pub struct DaemonOptions {
    /// 数据库路径；None = 内存库（测试）。
    pub db_path: Option<std::path::PathBuf>,
    /// 局域网绑定（§12.6 M3：显式开启；默认仅 127.0.0.1）。
    pub lan_bind: bool,
    /// 全局配置。
    pub config: Config,
    /// 注入的 provider（测试 / 自定义接入）；否则按 config.models.providers 构建。
    pub providers: Vec<Arc<dyn ModelProvider>>,
    /// 默认 provider 名。
    pub default_provider: String,
    /// 快照库根目录；None = ~/.tenon/snapshots。
    pub snapshots_root: Option<std::path::PathBuf>,
    /// 受管 worktree 根目录；None = ~/.tenon/worktrees（v1.87 §9.7）。
    pub worktrees_root: Option<std::path::PathBuf>,
    /// 启动时注册的项目根（§6.2 `--project`；/pairing 回传给 UI）。
    pub project: Option<String>,
    /// 握手 endpoint 文件；None = `~/.tenon/daemon.endpoint`（测试必须覆盖避免并行竞争）。
    pub endpoint_path: Option<std::path::PathBuf>,
    /// 设置覆盖文件；None = `~/.tenon/settings.json`（测试必须覆盖避免并行竞争）。
    pub settings_path: Option<std::path::PathBuf>,
    /// 权限策略文件；None = `~/.tenon/policy.toml`（测试必须覆盖避免并行竞争）。
    pub policy_path: Option<std::path::PathBuf>,
    /// 更新 staging 目录；None = `~/.tenon/updates/staged`（v1.86）。
    pub updates_staging_dir: Option<std::path::PathBuf>,
    /// 固定端口（开发热重载 `--port`）；None = 随机端口（默认，§12.6）。
    pub bind_port: Option<u16>,
    /// 固定握手 token（开发热重载 `--token`）；None = 随机 token（默认）。
    pub fixed_token: Option<String>,
    /// Laya 自动下载 registry 覆盖（§9.8 v1.71；None = 官方静态 registry；测试注入本地地址）。
    pub laya_registry_url: Option<String>,
    /// Laya 清单验签公钥覆盖（§9.8；None = 官方解析链；测试注入，不读进程 env）。
    pub laya_public_key: Option<String>,
    /// Laya 模型目录覆盖（§9.8；None = ~/.tenon/models/laya；测试注入临时目录）。
    pub laya_models_dir: Option<std::path::PathBuf>,
    /// 技能全局目录覆盖（§13.4 v1.130；None = ~/.tenon/skills；测试注入临时目录）。
    pub skills_dir: Option<std::path::PathBuf>,
    /// 市场镜像链 base 覆盖（§13.5 v1.145；None = jsDelivr/raw 官方链；测试注入本地 mock）。
    pub market_base: Option<String>,
    /// 文件监听轮询后端间隔（v1.128 测试确定性通道）；None = 原生后端
    ///（FSEvents/inotify，注册握手 2s 就绪预算 + 后台补注册）。
    pub watch_poll_interval: Option<std::time::Duration>,
}

impl DaemonOptions {
    pub fn in_memory() -> Self {
        Self {
            db_path: None,
            lan_bind: false,
            // 测试基座不出网（§9.8）：Laya 自动下载默认关，
            // 需要时显式打开并注入 laya_registry_url 指向本地 registry
            config: {
                let mut cfg = Config::default();
                cfg.models.laya.auto_download = false;
                cfg
            },
            providers: vec![],
            default_provider: String::new(),
            snapshots_root: None,
            worktrees_root: None,
            project: None,
            endpoint_path: None,
            settings_path: None,
            policy_path: None,
            updates_staging_dir: None,
            bind_port: None,
            fixed_token: None,
            laya_registry_url: None,
            laya_public_key: None,
            laya_models_dir: None,
            skills_dir: None,
            market_base: None,
            watch_poll_interval: None,
        }
    }
}

/// v1.147 发送消息队列条目（§9.1）：运行态入队的待发消息，内存瞬时态不落 events 表。
#[derive(Debug, Clone)]
pub struct QueuedMessage {
    pub id: String,
    pub text: String,
}

/// v1.147 单会话排队上限（§9.1）：超出 409。
pub const MESSAGE_QUEUE_MAX: usize = 10;

/// v1.147 发送消息队列（§9.1）：会话级 FIFO；id 按会话内单调序号生成。
/// 出队仅在回合自然完成（Done）后由 send_message 的 drain 循环消费（routes.rs）。
#[derive(Default)]
pub struct MessageQueue {
    items: VecDeque<QueuedMessage>,
    seq: u64,
}

impl MessageQueue {
    /// 入队一条消息；队满返回 Err（routes 映射 409）。返回 (msg_id, 1-based 位次)。
    pub fn enqueue(&mut self, text: String) -> Result<(String, usize), &'static str> {
        if self.items.len() >= MESSAGE_QUEUE_MAX {
            return Err("发送队列已满（上限 10 条）");
        }
        self.seq += 1;
        let id = format!("q{}", self.seq);
        self.items.push_back(QueuedMessage {
            id: id.clone(),
            text,
        });
        Ok((id, self.items.len()))
    }

    /// 回合自然完成后出队首条（FIFO）。
    pub fn pop(&mut self) -> Option<QueuedMessage> {
        self.items.pop_front()
    }

    /// 按 id 移除排队消息（编辑 = 移除后重发）；不存在返回 false。
    pub fn remove(&mut self, msg_id: &str) -> bool {
        let before = self.items.len();
        self.items.retain(|m| m.id != msg_id);
        self.items.len() != before
    }

    /// 队列快照（GET /session/:id `queue` 字段，多窗口一致）。
    pub fn snapshot(&self) -> Vec<QueuedMessage> {
        self.items.iter().cloned().collect()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }
}

pub struct SessionEntry {
    pub session: Arc<AgentSession>,
    pub project_root: std::path::PathBuf,
    pub project_id: String,
    /// 会话级受管 worktree 根（v1.87；None = 主根会话）。
    pub managed_worktree: Option<std::path::PathBuf>,
    pub last_outcome: Mutex<Option<TaskOutcome>>,
    /// v1.147 发送消息队列：运行态发送转入的 FIFO（§9.1，内存瞬时随 runtime 存活）。
    pub queue: Mutex<MessageQueue>,
    /// v1.147 任务执行中标志（含 drain 续跑期间）：send_message 在 sessions 锁内
    /// 同步查改，杜绝并发双发竞态；AgentSession::running（暂停挂起期间同真）为兜底。
    pub busy: std::sync::atomic::AtomicBool,
    pub last_seq: i64,
}

#[cfg(test)]
mod message_queue_tests {
    use super::*;

    #[test]
    fn enqueue_fifo_and_position() {
        let mut q = MessageQueue::default();
        let (id1, pos1) = q.enqueue("一".into()).unwrap();
        let (id2, pos2) = q.enqueue("二".into()).unwrap();
        assert_eq!((pos1, pos2), (1, 2));
        let snap = q.snapshot();
        assert_eq!(snap.len(), 2);
        assert_eq!(snap[0].id, id1);
        assert_eq!(snap[0].text, "一");
        assert_eq!(snap[1].id, id2);
        // FIFO：先入先出
        let first = q.pop().unwrap();
        assert_eq!(first.text, "一");
        assert_eq!(q.len(), 1);
    }

    #[test]
    fn enqueue_rejects_when_full() {
        let mut q = MessageQueue::default();
        for i in 0..MESSAGE_QUEUE_MAX {
            assert!(q.enqueue(format!("m{i}")).is_ok());
        }
        assert!(q.enqueue("溢出".into()).is_err());
        assert_eq!(q.len(), MESSAGE_QUEUE_MAX);
    }

    #[test]
    fn remove_by_id() {
        let mut q = MessageQueue::default();
        let (id, _) = q.enqueue("保留".into()).unwrap();
        let (id2, _) = q.enqueue("移除".into()).unwrap();
        assert!(q.remove(&id2));
        assert!(!q.remove(&id2)); // 二次移除不存在
        assert_eq!(q.len(), 1);
        assert!(q.remove(&id));
        assert_eq!(q.len(), 0);
    }
}

/// WS / ProjectRuntime 文件变更事件（§7.2 / §8.1；项目作用域）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProjectFileEvent {
    pub project_id: String,
    pub path: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub created_at: String,
}

struct WatchHandle {
    stop: Arc<std::sync::atomic::AtomicBool>,
    handle: std::sync::Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl WatchHandle {
    fn stop(&self) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(handle) = self.handle.lock().expect("watch handle lock").take() {
            let _ = handle.join();
        }
    }
}

/// 注册 watcher 并启动事件泵线程（v1.128）：`ready_budget` 默认 2s 快速降级，
/// 补注册路径传宽预算。注册失败 / 超时返回 None，不阻塞 async 上下文。
fn spawn_watcher(
    project_id: &str,
    root: &std::path::Path,
    events: &tokio::sync::broadcast::Sender<ProjectFileEvent>,
    global_events: &tokio::sync::broadcast::Sender<ProjectFileEvent>,
    ready_budget: Option<std::time::Duration>,
    poll_interval: Option<std::time::Duration>,
) -> Option<WatchHandle> {
    let watched = match poll_interval {
        // 测试确定性通道（v1.128）：PollWatcher 内容比对，小目录同步注册即达。
        Some(interval) => tenon_fs::FileWatcher::watch_with_poll_interval(root, interval).ok()?,
        None => match ready_budget {
            Some(budget) => tenon_fs::FileWatcher::watch_with_budget(root, budget).ok()?,
            None => tenon_fs::FileWatcher::watch(root).ok()?,
        },
    };
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let project_id = project_id.to_string();
    let sender = events.clone();
    let global_sender = global_events.clone();
    let stop_for_thread = stop.clone();
    let handle = std::thread::Builder::new()
        .name(format!("tenon-watch-{project_id}"))
        .spawn(move || {
            while !stop_for_thread.load(std::sync::atomic::Ordering::SeqCst) {
                for change in watched.next_batch(std::time::Duration::from_millis(250)) {
                    let kind = match change.kind {
                        tenon_fs::watcher::ChangeKind::Created => "created",
                        tenon_fs::watcher::ChangeKind::Modified => "modified",
                        tenon_fs::watcher::ChangeKind::Removed => "removed",
                    };
                    let event = ProjectFileEvent {
                        project_id: project_id.clone(),
                        path: change.path,
                        kind: kind.into(),
                        created_at: chrono::Utc::now().to_rfc3339(),
                    };
                    let _ = sender.send(event.clone());
                    let _ = global_sender.send(event);
                }
            }
        })
        .expect("spawn project watcher");
    Some(WatchHandle {
        stop,
        handle: std::sync::Mutex::new(Some(handle)),
    })
}

/// 一个已打开项目的轻量 runtime：watcher 生命周期 + 活跃度追踪。
pub struct ProjectRuntime {
    pub root: std::path::PathBuf,
    events: tokio::sync::broadcast::Sender<ProjectFileEvent>,
    watcher: std::sync::Mutex<Option<WatchHandle>>,
    pub last_accessed: std::sync::Mutex<std::time::Instant>,
}

impl ProjectRuntime {
    fn open(
        project_id: &str,
        root: std::path::PathBuf,
        global_events: tokio::sync::broadcast::Sender<ProjectFileEvent>,
        poll_interval: Option<std::time::Duration>,
    ) -> Arc<Self> {
        let (events, _) = tokio::sync::broadcast::channel(1024);
        let runtime = Arc::new(Self {
            root: root.clone(),
            events: events.clone(),
            watcher: std::sync::Mutex::new(None),
            last_accessed: std::sync::Mutex::new(std::time::Instant::now()),
        });
        // 轮询后端（测试确定性通道）同步接回、无需补注册；原生后端按预算握手。
        if poll_interval.is_some() {
            if let Some(handle) = spawn_watcher(
                project_id,
                &root,
                &events,
                &global_events,
                None,
                poll_interval,
            ) {
                *runtime.watcher.lock().expect("watcher slot lock") = Some(handle);
            }
            return runtime;
        }
        match spawn_watcher(project_id, &root, &events, &global_events, None, None) {
            Some(handle) => {
                *runtime.watcher.lock().expect("watcher slot lock") = Some(handle);
            }
            None => {
                // 注册预算（2s）内未就绪：fseventsd 高负载时 FSEventStreamStart
                // 可达数秒——激活链路已按「无 watcher」快速降级返回，这里后台
                // 以宽预算补注册，成功即热接回（v1.128）。runtime 被回收或已被
                // 其他路径接回时自然退出；最多补试 5 次，仍失败则保持降级。
                let weak = Arc::downgrade(&runtime);
                let retry_id = project_id.to_string();
                let retry_events = events.clone();
                let retry_root = root.clone();
                std::thread::Builder::new()
                    .name(format!("tenon-watch-retry-{retry_id}"))
                    .spawn(move || {
                        let mut backoff_ms = 250u64;
                        for _ in 0..5 {
                            std::thread::sleep(std::time::Duration::from_millis(backoff_ms));
                            let Some(rt) = weak.upgrade() else { return };
                            if rt.watcher.lock().expect("watcher slot lock").is_some() {
                                return;
                            }
                            if let Some(handle) = spawn_watcher(
                                &retry_id,
                                &retry_root,
                                &retry_events,
                                &global_events,
                                Some(std::time::Duration::from_secs(10)),
                                None,
                            ) {
                                let mut slot = rt.watcher.lock().expect("watcher slot lock");
                                if slot.is_none() {
                                    *slot = Some(handle);
                                }
                                return;
                            }
                            backoff_ms = (backoff_ms * 2).min(2_000);
                        }
                    })
                    .ok();
            }
        }
        runtime
    }

    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<ProjectFileEvent> {
        self.events.subscribe()
    }

    pub async fn touch(&self) {
        *self.last_accessed.lock().expect("runtime activity lock") = std::time::Instant::now();
    }

    async fn stop(&self) {
        if let Some(handle) = self.watcher.lock().expect("runtime watcher lock").take() {
            handle.stop();
        }
    }
}

impl Drop for ProjectRuntime {
    fn drop(&mut self) {
        // 兜底回收（v1.128）：未经 close_project_runtime 的释放（如测试
        // teardown 直接丢 DaemonState）也必须停 watcher——FSEvents 流与事件
        // 泵线程跨 daemon 生命周期累积，会把系统 fseventsd 拖垮、拖慢后续
        // 所有注册。显式关闭路径已先 take，这里通常是空操作。
        if let Some(handle) = self.watcher.lock().expect("runtime watcher lock").take() {
            handle.stop();
        }
    }
}

/// L4 worker 状态（诊断面板 / §10.1）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct L4IndexStatus {
    pub state: String,
    pub updated_at: String,
    pub error: Option<String>,
}

/// WS 推送的 L4 状态变化；统计值仍以 `/l4/stats` 为权威快照。
#[derive(Debug, Clone, serde::Serialize)]
pub struct L4StatusEvent {
    pub project_id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub state: String,
    pub updated_at: String,
    pub error: Option<String>,
}

pub struct DaemonState {
    pub store: Arc<Mutex<Store>>,
    /// 共享 LSP 管理器（§8.5：一项目 × 语言一个宿主，编辑器与代理共用）。
    pub lsp: Arc<LspManager>,
    /// Laya 本地决策模型运行时（§9.8；未下载即整体回退）。
    pub laya: Arc<LayaRuntime>,
    /// Laya 清单验签公钥覆盖（§9.8 v1.71；None = 官方解析链；测试注入）。
    pub(crate) laya_public_key: Option<String>,
    /// 文件监听轮询间隔覆盖（v1.128 测试确定性通道）；None = 原生后端。
    pub(crate) watch_poll_interval: Option<std::time::Duration>,
    /// 脏缓冲注册表按项目隔离（§6.4 / §8.6）；key = project_id，value 是该项目相对路径表。
    pub dirty_buffers: Mutex<HashMap<String, Arc<tenon_fs::DirtyBufferRegistry>>>,
    /// 局域网配对（M3 §12.6：显式开启 + 一次性码 + 可吊销令牌；默认关闭）。
    pub lan_pairing: Arc<PairingStore>,
    /// daemon 监听端口（/pairing 自发现回传给 UI）。
    pub port: std::sync::atomic::AtomicU16,
    /// 权限高级策略（v1.85：只收窄；运行时热更新，仅新会话生效）。
    pub team_policy: std::sync::RwLock<tenon_core::policy::TeamPolicy>,
    pub config: Config,
    pub token: String,
    pub tickets: TicketStore,
    pub sessions: Mutex<HashMap<String, SessionEntry>>,
    /// provider 表（v1.40 起读写锁：PUT /settings 模型键即时重建，见 `rebuild_providers`）。
    pub providers: std::sync::RwLock<HashMap<String, Arc<dyn ModelProvider>>>,
    pub default_provider: std::sync::RwLock<String>,
    /// 注入的 provider（测试 / 自定义接入）；重建 provider 表时始终保留。
    injected_providers: Vec<Arc<dyn ModelProvider>>,
    /// CLI `--provider` 指定；优先级高于设置覆盖（v1.40）。
    cli_default: Option<String>,
    /// 启动时注册的项目根（/pairing 自发现回传）。
    pub default_project: Option<String>,
    /// 项目级写锁表（§9.7：同一项目同时刻仅一个会话 EXECUTING）。
    pub project_locks: Mutex<HashMap<String, ProjectWriteLock>>,
    /// 受管 worktree 根（v1.87 §9.7：`~/.tenon/worktrees/<project_id>/<session_id>/`）。
    pub worktrees_root: std::path::PathBuf,
    /// 已打开 ProjectRuntime 的 project_id → canonical root（§6.4）。
    pub open_projects: Mutex<HashMap<String, Arc<ProjectRuntime>>>,
    /// 跨项目并发调度（§6.4 / §9.7）。
    pub execution_permits: Arc<Semaphore>,
    /// v1.15 项目组合任务聚合（父任务内存态；子会话/事件持久于 SQLite）。
    /// 进程内文件变更事件总线；WS 订阅者可全量或按 project_id 过滤。
    pub file_events: tokio::sync::broadcast::Sender<ProjectFileEvent>,
    /// L4 状态变化进程内广播；WS 订阅者可全量或按 project_id 过滤。
    pub l4_status_events: tokio::sync::broadcast::Sender<L4StatusEvent>,
    pub snapshots_root: std::path::PathBuf,
    /// 技能全局目录（§13.4 v1.130：`~/.tenon/skills/`；测试隔离注入）。
    pub skills_root: std::path::PathBuf,
    /// 市场清单缓存（§13.5 v1.145：source@ref → (时间, 清单)，5 分钟 TTL；
    /// 短临界区无 await，std 锁）。
    pub market_cache: std::sync::Mutex<
        HashMap<String, (std::time::Instant, tenon_registry::market::MarketManifest)>,
    >,
    /// 市场镜像链 base 覆盖（§13.5 v1.145；None = 官方链；测试注入本地 mock）。
    pub market_client_base: Option<String>,
    /// 设置面板运行时覆盖（§15 /settings；新会话生效）。
    pub settings_overrides: std::sync::Mutex<SettingsOverrides>,
    /// 设置覆盖权威文件；测试显式隔离。
    pub settings_path: std::path::PathBuf,
    /// 权限高级策略权威文件（v1.85）；测试显式隔离。
    pub policy_path: std::path::PathBuf,
    /// 更新 staging 目录（v1.86）；测试显式隔离。
    pub updates_staging_dir: std::path::PathBuf,
    /// 更新执行器最近检查 / 错误（内存态，重启重置）。
    pub update_last_check: std::sync::Mutex<Option<String>>,
    pub update_last_error: std::sync::Mutex<Option<String>>,
    /// L4 增量索引队列（§10.1）；ProjectRuntime 激活 / watcher 变化入队。
    pub l4_index_tx: tokio::sync::mpsc::Sender<L4IndexRequest>,
    pub l4_index_rx: std::sync::Mutex<Option<tokio::sync::mpsc::Receiver<L4IndexRequest>>>,
    /// 最近 L4 worker 状态（内存态；重启后从 queued 重新建立）。
    pub l4_status: std::sync::Mutex<HashMap<String, L4IndexStatus>>,
}

/// L4 索引请求。
#[derive(Debug, Clone)]
pub enum L4IndexRequest {
    /// 项目激活：全量重建（gitignore-aware）。
    Project { project_id: String, root: PathBuf },
}

/// 按项目去抖的 L4 待处理批。
#[derive(Debug)]
struct PendingL4Index {
    root: PathBuf,
    full: bool,
    files: HashSet<(String, bool)>,
}

impl DaemonState {
    /// 运行中更新通道：设置覆盖优先于 config（v1.83）。
    pub fn effective_update_channel(&self) -> String {
        self.settings_overrides
            .lock()
            .unwrap()
            .update_channel
            .clone()
            .unwrap_or_else(|| match self.config.update.channel {
                tenon_config::UpdateChannel::Auto => "auto".into(),
                tenon_config::UpdateChannel::Manual => "manual".into(),
            })
    }

    /// updater 完成一次检查后记录权威状态。
    pub fn record_update_check(&self, error: Option<&str>) {
        *self.update_last_check.lock().expect("update check lock") =
            Some(chrono::Utc::now().to_rfc3339());
        *self.update_last_error.lock().expect("update error lock") = error.map(str::to_string);
    }
}

/// 团队策略加载（M3）：`~/.tenon/policy.toml`（只收窄字段）。
pub fn load_team_policy(path: &std::path::Path) -> tenon_core::policy::TeamPolicy {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| toml::from_str(&text).ok())
        .unwrap_or_default()
}

/// 校验权限高级策略（v1.85）。PUT 是全量原子替换：缺失字段回落默认值，
/// 禁止未知字段避免 UI 与后端演进时静默丢约束。
pub fn validate_team_policy(
    body: &serde_json::Value,
) -> Result<tenon_core::policy::TeamPolicy, String> {
    let obj = body.as_object().ok_or("team policy 须为对象")?;
    for key in obj.keys() {
        if !matches!(key.as_str(), "denied_tools" | "max_cost_usd") {
            return Err(format!("team policy 不支持字段: {key}"));
        }
    }
    let mut policy = tenon_core::policy::TeamPolicy::default();
    if let Some(v) = obj.get("denied_tools") {
        let values = v.as_array().ok_or("denied_tools 须为字符串数组")?;
        let mut names = std::collections::BTreeSet::new();
        for value in values {
            let name = value.as_str().ok_or("denied_tools 须为字符串数组")?;
            let name = name.trim();
            if name.is_empty() || name.len() > 128 {
                return Err("denied_tools 名称须为 1-128 字符".into());
            }
            names.insert(name.to_string());
        }
        policy.denied_tools = names.into_iter().collect();
    }
    if let Some(v) = obj.get("max_cost_usd") {
        if v.is_null() {
            policy.max_cost_usd = None;
        } else {
            let limit = v.as_f64().ok_or("max_cost_usd 须为非负数字或 null")?;
            if !limit.is_finite() || !(0.0..=1_000_000.0).contains(&limit) {
                return Err("max_cost_usd 取值须为 0-1000000".into());
            }
            policy.max_cost_usd = Some(limit);
        }
    }
    Ok(policy)
}

/// 策略文件原子持久化；权限错误不使 daemon 崩溃，但请求返回失败。
pub fn persist_team_policy(
    policy: &tenon_core::policy::TeamPolicy,
    path: &std::path::Path,
) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建策略目录失败: {e}"))?;
    }
    let text = toml::to_string_pretty(policy).map_err(|e| format!("序列化策略失败: {e}"))?;
    let temp = path.with_extension("toml.tmp");
    std::fs::write(&temp, text).map_err(|e| format!("写入策略失败: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) = std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o600)) {
            let _ = std::fs::remove_file(&temp);
            return Err(format!("设置策略文件权限失败: {e}"));
        }
    }
    std::fs::rename(&temp, path).map_err(|e| format!("替换策略文件失败: {e}"))
}

impl DaemonState {
    pub async fn new(options: DaemonOptions) -> Self {
        let store = match options.db_path {
            Some(path) => Store::open(&path).expect("open store"),
            None => Store::open_in_memory().expect("in-memory store"),
        };
        let mut injected_providers: Vec<Arc<dyn ModelProvider>> = Vec::new();
        for p in options.providers {
            injected_providers.push(p);
        }
        let cli_default = if options.default_provider.is_empty() {
            None
        } else {
            Some(options.default_provider.clone())
        };
        let snapshots_root = options
            .snapshots_root
            .unwrap_or_else(|| Config::data_dir().join("snapshots"));
        let skills_root = options
            .skills_dir
            .clone()
            .unwrap_or_else(|| Config::data_dir().join("skills"));
        let market_cache = std::sync::Mutex::new(HashMap::new());
        let worktrees_root = options
            .worktrees_root
            .clone()
            .unwrap_or_else(|| Config::data_dir().join("worktrees"));
        let laya_dir = options
            .laya_models_dir
            .clone()
            .unwrap_or_else(|| Config::data_dir().join("models/laya"));
        let execution_permits = Arc::new(Semaphore::new(
            options.config.projects.max_concurrent_agent_tasks.max(1),
        ));
        let (file_events, _) = tokio::sync::broadcast::channel(2048);
        let (l4_status_events, _) = tokio::sync::broadcast::channel(512);
        let (l4_index_tx, l4_index_rx) = tokio::sync::mpsc::channel(1024);
        let settings_path = options
            .settings_path
            .clone()
            .unwrap_or_else(|| Config::data_dir().join("settings.json"));
        let settings_overrides = SettingsOverrides::load_from_path(&settings_path);
        let policy_path = options
            .policy_path
            .clone()
            .unwrap_or_else(|| Config::data_dir().join("policy.toml"));
        let updates_staging_dir = options
            .updates_staging_dir
            .clone()
            .unwrap_or_else(|| Config::data_dir().join("updates/staged"));
        let team_policy = load_team_policy(&policy_path);
        let state = Self {
            store: Arc::new(Mutex::new(store)),
            lsp: Arc::new(LspManager::new()),
            lan_pairing: Arc::new(PairingStore::new()),
            port: std::sync::atomic::AtomicU16::new(0),
            team_policy: std::sync::RwLock::new(team_policy),
            dirty_buffers: Mutex::new(HashMap::new()),
            laya: Arc::new(LayaRuntime::open(
                &laya_dir,
                &options.config.models.laya.features,
            )),
            laya_public_key: options.laya_public_key.clone(),
            watch_poll_interval: options.watch_poll_interval,
            config: options.config,
            token: options
                .fixed_token
                .clone()
                .unwrap_or_else(crate::generate_token),
            tickets: TicketStore::default(),
            sessions: Mutex::new(HashMap::new()),
            providers: std::sync::RwLock::new(HashMap::new()),
            default_provider: std::sync::RwLock::new(String::new()),
            injected_providers,
            cli_default,
            default_project: options.project,
            project_locks: Mutex::new(HashMap::new()),
            worktrees_root,
            open_projects: Mutex::new(HashMap::new()),
            execution_permits,
            file_events,
            l4_status_events,
            snapshots_root,
            skills_root,
            market_cache,
            market_client_base: options.market_base.clone(),
            settings_path,
            policy_path,
            updates_staging_dir,
            settings_overrides: std::sync::Mutex::new(settings_overrides),
            update_last_check: std::sync::Mutex::new(None),
            update_last_error: std::sync::Mutex::new(None),
            l4_index_tx,
            l4_index_rx: std::sync::Mutex::new(Some(l4_index_rx)),
            l4_status: std::sync::Mutex::new(HashMap::new()),
        };
        // provider 表统一入口：基础 config + 设置覆盖（settings.json）合并构建（v1.40）
        state.rebuild_providers();
        state
    }

    /// §13.5 v1.145：市场源列表（settings `market.sources` 快照）。
    pub fn market_sources_list(&self) -> Vec<String> {
        self.settings_overrides
            .lock()
            .unwrap()
            .market_sources
            .clone()
            .unwrap_or_default()
    }

    /// §13.5 v1.145：从设置快照构建 MCP 宿主（无 enabled 服务器返回 None；
    /// 新会话生效——既有会话持有各自快照实例）。
    pub fn mcp_host(&self) -> Option<std::sync::Arc<tenon_mcp::McpHost>> {
        let ov = self.settings_overrides.lock().unwrap();
        let servers = ov.mcp_servers.clone().unwrap_or_default();
        if servers.values().all(|c| !c.enabled) {
            return None;
        }
        Some(std::sync::Arc::new(tenon_mcp::McpHost::new(
            servers,
            Config::data_dir(),
        )))
    }

    /// 按「基础 config.models + 设置覆盖」重建 provider 表与默认 provider（v1.40）。
    /// 优先级：CLI `--provider` > 设置覆盖 > 配置文件。注入 provider 始终保留。
    pub fn rebuild_providers(&self) {
        let ov = self.settings_overrides.lock().unwrap().clone();
        let mut models = self.config.models.clone();
        ov.apply_models_to(&mut models);
        let keys = tenon_models::ChainKeyStore::new();
        let mut map: HashMap<String, Arc<dyn ModelProvider>> = HashMap::new();
        for p in &self.injected_providers {
            map.insert(p.name().to_string(), p.clone());
        }
        for (name, pcfg) in &models.providers {
            if map.contains_key(name) {
                continue;
            }
            if let Ok(built) = tenon_models::build_provider(name, pcfg, &keys) {
                map.insert(name.clone(), built);
            }
        }
        let default = self
            .cli_default
            .clone()
            .unwrap_or_else(|| models.default.clone());
        *self.providers.write().unwrap() = map;
        *self.default_provider.write().unwrap() = default;
    }

    /// 查询项目 canonical root：打开表优先，随后持久登记。
    pub async fn project_root(&self, project_id: &str) -> Option<std::path::PathBuf> {
        {
            let open = self.open_projects.lock().await;
            if let Some(runtime) = open.get(project_id) {
                runtime.touch().await;
                return Some(runtime.root.clone());
            }
        }
        let mut store = self.store.lock().await;
        store
            .project(project_id)
            .ok()
            .flatten()
            .map(|p| std::path::PathBuf::from(p.path))
    }

    /// 登记即用：已注册项目首次访问时隐式激活 runtime（v1.60）。
    /// ProjectRuntime 只是内部 LRU 缓存，被逐出不等于项目不可用。
    pub async fn ensure_project_runtime(&self, project_id: &str) -> Option<std::path::PathBuf> {
        if let Some(runtime) = self.project_runtime(project_id).await {
            return Some(runtime.root.clone());
        }
        let stored_path = {
            let mut store = self.store.lock().await;
            store.project(project_id).ok().flatten()?.path
        };
        let path = std::path::PathBuf::from(stored_path);
        self.evict_runtime_capacity(project_id).await;
        self.activate_project(project_id, path.clone()).await;
        Some(path)
    }

    /// 打开 / 激活 runtime；watcher 懒启动并立即记录活跃时间。
    pub async fn activate_project(
        &self,
        project_id: &str,
        root: std::path::PathBuf,
    ) -> Arc<ProjectRuntime> {
        let mut open = self.open_projects.lock().await;
        if let Some(runtime) = open.get(project_id) {
            runtime.touch().await;
            return runtime.clone();
        }
        // watcher 注册握手含同步等待（就绪预算 2s，超时按「无 watcher」降级），
        // 不能占住 async runtime 线程；锁内 await 保持并发激活去重语义。
        let events = self.file_events.clone();
        let opened_id = project_id.to_string();
        let opened_root = root.clone();
        let poll_interval = self.watch_poll_interval;
        let runtime = tokio::task::spawn_blocking(move || {
            ProjectRuntime::open(&opened_id, opened_root, events, poll_interval)
        })
        .await
        .expect("project runtime open");
        open.insert(project_id.to_string(), runtime.clone());
        drop(open);
        self.set_l4_status(project_id, "queued", None);
        if let Err(e) = self
            .l4_index_tx
            .send(L4IndexRequest::Project {
                project_id: project_id.to_string(),
                root,
            })
            .await
        {
            self.set_l4_status(project_id, "failed", Some(e.to_string()));
            tracing::warn!("L4 project request dropped: {e}");
        }
        runtime
    }

    pub async fn project_runtime(&self, project_id: &str) -> Option<Arc<ProjectRuntime>> {
        let runtime = self.open_projects.lock().await.get(project_id).cloned()?;
        runtime.touch().await;
        Some(runtime)
    }

    /// 关闭并回收 watcher；脏缓冲随 runtime 释放。
    pub async fn close_project_runtime(&self, project_id: &str) -> Option<Arc<ProjectRuntime>> {
        let runtime = self.open_projects.lock().await.remove(project_id)?;
        runtime.stop().await;
        self.dirty_buffers.lock().await.remove(project_id);
        Some(runtime)
    }

    /// shadow 快照库 gc（§10.3 v1.93）：对全部登记项目按 keep_days prune。
    /// 仅做对象级清理；失败逐项目静默（日志留痕），不阻塞其余项目。
    pub async fn gc_snapshot_stores(&self, keep_days: u32) {
        let projects: Vec<(String, String)> = {
            let mut store = self.store.lock().await;
            store
                .list_projects()
                .map(|ps| ps.into_iter().map(|p| (p.id, p.path)).collect::<Vec<_>>())
                .unwrap_or_default()
        };
        for (project_id, path) in projects {
            let root = match std::fs::canonicalize(&path) {
                Ok(p) => p,
                Err(_) => continue,
            };
            let Ok(snapshots) = SnapshotStore::open(
                &self.snapshots_root,
                &project_id,
                &root,
                self.config.checkpoint.max_untracked_mb,
            ) else {
                continue;
            };
            if let Err(e) = snapshots.gc(keep_days) {
                tracing::warn!("快照 gc 失败（{project_id}）: {e}");
            }
        }
    }

    /// 空闲回收：无活跃代理会话且超过 TTL 的 runtime；返回已关闭项目。
    pub async fn reclaim_idle_projects(&self) -> Vec<String> {
        let ttl = std::time::Duration::from_secs(self.config.projects.idle_runtime_ttl_seconds);
        let active_ids = self.active_session_project_ids().await;
        let candidates: Vec<(String, Arc<ProjectRuntime>)> = {
            let open = self.open_projects.lock().await;
            open.iter()
                .filter(|(id, runtime)| {
                    let idle = runtime
                        .last_accessed
                        .lock()
                        .expect("runtime activity lock")
                        .elapsed()
                        >= ttl;
                    idle && !active_ids.contains(*id)
                })
                .map(|(id, runtime)| (id.clone(), runtime.clone()))
                .collect()
        };
        let mut closed = Vec::new();
        for (id, runtime) in candidates {
            self.lsp.close_project(&runtime.root).await;
            if self.close_project_runtime(&id).await.is_some() {
                closed.push(id);
            }
        }
        closed
    }

    /// 活跃代理会话所属的项目集合（Executing / Verifying / Fixing）。
    async fn active_session_project_ids(&self) -> HashSet<String> {
        let mut active_ids = HashSet::new();
        let sessions = self.sessions.lock().await;
        for entry in sessions.values() {
            if matches!(
                entry.session.current_state().await,
                tenon_core::machine::State::Executing
                    | tenon_core::machine::State::Verifying
                    | tenon_core::machine::State::Fixing
            ) {
                active_ids.insert(entry.project_id.clone());
            }
        }
        active_ids
    }

    /// 容量逐出（v1.60 登记即用）：`max_open` 只约束内部运行时缓存——
    /// 激活新项目超限时按最久未用逐出无活跃代理会话的 runtime（排除 keep_id）；
    /// 无候选可逐出时允许暂超限，由空闲回收兜底。返回已逐出项目。
    pub async fn evict_runtime_capacity(&self, keep_id: &str) -> Vec<String> {
        let max = self.config.projects.max_open.max(1);
        let need = {
            let open = self.open_projects.lock().await;
            if open.contains_key(keep_id) {
                0
            } else {
                (open.len() + 1).saturating_sub(max)
            }
        };
        if need == 0 {
            return Vec::new();
        }
        let active_ids = self.active_session_project_ids().await;
        let mut candidates: Vec<(String, Arc<ProjectRuntime>)> = {
            let open = self.open_projects.lock().await;
            open.iter()
                .filter(|(id, _)| id.as_str() != keep_id && !active_ids.contains(*id))
                .map(|(id, runtime)| (id.clone(), runtime.clone()))
                .collect()
        };
        candidates.sort_by_key(|(_, runtime)| {
            *runtime.last_accessed.lock().expect("runtime activity lock")
        });
        let mut evicted = Vec::new();
        for (id, runtime) in candidates {
            if evicted.len() >= need {
                break;
            }
            self.lsp.close_project(&runtime.root).await;
            if self.close_project_runtime(&id).await.is_some() {
                evicted.push(id);
            }
        }
        evicted
    }

    /// 项目私有脏缓冲表；避免不同项目同名相对路径互相污染。
    pub async fn dirty_buffers_for(&self, project_id: &str) -> Arc<tenon_fs::DirtyBufferRegistry> {
        let mut tables = self.dirty_buffers.lock().await;
        tables
            .entry(project_id.to_string())
            .or_insert_with(|| Arc::new(tenon_fs::DirtyBufferRegistry::new()))
            .clone()
    }

    pub async fn write_lock_for(&self, project_id: &str) -> ProjectWriteLock {
        let mut locks = self.project_locks.lock().await;
        locks
            .entry(project_id.to_string())
            .or_insert_with(ProjectWriteLock::new)
            .clone()
    }

    pub async fn provider_or_default(&self, name: &str) -> Result<Arc<dyn ModelProvider>, String> {
        let key = if name.is_empty() {
            self.default_provider.read().unwrap().clone()
        } else {
            name.to_string()
        };
        self.providers
            .read()
            .unwrap()
            .get(&key)
            .cloned()
            .ok_or_else(|| format!("provider 不可用：{key}（未配置或缺少 Key）"))
    }

    /// 取出唯一 L4 worker receiver；重复调用返回 None。
    pub fn take_l4_requests(&self) -> Option<tokio::sync::mpsc::Receiver<L4IndexRequest>> {
        self.l4_index_rx.lock().expect("l4 index rx").take()
    }

    /// Laya 自动下载并启用（§9.8 v1.71）：`enabled` + `auto_download` 时后台拉取
    /// 官方静态 registry 签名清单（v1.102 多镜像链顺序尝试；显式 registry_url
    /// 为测试注入，单地址），版本新于已装即按清单镜像列表下载安装热装载；
    /// 全程无确认卡（产品自管签名资产、推理不出网，非代理动作）。
    /// 失败静默回退：本地未装载时安装内置 starter 模型兜底（v1.102，保证可用），
    /// 否则仅日志，下次启动重试；未启用 / 已是最新即跳过。
    pub fn spawn_laya_auto_download(self: &Arc<Self>, registry_url: Option<String>) {
        if !self.config.models.laya.enabled || !self.config.models.laya.auto_download {
            return;
        }
        let state = self.clone();
        tokio::spawn(async move {
            let outcome = async {
                let manifest =
                    tenon_laya::registry::fetch_manifest(registry_url.as_deref()).await?;
                let plan = tenon_laya::registry::plan_install_with_key(
                    &manifest,
                    state.laya_public_key.as_deref(),
                )?;
                if state
                    .laya
                    .version()
                    .await
                    .map(|(_, v)| v >= plan.version)
                    .unwrap_or(false)
                {
                    tracing::info!("Laya 已是最新（v{}），跳过自动下载", plan.version);
                    return Ok(());
                }
                let bytes = tenon_laya::registry::download_model(&plan).await?;
                state.laya.install(&bytes).await?;
                tracing::info!("Laya 自动下载完成：v{} 已启用", plan.version);
                Ok::<(), tenon_laya::LayaError>(())
            }
            .await;
            if let Err(e) = outcome {
                // 兜底（v1.102）：在线链路全部失败且本地无模型 → 内置 starter 热装载，
                // 离线首启同样可用；registry 日后可达按版本比较升级覆盖。
                if !state.laya.is_loaded().await {
                    match state.laya.install(tenon_laya::STARTER_MODEL).await {
                        Ok(()) => {
                            tracing::info!("Laya 在线下载不可用（{e}），已启用内置 starter 模型；下次启动重试在线更新")
                        }
                        Err(e2) => tracing::warn!("Laya starter 兜底安装失败：{e2}；在线错误：{e}"),
                    }
                } else {
                    tracing::warn!("Laya 自动下载失败：{e}；下次启动重试");
                }
            }
        });
    }

    /// 更新 L4 worker 状态；失败不阻塞索引任务。
    pub fn set_l4_status(&self, project_id: &str, state: &str, error: Option<String>) {
        let status = L4IndexStatus {
            state: state.to_string(),
            updated_at: chrono::Utc::now().to_rfc3339(),
            error,
        };
        self.l4_status
            .lock()
            .expect("l4 status lock")
            .insert(project_id.to_string(), status.clone());
        let _ = self.l4_status_events.send(L4StatusEvent {
            project_id: project_id.to_string(),
            kind: "l4_status".to_string(),
            state: status.state,
            updated_at: status.updated_at,
            error: status.error,
        });
    }

    /// 启动 L4 后台 worker：项目激活全量重建；watcher 变更 500ms 去抖增量更新。
    pub fn spawn_l4_worker(
        self: &Arc<Self>,
        mut requests: tokio::sync::mpsc::Receiver<L4IndexRequest>,
    ) -> tokio::task::JoinHandle<()> {
        let state = self.clone();
        tokio::spawn(async move {
            let mut watcher_events = state.file_events.subscribe();
            let mut pending: HashMap<String, PendingL4Index> = HashMap::new();
            let mut ticker = tokio::time::interval(std::time::Duration::from_millis(500));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                let event = tokio::select! {
                    request = requests.recv() => {
                        match request {
                            Some(L4IndexRequest::Project { project_id, root }) => {
                                                                pending.insert(project_id, PendingL4Index { root, full: true, files: HashSet::new() });
                            }
                            None => break,
                        }
                        continue;
                    }
                    event = watcher_events.recv() => {
                        match event {
                            Ok(event) => event,
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(count)) => {
                                tracing::warn!("L4 watcher lagged; {count} events skipped");
                                continue;
                            }
                            Err(_) => break,
                        }
                    }
                    _ = ticker.tick() => {
                        let ids: Vec<String> = pending.keys().cloned().collect();
                                                for project_id in ids {
                            if let Some(batch) = pending.remove(&project_id) {
                                state.flush_l4_index(&project_id, batch).await;
                            }
                        }
                        continue;
                    }
                };
                // 只索引仍持有 runtime 的项目事件；不为后台事件隐式唤醒已逐出的 runtime。
                if let Some(runtime) = state.project_runtime(&event.project_id).await {
                    let root = runtime.root.clone();
                    let entry = pending
                        .entry(event.project_id)
                        .or_insert_with(|| PendingL4Index {
                            root,
                            full: false,
                            files: HashSet::new(),
                        });
                    entry.files.insert((event.path, event.kind == "removed"));
                }
            }
        })
    }

    async fn flush_l4_index(&self, project_id: &str, batch: PendingL4Index) {
        self.set_l4_status(project_id, "indexing", None);
        if batch.full {
            let root = batch.root.clone();
            let documents = tokio::task::spawn_blocking(move || {
                tenon_fs::l4::scan_root(&root)
                    .into_iter()
                    .map(|document| {
                        let chunks = document
                            .chunks
                            .into_iter()
                            .map(|chunk| tenon_store::L4ChunkRecord {
                                symbol: chunk.symbol,
                                start_line: chunk.start_line,
                                end_line: chunk.end_line,
                                text: chunk.text.clone(),
                                embedding: tenon_fs::l4::embed(&document.path, &chunk.text),
                            })
                            .collect::<Vec<_>>();
                        (document.path, chunks)
                    })
                    .collect::<Vec<_>>()
            })
            .await;
            match documents {
                Ok(documents) => {
                    let mut store = self.store.lock().await;
                    if let Err(e) = store.clear_l4_project(project_id) {
                        tracing::warn!("L4 clear failed: {e}");
                        return;
                    }
                    for (path, chunks) in documents {
                        if let Err(e) = store.replace_l4_file(project_id, &path, &chunks) {
                            tracing::warn!("L4 full index failed for {path}: {e}");
                        }
                    }
                    self.set_l4_status(project_id, "ready", None);
                    tracing::info!("L4 full index complete for {project_id}");
                }
                Err(e) => {
                    self.set_l4_status(project_id, "failed", Some(e.to_string()));
                    tracing::warn!("L4 full scan failed: {e}");
                }
            }
            return;
        }

        for (path, removed) in batch.files {
            if removed {
                let mut store = self.store.lock().await;
                if let Err(e) = store.delete_l4_file(project_id, &path) {
                    tracing::warn!("L4 delete failed for {path}: {e}");
                }
                continue;
            }
            let root = batch.root.clone();
            let indexed_path = path.clone();
            let result = tokio::task::spawn_blocking(move || {
                tenon_fs::l4::build_document(&root, &indexed_path).map(|document| {
                    document
                        .chunks
                        .into_iter()
                        .map(|chunk| tenon_store::L4ChunkRecord {
                            symbol: chunk.symbol,
                            start_line: chunk.start_line,
                            end_line: chunk.end_line,
                            text: chunk.text.clone(),
                            embedding: tenon_fs::l4::embed(&document.path, &chunk.text),
                        })
                        .collect::<Vec<L4ChunkRecord>>()
                })
            })
            .await;
            let chunks = match result {
                Ok(Some(chunks)) => chunks,
                Ok(None) => {
                    tracing::debug!("L4 skip {path}: empty/binary/too large");
                    continue;
                }
                Err(e) => {
                    tracing::warn!("L4 index join failed: {e}");
                    continue;
                }
            };
            let mut store = self.store.lock().await;
            if let Err(e) = store.replace_l4_file(project_id, &path, &chunks) {
                self.set_l4_status(project_id, "failed", Some(e.to_string()));
                tracing::warn!("L4 incremental index failed for {path}: {e}");
                continue;
            }
            self.set_l4_status(project_id, "ready", None);
        }
    }
}
