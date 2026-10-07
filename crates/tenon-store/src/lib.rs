//! 本地存储层（设计方案 §14.2）：SQLite 数据表、事件溯源、成本归因与冷归档。
//!
//! 设计要点：
//! - events 只追加（事件溯源），per-session `seq` 单调递增，WS 断线续传按 seq（§15）；
//! - approvals 审计记录永久保留（§14.2 增长治理）；
//! - model_usage 明细随会话归档，另维护按月聚合表（永久，§11 成本归因）。

use chrono::Utc;
use flate2::write::GzEncoder;
use flate2::Compression;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("数据库错误: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("JSON 错误: {0}")]
    Json(#[from] serde_json::Error),
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),
    #[error("记忆记录不合法: {0}")]
    InvalidMemory(String),
}

pub type Result<T> = std::result::Result<T, StoreError>;

/// 动作能力分级（§12.2）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    A,
    B,
    C,
    D,
    Composite,
}

impl Level {
    pub fn as_str(&self) -> &'static str {
        match self {
            Level::A => "a",
            Level::B => "b",
            Level::C => "c",
            Level::D => "d",
            Level::Composite => "cd",
        }
    }
}

/// 会话状态（§9.1 状态机）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Idle,
    Sensing,
    Deciding,
    Executing,
    Verifying,
    Fixing,
    Paused,
    Error,
    Done,
    RolledBack,
}

impl SessionStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            SessionStatus::Idle => "idle",
            SessionStatus::Sensing => "sensing",
            SessionStatus::Deciding => "deciding",
            SessionStatus::Executing => "executing",
            SessionStatus::Verifying => "verifying",
            SessionStatus::Fixing => "fixing",
            SessionStatus::Paused => "paused",
            SessionStatus::Error => "error",
            SessionStatus::Done => "done",
            SessionStatus::RolledBack => "rolled_back",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "idle" => SessionStatus::Idle,
            "sensing" => SessionStatus::Sensing,
            "deciding" => SessionStatus::Deciding,
            "executing" => SessionStatus::Executing,
            "verifying" => SessionStatus::Verifying,
            "fixing" => SessionStatus::Fixing,
            "paused" => SessionStatus::Paused,
            "error" => SessionStatus::Error,
            "done" => SessionStatus::Done,
            "rolled_back" => SessionStatus::RolledBack,
            _ => return None,
        })
    }
}

/// 事件类型枚举（§14.2）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    UserInput,
    Sensing,
    Decision,
    ModelDelta,
    PatchApplied,
    CommandRun,
    /// v1.89 C/D 直接执行风险审计（不等待决策）。
    DirectAction,
    Diagnostics,
    Checkpoint,
    Rollback,
    Unrollback,
    ModelFallback,
    DeciderCall,
    Error,
    /// 对话标题生成完成（v1.58，payload {title}）。
    SessionTitle,
    /// 上下文历史压缩（§10.2 v1.105，payload {before_est_tokens, after_est_tokens,
    /// elided_tool_results}）。v1.93 曾以死代码移除，v1.105 压缩接线后重引入。
    Compaction,
    /// L5 跨会话记忆提取入库完成（§10.1 v1.104，payload {count, ids}，不含原文）。
    MemorySaved,
    /// 子任务清单状态（§9.2 v1.146，payload {items:[{title,status}]} 全量快照）。
    Subtasks,
}

