//! daemon 状态：store / 会话表 / provider 注册表 / 配置。

use std::collections::{HashMap, HashSet};
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
use tenon_store::Store;

use crate::auth::TicketStore;

/// 设置面板运行时覆盖（§7.2 / §15）：known-keys 子集。
/// 持久化 `~/.tenon/settings.json`，**新会话**生效（既有会话保持各自配置）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SettingsOverrides {
    /// interactive | auto
    pub mode: Option<String>,
    pub first_edit_buffer_ms: Option<u64>,
    pub approval_timeout_s: Option<u64>,
    pub command_timeout_s: Option<u64>,
}

impl SettingsOverrides {
    /// 校验并合并 PUT body；非法值返回错误文案。
    pub fn merge_json(&mut self, body: &serde_json::Value) -> Result<(), String> {
        if let Some(session) = body.get("session") {
            if let Some(m) = session.get("mode") {
                let m = m.as_str().ok_or("session.mode 须为字符串")?;
                if m != "interactive" && m != "auto" {
                    return Err("session.mode 仅支持 interactive | auto".into());
                }
                self.mode = Some(m.to_string());
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
            if let Some(v) = session.get("approval_timeout_s") {
                let v = v
                    .as_u64()
                    .ok_or("session.approval_timeout_s 须为非负整数")?;
                if !(5..=3600).contains(&v) {
                    return Err("session.approval_timeout_s 取值 5-3600s".into());
                }
                self.approval_timeout_s = Some(v);
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
        Ok(())
    }

    pub fn to_json(&self) -> serde_json::Value {
        let mut session = serde_json::Map::new();
        if let Some(m) = &self.mode {
            session.insert("mode".into(), serde_json::Value::String(m.clone()));
        }
        if let Some(v) = self.first_edit_buffer_ms {
            session.insert("first_edit_buffer_ms".into(), serde_json::json!(v));
        }
        if let Some(v) = self.approval_timeout_s {
            session.insert("approval_timeout_s".into(), serde_json::json!(v));
        }
        let mut exec = serde_json::Map::new();
        if let Some(v) = self.command_timeout_s {
            exec.insert("command_timeout_s".into(), serde_json::json!(v));
        }
        serde_json::json!({"session": session, "exec": exec})
    }

    /// 从 `~/.tenon/settings.json` 读取（损坏 / 非法条目逐项忽略）。
    pub fn load_from_disk() -> Self {
        let mut out = Self::default();
        let Ok(text) = std::fs::read_to_string(Self::file_path()) else {
            return out;
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
            return out;
        };
        let _ = out.merge_json(&v);
        out
    }

    pub fn file_path() -> PathBuf {
        tenon_config::Config::data_dir().join("settings.json")
    }

    pub fn persist(&self) {
        let path = Self::file_path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let json = self.to_json().to_string();
        if std::fs::write(&path, json).is_ok() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
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
    /// 启动时注册的项目根（§6.2 `--project`；/pairing 回传给 UI）。
    pub project: Option<String>,
    /// 握手 endpoint 文件；None = `~/.tenon/daemon.endpoint`（测试必须覆盖避免并行竞争）。
    pub endpoint_path: Option<std::path::PathBuf>,
    /// Laya 自动下载 registry 覆盖（§9.8 v1.71；None = 官方静态 registry；测试注入本地地址）。
    pub laya_registry_url: Option<String>,
    /// Laya 清单验签公钥覆盖（§9.8；None = 官方解析链；测试注入，不读进程 env）。
    pub laya_public_key: Option<String>,
    /// Laya 模型目录覆盖（§9.8；None = ~/.tenon/models/laya；测试注入临时目录）。
    pub laya_models_dir: Option<std::path::PathBuf>,
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
            project: None,
            endpoint_path: None,
            laya_registry_url: None,
            laya_public_key: None,
            laya_models_dir: None,
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
    ) -> Arc<Self> {
        let (events, _) = tokio::sync::broadcast::channel(1024);
        let watcher = tenon_fs::FileWatcher::watch(&root).ok().map(|watcher| {
            let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let project_id = project_id.to_string();
            let sender = events.clone();
            let global_sender = global_events.clone();
            let stop_for_thread = stop.clone();
            let handle = std::thread::Builder::new()
                .name(format!("tenon-watch-{project_id}"))
                .spawn(move || {
                    while !stop_for_thread.load(std::sync::atomic::Ordering::SeqCst) {
                        for change in watcher.next_batch(std::time::Duration::from_millis(250)) {
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
            WatchHandle {
                stop,
                handle: std::sync::Mutex::new(Some(handle)),
            }
        });
        Arc::new(Self {
            root,
            events,
            watcher: std::sync::Mutex::new(watcher),
            last_accessed: std::sync::Mutex::new(std::time::Instant::now()),
        })
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

pub struct DaemonState {
    pub store: Arc<Mutex<Store>>,
    /// 共享 LSP 管理器（§8.5：一项目 × 语言一个宿主，编辑器与代理共用）。
    pub lsp: Arc<LspManager>,
    /// Laya 本地决策模型运行时（§9.8；未下载即整体回退）。
    pub laya: Arc<LayaRuntime>,
    /// Laya 清单验签公钥覆盖（§9.8 v1.71；None = 官方解析链；测试注入）。
    pub(crate) laya_public_key: Option<String>,
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
    pub open_projects: Mutex<HashMap<String, Arc<ProjectRuntime>>>,
    /// 跨项目并发调度（§6.4 / §9.7）。
    pub execution_permits: Arc<Semaphore>,
    /// v1.15 项目组合任务聚合（父任务内存态；子会话/事件持久于 SQLite）。
    pub portfolio_tasks: Mutex<HashMap<String, PortfolioTask>>,
    /// 进程内文件变更事件总线；WS 订阅者可全量或按 project_id 过滤。
    pub file_events: tokio::sync::broadcast::Sender<ProjectFileEvent>,
    pub snapshots_root: std::path::PathBuf,
    /// 设置面板运行时覆盖（§15 /settings；新会话生效）。
    pub settings_overrides: std::sync::Mutex<SettingsOverrides>,
    /// L4 增量索引队列（§10.1）；ProjectRuntime 激活 / watcher 变化入队。
    pub l4_index_tx: tokio::sync::mpsc::Sender<L4IndexRequest>,
    pub l4_index_rx: std::sync::Mutex<Option<tokio::sync::mpsc::Receiver<L4IndexRequest>>>,
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
    /// 新会话的默认档位（设置覆盖 > 全局配置）。
    pub fn default_session_mode(&self) -> String {
        self.settings_overrides
            .lock()
            .unwrap()
            .mode
            .clone()
            .unwrap_or_else(|| "interactive".into())
    }
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
        let laya_dir = options
            .laya_models_dir
            .clone()
            .unwrap_or_else(|| Config::data_dir().join("models/laya"));
        let execution_permits = Arc::new(Semaphore::new(
            options.config.projects.max_concurrent_agent_tasks.max(1),
        ));
        let (file_events, _) = tokio::sync::broadcast::channel(2048);
        let (l4_index_tx, l4_index_rx) = tokio::sync::mpsc::channel(1024);
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
            laya_public_key: options.laya_public_key.clone(),
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
            file_events,
            snapshots_root,
            settings_overrides: std::sync::Mutex::new(SettingsOverrides::load_from_disk()),
            l4_index_tx,
            l4_index_rx: std::sync::Mutex::new(Some(l4_index_rx)),
        }
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

    /// 只返回已打开 ProjectRuntime；关闭后的项目不得继续文件 / LSP 操作。
    pub async fn open_project_root(&self, project_id: &str) -> Option<std::path::PathBuf> {
        let runtime = self.project_runtime(project_id).await?;
        Some(runtime.root.clone())
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
        let runtime = ProjectRuntime::open(project_id, root.clone(), self.file_events.clone());
        open.insert(project_id.to_string(), runtime.clone());
        drop(open);
        if let Err(e) = self
            .l4_index_tx
            .send(L4IndexRequest::Project {
                project_id: project_id.to_string(),
                root,
            })
            .await
        {
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

    /// 空闲回收：无活跃代理会话且超过 TTL 的 runtime；返回已关闭项目。
    pub async fn reclaim_idle_projects(&self) -> Vec<String> {
        let ttl = std::time::Duration::from_secs(self.config.projects.idle_runtime_ttl_seconds);
        let mut active_ids = HashSet::new();
        {
            let sessions = self.sessions.lock().await;
            for entry in sessions.values() {
                if matches!(
                    entry.session.current_state().await,
                    tenon_core::machine::State::Executing
                        | tenon_core::machine::State::Verifying
                        | tenon_core::machine::State::Fixing
                        | tenon_core::machine::State::AwaitingApproval
                ) {
                    active_ids.insert(entry.project_id.clone());
                }
            }
        }
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

    /// 取出唯一 L4 worker receiver；重复调用返回 None。
    pub fn take_l4_requests(&self) -> Option<tokio::sync::mpsc::Receiver<L4IndexRequest>> {
        self.l4_index_rx.lock().expect("l4 index rx").take()
    }

    /// Laya 自动下载并启用（§9.8 v1.71）：`enabled` + `auto_download` 时后台拉取
    /// 官方静态 registry 签名清单，版本新于已装即下载安装热装载；全程无审批卡
    /// （产品自管签名资产、推理不出网，非代理动作）。失败静默回退：仅日志，
    /// 下次启动重试；未启用 / 已是最新即跳过。
    pub fn spawn_laya_auto_download(self: &Arc<Self>, registry_url: Option<String>) {
        if !self.config.models.laya.enabled || !self.config.models.laya.auto_download {
            return;
        }
        let state = self.clone();
        tokio::spawn(async move {
            let manifest = match tenon_laya::registry::fetch_manifest(registry_url.as_deref()).await
            {
                Ok(m) => m,
                Err(e) => {
                    tracing::info!("Laya 自动下载跳过：registry 不可达（{e}）；下次启动重试");
                    return;
                }
            };
            let plan = match tenon_laya::registry::plan_install_with_key(
                &manifest,
                state.laya_public_key.as_deref(),
            ) {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!("Laya 自动下载跳过：清单校验失败（{e}）");
                    return;
                }
            };
            if state
                .laya
                .version()
                .await
                .map(|(_, v)| v >= plan.version)
                .unwrap_or(false)
            {
                tracing::info!("Laya 已是最新（v{}），跳过自动下载", plan.version);
                return;
            }
            match tenon_laya::registry::download_model(&plan).await {
                Ok(bytes) => match state.laya.install(&bytes).await {
                    Ok(()) => tracing::info!("Laya 自动下载完成：v{} 已启用", plan.version),
                    Err(e) => tracing::warn!("Laya 自动下载安装失败：{e}"),
                },
                Err(e) => tracing::warn!("Laya 自动下载失败：{e}；下次启动重试"),
            }
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
                if let Some(root) = state.open_project_root(&event.project_id).await {
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
                    tracing::info!("L4 full index complete for {project_id}");
                }
                Err(e) => tracing::warn!("L4 full scan failed: {e}"),
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
                tracing::warn!("L4 incremental index failed for {path}: {e}");
            }
        }
    }
}
