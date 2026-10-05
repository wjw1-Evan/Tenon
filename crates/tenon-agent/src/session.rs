//! Agent 会话（设计方案 §9.1 / §9.3 / §9.4 / §9.7 / §10.3）：
//! 状态机驱动任务循环，联动权限、快照、熔断与事件流；v1.89 无审批。
//!
//! 转移规则由 `tenon_core::machine::StateMachine` 单测覆盖；运行态经
//! `force_state` 对齐并保留计数（拒绝改案 ≤2、模型重试 ≤2、修复轮次）。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc as StdArc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc, Mutex, Notify, RwLock};

use futures::StreamExt;
use tenon_core::circuit::{CircuitBreaker, CircuitLimits, CircuitStatus, PatchFootprint};
use tenon_core::context::{
    render_working_set, ContextSlice, ProjectRules, SessionMemory, WorkingSet,
};
use tenon_core::machine::{Limits as MachineLimits, State, StateMachine};
use tenon_core::policy::{Action, Decision, Level, Policy};
use tenon_core::tools::Tool;
use tenon_models::{
    ChatMessage, ChatRequest, ChatStreamEvent, ModelProvider, PriceTable, ToolSpec, Usage,
    TITLE_MARKER,
};
use tenon_snapshot::SnapshotStore;
use tenon_store::{Event, EventKind, Level as StoreLevel, SessionStatus, Store};

use crate::executor::{execute_tool, ToolContext};

#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub project_root: PathBuf,
    pub snapshots_root: PathBuf,
    pub project_id: String,
    pub policy: Policy,
    /// 首改缓冲毫秒（§9.3）。
    pub first_edit_buffer_ms: u64,
    pub circuit: CircuitLimits,
    /// 修复循环轮次上限（§9.4；无测试仓库降为 1）。
    pub fix_rounds: u32,
    /// 单命令超时秒（§9.2）。
    pub command_timeout_s: u64,
    /// 每任务最大模型回合数（防失控；收敛条件之一）。
    pub max_tool_rounds: u32,
    /// Laya 本地决策模型（§9.8；None = 未启用，各集成点回退现状）。
    pub laya: Option<Arc<tenon_laya::LayaRuntime>>,
    /// daemon 共享 LSP 宿主（§8.5 / §9.2：编辑器与代理同实例）。
    pub lsp: Option<Arc<tenon_lsp::LspManager>>,
    /// 脏缓冲注册表（§8.6 人机共编；None = daemon 未接入）。
    pub dirty: Option<Arc<tenon_fs::DirtyBufferRegistry>>,
    /// 团队策略工具黑名单（M3：跨会话只收窄）。
    pub team_denied_tools: Vec<String>,
    /// 项目内相对 / 绝对 cwd；命令在项目根沙箱内切到这里执行（§6.4）。
    pub working_dir: Option<PathBuf>,
    /// 预生成会话 ID（v1.87 受管 worktree 需先有 id 再建目录；None = store 生成）。
    pub session_id: Option<String>,
    /// 会话级受管 worktree 根（v1.87 §9.7）；Some = 写边界 / 命令 cwd / 快照均按它隔离。
    pub managed_worktree: Option<PathBuf>,
    /// 写锁作用域键（§9.7 并行写锁）：主根会话 "root"，受管 worktree 会话为其路径。
    pub write_scope: String,
    /// 价格表（§11 v1.93）：daemon 按 provider 配置构建；未定价模型计 0。
    pub price_table: PriceTable,
}

impl AgentConfig {
    pub fn for_project(project_root: PathBuf, project_id: &str) -> Self {
        Self {
            snapshots_root: tenon_config::Config::data_dir().join("snapshots"),
            project_root,
            project_id: project_id.to_string(),
            policy: Policy::default(),
            first_edit_buffer_ms: 2000,
            circuit: CircuitLimits::default(),
            fix_rounds: 3,
            command_timeout_s: 120,
            max_tool_rounds: 24,
            laya: None,
            lsp: None,
            dirty: None,
            team_denied_tools: Vec::new(),
            working_dir: None,
            session_id: None,
            managed_worktree: None,
            write_scope: "root".to_string(),
            price_table: PriceTable::new(),
        }
    }
}

/// 任务结果（证据卡，§9.4 SUMMARIZING）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceCard {
    pub answer: String,
    pub changed_files: Vec<String>,
    pub verification: String,
    /// high（测试通道）/ low（降级通道）/ none（纯回答）。
    pub verification_strength: String,
    pub steps: u32,
    pub rolled_back: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TaskOutcome {
    Done(EvidenceCard),
    Paused { state: String, reason: String },
    Error(String),
}

/// 控制命令（§15 `/session/:id/control`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlCommand {
    Pause,
    Resume,
    Stop,
    SetReadonly(bool),
}

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("存储错误: {0}")]
    Store(String),
    #[error("快照错误: {0}")]
    Snapshot(String),
    #[error("模型错误: {0}")]
    Model(String),
}

/// 项目写锁（§9.7 v1.87 并行写锁：按 `(project_id, worktree_scope)` 计——
/// 主根会话互斥，不同受管 worktree 会话可并行；全局配额由 daemon 控制）。
#[derive(Default, Clone)]
pub struct ProjectWriteLock {
    locks: Arc<Mutex<std::collections::HashMap<String, Arc<Mutex<()>>>>>,
}

impl ProjectWriteLock {
    pub fn new() -> Self {
        Self::default()
    }

    /// 取指定作用域的锁实例（惰性创建；调用方再 `.lock().await`）。
    pub async fn lock_for(&self, scope: &str) -> Arc<Mutex<()>> {
        let mut locks = self.locks.lock().await;
        locks.entry(scope.to_string()).or_default().clone()
    }
}

/// Agent 会话：一个会话一个状态机 + 事件流。
pub struct AgentSession {
    pub session_id: String,
    config: AgentConfig,
    machine: Mutex<StateMachine>,
    circuit: Mutex<CircuitBreaker>,
    memory: Mutex<SessionMemory>,
    rules: Mutex<ProjectRules>,
    store: Arc<Mutex<Store>>,
    snapshots: Arc<SnapshotStore>,
    /// 会话模型（§11 显式路由：可切换，上下文随迁）。
    provider: RwLock<StdArc<dyn ModelProvider>>,
    tool_ctx: ToolContext,
    /// 任务级写互斥（§9.7）。
    write_lock: ProjectWriteLock,
    /// 写锁作用域键（§9.7 v1.87 并行写锁）。
    write_scope: String,
    /// 会话级受管 worktree 根（v1.87；None = 项目主根会话）。
    managed_worktree: Option<PathBuf>,
    control_tx: mpsc::UnboundedSender<ControlCommand>,
    control_rx: Mutex<mpsc::UnboundedReceiver<ControlCommand>>,
    events_tx: broadcast::Sender<Event>,
    first_edit_done: AtomicBool,
    touched_files: Mutex<BTreeSet<String>>,
    /// 最近一次回滚前的安全快照（unrevert 恢复点，§10.3）。
    pre_rollback_tree: Mutex<Option<String>>,
    interrupt: Notify,
    /// 任务进行中标志（v1.93 并发守卫）：挂起等待恢复期间同样为 true。
    running: AtomicBool,
}