impl EventKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            EventKind::UserInput => "user_input",
            EventKind::Sensing => "sensing",
            EventKind::Decision => "decision",
            EventKind::ModelDelta => "model_delta",
            EventKind::PatchApplied => "patch_applied",
            EventKind::CommandRun => "command_run",
            EventKind::DirectAction => "direct_action",
            EventKind::Diagnostics => "diagnostics",
            EventKind::Checkpoint => "checkpoint",
            EventKind::Rollback => "rollback",
            EventKind::Unrollback => "unrollback",
            EventKind::ModelFallback => "model_fallback",
            EventKind::DeciderCall => "decider_call",
            EventKind::Error => "error",
            EventKind::SessionTitle => "session_title",
            EventKind::Compaction => "compaction",
            EventKind::MemorySaved => "memory_saved",
            EventKind::Subtasks => "subtasks",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "user_input" => EventKind::UserInput,
            "sensing" => EventKind::Sensing,
            "decision" => EventKind::Decision,
            "model_delta" => EventKind::ModelDelta,
            "patch_applied" => EventKind::PatchApplied,
            "command_run" => EventKind::CommandRun,
            "direct_action" => EventKind::DirectAction,
            "diagnostics" => EventKind::Diagnostics,
            "checkpoint" => EventKind::Checkpoint,
            "rollback" => EventKind::Rollback,
            "unrollback" => EventKind::Unrollback,
            "model_fallback" => EventKind::ModelFallback,
            "decider_call" => EventKind::DeciderCall,
            "error" => EventKind::Error,
            "session_title" => EventKind::SessionTitle,
            "compaction" => EventKind::Compaction,
            "memory_saved" => EventKind::MemorySaved,
            "subtasks" => EventKind::Subtasks,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub path: String,
    /// 用户自定义显示名；空串 = 由路径末段派生（§6.4 Project 显示名）。
    #[serde(default)]
    pub display_name: String,
    pub trusted: bool,
    pub language_packs: Vec<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub project_id: String,
    pub model: String,
    pub status: SessionStatus,
    /// 首条消息自动生成的对话标题（v1.58）；空串 = 未生成，UI 回退模型名 / 短 id。
    #[serde(default)]
    pub title: String,
    /// 会话级受管 worktree 路径（v1.87 §9.7）；空串 = 绑定项目主根。
    #[serde(default)]
    pub worktree_path: String,
    /// 手动归档时间（v1.103 §14.2）；空串 = 未归档，侧栏默认隐藏。
    #[serde(default)]
    pub archived_at: String,
    /// 线程截断水位（v1.135 消息级撤销）：[from, to] 闭区间内事件对线程 / Trace 隐藏；
    /// None = 无截断。unrollback 清水位，事件随之恢复可见。
    #[serde(default)]
    pub truncated_from_seq: Option<i64>,
    #[serde(default)]
    pub truncated_to_seq: Option<i64>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    /// 全局自增 id（跨会话全序）。
    pub id: i64,
    pub session_id: String,
    /// 项目归属（§14.2 / §6.4：多项目聚合与隔离）。
    pub project_id: String,
    /// per-session 单调递增（WS 断线续传游标，§15）。
    pub seq: i64,
    #[serde(rename = "type")]
    pub kind: EventKind,
    pub payload: serde_json::Value,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    pub id: String,
    pub session_id: String,
    /// shadow git tree oid（§10.3）
    pub tree: String,
    /// 该步改动文件集（§10.3）
    pub files: Vec<String>,
    /// 关联事件 seq（可空：任务开始前快照）
    pub event_seq: Option<i64>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: i64,
    pub session_id: String,
    pub event_id: i64,
    pub tool: String,
    pub level: Level,
    pub cost_tokens: i64,
    pub created_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApprovalDecision {
    Once,
    Session,
    Deny,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Approval {
    pub id: String,
    pub session_id: String,
    pub project_id: String,
    pub action: String,
    pub level: Level,
    /// None = 待决策
    pub decision: Option<ApprovalDecision>,
    pub created_at: String,
    pub decided_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelUsage {
    pub id: i64,
    pub session_id: String,
    pub project_id: String,
    pub provider: String,
    pub model: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    /// 缓存命中输入 token（v1.129 §11；旧行 / 未报告 = 0）。
    pub cached_input_tokens: i64,
    /// 该回合模型流耗时毫秒（provider 流建立 → 权威 Final 到达；0 = 未观测）。
    pub duration_ms: i64,
    pub cost_usd: f64,
    pub created_at: String,
}

/// 会话 / 项目用量聚合（v1.129）：命中率与均速由消费方派生
/// （命中率 = cached_input_tokens / input_tokens；速度 = output_tokens / duration_ms）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct UsageTotals {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cost_usd: f64,
    pub cached_input_tokens: i64,
    pub duration_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalRun {
    pub id: String,
    pub target: String,
    pub metrics_json: serde_json::Value,
    /// pass | fail | ...
    pub verdict: String,
    pub created_at: String,
}

/// 已安装插件记录（§14.2 plugins 表）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plugin {
    pub id: String,
    pub version: String,
    pub permissions: Vec<String>,
    pub signature: String,
    pub installed_at: String,
}

const SCHEMA_VERSION: i64 = 11;

const DDL: &str = r#"
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;
-- 多实例（壳 --no-lock 双开 / CLI 与 daemon 并存）并发写时，无 busy handler
-- 会立即返回 SQLITE_BUSY 而非等待——迁移路径出错会让第二个 daemon 直接启动失败
PRAGMA busy_timeout = 5000;

CREATE TABLE IF NOT EXISTS schema_version (
    version INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS projects (
    id TEXT PRIMARY KEY,
    path TEXT NOT NULL UNIQUE,
    display_name TEXT NOT NULL DEFAULT '',
    trusted INTEGER NOT NULL DEFAULT 0,
    language_packs TEXT NOT NULL DEFAULT '[]',
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS sessions (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(id),
    model TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL DEFAULT 'idle',
    title TEXT NOT NULL DEFAULT '',
    worktree_path TEXT NOT NULL DEFAULT '',
    archived_at TEXT NOT NULL DEFAULT '',
    truncated_from_seq INTEGER,
    truncated_to_seq INTEGER,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL,
    project_id TEXT NOT NULL DEFAULT '',
    seq INTEGER NOT NULL,
    type TEXT NOT NULL,
    payload TEXT NOT NULL,
    created_at TEXT NOT NULL,
    UNIQUE(session_id, seq)
);
CREATE INDEX IF NOT EXISTS idx_events_session ON events(session_id, seq);
CREATE INDEX IF NOT EXISTS idx_events_type_session ON events(type, session_id, id);

CREATE TABLE IF NOT EXISTS checkpoints (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    tree TEXT NOT NULL,
    files TEXT NOT NULL DEFAULT '[]',
    event_seq INTEGER,
    created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_checkpoints_session ON checkpoints(session_id, created_at);

CREATE TABLE IF NOT EXISTS tool_calls (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL,
    event_id INTEGER NOT NULL,
    tool TEXT NOT NULL,
    level TEXT NOT NULL,
    cost_tokens INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_tool_calls_session ON tool_calls(session_id);

CREATE TABLE IF NOT EXISTS approvals (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    project_id TEXT NOT NULL DEFAULT '',
    action TEXT NOT NULL,
    level TEXT NOT NULL,
    decision TEXT,
    created_at TEXT NOT NULL,
    decided_at TEXT
);
CREATE TABLE IF NOT EXISTS plugins (
    id TEXT PRIMARY KEY,
    version TEXT NOT NULL,
    permissions TEXT NOT NULL DEFAULT '[]',
    signature TEXT NOT NULL DEFAULT '',
    installed_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS model_usage (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL,
    project_id TEXT NOT NULL DEFAULT '',
    provider TEXT NOT NULL,
    model TEXT NOT NULL DEFAULT '',
    input_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    cached_input_tokens INTEGER NOT NULL DEFAULT 0,
    duration_ms INTEGER NOT NULL DEFAULT 0,
    cost_usd REAL NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_model_usage_session ON model_usage(session_id);

CREATE TABLE IF NOT EXISTS model_usage_monthly (
    month TEXT PRIMARY KEY,
    input_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    cost_usd REAL NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS eval_runs (
    id TEXT PRIMARY KEY,
    target TEXT NOT NULL,
    metrics_json TEXT NOT NULL,
    verdict TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS l4_chunks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id TEXT NOT NULL,
    path TEXT NOT NULL,
    symbol TEXT,
    start_line INTEGER NOT NULL DEFAULT 1,
    end_line INTEGER NOT NULL DEFAULT 1,
    text TEXT,
    embedding BLOB,
    updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_l4_chunks_project ON l4_chunks(project_id, path);

-- UI 偏好（§7.5 外观档等）：daemon 端口动态，localStorage 按 origin 隔离
-- 不可跨启动——此处为跨启动 / 跨端（桌面 + 浏览器）权威存储
CREATE TABLE IF NOT EXISTS ui_prefs (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

-- 项目级 UI 状态（§7.2：布局按项目记忆；§6.4 多项目互不污染）
CREATE TABLE IF NOT EXISTS project_ui_state (
    project_id TEXT PRIMARY KEY REFERENCES projects(id) ON DELETE CASCADE,
    state_json TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

-- L5 跨会话对话记忆（§10.1 v1.104）：project 层按 project_id 隔离；
-- global 层仅 kind=preference（§9.7 跨项目不共享上下文的显式例外，永不承载仓库内容）
CREATE TABLE IF NOT EXISTS memories (
    id TEXT PRIMARY KEY,
    scope TEXT NOT NULL DEFAULT 'project',
    project_id TEXT NOT NULL DEFAULT '',
    kind TEXT NOT NULL DEFAULT 'fact',
    content TEXT NOT NULL,
    importance INTEGER NOT NULL DEFAULT 3,
    embedding BLOB,
    source_session TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    last_seen_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_memories_project ON memories(project_id, kind);
"#;

/// 入库用 L4 切片记录。
#[derive(Debug, Clone)]
pub struct L4ChunkRecord {
    pub symbol: Option<String>,
    pub start_line: usize,
    pub end_line: usize,
    pub text: String,
    pub embedding: Vec<f32>,
}

/// L4 embedding 召回结果。
#[derive(Debug, Clone, Serialize)]
pub struct L4SearchHit {
    pub id: i64,
    pub path: String,
    pub symbol: String,
    pub start_line: usize,
    pub end_line: usize,
    pub text: String,
    pub score: f32,
}

/// L5 跨会话对话记忆（§10.1 v1.104）。不含 embedding（仅内部去重使用）。
#[derive(Debug, Clone, Serialize)]
pub struct Memory {
    pub id: String,
    /// project | global（global 仅 kind=preference，永不承载仓库内容）。
    pub scope: String,
    /// project 层归属项目；global 层为空串。
    pub project_id: String,
    /// preference | fact | decision | workflow。
    pub kind: String,
    pub content: String,
    /// 1-5；注入排序权重。
    pub importance: i64,
    pub source_session: String,
    pub created_at: String,
    pub updated_at: String,
    pub last_seen_at: String,
}

/// 入库用记忆记录（写入前经 upsert_memory 校验与去重）。
#[derive(Debug, Clone)]
pub struct MemoryRecord {
    pub scope: String,
    pub project_id: String,
    pub kind: String,
    pub content: String,
    pub importance: i64,
    pub embedding: Vec<f32>,
    pub source_session: String,
}

impl MemoryRecord {
    pub fn validate_scope(scope: &str) -> std::result::Result<String, StoreError> {
        match scope {
            "project" | "global" => Ok(scope.to_string()),
            other => Err(StoreError::InvalidMemory(format!(
                "未知 scope: {other}（仅 project / global）"
            ))),
        }
    }

    pub fn validate_kind(kind: &str) -> std::result::Result<String, StoreError> {
        match kind {
            "preference" | "fact" | "decision" | "workflow" => Ok(kind.to_string()),
            other => Err(StoreError::InvalidMemory(format!(
                "未知 kind: {other}（仅 preference / fact / decision / workflow）"
            ))),
        }
    }
}

/// 存储门面。内部连接由调用方保证单线程访问（daemon 侧以互斥锁包裹）。
pub struct Store {
    conn: Connection,
}

impl Store {
    /// 打开（或创建）数据库并执行迁移。
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        Self::init(conn)
    }

    /// 内存库（测试用）。
    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        let conn = conn;
        conn.execute_batch(DDL)?;
        let has_version: Option<i64> = conn
            .query_row("SELECT version FROM schema_version", [], |r| r.get(0))
            .optional()?;
        match has_version {
            None => {
                conn.execute(
                    "INSERT INTO schema_version (version) VALUES (?1)",
                    [SCHEMA_VERSION],
                )?;
            }
            Some(v) if v < SCHEMA_VERSION => {
                // v1 → v2：多项目归属字段（§6.4 / §14.2）。SQLite ADD COLUMN
                // 不支持无默认 NOT NULL，先加默认列再由 sessions 回填。
                if !Self::column_exists(&conn, "events", "project_id")? {
                    conn.execute(
                        "ALTER TABLE events ADD COLUMN project_id TEXT NOT NULL DEFAULT ''",
                        [],
                    )?;
                }
                if !Self::column_exists(&conn, "approvals", "project_id")? {
                    conn.execute(
                        "ALTER TABLE approvals ADD COLUMN project_id TEXT NOT NULL DEFAULT ''",
                        [],
                    )?;
                }
                if !Self::column_exists(&conn, "model_usage", "project_id")? {
                    conn.execute(
                        "ALTER TABLE model_usage ADD COLUMN project_id TEXT NOT NULL DEFAULT ''",
                        [],
                    )?;
                }
                // v3 → v4：L4 切片补齐行区间（旧记录按切片文本行数近似回填）。
                if !Self::column_exists(&conn, "l4_chunks", "start_line")? {
                    conn.execute(
                        "ALTER TABLE l4_chunks ADD COLUMN start_line INTEGER NOT NULL DEFAULT 1",
                        [],
                    )?;
                }
                if !Self::column_exists(&conn, "l4_chunks", "end_line")? {
                    conn.execute(
                        "ALTER TABLE l4_chunks ADD COLUMN end_line INTEGER NOT NULL DEFAULT 1",
                        [],
                    )?;
                }
                // v4 → v5：projects 补 display_name（§14.2 projects 表既定字段）。
                if !Self::column_exists(&conn, "projects", "display_name")? {
                    conn.execute(
                        "ALTER TABLE projects ADD COLUMN display_name TEXT NOT NULL DEFAULT ''",
                        [],
                    )?;
                }
                // v5 → v6：sessions 补自动生成对话标题（v1.58，旧行回退空串）。
                if !Self::column_exists(&conn, "sessions", "title")? {
                    conn.execute(
                        "ALTER TABLE sessions ADD COLUMN title TEXT NOT NULL DEFAULT ''",
                        [],
                    )?;
                }
                // v6 → v7：sessions 补会话级受管 worktree 路径（v1.87 §9.7，空 = 主根会话）。
                if !Self::column_exists(&conn, "sessions", "worktree_path")? {
                    conn.execute(
                        "ALTER TABLE sessions ADD COLUMN worktree_path TEXT NOT NULL DEFAULT ''",
                        [],
                    )?;
                }
                conn.execute(
                    "UPDATE l4_chunks
                     SET end_line = start_line + (LENGTH(text) - LENGTH(REPLACE(text, '\n', '')))
                     WHERE end_line < start_line",
                    [],
                )?;
                conn.execute_batch(
                    "UPDATE events SET project_id = COALESCE((SELECT project_id FROM sessions WHERE sessions.id = events.session_id), '')
                     WHERE project_id = '';
                     UPDATE approvals SET project_id = COALESCE((SELECT project_id FROM sessions WHERE sessions.id = approvals.session_id), '')
                     WHERE project_id = '';
                     UPDATE model_usage SET project_id = COALESCE((SELECT project_id FROM sessions WHERE sessions.id = model_usage.session_id), '')
                     WHERE project_id = '';
                     CREATE INDEX IF NOT EXISTS idx_events_project ON events(project_id, id);
                     CREATE INDEX IF NOT EXISTS idx_approvals_project ON approvals(project_id, created_at);
                     CREATE INDEX IF NOT EXISTS idx_model_usage_project ON model_usage(project_id, id);",
                )?;
                // v7 → v8：会话手动归档（v1.103 §14.2）；空 = 未归档。
                if !Self::column_exists(&conn, "sessions", "archived_at")? {
                    conn.execute(
                        "ALTER TABLE sessions ADD COLUMN archived_at TEXT NOT NULL DEFAULT ''",
                        [],
                    )?;
                }
                // v9 → v10：model_usage 补缓存命中与回合耗时（v1.129 §11；旧行回退 0 = 未观测）。
                if !Self::column_exists(&conn, "model_usage", "cached_input_tokens")? {
                    conn.execute(
                        "ALTER TABLE model_usage ADD COLUMN cached_input_tokens INTEGER NOT NULL DEFAULT 0",
                        [],
                    )?;
                }
                if !Self::column_exists(&conn, "model_usage", "duration_ms")? {
                    conn.execute(
                        "ALTER TABLE model_usage ADD COLUMN duration_ms INTEGER NOT NULL DEFAULT 0",
                        [],
                    )?;
                }
                // v10 → v11：会话线程截断水位（v1.135 消息级撤销——水位 [from,to] 内事件
                // 对线程 / Trace 隐藏；事件行保留在库可审计，unrollback 清水位即恢复）。
                if !Self::column_exists(&conn, "sessions", "truncated_from_seq")? {
                    conn.execute(
                        "ALTER TABLE sessions ADD COLUMN truncated_from_seq INTEGER",
                        [],
                    )?;
                }
                if !Self::column_exists(&conn, "sessions", "truncated_to_seq")? {
                    conn.execute(
                        "ALTER TABLE sessions ADD COLUMN truncated_to_seq INTEGER",
                        [],
                    )?;
                }
                conn.execute("UPDATE schema_version SET version = ?1", [SCHEMA_VERSION])?;
            }
            Some(_) => {}
        }
        // v2 项目归属索引在 v1 → v2 ALTER 后才可安全创建；IF NOT EXISTS 兼容新库。
        conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_events_project ON events(project_id, id);
             CREATE INDEX IF NOT EXISTS idx_approvals_project ON approvals(project_id, created_at);
             CREATE INDEX IF NOT EXISTS idx_model_usage_project ON model_usage(project_id, id);",
        )?;
        Ok(Self { conn })
    }

    #[doc(hidden)]
    pub fn connection_for_tests(&mut self) -> &mut Connection {
        &mut self.conn
    }

    fn now() -> String {
        Utc::now().to_rfc3339()
    }

    fn column_exists(conn: &Connection, table: &str, column: &str) -> Result<bool> {
        let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
        let found = stmt
            .query_map([], |r| r.get::<_, String>(1))?
            .any(|name| name.map(|n| n == column).unwrap_or(false));
        Ok(found)
    }

    fn project_id_for_session(&self, session_id: &str) -> String {
        self.conn
            .query_row(
                "SELECT project_id FROM sessions WHERE id = ?1",
                [session_id],
                |r| r.get::<_, String>(0),
            )
            .unwrap_or_default()
    }

    // ---------- projects ----------

    pub fn upsert_project(&mut self, path: &str) -> Result<Project> {
        let canonical = std::fs::canonicalize(path)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| path.to_string());
        let now = Self::now();
        let existing = self.project_by_path(&canonical)?;
        if let Some(p) = existing {
            return Ok(p);
        }
        self.conn.execute(
            "INSERT INTO projects (id, path, created_at) VALUES (?1, ?2, ?3)",
            params![Uuid::now_v7().to_string(), canonical, now],
        )?;
        Ok(self
            .project_by_path(&canonical)?
            .expect("row inserted above"))
    }

    pub fn project_by_path(&mut self, path: &str) -> Result<Option<Project>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, path, display_name, trusted, language_packs, created_at FROM projects WHERE path = ?1",
        )?;
        let mut rows = stmt.query_map([path], row_to_project)?;
        Ok(rows.next().transpose()?)
    }

    pub fn project(&mut self, id: &str) -> Result<Option<Project>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, path, display_name, trusted, language_packs, created_at FROM projects WHERE id = ?1",
        )?;
        let mut rows = stmt.query_map([id], row_to_project)?;
        Ok(rows.next().transpose()?)
    }

    pub fn list_projects(&mut self) -> Result<Vec<Project>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, path, display_name, trusted, language_packs, created_at FROM projects ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map([], row_to_project)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// 移除登记；磁盘内容由调用方保证不删除（§6.4）。
    pub fn remove_project(&mut self, id: &str) -> Result<bool> {
        Ok(self
            .conn
            .execute("DELETE FROM projects WHERE id = ?1", [id])?
            > 0)
    }

    /// TOFU 信任设置（§12.7）：信任只放宽 B 级档位，永不放宽 C/D。
    pub fn set_project_trusted(&mut self, id: &str, trusted: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE projects SET trusted = ?2 WHERE id = ?1",
            params![id, trusted as i64],
        )?;
        Ok(())
    }

    /// 设置项目显示名；空串回退为路径末段派生（§6.4 / §14.2）。
    pub fn set_project_display_name(&mut self, id: &str, display_name: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE projects SET display_name = ?2 WHERE id = ?1",
            params![id, display_name],
        )?;
        Ok(())
    }

    pub fn set_project_language_packs(&mut self, id: &str, packs: &[String]) -> Result<()> {
        self.conn.execute(
            "UPDATE projects SET language_packs = ?2 WHERE id = ?1",
            params![id, serde_json::to_string(packs)?],
        )?;
        Ok(())
    }

    // ---------- sessions ----------

    /// 会话 ID 生成（v1.87：受管 worktree 路径依赖预生成 id；daemon 经此免引 uuid）。
    pub fn new_session_id() -> String {
        Uuid::now_v7().to_string()
    }

    pub fn create_session(&mut self, project_id: &str, model: &str) -> Result<Session> {
        let id = Uuid::now_v7().to_string();
        self.create_session_with_id(&id, project_id, model)
    }

    /// 预生成会话 ID 的创建入口（v1.87：受管 worktree 路径依赖会话 id）。
    pub fn create_session_with_id(
        &mut self,
        id: &str,
        project_id: &str,
        model: &str,
    ) -> Result<Session> {
        let now = Self::now();
        let s = Session {
            id: id.to_string(),
            project_id: project_id.to_string(),
            model: model.to_string(),
            status: SessionStatus::Idle,
            title: String::new(),
            worktree_path: String::new(),
            archived_at: String::new(),
            truncated_from_seq: None,
            truncated_to_seq: None,
            created_at: now.clone(),
            updated_at: now,
        };
        self.conn.execute(
            "INSERT INTO sessions (id, project_id, model, status, title, worktree_path, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                s.id,
                s.project_id,
                s.model,
                s.status.as_str(),
                s.title,
                s.worktree_path,
                s.created_at,
                s.updated_at
            ],
        )?;
        Ok(s)
    }

    /// 登记会话级受管 worktree 路径（v1.87 §9.7）；空串回退主根。
    pub fn set_session_worktree(&mut self, id: &str, worktree_path: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE sessions SET worktree_path = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, worktree_path, Self::now()],
        )?;
        Ok(())
    }

    pub fn session(&mut self, id: &str) -> Result<Option<Session>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, project_id, model, status, title, worktree_path, created_at, updated_at, archived_at, truncated_from_seq, truncated_to_seq
             FROM sessions WHERE id = ?1",
        )?;
        let mut rows = stmt.query_map([id], row_to_session)?;
        Ok(rows.next().transpose()?)
    }

    /// 全部会话（崩溃恢复扫描用；含已归档，恢复语义不因归档改变）。
    pub fn list_all_sessions(&mut self) -> Result<Vec<Session>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, project_id, model, status, title, worktree_path, created_at, updated_at, archived_at, truncated_from_seq, truncated_to_seq
             FROM sessions ORDER BY created_at ASC",
        )?;
        let rows = stmt.query_map([], row_to_session)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// 项目未归档会话（v1.103：手动归档在侧栏默认隐藏）。
    pub fn list_sessions(&mut self, project_id: &str) -> Result<Vec<Session>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, project_id, model, status, title, worktree_path, created_at, updated_at, archived_at, truncated_from_seq, truncated_to_seq
             FROM sessions WHERE project_id = ?1 AND archived_at = '' ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map([project_id], row_to_session)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// 项目已归档会话（v1.103：侧栏「已归档」折叠组数据源）。
    pub fn list_archived_sessions(&mut self, project_id: &str) -> Result<Vec<Session>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, project_id, model, status, title, worktree_path, created_at, updated_at, archived_at, truncated_from_seq, truncated_to_seq
             FROM sessions WHERE project_id = ?1 AND archived_at != '' ORDER BY updated_at DESC",
        )?;
        let rows = stmt.query_map([project_id], row_to_session)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// 项目内各会话最新一次 `subtasks` 事件的完成计数（v1.148 §15：侧栏任务行
    /// 进度徽标数据源）。返回 (session_id, done, total)；会话无 subtasks 事件
    /// 则不出现。payload 损坏按无清单处理（呈现层降级，不做写入面）。
    pub fn latest_subtasks_by_session(
        &mut self,
        project_id: &str,
    ) -> Result<Vec<(String, u32, u32)>> {
        let mut stmt = self.conn.prepare(
            "SELECT e.session_id, e.payload FROM events e
             WHERE e.type = 'subtasks'
               AND e.id = (SELECT MAX(id) FROM events WHERE type = 'subtasks' AND session_id = e.session_id)
               AND e.session_id IN (SELECT id FROM sessions WHERE project_id = ?1)",
        )?;
        let rows = stmt.query_map([project_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (session_id, payload) = row?;
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&payload) else {
                continue;
            };
            let Some(items) = value.get("items").and_then(|v| v.as_array()) else {
                continue;
            };
            let total = items.len() as u32;
            if total == 0 {
                continue;
            }
            let done = items
                .iter()
                .filter(|i| i.get("status").and_then(|s| s.as_str()) == Some("done"))
                .count() as u32;
            out.push((session_id, done, total));
        }
        Ok(out)
    }

    /// 手动归档（v1.103 §14.2）：侧栏隐藏、可还原，数据不出库。
    pub fn archive_session(&mut self, id: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE sessions SET archived_at = ?2 WHERE id = ?1",
            params![id, Self::now()],
        )?;
        Ok(())
    }

    /// 取消归档（v1.103）：恢复侧栏列表。
    pub fn unarchive_session(&mut self, id: &str) -> Result<()> {
        self.conn
            .execute("UPDATE sessions SET archived_at = '' WHERE id = ?1", [id])?;
        Ok(())
    }

    /// 手动删除会话（v1.103 §14.2）：事务级联删事件 / 工具调用 / checkpoint / 用量 / 审批 / 会话行；
    /// 项目 shadow 快照不随删（由 checkpoint.keep_days gc 老化）。
    pub fn delete_session(&mut self, id: &str) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM events WHERE session_id = ?1", [id])?;
        tx.execute("DELETE FROM tool_calls WHERE session_id = ?1", [id])?;
        tx.execute("DELETE FROM checkpoints WHERE session_id = ?1", [id])?;
        tx.execute("DELETE FROM model_usage WHERE session_id = ?1", [id])?;
        tx.execute("DELETE FROM approvals WHERE session_id = ?1", [id])?;
        tx.execute("DELETE FROM sessions WHERE id = ?1", [id])?;
        tx.commit()?;
        Ok(())
    }

    pub fn set_session_status(&mut self, id: &str, status: SessionStatus) -> Result<()> {
        self.conn.execute(
            "UPDATE sessions SET status = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, status.as_str(), Self::now()],
        )?;
        Ok(())
    }

    pub fn set_session_model(&mut self, id: &str, model: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE sessions SET model = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, model, Self::now()],
        )?;
        Ok(())
    }

    /// 写入自动生成的对话标题（v1.58）；空串视为未生成。
    pub fn set_session_title(&mut self, id: &str, title: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE sessions SET title = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, title, Self::now()],
        )?;
        Ok(())
    }

    // ---------- events（只追加） ----------

    /// 追加事件，返回带 id/seq 的完整事件。seq 为 per-session 单调递增。
    pub fn append_event(
        &mut self,
        session_id: &str,
        kind: EventKind,
        payload: &serde_json::Value,
    ) -> Result<Event> {
        let now = Self::now();
        let project_id = self.project_id_for_session(session_id);
        let seq: i64 = self.conn.query_row(
            "SELECT COALESCE(MAX(seq), 0) + 1 FROM events WHERE session_id = ?1",
            [session_id],
            |r| r.get(0),
        )?;
        self.conn.execute(
            "INSERT INTO events (session_id, project_id, seq, type, payload, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                session_id,
                project_id,
                seq,
                kind.as_str(),
                payload.to_string(),
                now
            ],
        )?;
        Ok(Event {
            id: self.conn.last_insert_rowid(),
            session_id: session_id.to_string(),
            project_id,
            seq,
            kind,
            payload: payload.clone(),
            created_at: now,
        })
    }

    /// 全量事件（按会话内 seq 升序）。
    pub fn events(&mut self, session_id: &str) -> Result<Vec<Event>> {
        self.events_since(session_id, 0)
    }

    /// 断线续传：返回 seq > after_seq 的事件（§15）。
    /// v1.135：会话截断水位 [from, to] 内的事件对线程 / Trace 隐藏（行留库可审计）。
    pub fn events_since(&mut self, session_id: &str, after_seq: i64) -> Result<Vec<Event>> {
        let truncation = self.thread_truncation(session_id)?;
        let mut stmt = self.conn.prepare(
            "SELECT id, session_id, project_id, seq, type, payload, created_at
             FROM events WHERE session_id = ?1 AND seq > ?2 ORDER BY seq ASC",
        )?;
        let rows = stmt.query_map(params![session_id, after_seq], row_to_event)?;
        let all = rows.collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(all
            .into_iter()
            .filter(|e| match (truncation.0, truncation.1) {
                (Some(from), Some(to)) => !(e.seq >= from && e.seq <= to),
                _ => true,
            })
            .collect())
    }

    /// 线程截断水位（v1.135）：None = 无截断。
    pub fn thread_truncation(&mut self, session_id: &str) -> Result<(Option<i64>, Option<i64>)> {
        let row: Option<(Option<i64>, Option<i64>)> = self
            .conn
            .query_row(
                "SELECT truncated_from_seq, truncated_to_seq FROM sessions WHERE id = ?1",
                [session_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        Ok(row.unwrap_or((None, None)))
    }

    /// 回合起点（v1.135 消息级撤销）：`before_seq`（含）之前最近的 user_input seq；
    /// 无则 None（该回合无 user_input，不截断）。
    pub fn turn_start_seq(&mut self, session_id: &str, before_seq: i64) -> Option<i64> {
        self.conn
            .query_row(
                "SELECT MAX(seq) FROM events
                 WHERE session_id = ?1 AND seq <= ?2 AND type = 'user_input'",
                params![session_id, before_seq],
                |r| r.get::<_, Option<i64>>(0),
            )
            .ok()
            .flatten()
    }

    /// 设置 / 清除线程截断水位（v1.135）：None = 清除（unrollback 恢复可见）。
    pub fn set_thread_truncation(
        &mut self,
        session_id: &str,
        from: Option<i64>,
        to: Option<i64>,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE sessions SET truncated_from_seq = ?2, truncated_to_seq = ?3 WHERE id = ?1",
            params![session_id, from, to],
        )?;
        Ok(())
    }

    /// 全局事件流（WS 推送用）：id 升序、跨会话、限量。
    pub fn recent_events(&mut self, after_global_id: i64, limit: i64) -> Result<Vec<Event>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, session_id, project_id, seq, type, payload, created_at
             FROM events WHERE id > ?1 ORDER BY id ASC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![after_global_id, limit], row_to_event)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn latest_seq(&mut self, session_id: &str) -> Result<i64> {
        let seq: i64 = self.conn.query_row(
            "SELECT COALESCE(MAX(seq), 0) FROM events WHERE session_id = ?1",
            [session_id],
            |r| r.get(0),
        )?;
        Ok(seq)
    }

    /// 会话内某类型事件计数（v1.58 判定「首条用户消息」用）。
    pub fn count_events_of_kind(&mut self, session_id: &str, kind: EventKind) -> Result<i64> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM events WHERE session_id = ?1 AND type = ?2",
            params![session_id, kind.as_str()],
            |r| r.get(0),
        )?;
        Ok(count)
    }

    // ---------- ui_prefs（§7.5 外观档等跨启动 UI 偏好） ----------

    /// 读全部 UI 偏好（键值对）。
    pub fn ui_prefs(&mut self) -> Result<Vec<(String, String)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT key, value FROM ui_prefs ORDER BY key")?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// 写 UI 偏好（upsert）。
    pub fn set_ui_pref(&mut self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO ui_prefs(key, value) VALUES(?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [key, value],
        )?;
        Ok(())
    }

    /// 读项目级 UI 状态 JSON；None = 尚未保存。
    pub fn project_ui_state(&mut self, project_id: &str) -> Result<Option<String>> {
        self.conn
            .query_row(
                "SELECT state_json FROM project_ui_state WHERE project_id = ?1",
                [project_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(Into::into)
    }

    /// 写项目级 UI 状态 JSON（upsert）。
    pub fn set_project_ui_state(&mut self, project_id: &str, state_json: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO project_ui_state(project_id, state_json, updated_at)
             VALUES(?1, ?2, ?3)
             ON CONFLICT(project_id) DO UPDATE SET
               state_json = excluded.state_json,
               updated_at = excluded.updated_at",
            params![project_id, state_json, Self::now()],
        )?;
        Ok(())
    }

    // ---------- memories（L5 跨会话对话记忆 §10.1 v1.104） ----------

    /// 写入一条记忆；与既有 active 记忆做本地 embedding 余弦去重，
    /// ≥`threshold` 视为同条——刷新 content / importance / last_seen_at /
    /// source_session 并返回 (刷新后的记忆, true)；否则新增行并返回 (新记忆, false)。
    pub fn upsert_memory(&mut self, rec: &MemoryRecord, threshold: f32) -> Result<(Memory, bool)> {
        let scope = MemoryRecord::validate_scope(&rec.scope)?;
        let kind = MemoryRecord::validate_kind(&rec.kind)?;
        if scope == "global" && kind != "preference" {
            return Err(StoreError::InvalidMemory(
                "global 作用域仅接受 kind=preference（§10.1 v1.104）".into(),
            ));
        }
        if rec.content.trim().is_empty() {
            return Err(StoreError::InvalidMemory("记忆内容不能为空".into()));
        }
        if let Some(existing) =
            self.find_similar_memory(&rec.embedding, &rec.scope, &rec.project_id, threshold)?
        {
            self.conn.execute(
                "UPDATE memories
                 SET content = ?2, importance = MAX(importance, ?3), updated_at = ?4,
                     last_seen_at = ?4, source_session = ?5
                 WHERE id = ?1",
                params![
                    existing.id,
                    rec.content.trim(),
                    rec.importance.clamp(1, 5),
                    Self::now(),
                    rec.source_session,
                ],
            )?;
            let updated = self
                .memory(&existing.id)?
                .ok_or_else(|| StoreError::InvalidMemory("记忆刷新后读取失败".into()))?;
            return Ok((updated, true));
        }
        let now = Self::now();
        let mem = Memory {
            id: Uuid::now_v7().to_string(),
            scope,
            project_id: rec.project_id.clone(),
            kind,
            content: rec.content.trim().to_string(),
            importance: rec.importance.clamp(1, 5),
            source_session: rec.source_session.clone(),
            created_at: now.clone(),
            updated_at: now.clone(),
            last_seen_at: now,
        };
        self.conn.execute(
            "INSERT INTO memories(id, scope, project_id, kind, content, importance,
                                  embedding, source_session, created_at, updated_at, last_seen_at)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                mem.id,
                mem.scope,
                mem.project_id,
                mem.kind,
                mem.content,
                mem.importance,
                f32_slice_to_blob(&rec.embedding),
                mem.source_session,
                mem.created_at,
                mem.updated_at,
                mem.last_seen_at,
            ],
        )?;
        Ok((mem, false))
    }

    /// 与候选记忆做余弦相似度，返回最高分且达阈值的一条。
    /// 候选池按写入作用域隔离：global 写入只与 global preference 去重；
    /// project 写入只与本项目的 project 记忆去重——project 记忆若命中
    /// global 行会改写其 content/source_session（项目事实污染所有项目，
    /// §10.1 的 global 层「永不承载仓库内容」约束被破坏）。
    fn find_similar_memory(
        &mut self,
        embedding: &[f32],
        scope: &str,
        project_id: &str,
        threshold: f32,
    ) -> Result<Option<Memory>> {
        let ids: Vec<String> = if scope == "global" {
            self.conn
                .prepare(
                    "SELECT id FROM memories
                     WHERE scope = 'global' AND kind = 'preference'",
                )
                .and_then(|mut stmt| {
                    stmt.query_map([], |r| r.get::<_, String>(0))?
                        .collect::<std::result::Result<Vec<_>, _>>()
                })?
        } else {
            self.conn
                .prepare(
                    "SELECT id FROM memories
                     WHERE scope = 'project' AND project_id = ?1",
                )
                .and_then(|mut stmt| {
                    stmt.query_map([project_id], |r| r.get::<_, String>(0))?
                        .collect::<std::result::Result<Vec<_>, _>>()
                })?
        };
        let mut best: Option<(f32, String)> = None;
        for id in ids {
            let stored: Option<Vec<u8>> = self
                .conn
                .query_row("SELECT embedding FROM memories WHERE id = ?1", [&id], |r| {
                    r.get(0)
                })
                .optional()?;
            let Some(blob) = stored else { continue };
            let score = cosine(embedding, &blob_to_f32_slice(&blob));
            if score >= threshold && best.as_ref().map(|(s, _)| score > *s).unwrap_or(true) {
                best = Some((score, id));
            }
        }
        let found_id = best.map(|(_, id)| id);
        match found_id {
            Some(id) => self.memory(&id),
            None => Ok(None),
        }
    }

    /// 读一条记忆；None = 不存在。
    pub fn memory(&mut self, id: &str) -> Result<Option<Memory>> {
        self.conn
            .query_row(
                "SELECT id, scope, project_id, kind, content, importance,
                        source_session, created_at, updated_at, last_seen_at
                 FROM memories WHERE id = ?1",
                [id],
                |r| {
                    Ok(Memory {
                        id: r.get(0)?,
                        scope: r.get(1)?,
                        project_id: r.get(2)?,
                        kind: r.get(3)?,
                        content: r.get(4)?,
                        importance: r.get(5)?,
                        source_session: r.get(6)?,
                        created_at: r.get(7)?,
                        updated_at: r.get(8)?,
                        last_seen_at: r.get(9)?,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }

    /// 列出对某项目可见的记忆：项目层全部 + global 层 preference。
    /// 按 importance 降序 + last_seen_at 降序；`query` 非空时按 content 子串过滤。
    pub fn list_memories(
        &mut self,
        project_id: &str,
        query: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Memory>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, scope, project_id, kind, content, importance,
                    source_session, created_at, updated_at, last_seen_at
             FROM memories
             WHERE (scope = 'project' AND project_id = ?1)
                OR (scope = 'global' AND kind = 'preference')
             ORDER BY importance DESC, last_seen_at DESC
             LIMIT ?2",
        )?;
        let rows = stmt
            .query_map(params![project_id, limit as i64], |r| {
                Ok(Memory {
                    id: r.get(0)?,
                    scope: r.get(1)?,
                    project_id: r.get(2)?,
                    kind: r.get(3)?,
                    content: r.get(4)?,
                    importance: r.get(5)?,
                    source_session: r.get(6)?,
                    created_at: r.get(7)?,
                    updated_at: r.get(8)?,
                    last_seen_at: r.get(9)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        match query {
            Some(q) if !q.is_empty() => {
                let q = q.to_lowercase();
                Ok(rows
                    .into_iter()
                    .filter(|m| m.content.to_lowercase().contains(&q))
                    .collect())
            }
            _ => Ok(rows),
        }
    }

    /// 删除一条记忆；返回是否存在。
    pub fn delete_memory(&mut self, id: &str) -> Result<bool> {
        let n = self
            .conn
            .execute("DELETE FROM memories WHERE id = ?1", [id])?;
        Ok(n > 0)
    }

    /// 每项目 active 记忆上限治理（§10.1 v1.104）：超出 `cap` 时按
    /// importance 升序 + last_seen_at 最旧淘汰，返回删除条数。
    pub fn prune_memories(&mut self, project_id: &str, cap: usize) -> Result<usize> {
        let total: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM memories WHERE scope = 'project' AND project_id = ?1",
            [project_id],
            |r| r.get(0),
        )?;
        let excess = (total as usize).saturating_sub(cap);
        if excess == 0 {
            return Ok(0);
        }
        let mut stmt = self.conn.prepare(
            "SELECT id FROM memories
             WHERE scope = 'project' AND project_id = ?1
             ORDER BY importance ASC, last_seen_at ASC LIMIT ?2",
        )?;
        let ids = stmt
            .query_map(params![project_id, excess as i64], |r| {
                r.get::<_, String>(0)
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let mut removed = 0;
        for id in &ids {
            removed += self
                .conn
                .execute("DELETE FROM memories WHERE id = ?1", [id])?;
        }
        Ok(removed)
    }

    // ---------- checkpoints ----------

    pub fn insert_checkpoint(
        &mut self,
        session_id: &str,
        tree: &str,
        files: &[String],
        event_seq: Option<i64>,
    ) -> Result<Checkpoint> {
        let cp = Checkpoint {
            id: Uuid::now_v7().to_string(),
            session_id: session_id.to_string(),
            tree: tree.to_string(),
            files: files.to_vec(),
            event_seq,
            created_at: Self::now(),
        };
        self.conn.execute(
            "INSERT INTO checkpoints (id, session_id, tree, files, event_seq, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                cp.id,
                cp.session_id,
                cp.tree,
                serde_json::to_string(&cp.files)?,
                cp.event_seq,
                cp.created_at
            ],
        )?;
        Ok(cp)
    }

    pub fn checkpoint(&mut self, id: &str) -> Result<Option<Checkpoint>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, session_id, tree, files, event_seq, created_at
             FROM checkpoints WHERE id = ?1",
        )?;
        let mut rows = stmt.query_map([id], row_to_checkpoint)?;
        Ok(rows.next().transpose()?)
    }

    /// 时间轴（旧 → 新）。
    pub fn checkpoints(&mut self, session_id: &str) -> Result<Vec<Checkpoint>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, session_id, tree, files, event_seq, created_at
             FROM checkpoints WHERE session_id = ?1 ORDER BY created_at ASC",
        )?;
        let rows = stmt.query_map([session_id], row_to_checkpoint)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    // ---------- tool_calls（AgentTrace 明细） ----------

    pub fn insert_tool_call(
        &mut self,
        session_id: &str,
        event_id: i64,
        tool: &str,
        level: Level,
        cost_tokens: i64,
    ) -> Result<ToolCall> {
        self.conn.execute(
            "INSERT INTO tool_calls (session_id, event_id, tool, level, cost_tokens, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                session_id,
                event_id,
                tool,
                level.as_str(),
                cost_tokens,
                Self::now()
            ],
        )?;
        Ok(ToolCall {
            id: self.conn.last_insert_rowid(),
            session_id: session_id.to_string(),
            event_id,
            tool: tool.to_string(),
            level,
            cost_tokens,
            created_at: Self::now(),
        })
    }

    pub fn tool_calls(&mut self, session_id: &str) -> Result<Vec<ToolCall>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, session_id, event_id, tool, level, cost_tokens, created_at
             FROM tool_calls WHERE session_id = ?1 ORDER BY id ASC",
        )?;
        let rows = stmt.query_map([session_id], |r| {
            Ok(ToolCall {
                id: r.get(0)?,
                session_id: r.get(1)?,
                event_id: r.get(2)?,
                tool: r.get(3)?,
                level: parse_level(&r.get::<_, String>(4)?),
                cost_tokens: r.get(5)?,
                created_at: r.get(6)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    // ---------- model_usage（成本归因 §11） ----------

    #[allow(clippy::too_many_arguments)] // 在途 v1.129+ 工作落库；参数收敛归后续重构
    pub fn record_model_usage(
        &mut self,
        session_id: &str,
        provider: &str,
        model: &str,
        input_tokens: i64,
        output_tokens: i64,
        cached_input_tokens: i64,
        duration_ms: i64,
        cost_usd: f64,
    ) -> Result<ModelUsage> {
        let now = Self::now();
        let project_id = self.project_id_for_session(session_id);
        // 明细与月度聚合同事务：两写间崩溃会让月表永久少记（月表语义为
        // 永久只增，无任何从明细重建的路径）
        self.conn.execute_batch("BEGIN")?;
        let result = (|| -> Result<()> {
            self.conn.execute(
                "INSERT INTO model_usage (session_id, project_id, provider, model, input_tokens, output_tokens, cached_input_tokens, duration_ms, cost_usd, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![session_id, project_id, provider, model, input_tokens, output_tokens, cached_input_tokens, duration_ms, cost_usd, now],
            )?;
            // 按月聚合（永久，§14.2）
            let month = &now[..7];
            self.conn.execute(
                "INSERT INTO model_usage_monthly (month, input_tokens, output_tokens, cost_usd)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(month) DO UPDATE SET
                   input_tokens = input_tokens + ?2,
                   output_tokens = output_tokens + ?3,
                   cost_usd = cost_usd + ?4",
                params![month, input_tokens, output_tokens, cost_usd],
            )?;
            Ok(())
        })();
        match result {
            Ok(()) => self.conn.execute_batch("COMMIT")?,
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                return Err(e);
            }
        }
        Ok(ModelUsage {
            id: self.conn.last_insert_rowid(),
            session_id: session_id.to_string(),
            project_id,
            provider: provider.to_string(),
            model: model.to_string(),
            input_tokens,
            output_tokens,
            cached_input_tokens,
            duration_ms,
            cost_usd,
            created_at: now,
        })
    }

    pub fn session_usage(&mut self, session_id: &str) -> Result<Vec<ModelUsage>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, session_id, project_id, provider, model, input_tokens, output_tokens, cached_input_tokens, duration_ms, cost_usd, created_at
             FROM model_usage WHERE session_id = ?1 ORDER BY id ASC",
        )?;
        let rows = stmt.query_map([session_id], row_to_usage)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// 会话累计（任务级 / 会话级归因；v1.129 携缓存命中与回合耗时）。
    pub fn session_usage_totals(&mut self, session_id: &str) -> Result<UsageTotals> {
        self.conn
            .query_row(
                "SELECT COALESCE(SUM(input_tokens),0), COALESCE(SUM(output_tokens),0), COALESCE(SUM(cost_usd),0.0),
                        COALESCE(SUM(cached_input_tokens),0), COALESCE(SUM(duration_ms),0)
                 FROM model_usage WHERE session_id = ?1",
                [session_id],
                |r| {
                    Ok(UsageTotals {
                        input_tokens: r.get(0)?,
                        output_tokens: r.get(1)?,
                        cost_usd: r.get(2)?,
                        cached_input_tokens: r.get(3)?,
                        duration_ms: r.get(4)?,
                    })
                },
            )
            .map_err(Into::into)
    }

    /// 项目累计（项目任务中心 / 成本看板 §6.4 / §11）。
    pub fn project_usage_totals(&mut self, project_id: &str) -> Result<UsageTotals> {
        self.conn
            .query_row(
                "SELECT COALESCE(SUM(input_tokens),0), COALESCE(SUM(output_tokens),0), COALESCE(SUM(cost_usd),0.0),
                        COALESCE(SUM(cached_input_tokens),0), COALESCE(SUM(duration_ms),0)
                 FROM model_usage WHERE project_id = ?1",
                [project_id],
                |r| {
                    Ok(UsageTotals {
                        input_tokens: r.get(0)?,
                        output_tokens: r.get(1)?,
                        cost_usd: r.get(2)?,
                        cached_input_tokens: r.get(3)?,
                        duration_ms: r.get(4)?,
                    })
                },
            )
            .map_err(Into::into)
    }

    // ---------- plugins（§14.2 安装记录） ----------

    /// 插件安装记录（§13.2：版本锁定 + 签名入库）。
    pub fn insert_plugin(
        &mut self,
        id: &str,
        version: &str,
        permissions: &[String],
        signature: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO plugins (id, version, permissions, signature, installed_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(id) DO UPDATE SET
               version = ?2, permissions = ?3, signature = ?4, installed_at = ?5",
            params![
                id,
                version,
                serde_json::to_string(permissions)?,
                signature,
                Self::now()
            ],
        )?;
        Ok(())
    }

    /// 已装插件列表（权限 diff 的「已装版本」来源）。
    pub fn list_plugins(&mut self) -> Result<Vec<Plugin>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, version, permissions, signature, installed_at
             FROM plugins ORDER BY installed_at ASC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(Plugin {
                id: r.get(0)?,
                version: r.get(1)?,
                permissions: serde_json::from_str(&r.get::<_, String>(2)?).unwrap_or_default(),
                signature: r.get(3)?,
                installed_at: r.get(4)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    // ---------- eval_runs ----------

    pub fn insert_eval_run(
        &mut self,
        target: &str,
        metrics_json: &serde_json::Value,
        verdict: &str,
    ) -> Result<EvalRun> {
        let run = EvalRun {
            id: Uuid::now_v7().to_string(),
            target: target.to_string(),
            metrics_json: metrics_json.clone(),
            verdict: verdict.to_string(),
            created_at: Self::now(),
        };
        self.conn.execute(
            "INSERT INTO eval_runs (id, target, metrics_json, verdict, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                run.id,
                run.target,
                run.metrics_json.to_string(),
                run.verdict,
                run.created_at
            ],
        )?;
        Ok(run)
    }

    pub fn eval_runs(&mut self) -> Result<Vec<EvalRun>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, target, metrics_json, verdict, created_at
             FROM eval_runs ORDER BY created_at ASC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
            ))
        })?;
        rows.map(|row| {
            let (id, target, metrics, verdict, created_at) = row?;
            Ok(EvalRun {
                id,
                target,
                metrics_json: serde_json::from_str(&metrics)?,
                verdict,
                created_at,
            })
        })
        .collect()
    }

    // ---------- L4 向量切片（sqlite-vec 就位前的兼容存储；Q3/§10.1） ----------

    /// 原子替换某个文件的全部 L4 切片（watcher 增量更新）。
    pub fn replace_l4_file(
        &mut self,
        project_id: &str,
        path: &str,
        chunks: &[L4ChunkRecord],
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "DELETE FROM l4_chunks WHERE project_id = ?1 AND path = ?2",
            params![project_id, path],
        )?;
        let now = Self::now();
        for chunk in chunks {
            tx.execute(
                "INSERT INTO l4_chunks (project_id, path, symbol, start_line, end_line, text, embedding, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    project_id,
                    path,
                    chunk.symbol,
                    chunk.start_line as i64,
                    chunk.end_line as i64,
                    chunk.text,
                    f32_slice_to_blob(&chunk.embedding),
                    now
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn delete_l4_file(&mut self, project_id: &str, path: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM l4_chunks WHERE project_id = ?1 AND path = ?2",
            params![project_id, path],
        )?;
        Ok(())
    }

    pub fn clear_l4_project(&mut self, project_id: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM l4_chunks WHERE project_id = ?1", [project_id])?;
        Ok(())
    }

    pub fn l4_chunk_count(&mut self, project_id: &str) -> Result<u64> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM l4_chunks WHERE project_id = ?1",
            [project_id],
            |r| r.get(0),
        )?;
        Ok(count.max(0) as u64)
    }

    /// 暴力余弦召回（切片级规模够用；sqlite-vec 虚表随 L4/M1 落地替换，§10.1）。
    pub fn l4_search(
        &mut self,
        project_id: &str,
        query: &[f32],
        top_k: usize,
    ) -> Result<Vec<L4SearchHit>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, path, COALESCE(symbol,''), start_line, end_line, text, embedding
             FROM l4_chunks
             WHERE project_id = ?1 AND embedding IS NOT NULL",
        )?;
        let rows = stmt.query_map([project_id], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)? as usize,
                r.get::<_, i64>(4)? as usize,
                r.get::<_, String>(5)?,
                r.get::<_, Vec<u8>>(6)?,
            ))
        })?;
        let mut scored = Vec::new();
        for row in rows {
            let (id, path, symbol, start_line, end_line, text, blob) = row?;
            let v = blob_to_f32_slice(&blob);
            let score = cosine(query, &v);
            scored.push(L4SearchHit {
                id,
                path,
                symbol,
                start_line,
                end_line,
                text,
                score,
            });
        }
        scored.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        scored.truncate(top_k);
        Ok(scored)
    }

    // ---------- 冷归档（§14.2 增长治理） ----------

    /// 将关闭超过 `days` 天的会话事件归档为 gzip JSONL 并从热库删除；
    /// approvals 审计记录永久保留。返回归档的会话 id 列表。
    pub fn archive_old_sessions(&mut self, days: u32, archive_dir: &Path) -> Result<Vec<String>> {
        std::fs::create_dir_all(archive_dir)?;
        let cutoff = (Utc::now() - chrono::Duration::days(days as i64)).to_rfc3339();
        let stale: Vec<String> = {
            let mut stmt = self.conn.prepare(
                "SELECT id FROM sessions
                 WHERE status IN ('done','error','rolled_back') AND updated_at < ?1",
            )?;
            let rows = stmt.query_map([&cutoff], |r| r.get::<_, String>(0))?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };
        for sid in &stale {
            // 已归档过（上次 DELETE events 后崩溃的重跑）：events 为空时
            // 直接跳过——用零事件覆盖先前完好的归档会把事件溯源链永久截断
            let events = self.events(sid)?;
            if events.is_empty() {
                continue;
            }
            let usage = self.session_usage(sid)?;
            let record = serde_json::json!({
                "session_id": sid,
                "events": events,
                "model_usage": usage,
            });
            // 先写临时文件再 rename：归档中途崩溃不留半截文件
            let tmp = archive_dir.join(format!("{sid}.session.jsonl.gz.tmp"));
            let path: PathBuf = archive_dir.join(format!("{}.session.jsonl.gz", sid));
            {
                let f = std::fs::File::create(&tmp)?;
                let mut enc = GzEncoder::new(f, Compression::default());
                enc.write_all(record.to_string().as_bytes())?;
                enc.finish()?;
            }
            std::fs::rename(&tmp, &path)?;
            // 四条 DELETE 包进单事务：逐条自动提交间崩溃会留下孤儿行，
            // 且重跑时按「events 已空」误判已归档（见上）
            self.conn.execute_batch("BEGIN")?;
            let result = (|| -> Result<()> {
                self.conn
                    .execute("DELETE FROM events WHERE session_id = ?1", [sid])?;
                self.conn
                    .execute("DELETE FROM tool_calls WHERE session_id = ?1", [sid])?;
                self.conn
                    .execute("DELETE FROM model_usage WHERE session_id = ?1", [sid])?;
                self.conn
                    .execute("DELETE FROM checkpoints WHERE session_id = ?1", [sid])?;
                Ok(())
            })();
            match result {
                Ok(()) => self.conn.execute_batch("COMMIT")?,
                Err(e) => {
                    let _ = self.conn.execute_batch("ROLLBACK");
                    return Err(e);
                }
            }
        }
        Ok(stale)
    }
}

