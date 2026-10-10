//! Agent 会话（设计方案 §9.1 / §9.3 / §9.4 / §9.7 / §10.3）：
//! 状态机驱动任务循环，联动权限、快照、熔断与事件流；v1.89 无审批。
//!
//! 转移规则由 `tenon_core::machine::StateMachine` 单测覆盖；运行态经
//! `force_state` 对齐并保留计数（拒绝改案 ≤2、模型重试 ≤2、修复轮次）。

use std::collections::{BTreeSet, HashSet};
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
    render_working_set, ContextSlice, ProjectRules, SessionMemory, WorkingSet, MAX_L2_GOALS,
};
use tenon_core::gates::{ConfirmDecision, Gate, GateVerdict};
use tenon_core::machine::{Limits as MachineLimits, State, StateMachine};
use tenon_core::policy::{Action, Decision, Level, Policy};
use tenon_core::tools::Tool;
use tenon_models::{
    ChatMessage, ChatRequest, ChatStreamEvent, ModelProvider, PriceTable, Role, ToolSpec, Usage,
    MEMORY_MARKER, TITLE_MARKER,
};
use tenon_snapshot::SnapshotStore;
use tenon_store::{
    Checkpoint, Event, EventKind, Level as StoreLevel, MemoryRecord, SessionStatus, Store,
};

use crate::executor::{execute_tool, ToolContext, ToolOutput};

/// 每项目 L5 记忆 active 上限（§10.1 v1.104）。
const MAX_MEMORIES_PER_PROJECT: usize = 200;
/// L5 记忆去重余弦阈值（§10.1 v1.104）。
const MEMORY_DEDUPE_THRESHOLD: f32 = 0.90;

#[derive(Clone)]
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
    /// L5 跨会话对话记忆（§10.1 v1.104）：false = 不提取不注入。
    pub memories_enabled: bool,
    /// 技能全局目录（§13.4 v1.130；`~/.tenon/skills/`，daemon 注入测试隔离路径）。
    pub skills_global_dir: PathBuf,
    /// 停用技能名单（§13.4 v1.130；settings.json `skills.disabled` 快照）。
    pub skills_disabled: Vec<String>,
    /// MCP 插件宿主（§13.5 v1.145；settings `mcp.servers` 快照，daemon 构建；None = 未接入）。
    pub mcp: Option<Arc<tenon_mcp::McpHost>>,
    /// 价格表（§11 v1.93）：daemon 按 provider 配置构建；未定价模型计 0。
    pub price_table: PriceTable,
    /// 自动 fallback 备用链（§11 v1.171）：主 provider 瞬时错误重试穷尽后按序
    /// 自动切换（上下文随迁）；daemon 按 settings `models.fallback` 快照构建注入，
    /// 新会话生效。空 = 不自动 fallback（主 provider 穷尽即 ERROR，行为同 v1.170 前）。
    pub fallback_providers: Vec<StdArc<dyn ModelProvider>>,
    /// 生成参数（§11 v1.174）：会话模型回合的输出上限与采样温度
    /// （settings `models.generation` 覆盖，新会话生效；默认 16384 / 0.2）。
    pub generation_max_tokens: u32,
    pub generation_temperature: f32,
    /// 用户 hooks（§13.6 v1.180）：daemon 按 settings `[hooks]` 快照注入，
    /// 新会话生效；空 = 无回调。
    pub hooks: Vec<crate::hooks::HookConfig>,
    /// 子代理编排器（§9.5 v1.190）：daemon 实现注入（worktree 池 + 子会话
    /// 登记 + 批内并发）；None = 工具返回未接入。新会话生效。
    pub subagents: Option<std::sync::Arc<dyn crate::subagents::SubagentOrchestrator>>,
    /// v2.0 安全档位（design-v2.md §4.1）：ExecMode × Approval——read_only
    /// 派生会话只读、full_access 派生命令沙箱降级；approval 决定 Hold 面。
    pub gate: Gate,
}

impl std::fmt::Debug for AgentConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.debug_fields(f)
    }
}

impl AgentConfig {
    /// 手写 Debug（v1.171）：`fallback_providers` 是 trait object 无 Debug，
    /// 以条目数代替，其余字段照常。
    fn debug_fields(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentConfig")
            .field("project_id", &self.project_id)
            .field("project_root", &self.project_root)
            .field("fix_rounds", &self.fix_rounds)
            .field("max_tool_rounds", &self.max_tool_rounds)
            .field("memories_enabled", &self.memories_enabled)
            .field("fallback_providers", &self.fallback_providers.len())
            .finish_non_exhaustive()
    }

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
            memories_enabled: true,
            skills_global_dir: tenon_config::Config::data_dir().join("skills"),
            skills_disabled: Vec::new(),
            mcp: None,
            price_table: PriceTable::new(),
            fallback_providers: Vec::new(),
            generation_max_tokens: 16_384,
            generation_temperature: 0.2,
            hooks: Vec::new(),
            subagents: None,
            gate: Gate::default(),
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

/// v2.0 档位确认：待确认动作（design-v2.md §4.1；UI 确认卡与
/// `GET /session/:id` 的 `pending_confirm` 载荷）。args 为截断后的参数预览。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingConfirm {
    pub tool: String,
    pub level: String,
    pub args: String,
}

/// v1.171 模型回合自动恢复结果（§9.1）：穷尽落 ERROR，退避中被打断按暂停落地。
enum ModelTurnError {
    /// 自动恢复全链穷尽（重试 ≤10 + fallback 链），携带末次错误。
    Exhausted(tenon_models::ProviderError),
    /// 退避等待期间收到 interrupt（Esc / 停止）——短路退出，按暂停语义处理。
    Interrupted,
}

/// 控制命令（§15 `/session/:id/control`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlCommand {
    Pause,
    Resume,
    Stop,
    SetReadonly(bool),
    /// v1.161 手动压缩（§10.2）：运行态下跳过 24k 阈值，下一模型回合立即省略陈旧工具输出。
    Compact,
    /// v2.0 运行中切换确认档（design-v2.md §4.1）：下一工具步生效；
    /// Hold 等待期间收到即时重判（改 never/放行即解锁）。
    SetApproval(tenon_core::gates::ApprovalGear),
    /// v2.0 运行中切换执行边界档（design-v2.md §4.1）：read_only 置会话
    /// 只读、full_access 置命令沙箱降级，下一工具步生效。
    SetExecMode(tenon_core::gates::ExecMode),
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
    /// v1.161 手动压缩标志：ControlCommand::Compact 置位，下一模型回合消费（§10.2）。
    force_compact: AtomicBool,
    touched_files: Mutex<BTreeSet<String>>,
    /// 最近一次回滚前的安全快照（unrevert 恢复点，§10.3）。
    pre_rollback_tree: Mutex<Option<String>>,
    /// MCP 工具 schema 快照（§13.5 v1.145；会话创建时 tools/list，静态贯穿任务）。
    mcp_specs: Vec<ToolSpec>,
    interrupt: Notify,
    /// 任务进行中标志（v1.93 并发守卫）：挂起等待恢复期间同样为 true。
    running: AtomicBool,
    /// v2.0 档位运行态（design-v2.md §4.1）：初始取 AgentConfig.gate，
    /// Approval 可经 ControlCommand::SetApproval 运行中切换。
    gate: Mutex<Gate>,
    /// v2.0 allow_session 记忆（design-v2.md §4.1）：本会话已放行工具名集合。
    gate_allowed: Mutex<HashSet<String>>,
    /// v2.0 确认回路：待确认动作（UI / GET /session 载荷）+ 决议通道。
    confirm_pending: Mutex<Option<PendingConfirm>>,
    confirm_tx: Mutex<Option<tokio::sync::oneshot::Sender<ConfirmDecision>>>,
}