fn tool_specs() -> Vec<ToolSpec> {
    [
        ("read_file", "读取文本文件", serde_json::json!({
            "type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]
        })),
        ("list_dir", "列出目录", serde_json::json!({
            "type": "object", "properties": {"path": {"type": "string"}}
        })),
        ("grep", "正则搜索代码", serde_json::json!({
            "type": "object", "properties": {"pattern": {"type": "string"}}, "required": ["pattern"]
        })),
        ("git_read", "只读 git：status/log/diff", serde_json::json!({
            "type": "object", "properties": {"sub": {"type": "string", "enum": ["status", "log", "diff"]}}
        })),
        ("apply_patch", "编辑文件：file + range(1-based 行区间含端点，缺省追加) + content", serde_json::json!({
            "type": "object",
            "properties": {
                "file": {"type": "string"},
                "range": {"type": "array", "items": {"type": "integer"}, "minItems": 2, "maxItems": 2},
                "content": {"type": "string"}
            },
            "required": ["file", "content"]
        })),
        ("run_tests", "运行测试（沙箱断网）", serde_json::json!({
            "type": "object", "properties": {"command": {"type": "string"}}
        })),
        ("run_build", "构建（沙箱断网）", serde_json::json!({
            "type": "object", "properties": {"command": {"type": "string"}}
        })),
        ("install_deps", "安装依赖（沙箱镜像代理，如 npm install）", serde_json::json!({
            "type": "object", "properties": {"command": {"type": "string"}}, "required": ["command"]
        })),
        ("http_fetch", "抓取 URL（C 级直执并审计）", serde_json::json!({
            "type": "object", "properties": {"url": {"type": "string"}}, "required": ["url"]
        })),
        ("git_commit", "git 提交（D 级直执并审计）", serde_json::json!({
            "type": "object", "properties": {"message": {"type": "string"}}, "required": ["message"]
        })),
        ("git_push", "推送当前分支到已配置远端（D 级直执并审计；不支持 force）", serde_json::json!({
            "type": "object",
            "properties": {
                "remote": {"type": "string", "default": "origin"},
                "branch": {"type": "string"},
                "upstream": {"type": "boolean"}
            }
        })),
        ("create_pr", "创建 Pull Request（C+D 复合直执并审计；使用本机 gh CLI 凭据）", serde_json::json!({
            "type": "object",
            "properties": {
                "title": {"type": "string"},
                "body": {"type": "string"},
                "base": {"type": "string"},
                "head": {"type": "string"},
                "draft": {"type": "boolean"},
                "repository": {"type": "string"}
            },
            "required": ["title"]
        })),
    ]
    .into_iter()
    .map(|(name, desc, params)| ToolSpec {
        name: name.to_string(),
        description: desc.to_string(),
        parameters: params,
    })
    .collect()
}

/// 只读工具目录（意图预判为只读/问答时的首轮收窄；§9.8 预筛语义）。
fn tool_specs_read_only() -> Vec<ToolSpec> {
    tool_specs()
        .into_iter()
        .filter(|t| {
            matches!(
                t.name.as_str(),
                "read_file" | "list_dir" | "grep" | "git_read"
            )
        })
        .collect()
}

impl AgentSession {
    /// 会话级受管 worktree 根（v1.87；None = 主根会话）。
    pub fn managed_worktree(&self) -> Option<&Path> {
        self.managed_worktree.as_deref()
    }

    /// 创建会话（store 会话行 + shadow 快照库）。
    pub async fn create(
        store: Arc<Mutex<Store>>,
        snapshots: Arc<SnapshotStore>,
        provider: Arc<dyn ModelProvider>,
        config: AgentConfig,
        write_lock: ProjectWriteLock,
        rules: ProjectRules,
    ) -> Result<Arc<Self>, AgentError> {
        let model = provider.default_model();
        let session = {
            let mut st = store.lock().await;
            match &config.session_id {
                Some(id) => st.create_session_with_id(id, &config.project_id, &model),
                None => st.create_session(&config.project_id, &model),
            }
            .map_err(|e| AgentError::Store(e.to_string()))?
        };
        let readonly = config.policy.readonly || rules.readonly == Some(true);
        let tool_root = config
            .managed_worktree
            .clone()
            .unwrap_or_else(|| config.project_root.clone());
        let mut tool_ctx =
            ToolContext::new(&tool_root, Duration::from_secs(config.command_timeout_s));
        tool_ctx.readonly = std::sync::atomic::AtomicBool::new(readonly);
        if let Some(working_dir) = &config.working_dir {
            let joined = if working_dir.is_absolute() {
                working_dir.clone()
            } else {
                config.project_root.join(working_dir)
            };
            let canonical = joined
                .canonicalize()
                .map_err(|e| AgentError::Store(format!("working_dir 无效: {e}")))?;
            if !canonical.starts_with(&config.project_root) {
                return Err(AgentError::Store("working_dir 越出项目根".into()));
            }
            tool_ctx.command_cwd = canonical;
        }
        tool_ctx.dirty = config.dirty.clone();
        tool_ctx.team_denied_tools = config.team_denied_tools.clone();
        tool_ctx.lsp = config.lsp.clone();
        let (control_tx, control_rx) = mpsc::unbounded_channel();
        let (events_tx, _) = broadcast::channel(1024);
        let circuit_limits = config.circuit;
        let write_scope = config.write_scope.clone();
        let managed_worktree = config.managed_worktree.clone();
        let machine = StateMachine::with_limits(MachineLimits {
            model_retries: 2,
            fix_rounds: config.fix_rounds,
        });
        Ok(Arc::new(Self {
            session_id: session.id,
            config,
            circuit: Mutex::new(CircuitBreaker::new(circuit_limits)),
            machine: Mutex::new(machine),
            memory: Mutex::new(SessionMemory::default()),
            rules: Mutex::new(rules),
            store,
            snapshots,
            provider: RwLock::new(provider),
            tool_ctx,
            write_lock,
            write_scope,
            managed_worktree,
            control_tx,
            control_rx: Mutex::new(control_rx),
            running: AtomicBool::new(false),
            events_tx,
            first_edit_done: AtomicBool::new(false),
            touched_files: Mutex::new(BTreeSet::new()),
            pre_rollback_tree: Mutex::new(None),
            interrupt: Notify::new(),
        }))
    }

    /// 切换会话模型（§11 显式路由 / 降级：上下文随迁——消息流不动，
    /// 仅替换 provider，下一回合生效；model_fallback 事件入 Trace）。
    pub async fn switch_provider(&self, new_provider: std::sync::Arc<dyn ModelProvider>) {
        let old = self.provider.read().await.default_model();
        {
            let mut guard = self.provider.write().await;
            *guard = new_provider;
        }
        let new_model = self.provider.read().await.default_model();
        self.emit(
            EventKind::ModelFallback,
            &serde_json::json!({
                "from": old,
                "to": new_model,
                "context_migrated": true,
            }),
        )
        .await;
        let mut st = self.store.lock().await;
        let _ = st.set_session_model(&self.session_id, &new_model);
    }

