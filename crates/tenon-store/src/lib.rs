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
    pub cost_usd: f64,
    pub created_at: String,
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

const SCHEMA_VERSION: i64 = 7;

const DDL: &str = r#"
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;

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
            "SELECT id, project_id, model, status, title, worktree_path, created_at, updated_at
             FROM sessions WHERE id = ?1",
        )?;
        let mut rows = stmt.query_map([id], row_to_session)?;
        Ok(rows.next().transpose()?)
    }

    /// 全部会话（崩溃恢复扫描用）。
    pub fn list_all_sessions(&mut self) -> Result<Vec<Session>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, project_id, model, status, title, worktree_path, created_at, updated_at
             FROM sessions ORDER BY created_at ASC",
        )?;
        let rows = stmt.query_map([], row_to_session)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn list_sessions(&mut self, project_id: &str) -> Result<Vec<Session>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, project_id, model, status, title, worktree_path, created_at, updated_at
             FROM sessions WHERE project_id = ?1 ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map([project_id], row_to_session)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
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
    pub fn events_since(&mut self, session_id: &str, after_seq: i64) -> Result<Vec<Event>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, session_id, project_id, seq, type, payload, created_at
             FROM events WHERE session_id = ?1 AND seq > ?2 ORDER BY seq ASC",
        )?;
        let rows = stmt.query_map(params![session_id, after_seq], row_to_event)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
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

    pub fn record_model_usage(
        &mut self,
        session_id: &str,
        provider: &str,
        model: &str,
        input_tokens: i64,
        output_tokens: i64,
        cost_usd: f64,
    ) -> Result<ModelUsage> {
        let now = Self::now();
        let project_id = self.project_id_for_session(session_id);
        self.conn.execute(
            "INSERT INTO model_usage (session_id, project_id, provider, model, input_tokens, output_tokens, cost_usd, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![session_id, project_id, provider, model, input_tokens, output_tokens, cost_usd, now],
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
        Ok(ModelUsage {
            id: self.conn.last_insert_rowid(),
            session_id: session_id.to_string(),
            project_id,
            provider: provider.to_string(),
            model: model.to_string(),
            input_tokens,
            output_tokens,
            cost_usd,
            created_at: now,
        })
    }

    pub fn session_usage(&mut self, session_id: &str) -> Result<Vec<ModelUsage>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, session_id, project_id, provider, model, input_tokens, output_tokens, cost_usd, created_at
             FROM model_usage WHERE session_id = ?1 ORDER BY id ASC",
        )?;
        let rows = stmt.query_map([session_id], row_to_usage)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// 会话累计（任务级 / 会话级归因）。
    pub fn session_usage_totals(&mut self, session_id: &str) -> Result<(i64, i64, f64)> {
        self.conn.query_row(
            "SELECT COALESCE(SUM(input_tokens),0), COALESCE(SUM(output_tokens),0), COALESCE(SUM(cost_usd),0.0)
             FROM model_usage WHERE session_id = ?1",
            [session_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .map_err(Into::into)
    }

    /// 项目累计（项目任务中心 / 成本看板 §6.4 / §11）。
    pub fn project_usage_totals(&mut self, project_id: &str) -> Result<(i64, i64, f64)> {
        self.conn.query_row(
            "SELECT COALESCE(SUM(input_tokens),0), COALESCE(SUM(output_tokens),0), COALESCE(SUM(cost_usd),0.0)
             FROM model_usage WHERE project_id = ?1",
            [project_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
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
            let events = self.events(sid)?;
            let usage = self.session_usage(sid)?;
            let record = serde_json::json!({
                "session_id": sid,
                "events": events,
                "model_usage": usage,
            });
            let path: PathBuf = archive_dir.join(format!("{}.session.jsonl.gz", sid));
            let f = std::fs::File::create(path)?;
            let mut enc = GzEncoder::new(f, Compression::default());
            enc.write_all(record.to_string().as_bytes())?;
            enc.finish()?;
            self.conn
                .execute("DELETE FROM events WHERE session_id = ?1", [sid])?;
            self.conn
                .execute("DELETE FROM tool_calls WHERE session_id = ?1", [sid])?;
            self.conn
                .execute("DELETE FROM model_usage WHERE session_id = ?1", [sid])?;
            self.conn
                .execute("DELETE FROM checkpoints WHERE session_id = ?1", [sid])?;
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
        cost_usd: r.get(7)?,
        created_at: r.get(8)?,
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
        s.record_model_usage(&sess.id, "glm", "glm-4.6", 100, 50, 0.01)
            .unwrap();
        s.record_model_usage(&sess.id, "glm", "glm-4.6", 200, 80, 0.02)
            .unwrap();

        let (inp, out, cost) = s.session_usage_totals(&sess.id).unwrap();
        assert_eq!((inp, out), (300, 130));
        assert!((cost - 0.03).abs() < 1e-9);
        let (pinp, pout, pcost) = s.project_usage_totals(&p.id).unwrap();
        assert_eq!((pinp, pout), (300, 130));
        assert!((pcost - 0.03).abs() < 1e-9);
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
        s.record_model_usage(&old.id, "glm", "m", 10, 5, 0.001)
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
        s.set_project_ui_state(&p.id, r#"{"tabs":["a.ts"]}"#).unwrap();
        let state = s.project_ui_state(&p.id).unwrap().unwrap();
        assert!(state.contains("a.ts"));
    }

    #[test]
    fn model_usage_and_project_totals() {
        let mut s = mem();
        let dir = tempfile::tempdir().unwrap();
        let p = s.upsert_project(dir.path().to_str().unwrap()).unwrap();
        let sid = s.create_session(&p.id, "mock").unwrap().id;
        s.record_model_usage(&sid, "mock", "mock-1", 100, 200, 0.05).unwrap();
        let (inp, out, cost) = s.project_usage_totals(&p.id).unwrap();
        assert_eq!(inp, 100);
        assert_eq!(out, 200);
        assert!(cost > 0.0);
    }

    #[test]
    fn checkpoint_insert_and_list() {
        let mut s = mem();
        let dir = tempfile::tempdir().unwrap();
        let p = s.upsert_project(dir.path().to_str().unwrap()).unwrap();
        let sid = s.create_session(&p.id, "mock").unwrap().id;
        s.insert_checkpoint(&sid, "tree-abc", &["a.ts".into()], None).unwrap();
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
        s.insert_tool_call(&sid, 1, "apply_patch", Level::C, 10).unwrap();
        let calls = s.tool_calls(&sid).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].tool, "apply_patch");
    }

    #[test]
    fn plugin_insert_and_list() {
        let mut s = mem();
        s.insert_plugin("lsp-1", "1.0.0", &["fs.read".into()], "sig").unwrap();
        let plugins = s.list_plugins().unwrap();
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].id, "lsp-1");
    }

    #[test]
    fn eval_run_insert_and_list() {
        let mut s = mem();
        let run = s.insert_eval_run("mock", &serde_json::json!({"pass": true}), "pass").unwrap();
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

        let e1 = s.append_event(&sid, EventKind::UserInput, &serde_json::json!({"text": "hello"})).unwrap();
        let e2 = s.append_event(&sid, EventKind::ModelDelta, &serde_json::json!({"text": "world"})).unwrap();
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
        assert_eq!(s.count_events_of_kind(&sid, EventKind::UserInput).unwrap(), 1);
    }

    #[test]
    fn session_status_transitions() {
        let mut s = mem();
        let dir = tempfile::tempdir().unwrap();
        let p = s.upsert_project(dir.path().to_str().unwrap()).unwrap();
        let sid = s.create_session(&p.id, "mock").unwrap().id;

        s.set_session_status(&sid, SessionStatus::Executing).unwrap();
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
        s.set_project_language_packs(&p.id, &["typescript".into(), "python".into()]).unwrap();
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
}