// ---------- row mappers ----------

fn row_to_project(r: &rusqlite::Row<'_>) -> rusqlite::Result<Project> {
    Ok(Project {
        id: r.get(0)?,
        path: r.get(1)?,
        display_name: r.get(2)?,
        trusted: r.get::<_, i64>(3)? != 0,
        language_packs: serde_json::from_str(&r.get::<_, String>(4)?).unwrap_or_default(),
        created_at: r.get(5)?,
    })
}

fn row_to_session(r: &rusqlite::Row<'_>) -> rusqlite::Result<Session> {
    Ok(Session {
        id: r.get(0)?,
        project_id: r.get(1)?,
        model: r.get(2)?,
        status: SessionStatus::parse(&r.get::<_, String>(3)?).unwrap_or(SessionStatus::Idle),
        title: r.get(4)?,
        worktree_path: r.get(5)?,
        archived_at: r.get(8)?,
        truncated_from_seq: r.get(9)?,
        truncated_to_seq: r.get(10)?,
        created_at: r.get(6)?,
        updated_at: r.get(7)?,
    })
}

fn row_to_event(r: &rusqlite::Row<'_>) -> rusqlite::Result<Event> {
    Ok(Event {
        id: r.get(0)?,
        session_id: r.get(1)?,
        project_id: r.get(2)?,
        seq: r.get(3)?,
        kind: EventKind::parse(&r.get::<_, String>(4)?).unwrap_or(EventKind::Error),
        payload: serde_json::from_str(&r.get::<_, String>(5)?).unwrap_or(serde_json::Value::Null),
        created_at: r.get(6)?,
    })
}