    /// 当前会话模型名。
    pub async fn current_model(&self) -> String {
        self.provider.read().await.default_model()
    }

    /// AI ghost text（实验，默认 UI 关闭）：单轮只读补全，不进工具循环。
    /// 仅发送前缀 / suffix / 语言；失败或空响应返回错误，由 UI 静默回退。
    pub async fn inline_complete(
        &self,
        language: &str,
        prefix: &str,
        suffix: &str,
    ) -> Result<String, AgentError> {
        let provider = self.provider.read().await.clone();
        let model = provider.default_model();
        let mut request = ChatRequest::new(
            model,
            vec![
                ChatMessage::system(
                    "你是代码补全引擎。只输出光标处应插入的代码，不要解释、不要 Markdown 代码围栏、不要重复已有前缀。",
                ),
                ChatMessage::user(format!(
                    "Language: {language}\n\n<|before_cursor|>\n{prefix}\n<|after_cursor|>\n{suffix}"
                )),
            ],
        );
        request.max_tokens = 256;
        request.temperature = 0.1;
        let response = provider
            .chat(&request)
            .await
            .map_err(|e| AgentError::Model(e.to_string()))?;
        self.record_usage(response.usage).await;
        let mut text = response.content.trim().to_string();
        if text.starts_with("```") {
            text = text
                .trim_start_matches("```")
                .strip_prefix(language.trim())
                .unwrap_or(&text)
                .strip_prefix('\n')
                .unwrap_or(&text)
                .trim_end_matches("```")
                .trim_end()
                .to_string();
        }
        if text.is_empty() {
            return Err(AgentError::Model("empty inline completion".into()));
        }
        Ok(text)
    }

    /// 对话标题自动生成（v1.59；v1.91 质量修复）：单轮、无工具目录，请求带
    /// `TITLE_MARKER` 供测试替身识别；只发送首条消息开头，不外发完整任务文本。
    /// `reasoning_effort=low` 是质量关键：推理型 GLM 默认可消耗数百 reasoning
    /// token；`max_tokens=48` 且不带力度时曾以 `finish_reason=length` 结束且
    /// 正文为空，导致看似模型失败、实际全量退回本地截断。失败仍由调用方回退。
    pub async fn generate_title(&self, user_text: &str) -> Result<String, AgentError> {
        let provider = self.provider.read().await.clone();
        let excerpt: String = user_text.chars().take(2000).collect();
        let mut request = ChatRequest::new(
            provider.default_model(),
            vec![
                ChatMessage::system(format!(
                    "{TITLE_MARKER} Generate a concise title for the user's first request. \
                     Write it in the same language as the request. \
                     Name the user's task goal (for example: 修复登录超时 / Fix login timeout / 运行项目). \
                     Use 2-12 words, no more than 16 characters for CJK, 32 characters for Latin text. \
                     Do not explain, quote, wrap in markdown, or use a Title: prefix. \
                     Output only the title."
                )),
                ChatMessage::user(excerpt),
            ],
        );
        request.max_tokens = 128;
        request.temperature = 0.2;
        request.reasoning_effort = Some("low".into());
        let response = provider
            .chat(&request)
            .await
            .map_err(|e| AgentError::Model(e.to_string()))?;
        self.record_usage(response.usage).await;
        let title = sanitize_title(&response.content, 32);
        if title.is_empty() {
            return Err(AgentError::Model("empty session title".into()));
        }
        Ok(title)
    }

    /// 标题落库并广播 `session_title` 事件（WS 即时刷新对话列表，§14.2）。
    pub async fn set_title(&self, title: &str) -> Result<(), AgentError> {
        {
            let mut st = self.store.lock().await;
            st.set_session_title(&self.session_id, title)
                .map_err(|e| AgentError::Store(e.to_string()))?;
        }
        self.emit(
            EventKind::SessionTitle,
            &serde_json::json!({"title": title}),
        )
        .await;
        Ok(())
    }

    /// 设置熔断器预算（创建后按 config.toml 覆盖）。
    pub async fn set_circuit_limits(&self, limits: CircuitLimits) {
        *self.circuit.lock().await = CircuitBreaker::new(limits);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.events_tx.subscribe()
    }

    pub fn control(&self, cmd: ControlCommand) {
        let _ = self.control_tx.send(cmd);
    }

    pub fn interrupt_first_edit_buffer(&self) {
        self.interrupt.notify_one();
    }

    pub async fn current_state(&self) -> State {
        self.machine.lock().await.state()
    }

    async fn force_state(&self, next: State) {
        self.machine.lock().await.force_state(next);
    }

    async fn emit(&self, kind: EventKind, payload: &serde_json::Value) -> Option<Event> {
        let ev = {
            let mut st = self.store.lock().await;
            st.append_event(&self.session_id, kind, payload).ok()?
        };
        let _ = self.events_tx.send(ev.clone());
        Some(ev)
    }

    async fn set_status(&self, status: SessionStatus) {
        let mut st = self.store.lock().await;
        let _ = st.set_session_status(&self.session_id, status);
    }

    async fn record_usage(&self, usage: Usage) {
        if usage.input_tokens == 0 && usage.output_tokens == 0 {
            return;
        }
        let provider = self.provider.read().await.clone();
        let model = provider.default_model();
        // §11 v1.93：按 provider 配置单价折算（未定价模型计 0，宁少报不虚报），
        // 并作为熔断预算输入（§9.3）——超 token / 超预算在下一工具步检查点熔断。
        let cost = tenon_models::compute_cost(
            &self.config.price_table,
            &model,
            usage.input_tokens,
            usage.output_tokens,
        );
        let _status = self
            .circuit
            .lock()
            .await
            .record_usage(usage.input_tokens + usage.output_tokens, cost);
        let mut st = self.store.lock().await;
        let _ = st.record_model_usage(
            &self.session_id,
            provider.name(),
            &model,
            usage.input_tokens as i64,
            usage.output_tokens as i64,
            cost,
        );
    }

    /// 流式调用当前模型；权威 usage / tool calls 只取流末尾 Final。
    /// 小增量按 64 字符 / 120ms 合并，避免 SQLite 事件溯源被 token 级写入淹没。
    async fn stream_model_turn(
        &self,
        provider: &StdArc<dyn ModelProvider>,
        request: &ChatRequest,
    ) -> Result<tenon_models::ChatResponse, String> {
        let mut stream = provider
            .chat_stream(request)
            .await
            .map_err(|e| e.to_string())?;
        let mut pending = String::new();
        let mut last_flush = Instant::now();
        let mut final_response = None;
        while let Some(item) = stream.next().await {
            match item.map_err(|e| e.to_string())? {
                ChatStreamEvent::Delta(text) => {
                    pending.push_str(&text);
                    if pending.chars().count() >= 64
                        || last_flush.elapsed() >= Duration::from_millis(120)
                    {
                        self.emit(EventKind::ModelDelta, &serde_json::json!({"text": pending}))
                            .await;
                        pending.clear();
                        last_flush = Instant::now();
                    }
                }
                ChatStreamEvent::Final(resp) => final_response = Some(resp),
            }
        }
        if !pending.is_empty() {
            self.emit(EventKind::ModelDelta, &serde_json::json!({"text": pending}))
                .await;
        }
        final_response.ok_or_else(|| "模型流缺少最终响应".to_string())
    }

