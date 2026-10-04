//! daemon 状态：store / 会话表 / provider 注册表 / 配置。

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{Mutex, Semaphore};

use crate::pairing::PairingStore;
use tenon_agent::session::{AgentSession, ProjectWriteLock, TaskOutcome};
use tenon_config::Config;
use tenon_laya::LayaRuntime;
use tenon_lsp::LspManager;
use tenon_models::ModelProvider;
use tenon_store::Store;

use crate::auth::TicketStore;

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
    /// 启动时注册的项目根（§6.2 `--project`；/pairing 回传给 UI）。
    pub project: Option<String>,
}

impl DaemonOptions {
    pub fn in_memory() -> Self {
        Self {
            db_path: None,
            lan_bind: false,
            config: Config::default(),
            providers: vec![],
            default_provider: String::new(),
            snapshots_root: None,
            project: None,
        }
    }
}

pub struct SessionEntry {
    pub session: Arc<AgentSession>,
    pub project_root: std::path::PathBuf,
    pub project_id: String,
    pub last_outcome: Mutex<Option<TaskOutcome>>,
    pub last_seq: i64,
}

/// 项目组合任务子项：每条子会话仍强绑定一个项目（§6.4）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PortfolioChild {
    pub id: String,
    pub project_id: String,
    pub session_id: String,
    pub text: String,
    pub status: String,
}

/// 跨项目编排容器：只聚合状态 / 审批 / 成本，不共享代码上下文。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PortfolioTask {
    pub id: String,
    pub title: String,
    pub status: String,
    pub children: Vec<PortfolioChild>,
    pub created_at: String,
    pub updated_at: String,
}

pub struct DaemonState {
    pub store: Arc<Mutex<Store>>,
    /// 共享 LSP 管理器（§8.5：一项目 × 语言一个宿主，编辑器与代理共用）。
    pub lsp: Arc<LspManager>,
    /// Laya 本地决策模型运行时（§9.8；未下载即整体回退）。
    pub laya: Arc<LayaRuntime>,
    /// 脏缓冲注册表按项目隔离（§6.4 / §8.6）；key = project_id，value 是该项目相对路径表。
    pub dirty_buffers: Mutex<HashMap<String, Arc<tenon_fs::DirtyBufferRegistry>>>,
    /// 局域网配对（M3 §12.6：显式开启 + 一次性码 + 可吊销令牌；默认关闭）。
    pub lan_pairing: Arc<PairingStore>,
    /// daemon 监听端口（/pairing 自发现回传给 UI）。
    pub port: std::sync::atomic::AtomicU16,
    /// 团队策略（M3：只收窄；`~/.tenon/policy.toml` 不存在 = 无约束）。
    pub team_policy: tenon_core::policy::TeamPolicy,
    pub config: Config,
    pub token: String,
    pub tickets: TicketStore,
    pub sessions: Mutex<HashMap<String, SessionEntry>>,
    pub providers: HashMap<String, Arc<dyn ModelProvider>>,
    pub default_provider: String,
    /// 启动时注册的项目根（/pairing 自发现回传）。
    pub default_project: Option<String>,
    /// 项目级写锁表（§9.7：同一项目同时刻仅一个会话 EXECUTING）。
    pub project_locks: Mutex<HashMap<String, ProjectWriteLock>>,
    /// 已打开 ProjectRuntime 的 project_id → canonical root（§6.4）。
    pub open_projects: Mutex<HashMap<String, std::path::PathBuf>>,
    /// 跨项目并发调度（§6.4 / §9.7）。
    pub execution_permits: Arc<Semaphore>,
    /// v1.15 项目组合任务聚合（父任务内存态；子会话/事件持久于 SQLite）。
    pub portfolio_tasks: Mutex<HashMap<String, PortfolioTask>>,
    pub snapshots_root: std::path::PathBuf,
}

/// 团队策略加载（M3）：`~/.tenon/policy.toml`（只收窄字段）。
fn load_team_policy() -> tenon_core::policy::TeamPolicy {
    let path = Config::data_dir().join("policy.toml");
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| toml::from_str(&text).ok())
        .unwrap_or_default()
}

impl DaemonState {
    pub async fn new(options: DaemonOptions) -> Self {
        let store = match options.db_path {
            Some(path) => Store::open(&path).expect("open store"),
            None => Store::open_in_memory().expect("in-memory store"),
        };
        let mut providers: HashMap<String, Arc<dyn ModelProvider>> = HashMap::new();
        for p in options.providers {
            providers.insert(p.name().to_string(), p);
        }
        // 配置文件 provider（无 Key 时延迟构建：缺失 Key 的 provider 不注册）
        let keys = tenon_models::EnvKeyStore;
        for (name, pcfg) in &options.config.models.providers {
            if providers.contains_key(name) {
                continue;
            }
            if let Ok(built) = tenon_models::build_provider(name, pcfg, &keys) {
                providers.insert(name.clone(), built);
            }
        }
        let default_provider = if options.default_provider.is_empty() {
            options.config.models.default.clone()
        } else {
            options.default_provider
        };
        let snapshots_root = options
            .snapshots_root
            .unwrap_or_else(|| Config::data_dir().join("snapshots"));
        let laya_dir = Config::data_dir().join("models/laya");
        let execution_permits = Arc::new(Semaphore::new(
            options.config.projects.max_concurrent_agent_tasks.max(1),
        ));
        Self {
            store: Arc::new(Mutex::new(store)),
            lsp: Arc::new(LspManager::new()),
            lan_pairing: Arc::new(PairingStore::new()),
            port: std::sync::atomic::AtomicU16::new(0),
            team_policy: load_team_policy(),
            dirty_buffers: Mutex::new(HashMap::new()),
            laya: Arc::new(LayaRuntime::open(
                &laya_dir,
                &options.config.models.laya.features,
            )),
            config: options.config,
            token: crate::generate_token(),
            tickets: TicketStore::default(),
            sessions: Mutex::new(HashMap::new()),
            providers,
            default_provider,
            default_project: options.project,
            project_locks: Mutex::new(HashMap::new()),
            open_projects: Mutex::new(HashMap::new()),
            execution_permits,
            portfolio_tasks: Mutex::new(HashMap::new()),
            snapshots_root,
        }
    }

    /// 查询项目 canonical root：打开表优先，随后持久登记。
    pub async fn project_root(&self, project_id: &str) -> Option<std::path::PathBuf> {
        {
            let open = self.open_projects.lock().await;
            if let Some(root) = open.get(project_id) {
                return Some(root.clone());
            }
        }
        let mut store = self.store.lock().await;
        store
            .project(project_id)
            .ok()
            .flatten()
            .map(|p| std::path::PathBuf::from(p.path))
    }

    /// 只返回已打开 ProjectRuntime；关闭后的项目不得继续文件 / LSP 操作。
    pub async fn open_project_root(&self, project_id: &str) -> Option<std::path::PathBuf> {
        self.open_projects.lock().await.get(project_id).cloned()
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
            &self.default_provider
        } else {
            name
        };
        self.providers
            .get(key)
            .cloned()
            .ok_or_else(|| format!("provider 不可用：{key}（未配置或缺少 Key）"))
    }
}