fn tool_specs() -> Vec<ToolSpec> {
    [
        ("read_file", "读取文本文件", serde_json::json!({
            "type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]
        })),
        ("laya_decide", "本地决策器结构化判定（Laya，零 token、毫秒级；结果未校准，仅作排序/预筛/提示参考，不作事实结论、不替代测试与验证）：kind=intent 文本→意图标签（pure_qa/needs_change/read_only_analysis/needs_network）｜kind=risk 命令→0..1 风险分｜kind=route 任务文本→是否建议轻模型；仅结构化判定，非开放问答与生成", serde_json::json!({
            "type": "object",
            "properties": {
                "kind": {"type": "string", "enum": ["intent", "risk", "route"]},
                "text": {"type": "string"}
            },
            "required": ["kind", "text"]
        })),
        ("skill_use", "读取代理技能 SKILL.md 全文（§13.4：目录见系统提示「可用技能」节——需要某技能的方法指引时按名称调用加载正文）", serde_json::json!({
            "type": "object", "properties": {"name": {"type": "string"}}, "required": ["name"]
        })),
        ("subtasks", "子任务清单（多步任务主动分解，规则见系统提示「子任务清单」节）：items 为整张清单的全量状态 [{title, status}]，status ∈ pending|in_progress|done；开始或完成一项即重发整张清单更新状态，任务收尾前所有项必须 done；单步任务与纯问答不用", serde_json::json!({
            "type": "object",
            "properties": {
                "items": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "title": {"type": "string"},
                            "status": {"type": "string", "enum": ["pending", "in_progress", "done"]}
                        },
                        "required": ["title", "status"]
                    }
                }
            },
            "required": ["items"]
        })),
        ("spawn_subagents", "并行分发独立子任务（§9.5：worktree 隔离的子代理，B 级）：tasks 为 1-3 项 {instruction, files}——instruction 一句话完整自洽的子任务指令，files 为该任务计划触碰的文件集（相对路径；各任务文件集不得相交，相交者被拒绝）；每个子代理在独立受管 worktree 内运行并登记为独立会话，完成后返回各自摘要，合并 / 丢弃在子会话行处置；需要多文件并行改造且各文件归属清晰时使用", serde_json::json!({
            "type": "object",
            "properties": {
                "tasks": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "instruction": {"type": "string"},
                            "files": {"type": "array", "items": {"type": "string"}}
                        },
                        "required": ["instruction", "files"]
                    },
                    "minItems": 1,
                    "maxItems": 3
                }
            },
            "required": ["tasks"]
        })),
        ("submit_plan", "提交执行计划并暂停等待用户批准（§9.2 计划模式，Codex 形态；复杂任务先计划后执行，规则见系统提示「计划模式」节）：items 为 1-12 项的一句话计划（说明改什么、为什么、怎么验证）；提交后任务暂停，用户批准后再继续执行；单步任务、纯问答与简单改动不用", serde_json::json!({
            "type": "object",
            "properties": {
                "items": {
                    "type": "array",
                    "items": {"type": "string"},
                    "minItems": 1,
                    "maxItems": 12
                }
            },
            "required": ["items"]
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
        ("apply_patch", "编辑文件（二选一）：① file + range(1-based 行区间含端点，缺省追加) + content；② 定向替换 file + search + replace（search 须在文件中唯一，空白不一致可容忍）——定向编辑优先用 ②", serde_json::json!({
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
        ("web_search", "网络搜索（C 级直执并审计，免密钥 Bing/DDG 多后端）：返回标题/URL/摘要 JSON 列表；需要时效性信息（新版本、新闻、文档现状）时先用", serde_json::json!({
            "type": "object",
            "properties": {
                "query": {"type": "string"},
                "max_results": {"type": "integer", "minimum": 1, "maximum": 10}
            },
            "required": ["query"]
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
/// submit_plan 零工作区副作用（§9.2 v1.179），只读先验轮同样可用。
fn tool_specs_read_only() -> Vec<ToolSpec> {
    tool_specs()
        .into_iter()
        .filter(|t| {
            matches!(
                t.name.as_str(),
                "read_file"
                    | "list_dir"
                    | "grep"
                    | "git_read"
                    | "laya_decide"
                    | "skill_use"
                    | "submit_plan"
            )
        })
        .collect()
}

/// §13.3 v1.187：MCP 元工具 schema（跨服务器 resources / prompts；mcp 桥接入时
/// 附加在服务器工具目录之后；只读先验轮白名单不含——调用即拉起外部进程）。
fn mcp_meta_specs() -> [ToolSpec; 4] {
    [
        ToolSpec {
            name: "mcp_meta_resources_list".into(),
            description: "列出全部 MCP 服务器的可读资源（server / name / uri）".into(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
        },
        ToolSpec {
            name: "mcp_meta_resources_read".into(),
            description: "读取指定 MCP 服务器资源文本：server + uri".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {"server": {"type": "string"}, "uri": {"type": "string"}},
                "required": ["server", "uri"]
            }),
        },
        ToolSpec {
            name: "mcp_meta_prompts_list".into(),
            description: "列出全部 MCP 服务器的提示模板（server / name / description）".into(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
        },
        ToolSpec {
            name: "mcp_meta_prompts_get".into(),
            description:
                "取指定 MCP 服务器提示模板的渲染文本：server + name（可选 arguments 对象）".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "server": {"type": "string"},
                    "name": {"type": "string"},
                    "arguments": {"type": "object"}
                },
                "required": ["server", "name"]
            }),
        },
    ]
}

/// MCP 工具目录（§13.5 v1.145）：`mcp_{server}_{tool}` 进模型 schema；
/// inputSchema 非 object 时兜底空 object；描述标注来源 server。
/// 只读先验轮白名单天然不含 MCP 工具（外部进程能力面非只读）。
fn build_mcp_specs(tools: &[tenon_mcp::McpHostTool]) -> Vec<ToolSpec> {
    tools
        .iter()
        .map(|entry| {
            let name = tenon_mcp::mcp_tool_name(&entry.server, &entry.tool.name);
            let description = if entry.tool.description.is_empty() {
                format!("MCP 插件 {} 的工具 {}", entry.server, entry.tool.name)
            } else {
                format!(
                    "{}（MCP 插件 {} 提供）",
                    entry.tool.description, entry.server
                )
            };
            let parameters =
                if entry.tool.input_schema.get("type") == Some(&serde_json::json!("object")) {
                    entry.tool.input_schema.clone()
                } else {
                    serde_json::json!({"type": "object", "properties": {}})
                };
            ToolSpec {
                name,
                description,
                parameters,
            }
        })
        .collect()
}

/// laya_decide 参数解析（§9.8 #4，v1.124）：kind ∈ {intent, risk, route} + 非空 text。
fn parse_laya_decide_args(args: &serde_json::Value) -> Option<(tenon_laya::DecideKind, String)> {
    let kind = tenon_laya::DecideKind::parse(args.get("kind")?.as_str()?)?;
    let text = args.get("text")?.as_str()?.trim().to_string();
    if text.is_empty() {
        return None;
    }
    Some((kind, text))
}

/// 子任务状态（§9.2 v1.146）；事件 payload 以 snake_case 序列化。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubtaskStatus {
    Pending,
    InProgress,
    Done,
}

/// 子任务清单条目（§9.2 v1.146）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubtaskItem {
    pub title: String,
    pub status: SubtaskStatus,
}

/// subtasks 参数解析与校验（§9.2 v1.146）：1–12 项的全量清单，title 非空 ≤200 字符。
fn parse_subtasks_args(args: &serde_json::Value) -> Option<Vec<SubtaskItem>> {
    let items = args.get("items")?.as_array()?;
    if items.is_empty() || items.len() > 12 {
        return None;
    }
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let title = item.get("title")?.as_str()?.trim();
        if title.is_empty() || title.chars().count() > 200 {
            return None;
        }
        let status = match item.get("status")?.as_str()? {
            "pending" => SubtaskStatus::Pending,
            "in_progress" => SubtaskStatus::InProgress,
            "done" => SubtaskStatus::Done,
            _ => return None,
        };
        out.push(SubtaskItem {
            title: title.to_string(),
            status,
        });
    }
    Some(out)
}

/// decider_call Trace 的 result 字段（不含输入原文；数值与既有 risk 事件同口径保留两位）。
fn decider_result_summary(value: &tenon_laya::DecideValue) -> serde_json::Value {
    match value {
        tenon_laya::DecideValue::Intent { label, confidence } => serde_json::json!({
            "label": label.as_str(),
            "confidence": (f64::from(*confidence) * 100.0).round() / 100.0,
        }),
        tenon_laya::DecideValue::Risk { score } => {
            serde_json::json!((f64::from(*score) * 100.0).round() / 100.0)
        }
        tenon_laya::DecideValue::Route { suggest_light } => serde_json::json!(suggest_light),
    }
}

/// 历史压缩触发阈值（§10.2 v1.105）：任务内请求输入 token 的估算值或上一回合
/// provider 权威 usage 超过阈值即触发压缩（真实计数由 usage 回填，估算为兜底信号）。
const COMPACTION_INPUT_TOKENS: u64 = 24_000;
/// 压缩时保留原文的最近工具输出条数；更早的替换为存根。
const KEEP_RECENT_TOOL_RESULTS: usize = 4;

/// 任务内请求输入 token 估算（§10.2）：正文 + tool_calls 参数，字符近似。
fn estimate_messages_tokens(messages: &[ChatMessage]) -> u64 {
    messages
        .iter()
        .map(|m| {
            tenon_core::context::estimate_tokens(&m.content)
                + m.tool_calls
                    .iter()
                    .map(|tc| tenon_core::context::estimate_tokens(&tc.arguments.to_string()))
                    .sum::<u64>()
        })
        .sum()
}

/// 历史压缩（§10.2 v1.105）：确定性省略——工具输出是任务内历史的主要膨胀源，
/// 保留最近 `KEEP_RECENT_TOOL_RESULTS` 条原文，更早的 tool 消息替换为存根。
/// 只替换内容、不增删消息，tool_call_id 配对保持完整；首条 user（任务 + L1
/// 工作集）与 assistant 消息原样保留。无可省略条目时返回 None。
fn elide_stale_tool_outputs(messages: &[ChatMessage]) -> Option<(Vec<ChatMessage>, usize)> {
    // 已存根化的消息不算新陈旧项（幂等：重复触发不重写、不重复计数）
    let tool_positions: Vec<usize> = messages
        .iter()
        .enumerate()
        .filter(|(_, m)| m.role == Role::Tool && !m.content.starts_with("[工具输出已省略："))
        .map(|(i, _)| i)
        .collect();
    if tool_positions.len() <= KEEP_RECENT_TOOL_RESULTS {
        return None;
    }
    let stale_count = tool_positions.len() - KEEP_RECENT_TOOL_RESULTS;
    let stale: std::collections::HashSet<usize> =
        tool_positions[..stale_count].iter().copied().collect();
    // 工具名按 call id 反查（登记于前置 assistant 消息的 tool_calls），存根标明来源工具
    let mut names: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
    for m in messages {
        for tc in &m.tool_calls {
            names.insert(tc.id.as_str(), tc.name.as_str());
        }
    }
    let mut out = Vec::with_capacity(messages.len());
    for (i, m) in messages.iter().enumerate() {
        if stale.contains(&i) {
            let id = m.tool_call_id.clone().unwrap_or_default();
            let name = names.get(id.as_str()).copied().unwrap_or("tool");
            out.push(ChatMessage::tool_result(
                id,
                format!(
                    "[工具输出已省略：{name} 原约 {} 字符——需要时重新调用该工具获取]",
                    m.content.chars().count()
                ),
            ));
        } else {
            out.push(m.clone());
        }
    }
    Some((out, stale_count))
}

/// 压缩后确定性自检（§10.2 v1.105）：每条 tool 消息的 call id 都能在前置
/// assistant 的 tool_calls 中找到配对。省略式压缩不产生摘要失真，结构完整性
/// 由本校验兜底（替代原「自检问答」——省一次模型调用）。
fn tool_call_pairs_intact(messages: &[ChatMessage]) -> bool {
    let mut issued: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for m in messages {
        for tc in &m.tool_calls {
            issued.insert(tc.id.as_str());
        }
        if m.role == Role::Tool {
            match m.tool_call_id.as_deref() {
                Some(id) if issued.contains(id) => {}
                _ => return false,
            }
        }
    }
    true
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
        // §13.4 v1.130：技能全局目录与停用名单（停用经 settings 快照，新会话生效）。
        tool_ctx.skills_global_dir = Some(config.skills_global_dir.clone());
        tool_ctx.skills_disabled = config.skills_disabled.clone();
        // §13.5 v1.145：MCP 宿主注入 + 会话创建时 tools/list 快照进工具目录。
        tool_ctx.mcp = config.mcp.clone();
        let mcp_specs = match &config.mcp {
            Some(host) if !host.is_empty() => {
                let for_list = host.clone();
                let tools = tokio::task::spawn_blocking(move || for_list.list_tools())
                    .await
                    .map_err(|e| AgentError::Store(format!("MCP tools/list join 失败: {e}")))?;
                tool_ctx.mcp_policy = host.level_policy_with(&tools);
                // §13.3 v1.187：MCP 元工具（跨服务器 resources / prompts）随桥注入
                let mut specs = build_mcp_specs(&tools);
                specs.extend(mcp_meta_specs().iter().cloned());
                specs
            }
            _ => Vec::new(),
        };
        let (control_tx, control_rx) = mpsc::unbounded_channel();
        let (events_tx, _) = broadcast::channel(1024);
        let circuit_limits = config.circuit;
        let write_scope = config.write_scope.clone();
        let managed_worktree = config.managed_worktree.clone();
        // v2.0 档位派生（design-v2.md §4.1）：read_only → 会话只读开关；
        // full_access → run_tests/run_build/install_deps 沙箱降级（executor 消费）
        let gate = config.gate;
        match gate.exec_mode {
            tenon_core::gates::ExecMode::ReadOnly => {
                tool_ctx.readonly.store(true, Ordering::SeqCst);
            }
            tenon_core::gates::ExecMode::FullAccess => {
                tool_ctx.full_access.store(true, Ordering::SeqCst);
            }
            tenon_core::gates::ExecMode::WorkspaceWrite => {}
        }
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
            force_compact: AtomicBool::new(false),
            touched_files: Mutex::new(BTreeSet::new()),
            pre_rollback_tree: Mutex::new(None),
            interrupt: Notify::new(),
            mcp_specs,
            gate: Mutex::new(gate),
            gate_allowed: Mutex::new(HashSet::new()),
            confirm_pending: Mutex::new(None),
            confirm_tx: Mutex::new(None),
        }))
    }

    /// 切换会话模型（§11 显式路由 / 降级：上下文随迁——消息流不动，
    /// 仅替换 provider，下一回合生效；model_fallback 事件入 Trace）。
    pub async fn switch_provider(&self, new_provider: std::sync::Arc<dyn ModelProvider>) {
        self.switch_provider_origin(new_provider, "manual").await;
    }

    /// v1.171：origin 区分手动切换（/model 端点）与自动 fallback 链（§9.1 自动恢复）。
    async fn switch_provider_origin(
        &self,
        new_provider: std::sync::Arc<dyn ModelProvider>,
        origin: &str,
    ) {
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
                "origin": origin,
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
        let response = Self::aux_chat_resilient(&provider, &request)
            .await
            .map_err(|e| AgentError::Model(e.to_string()))?;
        // 非流式辅助调用（补全 / 标题 / 记忆）不计时：duration_ms = 0 = 未观测。
        self.record_usage(response.usage, 0).await;
        let mut text = response.content.trim().to_string();
        if text.starts_with("```") {
            // 逐级剥离围栏，失败时保留上一步结果（不能回退到含围栏原文，
            // 否则无语言标注的 ``` 围栏会整段原样插入光标处）
            let mut stripped = text.trim_start_matches("```").to_string();
            if let Some(rest) = stripped.strip_prefix(language.trim()) {
                stripped = rest.to_string();
            }
            if let Some(rest) = stripped.strip_prefix('\n') {
                stripped = rest.to_string();
            }
            text = stripped.trim_end_matches("```").trim_end().to_string();
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
        let response = Self::aux_chat_resilient(&provider, &request)
            .await
            .map_err(|e| AgentError::Model(e.to_string()))?;
        self.record_usage(response.usage, 0).await;
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

    /// L5 跨会话记忆提取（§10.1 v1.104）：任务成功完成后单轮无工具调用，
    /// 输入仅用户消息 + 最终回答（不含中间工具输出），严格 JSON 契约、
    /// 每任务 ≤5 条、每条 ≤200 字符；请求带 `MEMORY_MARKER` 供测试替身识别
    /// （同 TITLE_MARKER：不消耗脚本队列）。入库走 store 去重合并（本地
    /// embedding 余弦 ≥0.90 刷新既有条目）与每项目上限治理。任何失败静默
    /// 回退（日志留痕），不阻塞任务结果、不产生 Error 事件。
    async fn extract_memories(&self, user_text: &str, answer: &str) {
        let result = self.extract_memories_inner(user_text, answer).await;
        if let Err(e) = result {
            tracing::debug!("L5 memory extraction skipped: {e}");
        }
    }

    async fn extract_memories_inner(&self, user_text: &str, answer: &str) -> Result<(), String> {
        const MAX_ITEMS: usize = 5;
        const MAX_CONTENT_CHARS: usize = 200;
        let provider = self.provider.read().await.clone();
        let excerpt_text: String = user_text.chars().take(4000).collect();
        let excerpt_answer: String = answer.chars().take(4000).collect();
        let mut request = ChatRequest::new(
            provider.default_model(),
            vec![
                ChatMessage::system(format!(
                    "{MEMORY_MARKER} 你是记忆提取器：从这轮对话中提取值得跨会话记住的稳定信息\
                     （用户偏好 / 项目事实 / 已定决策 / 工作流要点）。规则：\
                     1. 只提取稳定、可复用的信息；一次性任务细节、代码片段、文件内容一律不要；\
                     2. 全局用户偏好 scope=global 且 kind=preference，其余 scope=project；\
                     3. 禁止把文件内容或代码写入记忆；每条 content ≤200 字符；\
                     4. 最多 5 条；没有值得记的就返回空数组；\
                     5. 只输出严格 JSON：\
                     {{\"memories\":[{{\"kind\":\"preference|fact|decision|workflow\",\"scope\":\"project|global\",\"content\":\"…\",\"importance\":1}}]}}，\
                     不要解释、不要 markdown 代码块。"
                )),
                ChatMessage::user(format!(
                    "用户消息：\n{excerpt_text}\n\n最终回答：\n{excerpt_answer}"
                )),
            ],
        );
        request.max_tokens = 512;
        request.temperature = 0.2;
        let response = Self::aux_chat_resilient(&provider, &request)
            .await
            .map_err(|e| e.to_string())?;
        self.record_usage(response.usage, 0).await;

        let parsed: serde_json::Value =
            serde_json::from_str(response.content.trim()).map_err(|e| format!("解析失败: {e}"))?;
        let empty = Vec::new();
        let items = parsed
            .get("memories")
            .and_then(|v| v.as_array())
            .unwrap_or(&empty);
        let mut saved_ids: Vec<String> = Vec::new();
        for item in items.iter().take(MAX_ITEMS) {
            let Some(content) = item.get("content").and_then(|v| v.as_str()) else {
                continue;
            };
            let kind = item
                .get("kind")
                .and_then(|v| v.as_str())
                .unwrap_or("fact")
                .to_string();
            let scope = item
                .get("scope")
                .and_then(|v| v.as_str())
                .unwrap_or("project")
                .to_string();
            // global 层只承载用户偏好（§10.1 v1.104：永不承载仓库内容），
            // 模型输出不合规时收敛为 project 层入库。
            let (scope, kind) = if scope == "global" && kind == "preference" {
                ("global".to_string(), "preference".to_string())
            } else {
                ("project".to_string(), kind)
            };
            let content: String = content.chars().take(MAX_CONTENT_CHARS).collect();
            let importance = item
                .get("importance")
                .and_then(|v| v.as_i64())
                .unwrap_or(3)
                .clamp(1, 5);
            let embedding = tenon_fs::l4::embed("", &content);
            let rec = MemoryRecord {
                scope,
                project_id: self.config.project_id.clone(),
                kind,
                content,
                importance,
                embedding,
                source_session: self.session_id.clone(),
            };
            let mut st = self.store.lock().await;
            match st.upsert_memory(&rec, MEMORY_DEDUPE_THRESHOLD) {
                Ok((mem, _merged)) => saved_ids.push(mem.id),
                Err(e) => return Err(format!("入库失败: {e}")),
            }
        }
        if saved_ids.is_empty() {
            return Ok(());
        }
        // 每项目 active 上限治理（§10.1 v1.104）
        if let Err(e) = self
            .store
            .lock()
            .await
            .prune_memories(&self.config.project_id, MAX_MEMORIES_PER_PROJECT)
        {
            tracing::debug!("L5 memory prune failed: {e}");
        }
        self.emit(
            EventKind::MemorySaved,
            &serde_json::json!({"count": saved_ids.len(), "ids": saved_ids}),
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

    async fn record_usage(&self, usage: Usage, duration_ms: u64) {
        if usage.input_tokens == 0 && usage.output_tokens == 0 {
            return;
        }
        let provider = self.provider.read().await.clone();
        let model = provider.default_model();
        // §11 v1.93：按 provider 配置单价折算（未定价模型计 0，宁少报不虚报），
        // 并作为熔断预算输入（§9.3）——超 token / 超预算在下一工具步检查点熔断。
        // v1.196 §11：缓存命中部分按缓存价计（未配置缓存价 = 输入价，行为不变）。
        let cost = tenon_models::compute_cost(
            &self.config.price_table,
            &model,
            usage.input_tokens,
            usage.output_tokens,
            usage.cached_input_tokens,
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
            usage.cached_input_tokens as i64,
            duration_ms as i64,
            cost,
        );
    }

    /// 流式调用当前模型；权威 usage / tool calls 只取流末尾 Final。
    /// 小增量按 64 字符 / 120ms 合并，避免 SQLite 事件溯源被 token 级写入淹没。
    /// 返回 (响应, 回合耗时毫秒)——耗时自流建立计至权威 Final（v1.129 §11 观测）。
    /// 错误携带 ProviderError 类型（v1.171：瞬时性判定与 Retry-After 供自动恢复）。
    async fn stream_model_turn(
        &self,
        provider: &StdArc<dyn ModelProvider>,
        request: &ChatRequest,
    ) -> Result<(tenon_models::ChatResponse, u64), tenon_models::ProviderError> {
        let started = Instant::now();
        let mut stream = provider.chat_stream(request).await?;
        let mut pending = String::new();
        let mut last_flush = Instant::now();
        let mut final_response = None;
        let mut reasoning_pending = String::new();
        let mut reasoning_flush = Instant::now();
        while let Some(item) = stream.next().await {
            match item? {
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
                // v1.210 §7.2：思考流独立合并推送（不混入正文流）
                ChatStreamEvent::ReasoningDelta(text) => {
                    reasoning_pending.push_str(&text);
                    if reasoning_pending.chars().count() >= 64
                        || reasoning_flush.elapsed() >= Duration::from_millis(120)
                    {
                        self.emit(
                            EventKind::ReasoningDelta,
                            &serde_json::json!({"text": reasoning_pending}),
                        )
                        .await;
                        reasoning_pending.clear();
                        reasoning_flush = Instant::now();
                    }
                }
                ChatStreamEvent::Final(resp) => final_response = Some(resp),
            }
        }
        if !pending.is_empty() {
            self.emit(EventKind::ModelDelta, &serde_json::json!({"text": pending}))
                .await;
        }
        if !reasoning_pending.is_empty() {
            self.emit(
                EventKind::ReasoningDelta,
                &serde_json::json!({"text": reasoning_pending}),
            )
            .await;
        }
        final_response
            .map(|resp| (resp, started.elapsed().as_millis() as u64))
            .ok_or_else(|| tenon_models::ProviderError::Parse("模型流缺少最终响应".into()))
    }

    /// v1.186 §9.1：主 provider 瞬时错误自动重试上限（用户裁定 10 次——GLM
    /// 免费档 429 长时段限流常态下，v1.171 的 ≤2 不足以穿越限流窗口）。
    const MODEL_TRANSIENT_RETRIES: u32 = 10;

    /// v1.171 韧性模型调用（§9.1 自动恢复 / §11 fallback 链）：瞬时错误
    /// （429 / 408 / 5xx / 网络）同 provider 自动重试 ≤10（v1.186 用户裁定
    /// 上限；退避见 `backoff_delay`，429 的 Retry-After 优先、上限 60s），
    /// 非瞬时错误不重试；主 provider 穷尽后按
    /// `config.fallback_providers` 顺序自动切换（上下文随迁、后续回合固定备用，
    /// `model_fallback` origin="auto"），每个备用 ≤1 次瞬时重试；全链穷尽返回
    /// `Exhausted`（调用方维持 ERROR 侧向出口语义）。退避等待可被 interrupt 打断
    /// （Esc / 停止）——返回 `Interrupted`，由调用方按暂停落地，不误报模型失败。
    /// 自动恢复期间不进 ERROR 态（停在当前工作状态）；每次重试发 `model_retry` 事件。
    async fn call_model_resilient(
        &self,
        primary: &StdArc<dyn ModelProvider>,
        mut request: ChatRequest,
    ) -> Result<(tenon_models::ChatResponse, u64, ChatRequest), ModelTurnError> {
        // (provider, 瞬时重试余量)：主 provider ≤10（v1.186），每个备用 ≤1
        let mut chain: Vec<(StdArc<dyn ModelProvider>, u32)> =
            vec![(primary.clone(), Self::MODEL_TRANSIENT_RETRIES)];
        for fallback in &self.config.fallback_providers {
            chain.push((fallback.clone(), 1));
        }
        let mut last_err: Option<tenon_models::ProviderError> = None;
        for (chain_idx, (provider, retries)) in chain.into_iter().enumerate() {
            if chain_idx > 0 {
                tracing::info!(
                    "模型自动 fallback（§9.1）：{} → {}",
                    last_err.as_ref().map(|e| e.to_string()).unwrap_or_default(),
                    provider.default_model()
                );
                self.switch_provider_origin(provider.clone(), "auto").await;
            }
            request.model = provider.default_model();
            let mut attempt: u32 = 0;
            loop {
                match self.stream_model_turn(&provider, &request).await {
                    Ok((resp, ms)) => return Ok((resp, ms, request)),
                    Err(e) => {
                        attempt += 1;
                        if !e.is_transient() || attempt > retries {
                            last_err = Some(e);
                            break; // 换下一个 provider（或穷尽）
                        }
                        let delay = Self::backoff_delay(&e, attempt);
                        self.emit(
                            EventKind::ModelRetry,
                            &serde_json::json!({
                                "provider": provider.name(),
                                "model": request.model,
                                "attempt": attempt,
                                "delay_ms": delay.as_millis() as u64,
                                "error": e.to_string(),
                            }),
                        )
                        .await;
                        if self.interruptible_sleep(delay).await {
                            return Err(ModelTurnError::Interrupted);
                        }
                    }
                }
            }
        }
        Err(ModelTurnError::Exhausted(last_err.unwrap_or_else(|| {
            tenon_models::ProviderError::Config("无可用模型（fallback 链为空）".into())
        })))
    }

    /// 退避时长（§9.1 v1.171/v1.186）：429 Retry-After 优先（上限 60s），否则
    /// 指数退避 2s→4s→8s→16s、封顶 30s（重试上限 10 次，无 Retry-After 全程约
    /// 3.5 分钟）。备用链 / 辅助调用首试（attempt=1）仍为 2s，语义不变。
    fn backoff_delay(err: &tenon_models::ProviderError, attempt: u32) -> Duration {
        const RETRY_AFTER_CAP: Duration = Duration::from_secs(60);
        let base = match attempt {
            0 | 1 => Duration::from_secs(2),
            2 => Duration::from_secs(4),
            3 => Duration::from_secs(8),
            4 => Duration::from_secs(16),
            _ => Duration::from_secs(30),
        };
        err.retry_after().map_or(base, |d| d.min(RETRY_AFTER_CAP))
    }

    /// 可打断退避：true = 期间收到 interrupt（Esc / 停止）。
    async fn interruptible_sleep(&self, delay: Duration) -> bool {
        tokio::select! {
            _ = tokio::time::sleep(delay) => false,
            _ = self.interrupt.notified() => true,
        }
    }

    /// v1.171 辅助单轮调用韧性（§11）：瞬时错误重试 ≤1（2s 退避，429 尊重
    /// Retry-After 上限 60s），仍失败返回末次错误——调用方维持既有静默回退
    /// 语义（标题回退本地截断 / 提取静默跳过 / 补全 UI 静默）。
    async fn aux_chat_resilient(
        provider: &StdArc<dyn ModelProvider>,
        request: &ChatRequest,
    ) -> Result<tenon_models::ChatResponse, tenon_models::ProviderError> {
        match provider.chat(request).await {
            Ok(resp) => Ok(resp),
            Err(e) if e.is_transient() => {
                let delay = Self::backoff_delay(&e, 1);
                tokio::time::sleep(delay).await;
                provider.chat(request).await
            }
            Err(e) => Err(e),
        }
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
    /// 兼容入口：无图片任务（全库既有调用零改动）。
    pub async fn run_task(&self, user_text: &str) -> TaskOutcome {
        self.run_task_guarded(user_text, Vec::new()).await
    }

    /// v1.191 §11 多模态任务入口：user 消息携带图片（base64 内联，
    /// 历史重发由供应商侧 prompt caching 吸收——v1.173）。
    pub async fn run_task_with_images(
        &self,
        user_text: &str,
        images: Vec<tenon_models::ImagePart>,
    ) -> TaskOutcome {
        self.run_task_guarded(user_text, images).await
    }

    async fn run_task_guarded(
        &self,
        user_text: &str,
        task_images: Vec<tenon_models::ImagePart>,
    ) -> TaskOutcome {
        if self.running.swap(true, Ordering::SeqCst) {
            return TaskOutcome::Error(
                "任务进行中（暂停 = 挂起待恢复）：请先停止或等待完成".into(),
            );
        }
        let outcome = self.run_task_inner(user_text, task_images).await;
        // 任务收尾后排空残留控制命令（v1.166）：Stop/Pause 发出时任务可能恰好
        // 在最后一段无检查点的流式回答/验证中收尾，命令滞留通道会让下一个任务
        // 的首个检查点误暂停/误停（SetReadonly 延一拍生效同理）——它们指向的是
        // 已结束的任务。任务启动前不能排空：空闲期预发的 Pause 要在下一任务
        // 首个检查点生效（既有语义，测试钉死）。
        while self.drain_control().await.is_some() {}
        // L5 记忆提取（§10.1 v1.104）：任务成功完成后单轮提取；失败静默回退，
        // 不改变任务结果、不阻塞返回。提取调用照常经 record_usage 入成本归因。
        if let TaskOutcome::Done(card) = &outcome {
            if self.config.memories_enabled {
                self.extract_memories(user_text, &card.answer).await;
            }
        }
        self.running.store(false, Ordering::SeqCst);
        outcome
    }

    /// v1.147（§9.1）：任务执行中（含暂停真挂起等待恢复）——发送消息队列的入队判定
    /// 信号；与 v1.93 重入守卫同源（running 原子标志），daemon 侧另有 SessionEntry::busy
    /// 在 sessions 锁内同步判定，此处为底层兜底视图。
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    async fn run_task_inner(
        &self,
        user_text: &str,
        task_images: Vec<tenon_models::ImagePart>,
    ) -> TaskOutcome {
        // §9.7 v1.87 并行写锁：按 (project_id, worktree_scope) 计——主根会话互斥，
        // 不同受管 worktree 会话可与主根及彼此并行；全局配额由 daemon 控制。
        let scope_lock = self.write_lock.lock_for(&self.write_scope).await;
        let _guard = scope_lock.lock().await;
        self.first_edit_done.store(false, Ordering::SeqCst);
        self.touched_files.lock().await.clear();
        {
            let mut mem = self.memory.lock().await;
            mem.goals.push(user_text.to_string());
            if mem.goals.len() > MAX_L2_GOALS {
                // L2 有界（§10.2 v1.105）：目标只保留最近窗口，系统提示不随会话膨胀
                *mem = mem.compact();
            }
        }

        // ---- IDLE → SENSING ----
        self.emit(
            EventKind::UserInput,
            &serde_json::json!({"text": user_text, "images": task_images.len()}),
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
        // L5 跨会话记忆注入（§10.1 v1.104）：项目层 + global preference 层，
        // importance × 新鲜度排序、条数 / token 预算在 render_memories 内裁剪；
        // 记忆为参考数据非指令（§12.1 不可信数据），检索失败静默回退空集。
        let memory_items = if self.config.memories_enabled {
            match self
                .store
                .lock()
                .await
                .list_memories(&self.config.project_id, None, 16)
            {
                Ok(rows) => rows
                    .into_iter()
                    .map(|m| tenon_core::prompt::MemoryItem {
                        kind: m.kind,
                        scope: m.scope,
                        content: m.content,
                        importance: m.importance,
                    })
                    .collect::<Vec<_>>(),
                Err(e) => {
                    tracing::debug!("L5 memory recall unavailable: {e}");
                    Vec::new()
                }
            }
        } else {
            Vec::new()
        };
        // §13.4 v1.130：可用技能目录注入（渐进披露——目录只含名称与描述，
        // 正文经 skill_use 按需读取；每任务扫描一次，改文件即时生效；
        // 扫描跟随会话工作根（主根 / 受管 worktree），失败静默回退空集）。
        let skill_entries = tenon_core::skills::scan_skills(
            &self.config.skills_global_dir,
            self.managed_worktree.as_deref(),
            &self.config.skills_disabled,
        );
        let mut messages: Vec<ChatMessage> = vec![
            ChatMessage::system(tenon_core::prompt::build_system_prompt(
                &self.rules.lock().await.clone(),
                &self.memory.lock().await.clone(),
                &memory_items,
                &skill_entries,
            )),
            ChatMessage::user_with_images(user_message, task_images.clone()),
        ];

        let mut changed_files: Vec<String> = Vec::new();
        let mut last_pre_tree: Option<String> = None;
        let mut final_answer: Option<String> = None;
        let mut steps = 0u32;
        let mut verification = String::new();
        let mut verification_strength = "none";

        let mut paused_reason: Option<String> = None;
        let mut error_msg: Option<String> = None;
        // §9.2 v1.179 计划模式：本回合提交过计划（工具循环结束后暂停待批准）
        let mut plan_pause = false;
        // §9.1 v1.53：截断续跑——截断的中间输出不是回答，连续多次才按模型失败处理
        let mut consecutive_truncations = 0u32;
        // 上一回合 provider 权威输入 token（§10.2 v1.105 压缩触发信号之一）
        let mut last_input_tokens: u64 = 0;

        // §13.6 v1.180 pre_turn hooks（任务开始前；失败仅记 Trace）
        self.run_turn_hooks(crate::hooks::HookEvent::PreTurn).await;

        // ---- 模型回合循环（SENSING / DECIDING / EXECUTING 在回合内展开）----
        'rounds: for _round in 0..self.config.max_tool_rounds {
            self.force_state(State::Deciding).await;
            self.set_status(SessionStatus::Deciding).await;

            // ---- 历史压缩（§10.2 v1.105）：输入预算超限即省略陈旧工具输出。
            // v1.161 手动压缩：ControlCommand::Compact 置位后跳过阈值强制执行一次（标志即消费）。
            // 压缩后自检配对完整性，失败则本回合放弃压缩（保持原历史）----
            let est_tokens = estimate_messages_tokens(&messages);
            let manual_compact = self.force_compact.swap(false, Ordering::SeqCst);
            if manual_compact
                || est_tokens > COMPACTION_INPUT_TOKENS
                || last_input_tokens > COMPACTION_INPUT_TOKENS
            {
                if let Some((compacted, elided)) = elide_stale_tool_outputs(&messages) {
                    if tool_call_pairs_intact(&compacted) {
                        let after_tokens = estimate_messages_tokens(&compacted);
                        messages = compacted;
                        self.emit(
                            EventKind::Compaction,
                            &serde_json::json!({
                                "round": _round,
                                "before_est_tokens": est_tokens,
                                "after_est_tokens": after_tokens,
                                "elided_tool_results": elided,
                                "manual": manual_compact,
                            }),
                        )
                        .await;
                    } else {
                        tracing::error!("历史压缩自检失败：tool_call_id 配对破损，本回合跳过压缩");
                    }
                } else if manual_compact {
                    // 手动压缩无可省略对象（无陈旧工具输出）——照发事件给 UI 反馈，不静默。
                    self.emit(
                        EventKind::Compaction,
                        &serde_json::json!({
                            "round": _round,
                            "before_est_tokens": est_tokens,
                            "after_est_tokens": est_tokens,
                            "elided_tool_results": 0,
                            "manual": true,
                        }),
                    )
                    .await;
                }
            }

            // 只读先验：首轮仅开放 A 级工具（§9.8 预筛语义，只收窄不放宽）；
            // 模型判断确需改动 → 后续回合恢复全目录（含 MCP 工具，§13.5）
            let tools = if read_only_prior && _round == 0 {
                tool_specs_read_only()
            } else {
                let mut all = tool_specs();
                all.extend(self.mcp_specs.iter().cloned());
                all
            };
            let provider = self.provider.read().await.clone();
            let mut request = ChatRequest {
                model: provider.default_model(),
                messages: messages.clone(),
                tools,
                max_tokens: self.config.generation_max_tokens,
                temperature: self.config.generation_temperature,
                reasoning_effort: None,
            };
            // v1.171 §9.1 自动恢复：瞬时重试 + 自动 fallback 链在 call_model_resilient
            // 内完成（不进 ERROR 态）；穷尽才落 ERROR 侧向出口（用户重试 / 切模型 /
            // 中止语义不变）。返回的 request 已按实际生效 provider 校正 model，
            // 决策卡如实标注（v1.131 语义）。
            //
            // v1.191 §10.2 溢出恢复（DSH 确定借鉴项落地）：穷尽且末次错误为上下文
            // 超限时，同一回合内强制省略一次陈旧工具输出再重试一次（压缩事件
            // overflow:true）；再失败照常落 ERROR。压缩不可行（无可省略对象 /
            // 配对自检失败）不重试——单回合至多恢复一次，防循环。
            let mut overflow_recovered = false;
            let (resp, turn_duration_ms, request) = 'overflow_recover: loop {
                match self.call_model_resilient(&provider, request.clone()).await {
                    Ok((r, ms, req)) => break (r, ms, req),
                    Err(ModelTurnError::Interrupted) => {
                        // 退避等待被打断（Esc / 停止）：按暂停落地，不误报模型失败
                        self.force_state(State::Paused).await;
                        self.set_status(SessionStatus::Paused).await;
                        paused_reason = Some("模型重试等待被打断（Esc / 停止）".into());
                        break 'rounds;
                    }
                    Err(ModelTurnError::Exhausted(e)) => {
                        if !overflow_recovered && e.is_context_overflow() {
                            let before = estimate_messages_tokens(&messages);
                            if let Some((compacted, elided)) = elide_stale_tool_outputs(&messages) {
                                if tool_call_pairs_intact(&compacted) {
                                    overflow_recovered = true;
                                    messages = compacted;
                                    request.messages = messages.clone();
                                    self.emit(
                                        EventKind::Compaction,
                                        &serde_json::json!({
                                            "round": _round,
                                            "before_est_tokens": before,
                                            "after_est_tokens": estimate_messages_tokens(&messages),
                                            "elided_tool_results": elided,
                                            "manual": false,
                                            "overflow": true,
                                        }),
                                    )
                                    .await;
                                    continue 'overflow_recover;
                                }
                            }
                        }
                        // 侧向出口：自动恢复穷尽 → ERROR
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
                }
            };
            last_input_tokens = resp.usage.input_tokens;
            self.record_usage(resp.usage, turn_duration_ms).await;
            steps += 1;

            // 决策意图卡（payload.usage 供 UI 回合页脚缓存命中率 / 输出速度聚合，v1.129）
            // payload.model 供 UI 气泡下回合模型标注（v1.131：热切换 / fallback 后各回合如实标注）
            self.emit(
                EventKind::Decision,
                &serde_json::json!({
                    "intent": resp.content,
                    "model": request.model,
                    "tool_calls": resp.tool_calls.len(),
                    "usage": {
                        "input_tokens": resp.usage.input_tokens,
                        "output_tokens": resp.usage.output_tokens,
                        "cached_input_tokens": resp.usage.cached_input_tokens,
                        "duration_ms": turn_duration_ms,
                    },
                }),
            )
            .await;

            // v1.53：截断的回复（OpenAI finish_reason=length / Anthropic
            // stop_reason=max_tokens，各 provider 原样透传）没有工具调用不等于
            // 任务完成——推送已输出的部分并要求续写，避免长规划被 max_tokens
            // 剪断后静默 Done。
            if resp.tool_calls.is_empty()
                && matches!(
                    resp.finish_reason.as_deref(),
                    Some("length") | Some("max_tokens")
                )
            {
                consecutive_truncations += 1;
                if consecutive_truncations >= 3 {
                    self.force_state(State::Error).await;
                    self.set_status(SessionStatus::Error).await;
                    self.emit(
                        EventKind::Error,
                        &serde_json::json!({"error": "模型输出连续 3 次被截断（length/max_tokens）"}),
                    )
                    .await;
                    error_msg = Some("模型输出连续截断（length/max_tokens）".into());
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
                self.run_turn_hooks(crate::hooks::HookEvent::PostTurn).await;
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
                                    Some(ControlCommand::Compact) => {
                                        self.force_compact.store(true, Ordering::SeqCst);
                                    }
                                    Some(ControlCommand::SetApproval(gear)) => {
                                        self.gate.lock().await.approval = gear;
                                    }
                                    Some(ControlCommand::SetExecMode(mode)) => {
                                        self.apply_exec_mode(mode).await;
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
                        // v1.161 手动压缩：置位标志，下一模型回合跳过阈值强制省略陈旧工具输出
                        ControlCommand::Compact => {
                            self.force_compact.store(true, Ordering::SeqCst);
                        }
                        // v2.0 运行中切换确认档（design-v2.md §4.1）：下一工具步生效
                        ControlCommand::SetApproval(gear) => {
                            self.gate.lock().await.approval = gear;
                        }
                        // v2.0 运行中切换执行边界档：只读 / 命令沙箱降级即时派生
                        ControlCommand::SetExecMode(mode) => {
                            self.apply_exec_mode(mode).await;
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
                // §13.5 v1.145：MCP 工具分级查策略（默认 D，net:* 服务器工具 → C），
                // 不落入未知工具的 C 兜底（§13.3：外部插件永不静默升 A/B）。
                let level = if call.name.starts_with("mcp_") {
                    match self.tool_ctx.mcp_policy.level_for(&call.name) {
                        tenon_mcp::McpToolLevel::C => Level::C,
                        tenon_mcp::McpToolLevel::D => Level::D,
                    }
                } else {
                    tool.and_then(|t| t.level()).unwrap_or(Level::C)
                };

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

                // ---- v2.0 档位闸门（design-v2.md §4.1）：Approval 档 Hold ----
                // ExecMode read_only 已由会话 readonly 开关承载（上游 Decision 拒绝）；
                // allow_session 记忆优先；verdict 先行求值释放锁再进等待。
                let gate_verdict = {
                    let gate = self.gate.lock().await;
                    let allowed = self.gate_allowed.lock().await.contains(&call.name);
                    gate.judge(level, allowed)
                };
                if gate_verdict == GateVerdict::Hold {
                    match self
                        .hold_for_confirm(&call.name, level, &call.arguments)
                        .await
                    {
                        ConfirmDecision::AllowSession => {
                            self.gate_allowed.lock().await.insert(call.name.clone());
                        }
                        ConfirmDecision::AllowOnce => {}
                        ConfirmDecision::Deny => {
                            tool_messages.push(ChatMessage::tool_result(
                                call.id.clone(),
                                format!(
                                    "用户拒绝执行 {}（v2.0 审批档：不可逆动作需确认）——请改用其他方式完成，或向用户说明后等待指示",
                                    call.name
                                ),
                            ));
                            continue;
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
                            .tool_ctx
                            .root
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
                // §9.8 #4（v1.124）：laya_decide 走会话循环内联分发（LayaRuntime
                // 异步推理），不经 execute_tool 同步面；团队策略黑名单在
                // exec_laya_decide 内同轨检查。
                // §13.6 v1.180 pre_tool hooks：block 即拒绝（stderr 回模型），
                // 不执行不记 CommandRun（hook_run 事件已记 block）。
                if !self.config.hooks.is_empty() {
                    let hook_outcome = crate::hooks::run_hooks(
                        &self.config.hooks,
                        crate::hooks::HookEvent::PreTool,
                        &crate::hooks::HookContext {
                            session_id: &self.session_id,
                            root: &self.tool_ctx.root,
                            tool: Some(&call.name),
                            tool_args: Some(&call.arguments.to_string()),
                            tool_output: None,
                        },
                    )
                    .await;
                    self.emit_hook_run(&hook_outcome).await;
                    if hook_outcome.blocked {
                        tool_messages.push(ChatMessage::tool_result(
                            call.id.clone(),
                            format!(
                                "被 pre_tool hook 阻断: {}",
                                hook_outcome.block_reason.unwrap_or_default()
                            ),
                        ));
                        continue;
                    }
                }
                // §9.2（v1.146）：subtasks 子任务清单同走内联分发（会话内计划
                // 状态，零工作区副作用），边界与 Trace 在 exec_subtasks 内同轨。
                // §9.2（v1.179）：submit_plan 计划提交同轨（零副作用），提交过
                // 即暂停待批准（plan_pause 在本回合工具循环结束后生效）。
                let mut output = if call.name == "laya_decide" {
                    self.exec_laya_decide(&call.arguments).await
                } else if call.name == "subtasks" {
                    self.exec_subtasks(&call.arguments).await
                } else if call.name == "submit_plan" {
                    let out = self.exec_submit_plan(&call.arguments).await;
                    if out.ok {
                        plan_pause = true;
                    }
                    out
                } else if call.name == "spawn_subagents" {
                    self.exec_spawn_subagents(&call.arguments).await
                } else {
                    execute_tool(&self.tool_ctx, &call.name, &call.arguments)
                };
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
                // §13.6 v1.180 post_tool hooks（工具执行后；失败仅记 Trace）
                if !self.config.hooks.is_empty() {
                    let hook_outcome = crate::hooks::run_hooks(
                        &self.config.hooks,
                        crate::hooks::HookEvent::PostTool,
                        &crate::hooks::HookContext {
                            session_id: &self.session_id,
                            root: &self.tool_ctx.root,
                            tool: Some(&call.name),
                            tool_args: None,
                            tool_output: Some(&output.content),
                        },
                    )
                    .await;
                    self.emit_hook_run(&hook_outcome).await;
                }
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
                    // 熔断行数按本回合真实 diff（新增+删除）记账：按文件总行数
                    // 累计会 把「重复编辑同一大文件」误记为巨额改动而提前熔断
                    let lines = output.lines_changed.unwrap_or(0);
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

            // §9.2 v1.179 计划模式：计划提交成功 → 会话转 PAUSED 等待用户批准
            // （批准 = 发送新回合「按计划执行」，复用 v1.147 发送链路零新控制命令）。
            // 在工具结果入上下文后暂停——批准后的新回合带完整计划上下文续跑。
            if plan_pause {
                self.force_state(State::Paused).await;
                self.set_status(SessionStatus::Paused).await;
                paused_reason =
                    Some("计划已提交，等待批准（批准 = 发送消息「按计划执行」）".into());
                break 'rounds;
            }
        }

        // ---- 回合上限耗尽：无收尾回答 ≠ 任务完成 ----
        // 模型 24 轮持续只发工具调用时，此前静默走「Done + 空答案 + 发送队列
        // 继续出队」；应转 Paused 交用户处置（续跑 / 停止）
        if paused_reason.is_none() && error_msg.is_none() && final_answer.is_none() {
            self.force_state(State::Paused).await;
            self.set_status(SessionStatus::Paused).await;
            paused_reason = Some(format!(
                "达到回合上限（{} 轮）仍未产出收尾回答；可再发消息续跑或停止",
                self.config.max_tool_rounds
            ));
            self.emit(
                EventKind::Error,
                &serde_json::json!({"rounds_exhausted": self.config.max_tool_rounds}),
            )
            .await;
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
        self.run_turn_hooks(crate::hooks::HookEvent::PostTurn).await;
        if let Some(reason) = paused_reason {
            return TaskOutcome::Paused {
                state: self.current_state().await.to_string(),
                reason,
            };
        }
        self.run_turn_hooks(crate::hooks::HookEvent::PostTurn).await;
        if let Some(mut err) = error_msg {
            // 失败语义：回滚到最近写前快照（§10.3 崩溃恢复：EXECUTING 中失败不保留半成品）
            if let Some(pre) = last_pre_tree {
                match self.snapshots.restore(&pre) {
                    Ok(()) => {
                        self.emit(EventKind::Rollback, &serde_json::json!({"to": pre}))
                            .await;
                    }
                    Err(e) => {
                        // restore 失败时半成品留在工作区：不得照发 Rollback 造成
                        // Trace 与磁盘不一致，显式上报回滚失败（不变式 §10.3）
                        tracing::error!("失败收尾回滚失败（快照点 {pre}）: {e}");
                        self.emit(
                            EventKind::Error,
                            &serde_json::json!({
                                "rollback_failed": e.to_string(),
                                "intended_tree": pre,
                            }),
                        )
                        .await;
                        err = format!("{err}（自动回滚失败：{e}；快照点 {pre}）");
                    }
                }
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
        self.run_turn_hooks(crate::hooks::HookEvent::PostTurn).await;
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

    /// §9.8 #4（v1.124）：agent 可调用判定工具 laya_decide——A 级只读、本地
    /// CPU 推理；判定仅作排序 / 预筛 / 提示参考，并入 decider_call Trace
    ///（origin = agent_tool，类型 / 结果 / 耗时，不含输入原文）。团队策略
    /// 黑名单与全工具目录同轨检查；未启用 / 未下载 / 超时按「工具暂不可用」
    /// 返回，模型回退自行判断。
    async fn exec_laya_decide(&self, args: &serde_json::Value) -> ToolOutput {
        if self
            .tool_ctx
            .team_denied_tools
            .iter()
            .any(|t| t == "laya_decide")
        {
            return ToolOutput::err("团队策略禁用工具: laya_decide（只收窄，§19/§12.2）");
        }
        let Some(laya) = &self.config.laya else {
            return ToolOutput::err("laya_decide 不可用：Laya 未启用");
        };
        let Some((kind, text)) = parse_laya_decide_args(args) else {
            return ToolOutput::err("参数无效：需要 kind（intent | risk | route）与非空 text");
        };
        match laya.decide(kind, text).await {
            tenon_laya::LayaOutcome::Success {
                value, duration_ms, ..
            } => {
                self.emit(
                    EventKind::DeciderCall,
                    &serde_json::json!({
                        "feature": "agent_tool",
                        "kind": value.kind_str(),
                        "result": decider_result_summary(&value),
                        "duration_ms": duration_ms,
                        "origin": "agent_tool",
                    }),
                )
                .await;
                ToolOutput::ok(serde_json::to_string(&value).unwrap_or_default())
            }
            tenon_laya::LayaOutcome::Disabled => {
                ToolOutput::err("laya_decide 已关闭（models.laya.features 未含 agent_tool）")
            }
            tenon_laya::LayaOutcome::Unavailable(r) => {
                ToolOutput::err(format!("laya_decide 暂不可用：{r}（可自行判断）"))
            }
            tenon_laya::LayaOutcome::TimedOut => {
                ToolOutput::err("laya_decide 推理超时（已回退），可自行判断")
            }
        }
    }

    /// §9.2（v1.146）：子任务清单 subtasks——A 级零工作区副作用（只写会话内
    /// 计划状态），`{items:[{title,status}]}` 全量状态替换（幂等）；校验失败
    /// 返回错误提示、不改现有状态。每次调用先落 `subtasks` 事件（payload
    /// `{items}` 全量快照），再由常规路径落 command_run 与 tool_calls Trace；
    /// 团队策略黑名单与全工具目录同轨检查。
    /// §9.2（v1.179 计划模式）：submit_plan——A 级零工作区副作用。全量校验
    /// （1–12 项、每项 trim 非空 ≤200 字符，失败返回错误提示、模型可重试不
    /// 暂停）；校验过发 `plan_submitted` 事件，调用方置 plan_pause 在工具循环
    /// 结束后转 PAUSED。团队策略 denied_tools 与全目录同轨。
    /// §13.6 v1.180：turn 级 hook（pre/post_turn）执行并记 Trace。
    async fn run_turn_hooks(&self, event: crate::hooks::HookEvent) {
        if self.config.hooks.is_empty() {
            return;
        }
        let outcome = crate::hooks::run_hooks(
            &self.config.hooks,
            event,
            &crate::hooks::HookContext {
                session_id: &self.session_id,
                root: &self.tool_ctx.root,
                tool: None,
                tool_args: None,
                tool_output: None,
            },
        )
        .await;
        self.emit_hook_run(&outcome).await;
    }

    /// §13.6 v1.180：hook_run 事件入 Trace（结果序列化，不含参数原文）。
    async fn emit_hook_run(&self, outcome: &crate::hooks::HookOutcome) {
        for r in &outcome.results {
            self.emit(
                EventKind::HookRun,
                &serde_json::to_value(r).unwrap_or_default(),
            )
            .await;
        }
    }

    /// §9.5（v1.190）：spawn_subagents——并行子代理分发。校验（1-3 项、
    /// instruction 1-4000 字符、files 1-8 条相对路径）→ 委托编排器
    /// （daemon 实现：worktree 池 + 子会话登记 + 批内并发）。B 级：只读
    /// 会话在级别闸门已拒；denied_tools 与全目录同轨（tools.rs 注册）。
    async fn exec_spawn_subagents(&self, args: &serde_json::Value) -> ToolOutput {
        let Some(orchestrator) = &self.config.subagents else {
            return ToolOutput::err("子代理编排未接入（daemon 未注入）");
        };
        let Some(tasks) = args.get("tasks").and_then(|v| v.as_array()) else {
            return ToolOutput::err("缺少 tasks 参数（数组，1-3 项）");
        };
        if tasks.is_empty() || tasks.len() > 3 {
            return ToolOutput::err("tasks 须为 1-3 项（§9.5 并发 ≤3）");
        }
        let mut spawns: Vec<crate::subagents::SubagentSpawn> = Vec::with_capacity(tasks.len());
        for (i, t) in tasks.iter().enumerate() {
            let Some(instruction) = t.get("instruction").and_then(|v| v.as_str()) else {
                return ToolOutput::err(format!("tasks[{i}].instruction 缺失"));
            };
            let instruction = instruction.trim();
            if instruction.is_empty() || instruction.chars().count() > 4000 {
                return ToolOutput::err(format!("tasks[{i}].instruction 须为 1-4000 字符"));
            }
            let Some(files) = t.get("files").and_then(|v| v.as_array()) else {
                return ToolOutput::err(format!("tasks[{i}].files 缺失（计划触碰文件集）"));
            };
            if files.is_empty() || files.len() > 8 {
                return ToolOutput::err(format!("tasks[{i}].files 须为 1-8 条"));
            }
            let mut set = Vec::with_capacity(files.len());
            for f in files {
                let Some(f) = f.as_str() else {
                    return ToolOutput::err(format!("tasks[{i}].files 元素须为字符串"));
                };
                let f = f.trim();
                if f.is_empty() || f.chars().count() > 256 {
                    return ToolOutput::err(format!(
                        "tasks[{i}].files 元素须为 1-256 字符相对路径"
                    ));
                }
                set.push(f.to_string());
            }
            spawns.push(crate::subagents::SubagentSpawn {
                instruction: instruction.to_string(),
                files: set,
            });
        }
        let results = match orchestrator.run_batch(&self.session_id, spawns).await {
            Ok(r) => r,
            Err(e) => return ToolOutput::err(format!("子代理编排失败: {e}")),
        };
        if results.is_empty() {
            return ToolOutput::err("所有子任务因文件集相交被拒绝——请合并任务或缩小文件集");
        }
        let mut lines = Vec::with_capacity(results.len() + 1);
        for r in &results {
            let excerpt: String = r.answer.chars().take(400).collect();
            let verification = if r.verification.is_empty() {
                String::new()
            } else {
                format!("\n  验收: {}", r.verification)
            };
            lines.push(format!(
                "[{}] {} {}\n  worktree: {}{}\n  {}",
                r.status, r.task_id, r.session_id, r.worktree, verification, excerpt
            ));
        }
        lines.push(
            "子代理各自运行于独立受管 worktree 并已登记为独立会话——合并 / 丢弃在子会话行处置。"
                .to_string(),
        );
        ToolOutput {
            ok: true,
            content: format!(
                "子代理批次完成（{} 项）\n{}",
                results.len(),
                lines.join("\n")
            ),
            changed_files: vec![],
            lines_changed: None,
            exit_code: None,
            dirty_conflict: None,
            dirty_merged: None,
        }
    }

    async fn exec_submit_plan(&self, args: &serde_json::Value) -> ToolOutput {
        if self
            .tool_ctx
            .team_denied_tools
            .iter()
            .any(|t| t == "submit_plan")
        {
            return ToolOutput::err("团队策略禁用工具: submit_plan（只收窄，§19/§12.2）");
        }
        let Some(items) = args.get("items").and_then(|v| v.as_array()) else {
            return ToolOutput::err("缺少 items 参数（字符串数组，1-12 项）");
        };
        if items.is_empty() || items.len() > 12 {
            return ToolOutput::err("items 须为 1-12 项");
        }
        let mut plan: Vec<String> = Vec::with_capacity(items.len());
        for item in items {
            let Some(s) = item.as_str() else {
                return ToolOutput::err("items 元素须为字符串");
            };
            let s = s.trim();
            if s.is_empty() || s.chars().count() > 200 {
                return ToolOutput::err("每项计划须非空且 ≤200 字符");
            }
            plan.push(s.to_string());
        }
        self.emit(
            EventKind::PlanSubmitted,
            &serde_json::json!({ "items": plan }),
        )
        .await;
        ToolOutput::ok(format!(
            "计划已提交（{} 项），已暂停等待用户批准；批准后按计划执行",
            plan.len()
        ))
    }

    async fn exec_subtasks(&self, args: &serde_json::Value) -> ToolOutput {
        if self
            .tool_ctx
            .team_denied_tools
            .iter()
            .any(|t| t == "subtasks")
        {
            return ToolOutput::err("团队策略禁用工具: subtasks（只收窄，§19/§12.2）");
        }
        let Some(items) = parse_subtasks_args(args) else {
            return ToolOutput::err(
                "参数无效：items 需为 1–12 项的整张清单 [{title(非空≤200字), status: pending|in_progress|done}]",
            );
        };
        self.emit(EventKind::Subtasks, &serde_json::json!({ "items": items }))
            .await;
        let done = items
            .iter()
            .filter(|i| i.status == SubtaskStatus::Done)
            .count();
        ToolOutput::ok(format!(
            "子任务清单已更新：{}/{} 完成。开始或完成一项时重发整张清单；任务收尾前所有项须为 done",
            done,
            items.len()
        ))
    }

    async fn drain_control(&self) -> Option<ControlCommand> {
        let mut rx = self.control_rx.lock().await;
        rx.try_recv().ok()
    }

    /// 首改缓冲等待：true = 被 Esc 打断。
    /// v2.0 档位确认等待（design-v2.md §4.1）：被 Hold 的动作在此等待用户
    /// 决议（`POST /session/:id/confirm`）。Esc / Pause / Stop 打断按拒绝
    /// 处理——Pause / Stop 回注控制通道，由下一工具步检查点执行暂停语义；
    /// 等待期间收到 `SetApproval` 即时改档重判（改为 never 或记忆命中即放行）。
    async fn hold_for_confirm(
        &self,
        tool: &str,
        level: Level,
        args: &serde_json::Value,
    ) -> ConfirmDecision {
        // args 截 2k 防事件膨胀（§14.2 payload 治理同口径）
        let args_text = {
            let s = args.to_string();
            if s.chars().count() > 2000 {
                format!("{}…", s.chars().take(2000).collect::<String>())
            } else {
                s
            }
        };
        self.emit(
            EventKind::ConfirmRequest,
            &serde_json::json!({
                "tool": tool,
                "level": level.as_str(),
                "args": args_text,
            }),
        )
        .await;
        self.set_status(SessionStatus::AwaitingConfirm).await;
        *self.confirm_pending.lock().await = Some(PendingConfirm {
            tool: tool.to_string(),
            level: level.as_str().to_string(),
            args: args_text,
        });
        let (tx, mut rx) = tokio::sync::oneshot::channel::<ConfirmDecision>();
        *self.confirm_tx.lock().await = Some(tx);
        let interrupt = self.interrupt.notified();
        tokio::pin!(interrupt);
        let mut requeue: Option<ControlCommand> = None;
        let decision = loop {
            tokio::select! {
                d = &mut rx => match d {
                    Ok(d) => break d,
                    Err(_) => break ConfirmDecision::Deny,
                },
                _ = &mut interrupt => break ConfirmDecision::Deny,
                _ = tokio::time::sleep(Duration::from_millis(150)) => {
                    match self.drain_control().await {
                        Some(cmd @ (ControlCommand::Pause | ControlCommand::Stop)) => {
                            requeue = Some(cmd);
                            break ConfirmDecision::Deny;
                        }
                        Some(ControlCommand::SetApproval(gear)) => {
                            // 改档即时重判：不再需要确认则直接放行
                            self.gate.lock().await.approval = gear;
                            let allowed = self.gate_allowed.lock().await.contains(tool);
                            if self.gate.lock().await.judge(level, allowed) == GateVerdict::Allow {
                                break ConfirmDecision::AllowOnce;
                            }
                        }
                        Some(ControlCommand::SetExecMode(mode)) => {
                            self.apply_exec_mode(mode).await;
                        }
                        Some(ControlCommand::SetReadonly(v)) => {
                            self.tool_ctx.readonly.store(v, Ordering::SeqCst);
                        }
                        _ => {}
                    }
                }
            }
        };
        *self.confirm_pending.lock().await = None;
        *self.confirm_tx.lock().await = None;
        self.set_status(SessionStatus::Executing).await;
        self.emit(
            EventKind::ConfirmResolved,
            &serde_json::json!({
                "tool": tool,
                "decision": decision.as_str(),
            }),
        )
        .await;
        if let Some(cmd) = requeue {
            let _ = self.control_tx.send(cmd);
        }
        decision
    }

    /// v2.0 应用执行边界档（design-v2.md §4.1）：read_only 派生会话只读；
    /// full_access 派生命令沙箱降级；readonly 开关自身的独立切换不受影响。
    async fn apply_exec_mode(&self, mode: tenon_core::gates::ExecMode) {
        match mode {
            tenon_core::gates::ExecMode::ReadOnly => {
                self.tool_ctx.readonly.store(true, Ordering::SeqCst);
                self.tool_ctx.full_access.store(false, Ordering::SeqCst);
            }
            tenon_core::gates::ExecMode::WorkspaceWrite => {
                self.tool_ctx.full_access.store(false, Ordering::SeqCst);
            }
            tenon_core::gates::ExecMode::FullAccess => {
                self.tool_ctx.full_access.store(true, Ordering::SeqCst);
            }
        }
        self.gate.lock().await.exec_mode = mode;
    }

    /// v2.0 决议入口（daemon `POST /session/:id/confirm` 调用）。
    pub async fn resolve_confirm(&self, decision: ConfirmDecision) -> Result<(), String> {
        let tx = self.confirm_tx.lock().await.take();
        match tx {
            Some(tx) => {
                let _ = tx.send(decision);
                Ok(())
            }
            None => Err("当前没有等待确认的动作".to_string()),
        }
    }

    /// v2.0 待确认动作快照（`GET /session/:id` 的 `pending_confirm` 载荷）。
    pub async fn pending_confirm(&self) -> Option<PendingConfirm> {
        self.confirm_pending.lock().await.clone()
    }

    /// v2.0 当前档位快照（`GET /session/:id` 的 `gate` 载荷，切换器回显）。
    pub async fn gate_snapshot(&self) -> Gate {
        *self.gate.lock().await
    }

    async fn wait_first_edit_buffer(&self, ms: u64) -> bool {
        let fut = self.interrupt.notified();
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(ms)) => false,
            _ = fut => true,
        }
    }

    /// 验证（§9.4）：测试双通道；无测试清单走降级通道（低强度）。
    async fn verify(&self, changed: Vec<String>) -> (bool, String, bool) {
        // 受管 worktree 会话的验证必须在 worktree 上跑：主根无改动 → 假「通过」
        let root = self.tool_ctx.root.clone();
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
                .request(&self.tool_ctx.root, file, "diagnostics", 0, 0, None)
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
        self.rollback_to_checkpoint(&target).await
    }

    /// 回滚到指定 checkpoint（v1.111 消息级撤销 / §10.3 快照恢复）：
    /// restore 目标快照树（= 该步写入前状态），unrevert 快照先行（§10.3）。
    pub async fn rollback_to_checkpoint(
        &self,
        target: &Checkpoint,
    ) -> Result<Vec<String>, AgentError> {
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

    #[test]
    fn laya_decide_args_parse_validates_kind_and_text() {
        let (kind, text) =
            parse_laya_decide_args(&serde_json::json!({"kind": "risk", "text": " rm -rf / "}))
                .unwrap();
        assert_eq!(kind, tenon_laya::DecideKind::Risk);
        assert_eq!(text, "rm -rf /");
        // 空白 text / 未知 kind / 缺参均拒绝
        assert!(
            parse_laya_decide_args(&serde_json::json!({"kind": "risk", "text": "  "})).is_none()
        );
        assert!(parse_laya_decide_args(&serde_json::json!({"kind": "qa", "text": "x"})).is_none());
        assert!(parse_laya_decide_args(&serde_json::json!({"text": "x"})).is_none());
    }

    #[test]
    fn laya_decide_graded_a_and_in_tool_directory() {
        // §9.8 #4：A 级只读（本地推理零副作用），进全目录与只读收窄目录
        assert_eq!(
            tenon_core::Tool::from_name("laya_decide").and_then(|t| t.level()),
            Some(tenon_core::policy::Level::A)
        );
        assert!(tool_specs().iter().any(|t| t.name == "laya_decide"));
        assert!(tool_specs_read_only()
            .iter()
            .any(|t| t.name == "laya_decide"));
    }

    #[test]
    fn skill_use_graded_a_and_in_tool_directory() {
        // §13.4 v1.130：A 级只读（本地文件读取零副作用），进全目录与只读收窄目录
        assert_eq!(
            tenon_core::Tool::from_name("skill_use").and_then(|t| t.level()),
            Some(tenon_core::policy::Level::A)
        );
        assert!(tool_specs().iter().any(|t| t.name == "skill_use"));
        assert!(tool_specs_read_only().iter().any(|t| t.name == "skill_use"));
    }

    #[test]
    fn subtasks_graded_a_in_directory_not_readonly_narrowed() {
        // §9.2 v1.146：A 级（零工作区副作用），进全目录；不进只读先验轮白名单
        assert_eq!(
            tenon_core::Tool::from_name("subtasks").and_then(|t| t.level()),
            Some(tenon_core::policy::Level::A)
        );
        assert!(tool_specs().iter().any(|t| t.name == "subtasks"));
        assert!(!tool_specs_read_only().iter().any(|t| t.name == "subtasks"));
    }

    #[test]
    fn subtasks_args_parse_validates_items() {
        let ok = parse_subtasks_args(&serde_json::json!({ "items": [
            {"title": "改词法", "status": "done"},
            {"title": "补单测", "status": "in_progress"}
        ]}))
        .unwrap();
        assert_eq!(ok.len(), 2);
        assert_eq!(ok[0].status, SubtaskStatus::Done);
        assert_eq!(ok[1].status, SubtaskStatus::InProgress);
        // 空 items / 超 12 项 / 空 title / 未知 status / 缺字段均拒绝
        assert!(parse_subtasks_args(&serde_json::json!({"items": []})).is_none());
        let many: Vec<_> = (0..13)
            .map(|i| serde_json::json!({"title": format!("t{i}"), "status": "pending"}))
            .collect();
        assert!(parse_subtasks_args(&serde_json::json!({ "items": many })).is_none());
        assert!(parse_subtasks_args(
            &serde_json::json!({"items": [{"title": "  ", "status": "pending"}]})
        )
        .is_none());
        assert!(parse_subtasks_args(
            &serde_json::json!({"items": [{"title": "t", "status": "doing"}]})
        )
        .is_none());
        assert!(parse_subtasks_args(&serde_json::json!({"items": [{"title": "t"}]})).is_none());
        // 标题去首尾空白；超 200 字符拒绝
        let trimmed = parse_subtasks_args(
            &serde_json::json!({"items": [{"title": "  x  ", "status": "pending"}]}),
        )
        .unwrap();
        assert_eq!(trimmed[0].title, "x");
        let long = "长".repeat(201);
        assert!(parse_subtasks_args(
            &serde_json::json!({"items": [{"title": long, "status": "pending"}]})
        )
        .is_none());
    }

    #[test]
    fn decider_result_summary_shapes_by_kind() {
        use tenon_laya::{DecideValue, IntentLabel};
        let intent = decider_result_summary(&DecideValue::Intent {
            label: IntentLabel::NeedsChange,
            confidence: 0.8712,
        });
        assert_eq!(intent["label"], "needs_change");
        assert_eq!(intent["confidence"], 0.87);
        assert_eq!(
            decider_result_summary(&DecideValue::Risk { score: 0.834 }),
            serde_json::json!(0.83)
        );
        assert_eq!(
            decider_result_summary(&DecideValue::Route {
                suggest_light: true
            }),
            serde_json::json!(true)
        );
    }
}

#[cfg(test)]
mod compaction_tests {
    //! 历史压缩纯函数（§10.2 v1.105）。

    use super::*;
    use tenon_models::ToolCallReq;

    /// assistant(工具调用) + tool(结果) 一对。
    fn tool_round(id: &str, name: &str, output: &str) -> Vec<ChatMessage> {
        let mut assistant = ChatMessage::assistant(format!("意图：{name}"));
        assistant.tool_calls = vec![ToolCallReq {
            id: id.to_string(),
            name: name.to_string(),
            arguments: serde_json::json!({}),
        }];
        vec![
            assistant,
            ChatMessage::tool_result(id.to_string(), output.to_string()),
        ]
    }

    fn base_history(rounds: usize) -> Vec<ChatMessage> {
        let mut messages = vec![
            ChatMessage::system("sys"),
            ChatMessage::user("任务 + L1 工作集"),
        ];
        for i in 0..rounds {
            messages.extend(tool_round(
                &format!("call_{i}"),
                "read_file",
                &format!("out-{i}"),
            ));
        }
        messages
    }

    #[test]
    fn elide_keeps_recent_four_and_stubs_older_with_tool_name() {
        let messages = base_history(6);
        let (compacted, elided) = elide_stale_tool_outputs(&messages).unwrap();
        assert_eq!(elided, 2);
        // 最早两条被存根化：标明来源工具与原文规模，call id 配对保持
        assert!(
            compacted[3]
                .content
                .contains("[工具输出已省略：read_file 原约 5 字符"),
            "存根含工具名与原字符数：{}",
            compacted[3].content
        );
        assert_eq!(compacted[3].tool_call_id.as_deref(), Some("call_0"));
        assert_eq!(compacted[5].tool_call_id.as_deref(), Some("call_1"));
        assert!(compacted[5].content.contains("工具输出已省略"));
        // 最近四条保留原文
        assert_eq!(compacted[7].content, "out-2");
        assert_eq!(compacted[9].content, "out-3");
        assert_eq!(compacted[11].content, "out-4");
        assert_eq!(compacted[13].content, "out-5");
        // 系统提示、首条 user（任务 + L1 工作集）与 assistant 消息原样
        assert_eq!(compacted[0].content, "sys");
        assert_eq!(compacted[1].content, "任务 + L1 工作集");
        assert_eq!(compacted[2].content, "意图：read_file");
        assert_eq!(compacted[2].tool_calls.len(), 1);
        // 压缩后配对自检通过
        assert!(tool_call_pairs_intact(&compacted));
    }

    #[test]
    fn elide_noop_within_keep_window() {
        assert!(elide_stale_tool_outputs(&base_history(4)).is_none());
        assert!(elide_stale_tool_outputs(&base_history(0)).is_none());
    }

    #[test]
    fn elide_is_idempotent_over_already_stubbed_history() {
        let messages = base_history(6);
        let (once, _) = elide_stale_tool_outputs(&messages).unwrap();
        // 再次压缩：原文只剩 4 条（≤保留窗），已存根消息不重写、不重复计数
        assert!(elide_stale_tool_outputs(&once).is_none());
    }

    #[test]
    fn pairs_intact_detects_broken_pairing() {
        let mut messages = base_history(1);
        // 篡改 tool 消息的 call id → 配对破损
        messages[3].tool_call_id = Some("call_missing".to_string());
        assert!(!tool_call_pairs_intact(&messages));
        // 删除 call id → 配对破损
        messages[3].tool_call_id = None;
        assert!(!tool_call_pairs_intact(&messages));
    }

    #[test]
    fn estimate_covers_tool_call_arguments() {
        let mut m = ChatMessage::assistant("hi");
        let bare = estimate_messages_tokens(std::slice::from_ref(&m));
        m.tool_calls = vec![ToolCallReq {
            id: "c".into(),
            name: "apply_patch".into(),
            arguments: serde_json::json!({"content": "x".repeat(3_000)}),
        }];
        let with_args = estimate_messages_tokens(std::slice::from_ref(&m));
        assert!(
            with_args > bare + 500,
            "apply_patch 参数（新文件内容）必须计入输入估算"
        );
    }
    #[test]
    fn build_mcp_specs_names_schema_and_fallback() {
        // §13.5 v1.145：mcp_{server}_{tool} 命名、inputSchema 透传、
        // 非 object schema 兜底空 object、描述标注来源 server。
        let tools = vec![
            tenon_mcp::McpHostTool {
                server: "github".into(),
                tool: tenon_mcp::McpTool {
                    name: "create_issue".into(),
                    description: "创建 issue".into(),
                    input_schema: serde_json::json!({
                        "type": "object",
                        "properties": {"title": {"type": "string"}}
                    }),
                },
            },
            tenon_mcp::McpHostTool {
                server: "fs".into(),
                tool: tenon_mcp::McpTool {
                    name: "weird".into(),
                    description: String::new(),
                    input_schema: serde_json::json!({"type": "string"}),
                },
            },
        ];
        let specs = build_mcp_specs(&tools);
        assert_eq!(specs.len(), 2);
        assert_eq!(specs[0].name, "mcp_github_create_issue");
        assert!(specs[0].description.contains("创建 issue"));
        assert!(specs[0].description.contains("github"));
        assert_eq!(
            specs[0].parameters["properties"]["title"]["type"],
            serde_json::json!("string"),
            "inputSchema 透传"
        );
        assert_eq!(specs[1].name, "mcp_fs_weird");
        assert_eq!(
            specs[1].parameters,
            serde_json::json!({"type": "object", "properties": {}}),
            "非 object schema 兜底"
        );
        // 空列表 = 无 MCP 工具进目录。
        assert!(build_mcp_specs(&[]).is_empty());
    }
}

#[cfg(test)]
mod backoff_tests {
    use super::*;

    /// v1.186 §9.1：退避序列 2s→4s→8s→16s→30s 封顶；429 的 Retry-After
    /// 优先且上限 60s；无 Retry-After 的 429 走同一指数序列。
    #[test]
    fn backoff_schedule_exponential_then_cap() {
        let net = |a: u32| {
            AgentSession::backoff_delay(&tenon_models::ProviderError::Network("x".into()), a)
        };
        assert_eq!(net(1), Duration::from_secs(2));
        assert_eq!(net(2), Duration::from_secs(4));
        assert_eq!(net(3), Duration::from_secs(8));
        assert_eq!(net(4), Duration::from_secs(16));
        assert_eq!(net(5), Duration::from_secs(30), "第 5 次起重试封顶 30s");
        assert_eq!(net(10), Duration::from_secs(30));

        let limited = |ms: u64, a: u32| {
            AgentSession::backoff_delay(
                &tenon_models::ProviderError::RateLimited {
                    body: "429".into(),
                    retry_after: Some(Duration::from_millis(ms)),
                },
                a,
            )
        };
        assert_eq!(
            limited(10, 1),
            Duration::from_millis(10),
            "Retry-After 优先"
        );
        assert_eq!(
            limited(90_000, 3),
            Duration::from_secs(60),
            "Retry-After 上限 60s（v1.171 语义不变）"
        );
        let plain_limited = AgentSession::backoff_delay(
            &tenon_models::ProviderError::RateLimited {
                body: "429".into(),
                retry_after: None,
            },
            2,
        );
        assert_eq!(
            plain_limited,
            Duration::from_secs(4),
            "无 Retry-After 走指数退避"
        );
    }
}