    /// 执行一个任务（完整 §9.1 循环）。
    /// L4 → L1：任务查询本地持久索引，构造受 token 预算约束的工作集。
    /// 检索失败 / 空索引回退为空；仓库内容按不可信数据注入。
    async fn l4_working_set(&self, query: &str) -> WorkingSet {
        const MAX_SLICES: usize = 4;
        const MAX_CONTEXT_TOKENS: u64 = 12_000;
        let embedding = tenon_fs::l4::embed(query, query);
        let hits = {
            let mut store = self.store.lock().await;
            match store.l4_search(&self.config.project_id, &embedding, MAX_SLICES * 2) {
                Ok(hits) => hits,
                Err(e) => {
                    tracing::debug!("L4 recall unavailable: {e}");
                    return WorkingSet::default();
                }
            }
        };
        let mut slices = Vec::new();
        let mut used_tokens = 0u64;
        for hit in hits {
            if hit.score <= 0.0 {
                continue;
            }
            let mut content = hit.text.clone();
            let mut tokens = tenon_core::context::estimate_tokens(&content);
            if used_tokens + tokens > MAX_CONTEXT_TOKENS {
                let remaining = MAX_CONTEXT_TOKENS.saturating_sub(used_tokens);
                if remaining < 128 {
                    break;
                }
                let end = content.char_indices().nth((remaining * 3) as usize);
                if let Some((idx, _)) = end {
                    content.truncate(idx);
                    tokens = remaining;
                }
            }
            slices.push(ContextSlice {
                path: hit.path,
                start_line: hit.start_line,
                end_line: hit.end_line,
                symbol: if hit.symbol.is_empty() {
                    None
                } else {
                    Some(hit.symbol)
                },
                content,
                relevance: Some(hit.score),
            });
            used_tokens += tokens;
            if slices.len() >= MAX_SLICES {
                break;
            }
        }
        WorkingSet { slices }
    }

    /// 执行一个任务（完整 §9.1 循环）。
    /// 任务入口（v1.93 并发守卫）：进行中（含挂起等待恢复）拒绝重入——
    /// 此前 Executing 中再发消息会并发跑两个任务循环，竞态改写会话消息历史。
    pub async fn run_task(&self, user_text: &str) -> TaskOutcome {
        if self.running.swap(true, Ordering::SeqCst) {
            return TaskOutcome::Error(
                "任务进行中（暂停 = 挂起待恢复）：请先停止或等待完成".into(),
            );
        }
        let outcome = self.run_task_inner(user_text).await;
        self.running.store(false, Ordering::SeqCst);
        outcome
    }