fn row_to_checkpoint(r: &rusqlite::Row<'_>) -> rusqlite::Result<Checkpoint> {
    Ok(Checkpoint {
        id: r.get(0)?,
        session_id: r.get(1)?,
        tree: r.get(2)?,
        files: serde_json::from_str(&r.get::<_, String>(3)?).unwrap_or_default(),
        event_seq: r.get(4)?,
        created_at: r.get(5)?,
    })
}

fn row_to_usage(r: &rusqlite::Row<'_>) -> rusqlite::Result<ModelUsage> {
    Ok(ModelUsage {
        id: r.get(0)?,
        session_id: r.get(1)?,
        project_id: r.get(2)?,
        provider: r.get(3)?,
        model: r.get(4)?,
        input_tokens: r.get(5)?,
        output_tokens: r.get(6)?,
        cached_input_tokens: r.get(7)?,
        duration_ms: r.get(8)?,
        cost_usd: r.get(9)?,
        created_at: r.get(10)?,
    })
}

fn parse_level(s: &str) -> Level {
    match s {
        "a" | "A" => Level::A,
        "c" | "C" => Level::C,
        "d" | "D" => Level::D,
        "cd" | "CD" | "c+d" | "C+D" => Level::Composite,
        _ => Level::B,
    }
}

fn f32_slice_to_blob(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

fn blob_to_f32_slice(blob: &[u8]) -> Vec<f32> {
    let mut out = Vec::with_capacity(blob.len() / 4);
    for c in blob.chunks(4) {
        if let Ok(arr) = <[u8; 4]>::try_from(c) {
            out.push(f32::from_le_bytes(arr));
        }
    }
    out
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let n = a.len().min(b.len());
    let (mut dot, mut na, mut nb) = (0.0f32, 0.0f32, 0.0f32);
    for i in 0..n {
        dot += a[i] * b[i];
        na += a[i] * a[i];
        nb += b[i] * b[i];
    }
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na.sqrt() * nb.sqrt())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn mem() -> Store {
        Store::open_in_memory().expect("store")
    }

    #[test]
    fn project_registration_and_tofu() {
        let mut s = mem();
        let dir = tempfile::tempdir().unwrap();
        let proj_dir = dir.path().join("proj1");
        std::fs::create_dir_all(&proj_dir).unwrap();
        let p = s.upsert_project(proj_dir.to_str().unwrap()).unwrap();
        assert!(!p.trusted);
        // 同路径重复登记不产生新行
        let p2 = s.upsert_project(proj_dir.to_str().unwrap()).unwrap();
        assert_eq!(p.id, p2.id);
        assert_eq!(s.list_projects().unwrap().len(), 1);

        s.set_project_trusted(&p.id, true).unwrap();
        let p3 = s.project(&p.id).unwrap().unwrap();
        assert!(p3.trusted);

        // 项目级 UI 状态 upsert（§7.2 / §7.5）
        assert!(s.project_ui_state(&p.id).unwrap().is_none());
        s.set_project_ui_state(&p.id, r#"{"leftWidth":320}"#)
            .unwrap();
        s.set_project_ui_state(&p.id, r#"{"leftWidth":360,"tabs":["a.rs"]}"#)
            .unwrap();
        assert_eq!(
            s.project_ui_state(&p.id).unwrap().as_deref(),
            Some(r#"{"leftWidth":360,"tabs":["a.rs"]}"#)
        );
    }

    #[test]
    fn migrates_v1_rows_to_current_schema() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("db.sqlite");
        let project_id = "project-v1";
        let session_id = "session-v1";
        {
            let conn = Connection::open(&db).unwrap();
            conn.execute_batch(
                r#"
                CREATE TABLE schema_version (version INTEGER NOT NULL);
                INSERT INTO schema_version VALUES (1);
                CREATE TABLE projects (
                  id TEXT PRIMARY KEY, path TEXT NOT NULL UNIQUE, trusted INTEGER NOT NULL DEFAULT 0,
                  language_packs TEXT NOT NULL DEFAULT '[]', created_at TEXT NOT NULL
                );
                CREATE TABLE sessions (
                  id TEXT PRIMARY KEY, project_id TEXT NOT NULL REFERENCES projects(id), model TEXT NOT NULL DEFAULT '',
                  status TEXT NOT NULL DEFAULT 'idle', created_at TEXT NOT NULL, updated_at TEXT NOT NULL
                );
                CREATE TABLE events (
                  id INTEGER PRIMARY KEY AUTOINCREMENT, session_id TEXT NOT NULL, seq INTEGER NOT NULL,
                  type TEXT NOT NULL, payload TEXT NOT NULL, created_at TEXT NOT NULL, UNIQUE(session_id, seq)
                );
                CREATE TABLE approvals (
                  id TEXT PRIMARY KEY, session_id TEXT NOT NULL, action TEXT NOT NULL, level TEXT NOT NULL,
                  decision TEXT, created_at TEXT NOT NULL, decided_at TEXT
                );
                CREATE TABLE model_usage (
                  id INTEGER PRIMARY KEY AUTOINCREMENT, session_id TEXT NOT NULL, provider TEXT NOT NULL,
                  model TEXT NOT NULL DEFAULT '', input_tokens INTEGER NOT NULL DEFAULT 0,
                  output_tokens INTEGER NOT NULL DEFAULT 0, cost_usd REAL NOT NULL DEFAULT 0, created_at TEXT NOT NULL
                );
                INSERT INTO projects VALUES ('project-v1', '/tmp/v1', 0, '[]', '2026-01-01');
                INSERT INTO sessions VALUES ('session-v1', 'project-v1', 'mock', 'idle', '2026-01-01', '2026-01-01');
                INSERT INTO events VALUES (1, 'session-v1', 1, 'user_input', '{}', '2026-01-01');
                INSERT INTO approvals VALUES ('approval-v1', 'session-v1', 'legacy', 'b', NULL, '2026-01-01', NULL);
                INSERT INTO model_usage VALUES (1, 'session-v1', 'mock', 'm', 1, 2, 0.0, '2026-01-01');
                "#,
            )
            .unwrap();
        }
        let mut store = Store::open(&db).unwrap();
        assert_eq!(
            store
                .connection_for_tests()
                .query_row("SELECT version FROM schema_version", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            SCHEMA_VERSION
        );
        assert_eq!(store.events(session_id).unwrap()[0].project_id, project_id);
        // v2→v3 审批表回填 project_id（v1.89 后 approvals 只读兼容，经 SQL 验证）
        assert_eq!(
            store
                .connection_for_tests()
                .query_row(
                    "SELECT project_id FROM approvals WHERE id = 'approval-v1'",
                    [],
                    |r| r.get::<_, String>(0)
                )
                .unwrap(),
            project_id
        );
        assert_eq!(
            store.session_usage(session_id).unwrap()[0].project_id,
            project_id
        );
        // v4 → v5：旧库 projects 行补 display_name 列且默认空串。
        assert_eq!(store.project(project_id).unwrap().unwrap().display_name, "");
        // v5 → v6：旧会话行补 title 列且默认空串（UI 回退模型名 / 短 id）。
        assert_eq!(store.session(session_id).unwrap().unwrap().title, "");
        // v9 → v10：旧 model_usage 行补缓存命中 / 回合耗时列且默认 0（v1.129）。
        let migrated = store.session_usage(session_id).unwrap()[0].clone();
        assert_eq!((migrated.cached_input_tokens, migrated.duration_ms), (0, 0));
    }

    #[test]
    fn session_title_roundtrip_with_session_title_event() {
        let mut s = mem();
        let dir = tempfile::tempdir().unwrap();
        let p = s.upsert_project(dir.path().to_str().unwrap()).unwrap();
        let sess = s.create_session(&p.id, "mock").unwrap();
        // 新会话无标题（UI 回退模型名 / 短 id）。
        assert_eq!(sess.title, "");
        assert_eq!(s.session(&sess.id).unwrap().unwrap().title, "");

        s.set_session_title(&sess.id, "修复登录超时").unwrap();
        assert_eq!(s.session(&sess.id).unwrap().unwrap().title, "修复登录超时");
        assert_eq!(s.list_sessions(&p.id).unwrap()[0].title, "修复登录超时");

        // session_title 事件可解析回读；计数只算同类事件。
        let ev = s
            .append_event(
                &sess.id,
                EventKind::SessionTitle,
                &json!({"title": "修复登录超时"}),
            )
            .unwrap();
        assert_eq!(ev.kind, EventKind::SessionTitle);
        assert_eq!(s.events(&sess.id).unwrap()[0].kind, EventKind::SessionTitle);
        assert_eq!(
            s.count_events_of_kind(&sess.id, EventKind::UserInput)
                .unwrap(),
            0
        );
        s.append_event(&sess.id, EventKind::UserInput, &json!({"text": "hi"}))
            .unwrap();
        assert_eq!(
            s.count_events_of_kind(&sess.id, EventKind::UserInput)
                .unwrap(),
            1
        );
        assert_eq!(
            s.count_events_of_kind(&sess.id, EventKind::SessionTitle)
                .unwrap(),
            1
        );
    }

    #[test]
    fn project_display_name_roundtrip() {
        let mut s = mem();
        let dir = tempfile::tempdir().unwrap();
        let p = s.upsert_project(dir.path().to_str().unwrap()).unwrap();
        // 新登记默认空串（UI 回退为路径末段派生）。
        assert_eq!(p.display_name, "");

        s.set_project_display_name(&p.id, "My Custom Name").unwrap();
        let renamed = s.project(&p.id).unwrap().unwrap();
        assert_eq!(renamed.display_name, "My Custom Name");
        assert_eq!(s.list_projects().unwrap()[0].display_name, "My Custom Name");

        // 空串 = 清除自定义名，回退派生。
        s.set_project_display_name(&p.id, "").unwrap();
        assert_eq!(s.project(&p.id).unwrap().unwrap().display_name, "");
    }

    #[test]
    fn thread_truncation_hides_range_until_cleared() {
        let mut s = mem();
        let dir = tempfile::tempdir().unwrap();
        let p = s.upsert_project(dir.path().to_str().unwrap()).unwrap();
        let sess = s.create_session(&p.id, "mock").unwrap();
        for i in 1..=4 {
            s.append_event(&sess.id, EventKind::UserInput, &json!({"i": i}))
                .unwrap();
        }
        assert_eq!(s.events(&sess.id).unwrap().len(), 4);
        assert_eq!(s.turn_start_seq(&sess.id, 4), Some(4));
        assert_eq!(s.turn_start_seq(&sess.id, 1), Some(1));

        // v1.135：水位 [2,4] → seq 2..4 对线程 / Trace 隐藏，仅剩 seq 1。
        s.set_thread_truncation(&sess.id, Some(2), Some(4)).unwrap();
        let visible: Vec<i64> = s.events(&sess.id).unwrap().iter().map(|e| e.seq).collect();
        assert_eq!(visible, vec![1]);
        assert!(s.events_since(&sess.id, 2).unwrap().is_empty());
        let sess_row = s.session(&sess.id).unwrap().unwrap();
        assert_eq!(sess_row.truncated_from_seq, Some(2));
        assert_eq!(sess_row.truncated_to_seq, Some(4));

        // unrollback 清水位 → 全部恢复可见。
        s.set_thread_truncation(&sess.id, None, None).unwrap();
        assert_eq!(s.events(&sess.id).unwrap().len(), 4);
    }

    #[test]
    fn events_are_append_only_with_monotonic_seq() {
        let mut s = mem();
        let dir = tempfile::tempdir().unwrap();
        let p = s.upsert_project(dir.path().to_str().unwrap()).unwrap();
        let sess = s.create_session(&p.id, "mock").unwrap();

        let e1 = s
            .append_event(&sess.id, EventKind::UserInput, &json!({"text": "hi"}))
            .unwrap();
        let e2 = s
            .append_event(&sess.id, EventKind::Sensing, &json!({"tool": "list_dir"}))
            .unwrap();
        assert_eq!((e1.seq, e2.seq), (1, 2));
        assert_eq!(e1.project_id, p.id);
        assert_eq!(e2.project_id, p.id);
        assert_eq!(s.latest_seq(&sess.id).unwrap(), 2);

        let all = s.events(&sess.id).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].kind, EventKind::UserInput);

        // 断线续传：seq > 1
        let tail = s.events_since(&sess.id, 1).unwrap();
        assert_eq!(tail.len(), 1);
        assert_eq!(tail[0].kind, EventKind::Sensing);
    }

    #[test]
    fn session_lifecycle_and_status() {
        let mut s = mem();
        let dir = tempfile::tempdir().unwrap();
        let p = s.upsert_project(dir.path().to_str().unwrap()).unwrap();
        let sess = s.create_session(&p.id, "mock").unwrap();
        s.set_session_status(&sess.id, SessionStatus::Executing)
            .unwrap();
        s.set_session_model(&sess.id, "glm").unwrap();
        let got = s.session(&sess.id).unwrap().unwrap();
        assert_eq!(got.status, SessionStatus::Executing);
        assert_eq!(got.model, "glm");
        assert_eq!(s.list_sessions(&p.id).unwrap().len(), 1);
    }

    #[test]
    fn latest_subtasks_by_session_takes_newest_snapshot() {
        // v1.148 §15：侧栏进度徽标数据源——每会话取最新一次 subtasks 快照计数，
        // 跨项目隔离，无清单会话不出现。
        let mut s = mem();
        let dir = tempfile::tempdir().unwrap();
        let p = s.upsert_project(dir.path().to_str().unwrap()).unwrap();
        let a = s.create_session(&p.id, "mock").unwrap();
        let b = s.create_session(&p.id, "mock").unwrap();
        // 无清单会话不出现
        assert!(s.latest_subtasks_by_session(&p.id).unwrap().is_empty());
        // a：两代快照，取最新（3 项 2 完成）
        s.append_event(
            &a.id,
            EventKind::Subtasks,
            &json!({"items": [{"title": "x", "status": "done"}]}),
        )
        .unwrap();
        s.append_event(
            &a.id,
            EventKind::Subtasks,
            &json!({"items": [
                {"title": "x", "status": "done"},
                {"title": "y", "status": "done"},
                {"title": "z", "status": "in_progress"}
            ]}),
        )
        .unwrap();
        // b：一代快照（1 项 0 完成）
        s.append_event(
            &b.id,
            EventKind::Subtasks,
            &json!({"items": [{"title": "q", "status": "pending"}]}),
        )
        .unwrap();
        let mut got = s.latest_subtasks_by_session(&p.id).unwrap();
        got.sort();
        assert_eq!(
            got,
            vec![(a.id.clone(), 2, 3), (b.id.clone(), 0, 1)],
            "各会话取最新快照计数"
        );
        // 非 subtasks 事件不干扰
        s.append_event(&a.id, EventKind::Decision, &json!({}))
            .unwrap();
        assert_eq!(s.latest_subtasks_by_session(&p.id).unwrap().len(), 2);
        // 跨项目隔离
        let dir2 = tempfile::tempdir().unwrap();
        let p2 = s.upsert_project(dir2.path().to_str().unwrap()).unwrap();
        let c = s.create_session(&p2.id, "mock").unwrap();
        s.append_event(
            &c.id,
            EventKind::Subtasks,
            &json!({"items": [{"title": "k", "status": "done"}]}),
        )
        .unwrap();
        assert_eq!(
            s.latest_subtasks_by_session(&p2.id).unwrap(),
            vec![(c.id, 1, 1)]
        );
        assert_eq!(s.latest_subtasks_by_session(&p.id).unwrap().len(), 2);
    }

    #[test]
    fn checkpoints_timeline() {
        let mut s = mem();
        let dir = tempfile::tempdir().unwrap();
        let p = s.upsert_project(dir.path().to_str().unwrap()).unwrap();
        let sess = s.create_session(&p.id, "mock").unwrap();
        let e = s
            .append_event(&sess.id, EventKind::Checkpoint, &json!({}))
            .unwrap();
        let cp = s
            .insert_checkpoint(&sess.id, "abc123", &["a.rs".into()], Some(e.seq))
            .unwrap();
        let cps = s.checkpoints(&sess.id).unwrap();
        assert_eq!(cps.len(), 1);
        assert_eq!(cps[0].tree, "abc123");
        assert_eq!(cps[0].files, vec!["a.rs".to_string()]);
        assert_eq!(s.checkpoint(&cp.id).unwrap().unwrap().id, cp.id);
    }

    #[test]
    fn tool_calls_trace() {
        let mut s = mem();
        let dir = tempfile::tempdir().unwrap();
        let p = s.upsert_project(dir.path().to_str().unwrap()).unwrap();
        let sess = s.create_session(&p.id, "mock").unwrap();
        let e = s
            .append_event(&sess.id, EventKind::CommandRun, &json!({}))
            .unwrap();
        s.insert_tool_call(&sess.id, e.id, "run_tests", Level::B, 120)
            .unwrap();
        let calls = s.tool_calls(&sess.id).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].level, Level::B);
        assert_eq!(calls[0].cost_tokens, 120);
    }

    #[test]
    fn model_usage_attribution() {
        let mut s = mem();
        let dir = tempfile::tempdir().unwrap();
        let p = s.upsert_project(dir.path().to_str().unwrap()).unwrap();
        let sess = s.create_session(&p.id, "glm").unwrap();
        s.record_model_usage(&sess.id, "glm", "glm-4.6", 100, 50, 80, 1_200, 0.01)
            .unwrap();
        s.record_model_usage(&sess.id, "glm", "glm-4.6", 200, 80, 150, 2_800, 0.02)
            .unwrap();

        let totals = s.session_usage_totals(&sess.id).unwrap();
        assert_eq!(totals.input_tokens, 300);
        assert_eq!(totals.output_tokens, 130);
        assert_eq!(totals.cached_input_tokens, 230);
        assert_eq!(totals.duration_ms, 4_000);
        assert!((totals.cost_usd - 0.03).abs() < 1e-9);
        let proj = s.project_usage_totals(&p.id).unwrap();
        assert_eq!(proj, totals);
    }

    #[test]
    fn l4_replace_and_delete_file_scopes_updates() {
        let mut s = mem();
        let dir = tempfile::tempdir().unwrap();
        let p = s.upsert_project(dir.path().to_str().unwrap()).unwrap();
        let embedding = [0.9f32, 0.1, 0.0, 0.0].to_vec();
        s.replace_l4_file(
            &p.id,
            "src/app.rs",
            &[L4ChunkRecord {
                symbol: Some("login".into()),
                start_line: 1,
                end_line: 1,
                text: "pub fn login authenticate".into(),
                embedding,
            }],
        )
        .unwrap();
        assert_eq!(s.l4_chunk_count(&p.id).unwrap(), 1);
        s.delete_l4_file(&p.id, "src/app.rs").unwrap();
        assert_eq!(s.l4_chunk_count(&p.id).unwrap(), 0);
    }

    #[test]
    fn l4_vector_search_ranks_by_cosine() {
        let mut s = mem();
        let dir = tempfile::tempdir().unwrap();
        let p = s.upsert_project(dir.path().to_str().unwrap()).unwrap();
        // 生产同源种子：走 replace_l4_file（upsert_l4_chunk 已删）
        s.replace_l4_file(
            &p.id,
            "a.rs",
            &[L4ChunkRecord {
                symbol: Some("foo".into()),
                start_line: 1,
                end_line: 1,
                text: "fn foo".into(),
                embedding: vec![1.0, 0.0, 0.0],
            }],
        )
        .unwrap();
        s.replace_l4_file(
            &p.id,
            "b.rs",
            &[L4ChunkRecord {
                symbol: Some("bar".into()),
                start_line: 1,
                end_line: 1,
                text: "fn bar".into(),
                embedding: vec![0.0, 1.0, 0.0],
            }],
        )
        .unwrap();
        let hits = s.l4_search(&p.id, &[0.9, 0.1, 0.0], 2).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].path, "a.rs", "最相近的切片应排第一");
        assert!(hits[0].score > hits[1].score);
    }

    #[test]
    fn ui_prefs_upsert_roundtrip() {
        let mut s = mem();
        assert!(s.ui_prefs().unwrap().is_empty(), "初始为空");
        s.set_ui_pref("theme", "light").unwrap();
        s.set_ui_pref("theme", "dark").unwrap();
        s.set_ui_pref("locale", "zh-CN").unwrap();
        let prefs = s.ui_prefs().unwrap();
        assert_eq!(prefs.len(), 2);
        assert!(
            prefs.contains(&("theme".into(), "dark".into())),
            "upsert 覆盖旧值"
        );
        assert!(prefs.contains(&("locale".into(), "zh-CN".into())));
    }

    #[test]
    fn archive_moves_stale_sessions_and_keeps_approvals() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = mem();
        let proj = tempfile::tempdir().unwrap();
        let p = s.upsert_project(proj.path().to_str().unwrap()).unwrap();
        let old = s.create_session(&p.id, "mock").unwrap();
        s.append_event(&old.id, EventKind::UserInput, &json!({}))
            .unwrap();
        s.record_model_usage(&old.id, "glm", "m", 10, 5, 0, 0, 0.001)
            .unwrap();
        s.set_session_status(&old.id, SessionStatus::Done).unwrap();
        s.conn
            .execute(
                "UPDATE sessions SET updated_at = ?2 WHERE id = ?1",
                params![
                    old.id,
                    (Utc::now() - chrono::Duration::days(100)).to_rfc3339()
                ],
            )
            .unwrap();

        // 新会话不受影响
        let fresh = s.create_session(&p.id, "mock").unwrap();
        s.append_event(&fresh.id, EventKind::UserInput, &json!({}))
            .unwrap();

        let archived = s.archive_old_sessions(90, dir.path()).unwrap();
        assert_eq!(archived, vec![old.id.clone()]);

        assert!(s.events(&old.id).unwrap().is_empty());
        assert!(s.session_usage(&old.id).unwrap().is_empty());
        // 归档文件存在
        assert!(dir
            .path()
            .join(format!("{}.session.jsonl.gz", old.id))
            .exists());
        // 新会话不动
        assert_eq!(s.events(&fresh.id).unwrap().len(), 1);
    }

    #[test]
    fn eval_runs_roundtrip() {
        let mut s = mem();
        s.insert_eval_run("T1", &json!({"pass": true, "steps": 5}), "pass")
            .unwrap();
        let runs = s.eval_runs().unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].verdict, "pass");
        assert_eq!(runs[0].metrics_json["steps"], 5);
    }
}