    async fn run_task_inner(&self, user_text: &str) -> TaskOutcome {
        // §9.7 v1.87 并行写锁：按 (project_id, worktree_scope) 计——主根会话互斥，
        // 不同受管 worktree 会话可与主根及彼此并行；全局配额由 daemon 控制。
        let scope_lock = self.write_lock.lock_for(&self.write_scope).await;
        let _guard = scope_lock.lock().await;
        self.first_edit_done.store(false, Ordering::SeqCst);
        self.touched_files.lock().await.clear();
        self.memory.lock().await.goals.push(user_text.to_string());

        // ---- IDLE → SENSING ----
        self.emit(
            EventKind::UserInput,
            &serde_json::json!({"text": user_text}),
        )
        .await;
        self.force_state(State::Sensing).await;
        self.set_status(SessionStatus::Sensing).await;

        // ---- Laya 意图预判（§9.8 集成点 #1）：只读先验 → 首轮收窄工具目录；
        // 不改变状态机转移，判定失败即回退（不阻塞）----
        let mut read_only_prior = false;
        if let Some(laya) = &self.config.laya {
            match laya.intent(user_text).await {
                tenon_laya::LayaOutcome::Success {
                    value, duration_ms, ..
                } => {
                    read_only_prior = matches!(
                        value,
                        tenon_laya::IntentLabel::PureQa | tenon_laya::IntentLabel::ReadOnlyAnalysis
                    );
                    self.emit(
                        EventKind::DeciderCall,
                        &serde_json::json!({
                            "feature": "intent",
                            "kind": "choice",
                            "result": value.as_str(),
                            "duration_ms": duration_ms,
                            "fallback": false,
                        }),
                    )
                    .await;
                }
                tenon_laya::LayaOutcome::Disabled => {}
                other => {
                    let reason = match &other {
                        tenon_laya::LayaOutcome::Unavailable(r) => (*r).to_string(),
                        tenon_laya::LayaOutcome::TimedOut => "timeout".to_string(),
                        _ => String::new(),
                    };
                    self.emit(
                        EventKind::DeciderCall,
                        &serde_json::json!({
                            "feature": "intent",
                            "kind": "choice",
                            "fallback": true,
                            "reason": reason,
                        }),
                    )
                    .await;
                }
            }
        }

        // 任务前快照（§10.3）
        let mut last_tree = match self.snapshots.snapshot() {
            Ok(t) => t,
            Err(e) => return self.snapshot_unavailable(e).await,
        };

        // L1 工作集：从 L4 本地索引召回任务相关切片（§10.1 / §10.2）。
        let working_set = self.l4_working_set(user_text).await;
        if !working_set.slices.is_empty() {
            self.emit(
                EventKind::Sensing,
                &serde_json::json!({
                    "l4_recall": true,
                    "slices": working_set.slices.iter().map(|slice| {
                        serde_json::json!({
                            "path": slice.path,
                            "start_line": slice.start_line,
                            "end_line": slice.end_line,
                            "symbol": slice.symbol,
                            "score": slice.relevance,
                        })
                    }).collect::<Vec<_>>(),
                }),
            )
            .await;
        }
        let user_message = match render_working_set(&working_set) {
            context if context.is_empty() => user_text.to_string(),
            context => format!("{user_text}\n\n{context}"),
        };
        let mut messages: Vec<ChatMessage> = vec![
            ChatMessage::system(tenon_core::prompt::build_system_prompt(
                &self.rules.lock().await.clone(),
                &self.memory.lock().await.clone(),
            )),
            ChatMessage::user(user_message),
        ];

        let mut changed_files: Vec<String> = Vec::new();
        let mut last_pre_tree: Option<String> = None;
        let mut final_answer: Option<String> = None;
        let mut steps = 0u32;
        let mut verification = String::new();
        let mut verification_strength = "none";

        let mut paused_reason: Option<String> = None;
        let mut error_msg: Option<String> = None;
        // §9.1 v1.53：截断续跑——截断的中间输出不是回答，连续多次才按模型失败处理
        let mut consecutive_truncations = 0u32;

        // ---- 模型回合循环（SENSING / DECIDING / EXECUTING 在回合内展开）----
        'rounds: for _round in 0..self.config.max_tool_rounds {
            self.force_state(State::Deciding).await;
            self.set_status(SessionStatus::Deciding).await;

            // 只读先验：首轮仅开放 A 级工具（§9.8 预筛语义，只收窄不放宽）；
            // 模型判断确需改动 → 后续回合恢复全目录
            let tools = if read_only_prior && _round == 0 {
                tool_specs_read_only()
            } else {
                tool_specs()
            };
            let provider = self.provider.read().await.clone();
            let request = ChatRequest {
                model: provider.default_model(),
                messages: messages.clone(),
                tools,
                max_tokens: 16_384,
                temperature: 0.2,
                reasoning_effort: None,
            };
            let resp = match self.stream_model_turn(&provider, &request).await {
                Ok(r) => r,
                Err(e) => {
                    // 侧向出口：模型失败 → ERROR（重试语义由 daemon 的 model_fallback 承接）
                    self.force_state(State::Error).await;
                    self.set_status(SessionStatus::Error).await;
                    self.emit(
                        EventKind::Error,
                        &serde_json::json!({"error": e.to_string()}),
                    )
                    .await;
                    error_msg = Some(format!("模型调用失败: {e}"));
                    break 'rounds;
                }
            };
            self.record_usage(resp.usage).await;
            steps += 1;

            // 决策意图卡
            self.emit(
                EventKind::Decision,
                &serde_json::json!({"intent": resp.content, "tool_calls": resp.tool_calls.len()}),
            )
            .await;

            // v1.53：截断的回复（finish_reason=length）没有工具调用不等于任务完成——
            // 推送已输出的部分并要求续写，避免长规划被 max_tokens 剪断后静默 Done。
            if resp.tool_calls.is_empty() && resp.finish_reason.as_deref() == Some("length") {
                consecutive_truncations += 1;
                if consecutive_truncations >= 3 {
                    self.force_state(State::Error).await;
                    self.set_status(SessionStatus::Error).await;
                    self.emit(
                        EventKind::Error,
                        &serde_json::json!({"error": "模型输出连续 3 次被截断（finish_reason=length）"}),
                    )
                    .await;
                    error_msg = Some("模型输出连续截断（finish_reason=length）".into());
                    break 'rounds;
                }
                messages.push(ChatMessage::assistant(resp.content.clone()));
                messages.push(ChatMessage::user(
                    "上一条回复因达到输出长度上限被截断。请从中断处直接继续：只输出剩余内容或发起工具调用，不要重复已输出的部分。",
                ));
                continue 'rounds;
            }
            consecutive_truncations = 0;

            if resp.tool_calls.is_empty() && changed_files.is_empty() {
                // ---- 纯回答（无需改动）：ANSWERING → SUMMARIZING → DONE ----
                self.force_state(State::Summarizing).await;
                self.set_status(SessionStatus::Done).await;
                self.force_state(State::Done).await;
                return TaskOutcome::Done(EvidenceCard {
                    answer: resp.content,
                    changed_files: vec![],
                    verification: String::new(),
                    verification_strength: "none".into(),
                    steps,
                    rolled_back: false,
                });
            }
            if resp.tool_calls.is_empty() {
                // 有改动后的收尾回答：跳出循环进入验证（回答入证据卡）
                messages.push(ChatMessage::assistant(resp.content.clone()));
                final_answer = Some(resp.content.clone());
                break 'rounds;
            }

            // ---- EXECUTING ----
            self.force_state(State::Executing).await;
            self.set_status(SessionStatus::Executing).await;

            let mut assistant = ChatMessage::assistant(resp.content.clone());
            assistant.tool_calls = resp.tool_calls.clone();
            let mut tool_messages: Vec<ChatMessage> = Vec::new();

            for call in &resp.tool_calls {
                // Esc / 熔断暂停检查点
                if let Some(cmd) = self.drain_control().await {
                    match cmd {
                        ControlCommand::Pause => {
                            self.force_state(State::Paused).await;
                            self.set_status(SessionStatus::Paused).await;
                            // v1.93 真挂起：任务停在原地等 Resume 继续 / Stop 退出。
                            // （此前直接 break 丢弃任务上下文，resume 命令为空操作。）
                            loop {
                                match self.drain_control().await {
                                    Some(ControlCommand::Resume) => {
                                        self.force_state(State::Executing).await;
                                        self.set_status(SessionStatus::Executing).await;
                                        break;
                                    }
                                    Some(ControlCommand::Stop) => {
                                        paused_reason = Some("用户停止".into());
                                        break 'rounds;
                                    }
                                    Some(ControlCommand::SetReadonly(v)) => {
                                        self.tool_ctx.readonly.store(v, Ordering::SeqCst);
                                    }
                                    _ => tokio::time::sleep(Duration::from_millis(150)).await,
                                }
                            }
                        }
                        ControlCommand::Stop => {
                            self.force_state(State::Paused).await;
                            self.set_status(SessionStatus::Paused).await;
                            paused_reason = Some("用户暂停".into());
                            break 'rounds;
                        }
                        // 非暂停态收到 resume 无事可做
                        ControlCommand::Resume => {}
                        // v1.93 实装：工具步间即时切换只读（此前为空操作）
                        ControlCommand::SetReadonly(v) => {
                            self.tool_ctx.readonly.store(v, Ordering::SeqCst);
                        }
                    }
                }
                // 熔断预算检查（§9.3 v1.93）：token / 成本按回合累计，超限即暂停
                if let CircuitStatus::Tripped(reason) = self.circuit.lock().await.status() {
                    self.emit(
                        EventKind::Error,
                        &serde_json::json!({"circuit_tripped": reason.label()}),
                    )
                    .await;
                    self.force_state(State::Paused).await;
                    self.set_status(SessionStatus::Paused).await;
                    paused_reason = Some(format!("熔断器触发（{}）", reason.label()));
                    break 'rounds;
                }

                let tool = Tool::from_name(&call.name);
                let level = tool.and_then(|t| t.level()).unwrap_or(Level::C);

                let decision = if self.tool_ctx.readonly.load(Ordering::SeqCst) && level != Level::A
                {
                    Decision::Denied("readonly")
                } else {
                    self.config.policy.decide(Action { level })
                };

                match decision {
                    Decision::Denied(reason) => {
                        self.emit(
                            EventKind::Error,
                            &serde_json::json!({"denied": call.name, "reason": reason}),
                        )
                        .await;
                        tool_messages.push(ChatMessage::tool_result(
                            call.id.clone(),
                            format!("拒绝（{reason}）：会话为只读，无法执行 {}", call.name),
                        ));
                        continue;
                    }
                    Decision::Auto => {
                        // v1.89：C / D 直接执行；目标域等关键参数进入风险审计。
                        if matches!(level, Level::C | Level::D | Level::Composite) {
                            self.emit(
                                EventKind::DirectAction,
                                &serde_json::json!({
                                    "direct_action": true,
                                    "tool": call.name,
                                    "level": level.as_str(),
                                    "args": call.arguments,
                                }),
                            )
                            .await;
                        }
                    }
                }

                // ---- 首改缓冲（§9.3：首个 B 级前，Esc 可断）----
                if level == Level::B && !self.first_edit_done.load(Ordering::SeqCst) {
                    self.emit(
                        EventKind::Decision,
                        &serde_json::json!({"first_edit": true, "tool": call.name}),
                    )
                    .await;
                    if self
                        .wait_first_edit_buffer(self.config.first_edit_buffer_ms)
                        .await
                    {
                        self.force_state(State::Paused).await;
                        self.set_status(SessionStatus::Paused).await;
                        paused_reason = Some("首改缓冲被打断（Esc）".into());
                        break 'rounds;
                    }
                    self.first_edit_done.store(true, Ordering::SeqCst);
                }

                // ---- B 级写前：熔断预检（文件预算，写前拦截）+ 快照（§10.3）----
                let mut pre_tree: Option<String> = None;
                if level == Level::B {
                    let prospective_new = call.name == "apply_patch"
                        && !self
                            .config
                            .project_root
                            .join(
                                call.arguments
                                    .get("file")
                                    .and_then(|f| f.as_str())
                                    .unwrap_or(""),
                            )
                            .exists();
                    {
                        let touched = self.touched_files.lock().await;
                        let circuit = self.circuit.lock().await;
                        let prospective = touched.len() as u32
                            + u32::from(
                                prospective_new
                                    && !touched.contains(
                                        &call
                                            .arguments
                                            .get("file")
                                            .and_then(|f| f.as_str())
                                            .unwrap_or("")
                                            .to_string(),
                                    ),
                            );
                        if prospective > circuit.limits().max_files {
                            drop(circuit);
                            drop(touched);
                            self.emit(
                                EventKind::Error,
                                &serde_json::json!({"circuit_tripped": "max_files"}),
                            )
                            .await;
                            self.force_state(State::Paused).await;
                            self.set_status(SessionStatus::Paused).await;
                            paused_reason = Some("熔断器触发（max_files）——写前拦截".into());
                            break 'rounds;
                        }
                    }
                    match self.snapshots.snapshot() {
                        Ok(t) => {
                            let anticipated: Vec<String> = call
                                .arguments
                                .get("file")
                                .and_then(|f| f.as_str())
                                .map(|f| vec![f.to_string()])
                                .unwrap_or_default();
                            // 「先快照后写入，同事务」（§10.3）：checkpoint 行在写盘前
                            // 落库——EXECUTING 中崩溃也能取到最近恢复点
                            let mut st = self.store.lock().await;
                            let _ = st.insert_checkpoint(&self.session_id, &t, &anticipated, None);
                            drop(st);
                            pre_tree = Some(t.clone());
                            last_pre_tree = Some(t);
                        }
                        Err(e) => return self.snapshot_unavailable(e).await,
                    }
                }

                // ---- Laya 命令风险辅助（§9.8 集成点 #2）：规则引擎为主、
                // Laya 补盲区；打分仅生成「建议人工确认」提示并入 Trace
                //（decider_call），不改变 A/B/C/D 分级与档位语义 ----
                let mut risk_hint: Option<String> = None;
                if matches!(
                    call.name.as_str(),
                    "run_tests" | "run_build" | "install_deps"
                ) {
                    let cmd = call
                        .arguments
                        .get("command")
                        .and_then(|c| c.as_str())
                        .unwrap_or("");
                    if !cmd.is_empty() {
                        match tenon_core::risk::rule_risk(cmd) {
                            Some(score) => {
                                self.emit(
                                    EventKind::DeciderCall,
                                    &serde_json::json!({
                                        "feature": "risk",
                                        "kind": "score",
                                        "result": (f64::from(score) * 100.0).round() / 100.0,
                                        "rule": true,
                                    }),
                                )
                                .await;
                                if score >= 0.7 {
                                    risk_hint = Some(format!(
                                        "⚠️ [Laya] 命令风险 {score:.2}（规则命中）——建议人工确认"
                                    ));
                                }
                            }
                            None => {
                                if let Some(laya) = &self.config.laya {
                                    match laya.risk(cmd).await {
                                        tenon_laya::LayaOutcome::Success {
                                            value,
                                            duration_ms,
                                            ..
                                        } => {
                                            let score = (f64::from(value) * 100.0).round() / 100.0;
                                            self.emit(
                                                EventKind::DeciderCall,
                                                &serde_json::json!({
                                                    "feature": "risk",
                                                    "kind": "score",
                                                    "result": score,
                                                    "duration_ms": duration_ms,
                                                    "rule": false,
                                                }),
                                            )
                                            .await;
                                            if score >= 0.7 {
                                                risk_hint = Some(format!(
                                                    "⚠️ [Laya] 命令风险 {score:.2}——建议人工确认"
                                                ));
                                            }
                                        }
                                        other => {
                                            let reason = match &other {
                                                tenon_laya::LayaOutcome::Unavailable(r) => {
                                                    (*r).to_string()
                                                }
                                                tenon_laya::LayaOutcome::TimedOut => {
                                                    "timeout".to_string()
                                                }
                                                _ => String::new(),
                                            };
                                            self.emit(
                                                EventKind::DeciderCall,
                                                &serde_json::json!({
                                                    "feature": "risk",
                                                    "kind": "score",
                                                    "fallback": true,
                                                    "reason": reason,
                                                }),
                                            )
                                            .await;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                // ---- 执行 ----
                let mut output = execute_tool(&self.tool_ctx, &call.name, &call.arguments);
                // 风险提示入工具输出（Trace 与模型可见；仅提示，不改分级）
                if let Some(hint) = &risk_hint {
                    output.content = format!("{hint}\n{}", output.content);
                }
                let output_json = serde_json::to_value(&output).unwrap_or_default();
                let ev = self
                    .emit(
                        if output.changed_files.is_empty() {
                            EventKind::CommandRun
                        } else {
                            EventKind::PatchApplied
                        },
                        &serde_json::json!({"tool": call.name, "output": output_json}),
                    )
                    .await;
                // 人机共编冲突（§8.6）：三栏预览事件（你的改动 / 代理改动 / base）
                if let Some(view) = &output.dirty_conflict {
                    self.emit(
                        EventKind::Diagnostics,
                        &serde_json::json!({
                            "dirty_conflict": true,
                            "path": view.path,
                            "base": view.base,
                            "ours": view.ours,
                            "theirs": view.theirs,
                        }),
                    )
                    .await;
                }

                // AgentTrace 明细（§14.2）
                if let Some(ev) = &ev {
                    let mut st = self.store.lock().await;
                    let _ = st.insert_tool_call(
                        &self.session_id,
                        ev.id,
                        &call.name,
                        match level {
                            Level::A => StoreLevel::A,
                            Level::B => StoreLevel::B,
                            Level::C => StoreLevel::C,
                            Level::D => StoreLevel::D,
                            Level::Composite => StoreLevel::Composite,
                        },
                        0,
                    );
                }

                // 熔断记账 + 事件级快照链（B 级）
                if level == Level::B {
                    {
                        let mut touched = self.touched_files.lock().await;
                        for f in &output.changed_files {
                            touched.insert(f.clone());
                            if !changed_files.contains(f) {
                                changed_files.push(f.clone());
                            }
                        }
                    }
                    let lines = self.count_project_lines(&output.changed_files);
                    let status = self.circuit.lock().await.record_patch(PatchFootprint {
                        total_files_touched: self.touched_files.lock().await.len() as u32,
                        lines_changed: lines,
                    });
                    let post_tree = self
                        .snapshots
                        .snapshot()
                        .unwrap_or_else(|_| last_tree.clone());
                    // 每步 checkpoint（该步快照点 + 改动文件集，§10.3）
                    if let Some(ev) = &ev {
                        let mut st = self.store.lock().await;
                        let _ = st.insert_checkpoint(
                            &self.session_id,
                            &pre_tree.unwrap_or_else(|| post_tree.clone()),
                            &output.changed_files,
                            Some(ev.seq),
                        );
                    }
                    last_tree = post_tree;

                    if let CircuitStatus::Tripped(reason) = status {
                        self.emit(
                            EventKind::Error,
                            &serde_json::json!({"circuit_tripped": reason.label()}),
                        )
                        .await;
                        self.force_state(State::Paused).await;
                        self.set_status(SessionStatus::Paused).await;
                        paused_reason = Some(format!("熔断器触发（{}）", reason.label()));
                        break 'rounds;
                    }
                }

                tool_messages.push(ChatMessage::tool_result(
                    call.id.clone(),
                    if output.ok {
                        output.content.clone()
                    } else {
                        format!("失败: {}", output.content)
                    },
                ));
            }

            if !tool_messages.is_empty() {
                messages.push(assistant);
                messages.extend(tool_messages);
            }
        }

        // ---- 验证（VERIFYING，双通道 / 降级通道，§9.4）----
        if paused_reason.is_none() && error_msg.is_none() && !changed_files.is_empty() {
            self.force_state(State::Verifying).await;
            self.set_status(SessionStatus::Verifying).await;
            let (ok, detail, low) = self.verify(changed_files.clone()).await;
            verification = format!("{}（{}）", if ok { "通过" } else { "失败" }, detail);
            verification_strength = if low { "low" } else { "high" };
            self.emit(
                EventKind::Diagnostics,
                &serde_json::json!({"verification": detail, "ok": ok, "low": low}),
            )
            .await;
        }

        // ---- 收尾 ----
        if let Some(reason) = paused_reason {
            return TaskOutcome::Paused {
                state: self.current_state().await.to_string(),
                reason,
            };
        }
        if let Some(err) = error_msg {
            // 失败语义：回滚到最近写前快照（§10.3 崩溃恢复：EXECUTING 中失败不保留半成品）
            if let Some(pre) = last_pre_tree {
                let _ = self.snapshots.restore(&pre);
                self.emit(EventKind::Rollback, &serde_json::json!({"to": pre}))
                    .await;
            }
            self.force_state(State::RolledBack).await;
            self.set_status(SessionStatus::Error).await;
            return TaskOutcome::Error(err);
        }

        // 终点 checkpoint（时间轴 + 整体恢复点）
        {
            let mut st = self.store.lock().await;
            let _ = st.insert_checkpoint(&self.session_id, &last_tree, &changed_files, None);
        }
        self.emit(
            EventKind::Checkpoint,
            &serde_json::json!({"tree": last_tree, "files": changed_files}),
        )
        .await;
        self.force_state(State::Summarizing).await;
        self.force_state(State::Done).await;
        self.set_status(SessionStatus::Done).await;
        TaskOutcome::Done(EvidenceCard {
            answer: final_answer.unwrap_or_default(),
            changed_files,
            verification,
            verification_strength: verification_strength.into(),
            steps,
            rolled_back: false,
        })
    }

    fn count_project_lines(&self, files: &[String]) -> u64 {
        let mut n = 0u64;
        for f in files {
            if let Ok(content) = std::fs::read_to_string(self.config.project_root.join(f)) {
                n += content.lines().count() as u64;
            }
        }
        n
    }

    async fn snapshot_unavailable(&self, e: tenon_snapshot::SnapshotError) -> TaskOutcome {
        // 「自动 = 必可回滚」不变式（§10.3）：快照不可用 → 暂停（交互档）
        self.emit(
            EventKind::Error,
            &serde_json::json!({"snapshot_unavailable": e.to_string()}),
        )
        .await;
        self.force_state(State::Paused).await;
        self.set_status(SessionStatus::Paused).await;
        TaskOutcome::Paused {
            state: "paused".into(),
            reason: format!("快照库不可用，自动档降级为交互档：{e}"),
        }
    }

    async fn drain_control(&self) -> Option<ControlCommand> {
        let mut rx = self.control_rx.lock().await;
        rx.try_recv().ok()
    }

    /// 首改缓冲等待：true = 被 Esc 打断。
    async fn wait_first_edit_buffer(&self, ms: u64) -> bool {
        let fut = self.interrupt.notified();
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(ms)) => false,
            _ = fut => true,
        }
    }

    /// 验证（§9.4）：测试双通道；无测试清单走降级通道（低强度）。
    async fn verify(&self, changed: Vec<String>) -> (bool, String, bool) {
        let root = self.config.project_root.clone();
        let command_cwd = match &self.config.working_dir {
            Some(dir) if dir.is_absolute() => dir.clone(),
            Some(dir) => root.join(dir),
            None => root.clone(),
        };
        if let Some(cmd) = crate::executor::detect_test_command(&root) {
            let spec = tenon_sandbox::SandboxSpec::Offline {
                project_root: root.clone(),
            };
            return match tenon_sandbox::exec_command(
                &cmd,
                &command_cwd,
                Duration::from_secs(self.config.command_timeout_s),
                &spec,
            ) {
                Ok(out) => {
                    let mut ok = out.success();
                    let mut detail = format!(
                        "$ {cmd}\nexit={}\n{}{}",
                        out.exit_code.unwrap_or(-1),
                        out.stdout.trim(),
                        out.stderr.trim()
                    );
                    if let Some((lsp_ok, lsp_detail)) = self.lsp_diagnostics(&changed).await {
                        ok &= lsp_ok;
                        detail.push_str("\nLSP: ");
                        detail.push_str(&lsp_detail);
                    }
                    (ok, detail, false)
                }
                Err(e) => (false, format!("测试执行失败: {e}"), false),
            };
        }
        // 降级通道：读回 diff 自检
        let mut detail = String::from("无测试清单——降级验证：读回改动自检");
        let mut ok = true;
        for f in &changed {
            match std::fs::read_to_string(root.join(f)) {
                Ok(content) => {
                    if content.is_empty() {
                        ok = false;
                        detail.push_str(&format!("\n{f}: 改后为空"));
                    }
                }
                Err(e) => {
                    ok = false;
                    detail.push_str(&format!("\n{f}: 读回失败 {e}"));
                }
            }
        }
        detail.push_str("\n验证强度：低（建议补测试）");
        if let Some((lsp_ok, lsp_detail)) = self.lsp_diagnostics(&changed).await {
            ok &= lsp_ok;
            detail.push_str("\nLSP: ");
            detail.push_str(&lsp_detail);
        }
        (ok, detail, true)
    }

    /// LSP 诊断通道（§9.4）：只在共享宿主接入时启用；最多检查 10 个改动文件。
    async fn lsp_diagnostics(&self, changed: &[String]) -> Option<(bool, String)> {
        let lsp = self.config.lsp.as_ref()?;
        let mut issues = Vec::new();
        let mut queried = 0usize;
        for file in changed.iter().take(10) {
            let result = lsp
                .request(&self.config.project_root, file, "diagnostics", 0, 0, None)
                .await;
            match result {
                Ok(value) => {
                    queried += 1;
                    for item in value
                        .get("items")
                        .and_then(|v| v.as_array())
                        .cloned()
                        .unwrap_or_default()
                    {
                        let severity = item["severity"].as_i64().unwrap_or(1);
                        if severity <= 2 {
                            issues.push(format!(
                                "{}: {}",
                                file,
                                item["message"].as_str().unwrap_or("diagnostic")
                            ));
                        }
                    }
                }
                Err(tenon_lsp::LspManagerError::BadPath(path)) => {
                    issues.push(format!("路径越界: {path}"));
                }
                Err(tenon_lsp::LspManagerError::PackUnavailable(_)) => {}
                Err(e) => issues.push(format!("{file}: {e}")),
            }
        }
        if queried == 0 && issues.is_empty() {
            return None;
        }
        if issues.is_empty() {
            Some((true, format!("诊断通过（{queried} 个文件）")))
        } else {
            Some((false, issues.join("\n")))
        }
    }

    /// 回滚到最近 checkpoint（§15 control rollback）：
    /// 目标 = 最近一个事件级快照（该事件写入前的状态）；unrevert 快照先行（§10.3）。
    pub async fn rollback_last(&self) -> Result<Vec<String>, AgentError> {
        let cps = {
            let mut st = self.store.lock().await;
            st.checkpoints(&self.session_id)
                .map_err(|e| AgentError::Store(e.to_string()))?
        };
        // 事件级快照（event_seq 非空）记录的是该步写前状态
        let target = cps
            .iter()
            .rev()
            .find(|c| c.event_seq.is_some() && !c.files.is_empty())
            .cloned();
        let Some(target) = target else {
            return Ok(vec![]);
        };
        let safety = self
            .snapshots
            .snapshot()
            .map_err(|e| AgentError::Snapshot(e.to_string()))?;
        *self.pre_rollback_tree.lock().await = Some(safety);
        self.snapshots
            .restore(&target.tree)
            .map_err(|e| AgentError::Snapshot(e.to_string()))?;
        self.emit(
            EventKind::Rollback,
            &serde_json::json!({"tree": target.tree, "files": target.files}),
        )
        .await;
        self.force_state(State::RolledBack).await;
        self.set_status(SessionStatus::RolledBack).await;
        Ok(target.files.clone())
    }

    /// 撤销回滚（§10.3 unrevert）：恢复到最近回滚前状态（回滚双向语义）。
    pub async fn unrevert(&self) -> Result<(), AgentError> {
        let target = self
            .pre_rollback_tree
            .lock()
            .await
            .clone()
            .ok_or_else(|| AgentError::Snapshot("无回滚记录".into()))?;
        self.snapshots
            .restore(&target)
            .map_err(|e| AgentError::Snapshot(e.to_string()))?;
        self.emit(EventKind::Unrollback, &serde_json::json!({"tree": target}))
            .await;
        Ok(())
    }

    pub fn config(&self) -> &AgentConfig {
        &self.config
    }
}

/// 模型标题常见的元文本前缀（实测 GLM 会输出 "The user says: …" /
/// "The user wants …" 类英文套壳，违反 v1.58「只输出标题本身」）：剥掉后取正文。
const TITLE_META_PREFIXES: &[&str] = &[
    "the user says:",
    "the user wants",
    "the user asks:",
    "user says:",
    "user wants",
    "title:",
    "标题：",
    "标题:",
];

/// 清理模型产出 / 本地回退的对话标题（v1.58）：取首个非空行、剥元文本前缀、
/// 去包裹引号与结尾句读、按字符截断；空输入返回空串（调用方保持「未生成」语义）。
pub fn sanitize_title(input: &str, max_chars: usize) -> String {
    let first_line = input
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    let stripped = TITLE_META_PREFIXES
        .iter()
        .find_map(|prefix| {
            // 前缀均为 ASCII，直接按字节做大小写无关匹配，避免 Unicode
            // 大小写转换改变字节长度导致切边 panic。
            if first_line.len() >= prefix.len()
                && first_line.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
            {
                Some(&first_line[prefix.len()..])
            } else {
                None
            }
        })
        .unwrap_or(first_line)
        .trim();
    let unwrapped = stripped
        .strip_prefix(['"', '“', '‘', '「', '『', '《'])
        .and_then(|s| s.strip_suffix(['"', '”', '’', '」', '』', '》']))
        .unwrap_or(stripped)
        .trim()
        .trim_end_matches(['。', '.', '！', '？', '?', '!', '；', ';'])
        .trim();
    // 尾部非引号时成对剥离失效（如 The user says: "xxx"正文）——补剥孤立前引号。
    let unwrapped = unwrapped
        .strip_prefix(['"', '“', '‘'])
        .unwrap_or(unwrapped)
        .trim();
    if unwrapped.chars().count() <= max_chars {
        return unwrapped.to_string();
    }
    let mut cut: String = unwrapped.chars().take(max_chars).collect();
    // 拉丁标题避免截到半个词；CJK 无空格时保留定长截断。
    if let Some(pos) = cut.rfind(char::is_whitespace) {
        cut.truncate(pos);
    }
    cut.trim_end().to_string()
}

#[cfg(test)]
mod parallel_write_lock_tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn different_worktree_scopes_run_in_parallel() {
        // §9.7 v1.87 并行写锁：主根与受管 worktree 作用域互不阻塞。
        let lock = ProjectWriteLock::new();
        let root = lock.lock_for("root").await;
        let wt = lock.lock_for("/tmp/wt/a").await;
        let _root_guard = root.lock().await;
        // 不同作用域立即可锁（并行）
        let wt_guard = tokio::time::timeout(Duration::from_millis(100), wt.lock())
            .await
            .expect("worktree 作用域不应被主根写锁阻塞");
        drop(wt_guard);
        // 同作用域仍互斥
        let root_again = lock.lock_for("root").await;
        let contended = tokio::time::timeout(Duration::from_millis(100), root_again.lock()).await;
        assert!(contended.is_err(), "同作用域写锁必须互斥");
    }
}

#[cfg(test)]
mod agent_config_tests {
    use super::*;

    #[test]
    fn agent_config_for_project_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = AgentConfig::for_project(dir.path().to_path_buf(), "test-project");
        assert_eq!(cfg.project_id, "test-project");
        assert_eq!(cfg.project_root, dir.path());
        assert_eq!(cfg.first_edit_buffer_ms, 2000);
        assert!(!cfg.policy.readonly);
    }

    #[test]
    fn agent_config_first_edit_buffer_override() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = AgentConfig::for_project(dir.path().to_path_buf(), "p1");
        cfg.first_edit_buffer_ms = 500;
        assert_eq!(cfg.first_edit_buffer_ms, 500);
    }



    #[test]
    fn evidence_card_serialization() {
        let card = EvidenceCard {
            answer: "done".into(),
            changed_files: vec!["a.ts".into()],
            verification: "high".into(),
            rolled_back: false,
            steps: 3,
            verification_strength: "high".into(),
        };
        let json = serde_json::to_string(&card).unwrap();
        let parsed: EvidenceCard = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.answer, "done");
    }

}