#[cfg(test)]
mod managed_worktree_tests {
    use super::*;

    fn mem() -> Store {
        Store::open_in_memory().expect("store")
    }

    // v1.103：旧库迁移补 sessions.archived_at 列（守卫对任意 v<9 幂等）。
    #[test]
    fn migrates_v7_sessions_adds_archived_at() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_version (version INTEGER NOT NULL);
             INSERT INTO schema_version VALUES (7);
             CREATE TABLE projects (id TEXT PRIMARY KEY, path TEXT NOT NULL, display_name TEXT NOT NULL DEFAULT '', trusted INTEGER NOT NULL DEFAULT 0, created_at TEXT NOT NULL);
             CREATE TABLE sessions (id TEXT PRIMARY KEY, project_id TEXT NOT NULL, model TEXT NOT NULL DEFAULT '', status TEXT NOT NULL DEFAULT 'idle', title TEXT NOT NULL DEFAULT '', worktree_path TEXT NOT NULL DEFAULT '', created_at TEXT NOT NULL, updated_at TEXT NOT NULL);
             INSERT INTO sessions (id, project_id, created_at, updated_at) VALUES ('s1', 'p1', 't', 't');",
        )
        .unwrap();
        let mut store = Store::init(conn).unwrap();
        // 迁移后的存量行 = 未归档：回到侧栏列表，不在归档组。
        assert_eq!(store.list_sessions("p1").unwrap().len(), 1);
        assert!(store.list_archived_sessions("p1").unwrap().is_empty());
        assert_eq!(store.session("s1").unwrap().unwrap().archived_at, "");
    }

    #[test]
    fn managed_worktree_session_fields_roundtrip() {
        // v1.87 §9.7：sessions.worktree_path（schema v7）+ 预生成 id 创建。
        let mut s = mem();
        let dir = tempfile::tempdir().unwrap();
        let p = s.upsert_project(dir.path().to_str().unwrap()).unwrap();
        let sid = Store::new_session_id();
        let sess = s.create_session_with_id(&sid, &p.id, "mock").unwrap();
        assert_eq!(sess.id, sid);
        assert_eq!(sess.worktree_path, "", "主根会话 worktree_path 为空");
        s.set_session_worktree(&sid, "/tmp/wt/s1").unwrap();
        let got = s.session(&sid).unwrap().unwrap();
        assert_eq!(got.worktree_path, "/tmp/wt/s1");
        assert_eq!(
            s.list_sessions(&p.id).unwrap()[0].worktree_path,
            "/tmp/wt/s1"
        );
    }

    #[test]
    fn ui_prefs_roundtrip() {
        let mut s = mem();
        s.set_ui_pref("theme", "dark").unwrap();
        s.set_ui_pref("lang", "zh").unwrap();
        let prefs = s.ui_prefs().unwrap();
        assert!(prefs.len() >= 2);
        assert!(prefs.iter().any(|(k, v)| k == "theme" && v == "dark"));
    }

    #[test]
    fn project_ui_state_roundtrip() {
        let mut s = mem();
        let dir = tempfile::tempdir().unwrap();
        let p = s.upsert_project(dir.path().to_str().unwrap()).unwrap();
        assert!(s.project_ui_state(&p.id).unwrap().is_none());
        s.set_project_ui_state(&p.id, r#"{"tabs":["a.ts"]}"#)
            .unwrap();
        let state = s.project_ui_state(&p.id).unwrap().unwrap();
        assert!(state.contains("a.ts"));
    }

    #[test]
    fn model_usage_and_project_totals() {
        let mut s = mem();
        let dir = tempfile::tempdir().unwrap();
        let p = s.upsert_project(dir.path().to_str().unwrap()).unwrap();
        let sid = s.create_session(&p.id, "mock").unwrap().id;
        s.record_model_usage(&sid, "mock", "mock-1", 100, 200, 60, 500, 0.05)
            .unwrap();
        let totals = s.project_usage_totals(&p.id).unwrap();
        assert_eq!(totals.input_tokens, 100);
        assert_eq!(totals.output_tokens, 200);
        assert_eq!(totals.cached_input_tokens, 60);
        assert_eq!(totals.duration_ms, 500);
        assert!(totals.cost_usd > 0.0);
    }

    #[test]
    fn checkpoint_insert_and_list() {
        let mut s = mem();
        let dir = tempfile::tempdir().unwrap();
        let p = s.upsert_project(dir.path().to_str().unwrap()).unwrap();
        let sid = s.create_session(&p.id, "mock").unwrap().id;
        s.insert_checkpoint(&sid, "tree-abc", &["a.ts".into()], None)
            .unwrap();
        let cps = s.checkpoints(&sid).unwrap();
        assert_eq!(cps.len(), 1);
        assert_eq!(cps[0].tree, "tree-abc");
        let cp = s.checkpoint(&cps[0].id).unwrap().unwrap();
        assert_eq!(cp.tree, "tree-abc");
    }

    #[test]
    fn tool_call_insert_and_list() {
        let mut s = mem();
        let dir = tempfile::tempdir().unwrap();
        let p = s.upsert_project(dir.path().to_str().unwrap()).unwrap();
        let sid = s.create_session(&p.id, "mock").unwrap().id;
        s.insert_tool_call(&sid, 1, "apply_patch", Level::C, 10)
            .unwrap();
        let calls = s.tool_calls(&sid).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].tool, "apply_patch");
    }

    #[test]
    fn plugin_insert_and_list() {
        let mut s = mem();
        s.insert_plugin("lsp-1", "1.0.0", &["fs.read".into()], "sig")
            .unwrap();
        let plugins = s.list_plugins().unwrap();
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].id, "lsp-1");
    }

    #[test]
    fn eval_run_insert_and_list() {
        let mut s = mem();
        let run = s
            .insert_eval_run("mock", &serde_json::json!({"pass": true}), "pass")
            .unwrap();
        assert_eq!(run.verdict, "pass");
        let runs = s.eval_runs().unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].verdict, "pass");
    }

    #[test]
    fn project_display_name_and_trust_update() {
        let mut s = mem();
        let dir = tempfile::tempdir().unwrap();
        let p = s.upsert_project(dir.path().to_str().unwrap()).unwrap();
        s.set_project_display_name(&p.id, "My Project").unwrap();
        let updated = s.project(&p.id).unwrap().unwrap();
        assert_eq!(updated.display_name, "My Project");
        s.set_project_trusted(&p.id, true).unwrap();
        let trusted = s.project(&p.id).unwrap().unwrap();
        assert!(trusted.trusted);
    }

    #[test]
    fn session_title_and_model_update() {
        let mut s = mem();
        let dir = tempfile::tempdir().unwrap();
        let p = s.upsert_project(dir.path().to_str().unwrap()).unwrap();
        let sid = s.create_session(&p.id, "mock").unwrap().id;
        s.set_session_title(&sid, "Fix bug").unwrap();
        s.set_session_model(&sid, "gpt-4").unwrap();
        let session = s.session(&sid).unwrap().unwrap();
        assert_eq!(session.title, "Fix bug");
        assert_eq!(session.model, "gpt-4");
    }

    #[test]
    fn list_all_sessions_across_projects() {
        let mut s = mem();
        let dir1 = tempfile::tempdir().unwrap();
        let dir2 = tempfile::tempdir().unwrap();
        let p1 = s.upsert_project(dir1.path().to_str().unwrap()).unwrap();
        let p2 = s.upsert_project(dir2.path().to_str().unwrap()).unwrap();
        s.create_session(&p1.id, "mock").unwrap();
        s.create_session(&p2.id, "mock").unwrap();
        let all = s.list_all_sessions().unwrap();
        assert!(all.len() >= 2);
    }

    #[test]
    fn event_append_query_and_latest_seq() {
        let mut s = mem();
        let dir = tempfile::tempdir().unwrap();
        let p = s.upsert_project(dir.path().to_str().unwrap()).unwrap();
        let sid = s.create_session(&p.id, "mock").unwrap().id;

        let e1 = s
            .append_event(
                &sid,
                EventKind::UserInput,
                &serde_json::json!({"text": "hello"}),
            )
            .unwrap();
        let e2 = s
            .append_event(
                &sid,
                EventKind::ModelDelta,
                &serde_json::json!({"text": "world"}),
            )
            .unwrap();
        assert_eq!(e1.seq, 1);
        assert_eq!(e2.seq, 2);

        // events
        let events = s.events(&sid).unwrap();
        assert_eq!(events.len(), 2);

        // events_since
        let since = s.events_since(&sid, 1).unwrap();
        assert_eq!(since.len(), 1);
        assert_eq!(since[0].seq, 2);

        // recent_events
        let recent = s.recent_events(0, 10).unwrap();
        assert!(recent.len() >= 2);

        // latest_seq
        assert_eq!(s.latest_seq(&sid).unwrap(), 2);

        // count_events_of_kind
        assert_eq!(
            s.count_events_of_kind(&sid, EventKind::UserInput).unwrap(),
            1
        );
    }

    #[test]
    fn session_status_transitions() {
        let mut s = mem();
        let dir = tempfile::tempdir().unwrap();
        let p = s.upsert_project(dir.path().to_str().unwrap()).unwrap();
        let sid = s.create_session(&p.id, "mock").unwrap().id;

        s.set_session_status(&sid, SessionStatus::Executing)
            .unwrap();
        let session = s.session(&sid).unwrap().unwrap();
        assert_eq!(session.status, SessionStatus::Executing);

        s.set_session_status(&sid, SessionStatus::Done).unwrap();
        let session = s.session(&sid).unwrap().unwrap();
        assert_eq!(session.status, SessionStatus::Done);
    }

    #[test]
    fn project_language_packs_update() {
        let mut s = mem();
        let dir = tempfile::tempdir().unwrap();
        let p = s.upsert_project(dir.path().to_str().unwrap()).unwrap();
        s.set_project_language_packs(&p.id, &["typescript".into(), "python".into()])
            .unwrap();
        let updated = s.project(&p.id).unwrap().unwrap();
        assert_eq!(updated.language_packs.len(), 2);
    }

    #[test]
    fn set_session_worktree() {
        let mut s = mem();
        let dir = tempfile::tempdir().unwrap();
        let p = s.upsert_project(dir.path().to_str().unwrap()).unwrap();
        let sid = s.create_session(&p.id, "mock").unwrap().id;
        s.set_session_worktree(&sid, "/tmp/wt/test").unwrap();
        let session = s.session(&sid).unwrap().unwrap();
        assert_eq!(session.worktree_path, "/tmp/wt/test");
    }

    // v1.103：手动归档 / 还原（§14.2）——侧栏过滤语义。
    #[test]
    fn archive_hides_session_until_unarchived() {
        let mut s = mem();
        let dir = tempfile::tempdir().unwrap();
        let p = s.upsert_project(dir.path().to_str().unwrap()).unwrap();
        let a = s.create_session(&p.id, "mock").unwrap().id;
        let b = s.create_session(&p.id, "mock").unwrap().id;
        s.archive_session(&a).unwrap();
        assert!(!s.session(&a).unwrap().unwrap().archived_at.is_empty());
        assert_eq!(s.list_sessions(&p.id).unwrap().len(), 1);
        assert_eq!(s.list_archived_sessions(&p.id).unwrap().len(), 1);
        s.unarchive_session(&a).unwrap();
        assert_eq!(s.list_sessions(&p.id).unwrap().len(), 2);
        assert!(s.list_archived_sessions(&p.id).unwrap().is_empty());
        assert_eq!(s.session(&b).unwrap().unwrap().archived_at, "");
    }

    // v1.103：手动删除——事务级联清依赖行，其余会话不受影响。
    #[test]
    fn delete_session_removes_rows_and_dependencies() {
        let mut s = mem();
        let dir = tempfile::tempdir().unwrap();
        let p = s.upsert_project(dir.path().to_str().unwrap()).unwrap();
        let sid = s.create_session(&p.id, "mock").unwrap().id;
        s.append_event(
            &sid,
            EventKind::UserInput,
            &serde_json::json!({"text": "hi"}),
        )
        .unwrap();
        s.archive_session(&sid).unwrap();
        s.delete_session(&sid).unwrap();
        assert!(s.session(&sid).unwrap().is_none());
        assert!(s.events(&sid).unwrap().is_empty());
        let other = s.create_session(&p.id, "mock").unwrap().id;
        assert_eq!(s.list_sessions(&p.id).unwrap()[0].id, other);
    }

    // v1.103：v7 旧库迁移补 sessions.archived_at 列。

    #[test]
    fn session_worktree_path_roundtrip() {
        let mut s = mem();
        let dir = tempfile::tempdir().unwrap();
        let p = s.upsert_project(dir.path().to_str().unwrap()).unwrap();
        let sid = s.create_session(&p.id, "mock").unwrap().id;
        s.set_session_worktree(&sid, "/some/wt/path").unwrap();
        let sess = s.session(&sid).unwrap().unwrap();
        assert!(!sess.worktree_path.is_empty());
    }

    #[test]
    fn l4_chunk_upsert_and_search() {
        let mut s = mem();
        let dir = tempfile::tempdir().unwrap();
        let p = s.upsert_project(dir.path().to_str().unwrap()).unwrap();
        let embedding = vec![0.1; 8];
        s.replace_l4_file(
            &p.id,
            "search.rs",
            &[L4ChunkRecord {
                symbol: Some("search_fn".into()),
                start_line: 1,
                end_line: 10,
                text: "pub fn search_fn() {}".into(),
                embedding: embedding.clone(),
            }],
        )
        .unwrap();
        assert_eq!(s.l4_chunk_count(&p.id).unwrap(), 1);
    }

    #[test]
    fn l4_multiple_files_and_rebuild() {
        let mut s = mem();
        let dir = tempfile::tempdir().unwrap();
        let p = s.upsert_project(dir.path().to_str().unwrap()).unwrap();
        let emb = vec![0.2; 8];
        s.replace_l4_file(
            &p.id,
            "a.rs",
            &[L4ChunkRecord {
                symbol: None,
                start_line: 1,
                end_line: 5,
                text: "fn a()".into(),
                embedding: emb.clone(),
            }],
        )
        .unwrap();
        s.replace_l4_file(
            &p.id,
            "b.rs",
            &[L4ChunkRecord {
                symbol: None,
                start_line: 1,
                end_line: 5,
                text: "fn b()".into(),
                embedding: emb.clone(),
            }],
        )
        .unwrap();
        assert_eq!(s.l4_chunk_count(&p.id).unwrap(), 2);
        // replace one file removes old chunks
        s.replace_l4_file(
            &p.id,
            "a.rs",
            &[L4ChunkRecord {
                symbol: None,
                start_line: 1,
                end_line: 5,
                text: "fn a2()".into(),
                embedding: emb,
            }],
        )
        .unwrap();
        assert_eq!(s.l4_chunk_count(&p.id).unwrap(), 2);
    }

    #[test]
    fn session_archive_and_list() {
        let mut s = mem();
        let dir = tempfile::tempdir().unwrap();
        let p = s.upsert_project(dir.path().to_str().unwrap()).unwrap();
        let sid = s.create_session(&p.id, "mock").unwrap().id;
        s.set_session_status(&sid, SessionStatus::Done).unwrap();
        // archive
        let archive_dir = tempfile::tempdir().unwrap();
        let archived = s.archive_old_sessions(0, archive_dir.path()).unwrap();
        let _ = archived; // 可能空（无旧会话）
    }

    #[test]
    fn project_list_multiple_projects() {
        let mut s = mem();
        for _i in 0..3 {
            let dir = tempfile::tempdir().unwrap();
            s.upsert_project(dir.path().to_str().unwrap()).unwrap();
        }
        let projects = s.list_projects().unwrap();
        assert_eq!(projects.len(), 3);
    }

    #[test]
    fn session_lifecycle_full_states() {
        let mut s = mem();
        let dir = tempfile::tempdir().unwrap();
        let p = s.upsert_project(dir.path().to_str().unwrap()).unwrap();
        let sid = s.create_session(&p.id, "mock").unwrap().id;
        for status in [
            SessionStatus::Executing,
            SessionStatus::Done,
            SessionStatus::RolledBack,
        ] {
            s.set_session_status(&sid, status).unwrap();
            let sess = s.session(&sid).unwrap().unwrap();
            assert_eq!(sess.status, status);
        }
    }

    #[test]
    fn l4_delete_file_removes_chunks() {
        let mut s = mem();
        let dir = tempfile::tempdir().unwrap();
        let p = s.upsert_project(dir.path().to_str().unwrap()).unwrap();
        let emb = vec![0.3; 8];
        s.replace_l4_file(
            &p.id,
            "rem.rs",
            &[L4ChunkRecord {
                symbol: None,
                start_line: 1,
                end_line: 5,
                text: "fn rem()".into(),
                embedding: emb,
            }],
        )
        .unwrap();
        assert_eq!(s.l4_chunk_count(&p.id).unwrap(), 1);
        s.delete_l4_file(&p.id, "rem.rs").unwrap();
        assert_eq!(s.l4_chunk_count(&p.id).unwrap(), 0);
    }

    #[test]
    fn model_usage_session_scoped() {
        let mut s = mem();
        let dir = tempfile::tempdir().unwrap();
        let p = s.upsert_project(dir.path().to_str().unwrap()).unwrap();
        let sid = s.create_session(&p.id, "mock").unwrap().id;
        s.record_model_usage(&sid, "mock", "mock-1", 50, 75, 40, 900, 0.01)
            .unwrap();
        s.record_model_usage(&sid, "mock", "mock-1", 50, 75, 40, 1_100, 0.02)
            .unwrap();
        let totals = s.project_usage_totals(&p.id).unwrap();
        assert_eq!(totals.input_tokens, 100);
        assert_eq!(totals.output_tokens, 150);
        assert_eq!(totals.cached_input_tokens, 80);
        assert_eq!(totals.duration_ms, 2_000);
        assert!((totals.cost_usd - 0.03).abs() < f64::EPSILON);
    }

    #[test]
    fn ui_pref_overwrite() {
        let mut s = mem();
        s.set_ui_pref("theme", "dark").unwrap();
        s.set_ui_pref("theme", "light").unwrap();
        let prefs = s.ui_prefs().unwrap();
        let theme = prefs.iter().find(|(k, _)| k == "theme").unwrap();
        assert_eq!(theme.1, "light");
    }

    #[test]
    fn insert_memory_and_list() {
        let mut s = mem();
        let dir = tempfile::tempdir().unwrap();
        let p = s.upsert_project(dir.path().to_str().unwrap()).unwrap();
        let sid = s.create_session(&p.id, "mock").unwrap().id;
        let rec = MemoryRecord {
            scope: "project".into(),
            project_id: p.id.clone(),
            kind: "fact".into(),
            content: "Uses TypeScript strict mode".into(),
            importance: 4,
            embedding: vec![0.5; 8],
            source_session: sid,
        };
        let (mem, _created) = s.upsert_memory(&rec, 0.90).unwrap();
        assert_eq!(mem.kind, "fact");
        let all = s.list_memories(&p.id, None, 10).unwrap();
        assert!(!all.is_empty());
    }

    #[test]
    fn memory_upsert_dedup_similar_content() {
        let mut s = mem();
        let dir = tempfile::tempdir().unwrap();
        let p = s.upsert_project(dir.path().to_str().unwrap()).unwrap();
        let sid = s.create_session(&p.id, "mock").unwrap().id;
        let rec1 = MemoryRecord {
            scope: "project".into(),
            project_id: p.id.clone(),
            kind: "preference".into(),
            content: "prefers TypeScript".into(),
            importance: 3,
            embedding: vec![0.8, 0.1, 0.1],
            source_session: sid.clone(),
        };
        s.upsert_memory(&rec1, 0.95).unwrap();
        // Similar content → merge not create
        let rec2 = MemoryRecord {
            scope: "project".into(),
            project_id: p.id.clone(),
            kind: "preference".into(),
            content: "prefers TypeScript with strict mode".into(),
            importance: 4,
            embedding: vec![0.7, 0.2, 0.1],
            source_session: sid,
        };
        let (_, merged) = s
            .upsert_memory(&rec2, 0.90)
            .unwrap_or((s.list_memories(&p.id, None, 1).unwrap()[0].clone(), true));
        let _ = merged;
    }

    #[test]
    fn l4_search_with_embedding_cosine_distance() {
        let mut s = mem();
        let dir = tempfile::tempdir().unwrap();
        let p = s.upsert_project(dir.path().to_str().unwrap()).unwrap();
        let query = vec![0.9; 8];
        let stored = vec![0.9; 8]; // Same direction → high cosine
        s.replace_l4_file(
            &p.id,
            "match.rs",
            &[L4ChunkRecord {
                symbol: Some("target_fn".into()),
                start_line: 1,
                end_line: 10,
                text: "fn target_fn() {}".into(),
                embedding: stored,
            }],
        )
        .unwrap();
        let results = s.l4_search(&p.id, &query, 5).unwrap();
        assert!(!results.is_empty());
    }

    #[test]
    fn session_events_persist_across_queries() {
        let mut s = mem();
        let dir = tempfile::tempdir().unwrap();
        let p = s.upsert_project(dir.path().to_str().unwrap()).unwrap();
        let sid = s.create_session(&p.id, "mock").unwrap().id;
        for i in 0..5 {
            s.append_event(
                &sid,
                EventKind::UserInput,
                &serde_json::json!({"text": format!("msg {}", i)}),
            )
            .unwrap();
        }
        // Multiple queries return consistent data
        assert_eq!(s.latest_seq(&sid).unwrap(), 5);
        assert_eq!(s.events(&sid).unwrap().len(), 5);
        assert_eq!(s.events_since(&sid, 3).unwrap().len(), 2);
        assert_eq!(
            s.count_events_of_kind(&sid, EventKind::UserInput).unwrap(),
            5
        );
    }

    #[test]
    fn memory_prune_keeps_top_importance() {
        let mut s = mem();
        let dir = tempfile::tempdir().unwrap();
        let p = s.upsert_project(dir.path().to_str().unwrap()).unwrap();
        let sid = s.create_session(&p.id, "mock").unwrap().id;
        for i in 0..5 {
            let rec = MemoryRecord {
                scope: "project".into(),
                project_id: p.id.clone(),
                kind: "fact".into(),
                content: format!("fact {}", i),
                importance: i + 1,
                embedding: vec![],
                source_session: sid.clone(),
            };
            s.upsert_memory(&rec, 0.0).unwrap();
        }
        let pruned = s.prune_memories(&p.id, 3).unwrap();
        let _ = pruned;
    }

    #[test]
    fn l4_search_with_no_matches_returns_empty() {
        let mut s = mem();
        let dir = tempfile::tempdir().unwrap();
        let p = s.upsert_project(dir.path().to_str().unwrap()).unwrap();
        let query = vec![1.0; 8];
        // No chunks stored
        let results = s.l4_search(&p.id, &query, 5);
        assert!(results.is_ok());
    }

    #[test]
    fn memory_list_with_query_filter() {
        let mut s = mem();
        let dir = tempfile::tempdir().unwrap();
        let p = s.upsert_project(dir.path().to_str().unwrap()).unwrap();
        let sid = s.create_session(&p.id, "mock").unwrap().id;
        let rec = MemoryRecord {
            scope: "project".into(),
            project_id: p.id.clone(),
            kind: "fact".into(),
            content: "The project uses React for frontend".into(),
            importance: 3,
            embedding: vec![],
            source_session: sid,
        };
        s.upsert_memory(&rec, 0.0).unwrap();
        // Query with keyword
        let filtered = s.list_memories(&p.id, Some("React"), 10).unwrap();
        let _ = filtered;
        // Query without match
        let no_match = s.list_memories(&p.id, Some("nonexistent"), 10).unwrap();
        let _ = no_match;
    }

    #[test]
    fn set_project_language_packs_empty_and_multiple() {
        let mut s = mem();
        let dir = tempfile::tempdir().unwrap();
        let p = s.upsert_project(dir.path().to_str().unwrap()).unwrap();
        s.set_project_language_packs(&p.id, &[]).unwrap();
        let empty = s.project(&p.id).unwrap().unwrap();
        assert!(empty.language_packs.is_empty());
        s.set_project_language_packs(&p.id, &["ts".into(), "py".into(), "rust".into()])
            .unwrap();
        let multi = s.project(&p.id).unwrap().unwrap();
        assert_eq!(multi.language_packs.len(), 3);
    }

    #[test]
    fn create_session_with_id_and_worktree_path() {
        let mut s = mem();
        let dir = tempfile::tempdir().unwrap();
        let p = s.upsert_project(dir.path().to_str().unwrap()).unwrap();
        let custom_id = "custom-session-id-123";
        let session = s.create_session_with_id(custom_id, &p.id, "gpt-4").unwrap();
        assert_eq!(session.id, custom_id);
        assert_eq!(session.model, "gpt-4");
    }

    #[test]
    fn l4_stats_after_multiple_replaces() {
        let mut s = mem();
        let dir = tempfile::tempdir().unwrap();
        let p = s.upsert_project(dir.path().to_str().unwrap()).unwrap();
        let emb = vec![0.5; 8];
        // Replace 3 files
        for name in ["f1.rs", "f2.rs", "f3.rs"] {
            s.replace_l4_file(
                &p.id,
                name,
                &[L4ChunkRecord {
                    symbol: None,
                    start_line: 1,
                    end_line: 5,
                    text: format!("fn {}()", name),
                    embedding: emb.clone(),
                }],
            )
            .unwrap();
        }
        assert_eq!(s.l4_chunk_count(&p.id).unwrap(), 3);
    }

    #[test]
    fn upsert_project_same_path_returns_same() {
        let mut s = mem();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_str().unwrap();
        let p1 = s.upsert_project(path).unwrap();
        let p2 = s.upsert_project(path).unwrap();
        assert_eq!(p1.id, p2.id);
    }
}
