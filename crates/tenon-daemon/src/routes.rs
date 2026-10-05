//! 路由（设计方案 §15 本地 API 表）。

use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Path, Query, State, WebSocketUpgrade};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use futures::SinkExt;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::Mutex;

use tenon_agent::session::{sanitize_title, AgentConfig, AgentSession, ControlCommand};
use tenon_core::context::ProjectRules;
use tenon_core::policy::Mode;
use tenon_snapshot::SnapshotStore;
use tenon_store::{EventKind, SessionStatus};

use crate::auth::auth_middleware;
use crate::state::{
    persist_team_policy, validate_team_policy, DaemonState, L4IndexRequest, SessionEntry,
};
use crate::updates::{current_target, find_staged_update, mark_staged_update, stage_update};

pub fn build_router(state: Arc<DaemonState>) -> Router {
    let token = state.token.clone();
    let auth_state = (token, state.lan_pairing.clone());
    Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/ws-ticket", post(ws_ticket))
        .route("/ws", get(ws_upgrade))
        // ---------- 会话与回滚（§15） ----------
        .route("/session", post(create_session))
        .route("/session/{id}/message", post(send_message))
        .route("/session/{id}", get(get_session))
        .route("/session/{id}/control", post(session_control))
        .route("/session/{id}/worktree/merge", post(merge_session_worktree))
        .route(
            "/session/{id}/worktree/discard",
            post(discard_session_worktree),
        )
        .route("/session/{id}/trace", get(session_trace))
        .route("/session/{id}/checkpoints", get(session_checkpoints))
        .route("/checkpoint/{id}/rollback", post(checkpoint_rollback))
        .route("/session/{id}/model", post(switch_session_model))
        .route("/model-suggest", post(model_suggest))
        // ---------- 多项目控制面（§6.4 / §15） ----------
        .route("/projects", get(list_projects))
        .route("/projects/open", post(open_project))
        .route("/projects/{id}", delete(delete_project))
        // ---------- 编辑器与文件（§15） ----------
        .route("/project/{id}/tree", get(project_tree))
        .route("/project/{id}/files/fuzzy", get(fuzzy_files))
        .route(
            "/project/{id}/buffers",
            put(set_dirty_buffer).delete(clear_dirty_buffer),
        )
        .route("/project/{id}/language-packs", get(detect_language_packs))
        .route(
            "/project/{id}/language-packs/install",
            post(install_language_pack),
        )
        .route("/project/{id}/file", get(read_file).put(write_file))
        .route("/project/{id}/file/ops", post(file_ops))
        .route("/project/{id}/highlight", get(syntax_highlight))
        .route("/project/{id}/git/view", get(git_source_view))
        .route("/project/{id}/search", get(search))
        .route("/project/{id}/search/replace", post(apply_search_replace))
        .route("/project/{id}/lsp", post(project_lsp_proxy))
        .route("/project/{id}/inline-complete", post(inline_complete))
        .route("/project/{id}/lsp/apply", post(apply_lsp_workspace_edit))
        .route("/plugins", get(list_plugins).put(search_registry))
        .route("/plugins/install", post(install_plugin))
        // ---------- 管理（§15） ----------
        .route("/project/trust", put(set_project_trust))
        .route(
            "/project/{id}/ui-state",
            get(get_project_ui_state).put(put_project_ui_state),
        )
        .route("/models", get(list_models))
        .route("/pairing", get(pairing_info))
        .route("/evals", get(list_evals))
        .route("/lan/enable", post(lan_enable))
        .route("/lan/pair", post(lan_pair))
        .route("/lan/revoke", post(lan_revoke))
        .route("/project/{id}/l4/stats", get(l4_stats))
        .route("/project/{id}/l4/rebuild", post(l4_rebuild))
        .route("/costs", get(costs))
        .route("/settings", get(get_settings).put(put_settings))
        .route("/team-policy", put(put_team_policy))
        .route("/updates", get(get_updates))
        .route("/updates/check", post(check_updates))
        .route("/updates/apply", post(apply_updates))
        // UI 偏好（§7.5 外观档）：daemon 端口动态导致 localStorage 按 origin
        // 隔离不可跨启动——此处为跨启动 / 跨端权威存储
        .route("/ui-prefs", get(get_ui_prefs).put(put_ui_prefs))
        .layer(axum::middleware::from_fn_with_state(
            auth_state,
            auth_middleware,
        ))
        .with_state(state)
}

fn api_err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(json!({"error": msg.into()}))).into_response()
}

type ApiResult<T> = Result<T, (StatusCode, String)>;

/// 共用的 project-scoped AgentSession 工厂。
async fn create_agent_session(
    state: &Arc<DaemonState>,
    project: &tenon_store::Project,
    provider: Arc<dyn tenon_models::ModelProvider>,
    working_dir: Option<String>,
    session_id: Option<String>,
    managed_worktree: Option<std::path::PathBuf>,
) -> Result<Arc<tenon_agent::session::AgentSession>, (StatusCode, String)> {
    // v1.87 §9.7：受管 worktree 会话的快照分片 / 写边界 / 命令 cwd 均按 worktree
    // 隔离；L4 索引、脏缓冲与事件仍按 project_id 归属（project_root 不变）。
    let snapshot_workspace = managed_worktree
        .as_deref()
        .unwrap_or(std::path::Path::new(&project.path));
    let snapshots = SnapshotStore::open(
        &state.snapshots_root,
        &project.id,
        snapshot_workspace,
        state.config.checkpoint.max_untracked_mb,
    )
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("快照库: {e}")))?;
    // v1.92：审批移除后 interactive/auto 档位无行为差异，mode 整链删除——
    // AgentConfig 恒按交互档构建（§9.1 状态机不受影响）。
    let mut agent_cfg = AgentConfig::for_project(
        std::path::PathBuf::from(&project.path),
        &project.id,
        project.trusted,
        Mode::Interactive,
    );
    agent_cfg.first_edit_buffer_ms = state.config.session.first_edit_buffer_ms;
    {
        let ov = state.settings_overrides.lock().unwrap().clone();
        if let Some(v) = ov.first_edit_buffer_ms {
            agent_cfg.first_edit_buffer_ms = v;
        }
        if let Some(v) = ov.command_timeout_s {
            agent_cfg.command_timeout_s = v;
        }
    }
    agent_cfg.circuit = (&state.config.agent.circuit).into();
    let team_policy = state.team_policy.read().expect("team policy lock").clone();
    agent_cfg.circuit.max_cost_usd = team_policy.clamp_cost(agent_cfg.circuit.max_cost_usd);
    agent_cfg.fix_rounds = state.config.agent.fix_loop.max_rounds;
    agent_cfg.command_timeout_s = state.config.agent.exec.command_timeout_s;
    agent_cfg.snapshots_root.clone_from(&state.snapshots_root);
    agent_cfg.working_dir = working_dir.map(std::path::PathBuf::from);
    agent_cfg.session_id = session_id;
    agent_cfg.managed_worktree = managed_worktree.clone();
    agent_cfg.write_scope = managed_worktree
        .as_deref()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "root".to_string());
    agent_cfg.dirty = Some(state.dirty_buffers_for(&project.id).await);
    agent_cfg.lsp = Some(state.lsp.clone());
    agent_cfg.team_denied_tools = team_policy.denied_tools;
    // §9.8 v1.92 接线：enabled 时把 Laya 运行时注入会话配置（#1 意图预判 /
    // #2 命令风险补盲区随之生效；未下载 / 未加载由运行时内部回退现状）。
    if state.config.models.laya.enabled {
        agent_cfg.laya = Some(state.laya.clone());
    }
    // §13.3 v1.92 接线：AGENTS.md「直读，只能收窄」——读取会话工作根的
    // AGENTS.md，解析 `<!-- tenon:rules -->` 限制块并入策略（缺文件即默认）。
    let project_rules = std::fs::read_to_string(snapshot_workspace.join("AGENTS.md"))
        .map(|md| ProjectRules::parse_agents_md(&md).0)
        .unwrap_or_default();
    let write_lock = state.write_lock_for(&project.id).await;
    AgentSession::create(
        state.store.clone(),
        Arc::new(snapshots),
        provider,
        agent_cfg,
        write_lock,
        project_rules,
    )
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

/// 路径末段派生名（§6.4 显示名回退）。
fn derived_project_name(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

/// 项目显示名解析（§6.4）：自定义名优先，空串回退为路径末段。
fn project_display_name(project: &tenon_store::Project) -> String {
    if !project.display_name.is_empty() {
        return project.display_name.clone();
    }
    derived_project_name(&project.path)
}

async fn register_session_entry(
    state: &Arc<DaemonState>,
    project: &tenon_store::Project,
    session: Arc<tenon_agent::session::AgentSession>,
) -> String {
    let sid = session.session_id.clone();
    let managed_worktree = session.managed_worktree().map(std::path::Path::to_path_buf);
    let mut sessions = state.sessions.lock().await;
    sessions.insert(
        sid.clone(),
        SessionEntry {
            session,
            project_root: std::path::PathBuf::from(&project.path),
            project_id: project.id.clone(),
            managed_worktree,
            last_outcome: Mutex::new(None),
            last_seq: 0,
        },
    );
    sid
}

/// 打开项目前 canonicalize、去重、检查嵌套根并维护 ProjectRuntime 表（§6.4）。
async fn ensure_open_project(
    state: &Arc<DaemonState>,
    path: &str,
    display_name: Option<&str>,
) -> ApiResult<tenon_store::Project> {
    let canonical = std::fs::canonicalize(path).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            format!("无法打开项目路径 {path}: {e}"),
        )
    })?;
    let project = {
        let mut store = state.store.lock().await;
        let canonical_text = canonical.to_string_lossy();
        let mut project = match store
            .project_by_path(&canonical_text)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        {
            Some(project) => project,
            None => {
                // 先完成嵌套校验，再登记；拒绝路径不得污染 ProjectRegistry。
                // 登记不受容量限制（v1.60：max_open 只约束内部运行时缓存）。
                {
                    let open = state.open_projects.lock().await;
                    if !state.config.projects.allow_linked_workspace {
                        for runtime in open.values() {
                            let root = runtime.root.as_path();
                            if root.starts_with(&canonical) || canonical.starts_with(root) {
                                return Err((
                                    StatusCode::CONFLICT,
                                    format!(
                                        "嵌套项目根默认拒绝：{} 与 {} 重叠；可在 [projects] 显式开启 linked workspace",
                                        root.display(),
                                        canonical.display()
                                    ),
                                ));
                            }
                        }
                    }
                }
                store
                    .upsert_project(&canonical_text)
                    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
            }
        };
        // 显式传入项目名（模态添加 / 重开）时落库；空串清除自定义名回退派生（§6.4）。
        if let Some(name) = display_name {
            let trimmed = name.trim();
            if trimmed != project.display_name {
                store
                    .set_project_display_name(&project.id, trimmed)
                    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            }
            project.display_name = if trimmed.is_empty() {
                derived_project_name(&project.path)
            } else {
                trimmed.to_string()
            };
        }
        project
    };

    // 运行时为内部缓存（v1.60 登记即用）：容量超限按 LRU 逐出最久未用 runtime，
    // 而非拒绝请求；激活前先腾位。
    state.evict_runtime_capacity(&project.id).await;
    let open = state.open_projects.lock().await;
    if !open.contains_key(&project.id) {
        if !state.config.projects.allow_linked_workspace {
            for runtime in open.values() {
                let root = runtime.root.as_path();
                if root.starts_with(&canonical) || canonical.starts_with(root) {
                    return Err((
                        StatusCode::CONFLICT,
                        format!(
                            "嵌套项目根默认拒绝：{} 与 {} 重叠；可在 [projects] 显式开启 linked workspace",
                            root.display(),
                            canonical.display()
                        ),
                    ));
                }
            }
        }
        drop(open);
        state.activate_project(&project.id, canonical).await;
    }
    Ok(project)
}

#[derive(Deserialize)]
struct OpenProjectBody {
    path: String,
    /// 可选显示名（模态添加 / 重开时设置）；空串清除自定义名。
    #[serde(default)]
    display_name: Option<String>,
}

async fn open_project(
    State(state): State<Arc<DaemonState>>,
    Json(body): Json<OpenProjectBody>,
) -> Response {
    match ensure_open_project(&state, &body.path, body.display_name.as_deref()).await {
        Ok(project) => Json(json!({
            "id": project.id,
            "path": project.path,
            "display_name": project_display_name(&project),
            "trusted": project.trusted,
        }))
        .into_response(),
        Err((status, message)) => api_err(status, message),
    }
}

async fn delete_project(State(state): State<Arc<DaemonState>>, Path(id): Path<String>) -> Response {
    // 移除登记不删除磁盘内容；有活跃会话时先拒绝，避免窗口引用悬空项目。
    {
        let sessions = state.sessions.lock().await;
        if sessions.values().any(|entry| entry.project_id == id) {
            return api_err(StatusCode::CONFLICT, "项目仍有活跃会话；先停止其代理任务");
        }
    }
    if let Some(runtime) = state.project_runtime(&id).await {
        state.lsp.close_project(&runtime.root).await;
    }
    state.close_project_runtime(&id).await;
    let mut store = state.store.lock().await;
    match store.list_sessions(&id) {
        Ok(sessions) if !sessions.is_empty() => {
            return api_err(StatusCode::CONFLICT, "项目有历史会话；登记不能删除");
        }
        Ok(_) => {}
        Err(e) => return api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
    match store.remove_project(&id) {
        Ok(true) => Json(json!({"removed": true, "disk_contents_deleted": false})).into_response(),
        Ok(false) => api_err(StatusCode::NOT_FOUND, "project not found"),
        Err(e) => api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

// ---------- 会话与回滚 ----------

#[derive(Deserialize)]
struct CreateSessionBody {
    /// v1.15 首选：稳定项目 ID（§6.4）。
    #[serde(default)]
    project_id: Option<String>,
    /// 兼容 v1.14 客户端；daemon 会 canonicalize 并复用既有项目。
    #[serde(default)]
    project_path: Option<String>,
    /// 项目内相对 / 绝对 cwd；命令仍在项目根沙箱内（B 级边界不变）。
    #[serde(default, alias = "cwd")]
    working_dir: Option<String>,
    /// v1.87 §9.7：`"managed"` = 会话级受管 worktree（git 仓库），可与主根会话并行。
    #[serde(default)]
    worktree: Option<String>,
    #[serde(default)]
    provider: String,
}

async fn create_session(
    State(state): State<Arc<DaemonState>>,
    Json(body): Json<CreateSessionBody>,
) -> Response {
    let provider = match state.provider_or_default(&body.provider).await {
        Ok(p) => p,
        Err(e) => return api_err(StatusCode::BAD_REQUEST, e),
    };
    let project = if let Some(project_id) = body.project_id.as_deref() {
        let Some(root) = state.project_root(project_id).await else {
            return api_err(StatusCode::NOT_FOUND, "project not found");
        };
        let stored_path = {
            let mut st = state.store.lock().await;
            match st.project(project_id) {
                Ok(Some(p)) if std::path::Path::new(&p.path) == root => p.path,
                Ok(Some(_p)) => return api_err(StatusCode::CONFLICT, "项目根不一致"),
                _ => return api_err(StatusCode::NOT_FOUND, "project not found"),
            }
        };
        match ensure_open_project(&state, &stored_path, None).await {
            Ok(project) => project,
            Err((status, message)) => return api_err(status, message),
        }
    } else if let Some(project_path) = body.project_path.as_deref() {
        match ensure_open_project(&state, project_path, None).await {
            Ok(p) => p,
            Err((status, message)) => return api_err(status, message),
        }
    } else {
        return api_err(StatusCode::CONFLICT, "PROJECT_REQUIRED");
    };
    if let Some(working_dir) = body.working_dir.as_deref() {
        let joined = std::path::Path::new(&project.path).join(working_dir);
        match std::fs::canonicalize(&joined) {
            Ok(p) if p.starts_with(&project.path) => {}
            _ => return api_err(StatusCode::BAD_REQUEST, "PATH_ESCAPE"),
        }
    }
    // v1.87 §9.7 会话级受管 worktree：先预生成会话 id，再在
    // `~/.tenon/worktrees/<project_id>/<session_id>/` 创建内核托管 worktree。
    // 非 git 仓库 / git 失败 → 400，不落会话行。
    let (managed_session_id, managed_worktree) = match body.worktree.as_deref() {
        None | Some("") | Some("root") => (None, None),
        Some("managed") => {
            let sid = tenon_store::Store::new_session_id();
            let pool = tenon_agent::subagents::WorktreePool::new(
                state
                    .worktrees_root
                    .join(sanitize_worktree_component(&project.id)),
            );
            match pool.create(std::path::Path::new(&project.path), &sid) {
                Ok(wt) => (Some(sid), Some(wt)),
                Err(e) => {
                    return api_err(
                        StatusCode::BAD_REQUEST,
                        format!("受管 worktree 创建失败（需 git 仓库）: {e}"),
                    )
                }
            }
        }
        Some(other) => {
            return api_err(
                StatusCode::BAD_REQUEST,
                format!("worktree 仅支持 \"managed\"，收到 {other:?}"),
            )
        }
    };
    let session = match create_agent_session(
        &state,
        &project,
        provider,
        body.working_dir.clone(),
        managed_session_id.clone(),
        managed_worktree.clone(),
    )
    .await
    {
        Ok(s) => s,
        Err((status, message)) => return api_err(status, message),
    };
    if let Some(wt) = &managed_worktree {
        let mut st = state.store.lock().await;
        let _ = st.set_session_worktree(&session.session_id, &wt.to_string_lossy());
    }
    let sid = register_session_entry(&state, &project, session).await;
    Json(json!({
        "session_id": sid,
        "project_id": project.id,
        "trusted": project.trusted,
        "worktree_path": managed_worktree.as_ref().map(|p| p.to_string_lossy()),
    }))
    .into_response()
}

#[derive(Deserialize)]
struct MessageBody {
    text: String,
}

async fn send_message(
    State(state): State<Arc<DaemonState>>,
    Path(id): Path<String>,
    Json(body): Json<MessageBody>,
) -> Response {
    let session = {
        let sessions = state.sessions.lock().await;
        match sessions.get(&id) {
            Some(e) => e.session.clone(),
            None => return api_err(StatusCode::NOT_FOUND, "session not found"),
        }
    };
    // v1.58 对话标题：判定须先于 run_task 追加 user_input 事件，否则首条
    // 消息会被误判为历史会话而走本地回填。
    let title_state = {
        let mut st = state.store.lock().await;
        let untitled = st
            .session(&id)
            .ok()
            .flatten()
            .map(|s| s.title.is_empty())
            .unwrap_or(false);
        let first_message = st
            .count_events_of_kind(&id, EventKind::UserInput)
            .map(|n| n == 0)
            .unwrap_or(false);
        (untitled, first_message)
    };
    // 后台执行任务；状态经 GET /session/{id} 轮询
    let state2 = state.clone();
    let sid = id.clone();
    let text = body.text.clone();
    let run_session = session.clone();
    tokio::spawn(async move {
        // §6.4 / §9.7：项目写锁在 AgentSession 内；这里提供跨项目全局上限。
        let _permit = state2
            .execution_permits
            .acquire()
            .await
            .map_err(|e| eprintln!("execution permit: {e}"));
        let outcome = run_session.run_task(&text).await;
        let mut sessions = state2.sessions.lock().await;
        if let Some(entry) = sessions.get_mut(&sid) {
            *entry.last_outcome.lock().await = Some(outcome);
        }
    });
    if title_state.0 {
        // 标题生成不阻塞任务循环：模型失败回退首条消息本地截断；历史遗留
        // 无标题会话直接回填首条 user_input，不再调模型。
        let state3 = state.clone();
        let sid2 = id.clone();
        let title_session = session.clone();
        let text2 = body.text.clone();
        tokio::spawn(async move {
            let title = if title_state.1 {
                title_session
                    .generate_title(&text2)
                    .await
                    .unwrap_or_else(|_| sanitize_title(&text2, 16))
            } else {
                let first_text = {
                    let mut st = state3.store.lock().await;
                    st.events(&sid2)
                        .ok()
                        .and_then(|events| {
                            events.into_iter().find(|e| e.kind == EventKind::UserInput)
                        })
                        .and_then(|e| {
                            e.payload
                                .get("text")
                                .and_then(|t| t.as_str())
                                .map(String::from)
                        })
                };
                match first_text {
                    Some(text) if !text.trim().is_empty() => sanitize_title(&text, 16),
                    _ => return,
                }
            };
            if title.is_empty() {
                return;
            }
            if let Err(e) = title_session.set_title(&title).await {
                eprintln!("session title: {e}");
            }
        });
    }
    (StatusCode::ACCEPTED, Json(json!({"accepted": true}))).into_response()
}

async fn get_session(State(state): State<Arc<DaemonState>>, Path(id): Path<String>) -> Response {
    let (status, seq) = {
        let mut store = state.store.lock().await;
        let sess = match store.session(&id) {
            Ok(Some(s)) => s,
            _ => return api_err(StatusCode::NOT_FOUND, "session not found"),
        };
        let seq = store.latest_seq(&id).unwrap_or(0);
        (sess.status, seq)
    };
    let last_outcome = {
        let sessions = state.sessions.lock().await;
        match sessions.get(&id) {
            Some(e) => Some(e.last_outcome.lock().await.clone()),
            None => None,
        }
    };
    let state_str = match status {
        SessionStatus::Idle => "idle",
        SessionStatus::Sensing => "sensing",
        SessionStatus::Deciding => "deciding",
        SessionStatus::Executing => "executing",
        SessionStatus::Verifying => "verifying",
        SessionStatus::Fixing => "fixing",
        SessionStatus::AwaitingApproval => "awaiting_approval",
        SessionStatus::Paused => "paused",
        SessionStatus::Error => "error",
        SessionStatus::Done => "done",
        SessionStatus::RolledBack => "rolled_back",
    };
    Json(json!({
        "session_id": id,
        "status": state_str,
        "latest_seq": seq,
        "outcome": last_outcome.flatten(),
    }))
    .into_response()
}

#[derive(Deserialize)]
struct ControlBody {
    action: String,
    #[serde(default)]
    value: Option<bool>,
}

async fn session_control(
    State(state): State<Arc<DaemonState>>,
    Path(id): Path<String>,
    Json(body): Json<ControlBody>,
) -> Response {
    let session = {
        let sessions = state.sessions.lock().await;
        match sessions.get(&id) {
            Some(e) => e.session.clone(),
            None => return api_err(StatusCode::NOT_FOUND, "session not found"),
        }
    };
    match body.action.as_str() {
        "pause" => session.control(ControlCommand::Pause),
        "resume" => session.control(ControlCommand::Resume),
        "stop" => session.control(ControlCommand::Stop),
        "set_readonly" => session.control(ControlCommand::SetReadonly(body.value.unwrap_or(true))),
        "rollback" => {
            return match session.rollback_last().await {
                Ok(files) => Json(json!({"rolled_back": files})).into_response(),
                Err(e) => api_err(StatusCode::CONFLICT, e.to_string()),
            };
        }
        "unrollback" => {
            return match session.unrevert().await {
                Ok(()) => Json(json!({"unrolled": true})).into_response(),
                Err(e) => api_err(StatusCode::CONFLICT, e.to_string()),
            };
        }
        other => return api_err(StatusCode::BAD_REQUEST, format!("未知 action: {other}")),
    }
    Json(json!({"ok": true})).into_response()
}

// ---------- 受管 worktree 收尾（v1.87 §9.7） ----------

/// worktree 路径组件消毒（§9.5 同语义；project id / session id 为 UUID，防御性兜底）。
fn sanitize_worktree_component(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn git_stdout(repo: &std::path::Path, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .ok()?;
    if out.status.success() {
        Some(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        None
    }
}

/// worktree 相对改动文件集 = 工作区 status（含未跟踪）∪ merge-base..HEAD 已提交 diff。
fn collect_worktree_changes(wt: &std::path::Path, project_root: &std::path::Path) -> Vec<String> {
    let mut changed = std::collections::BTreeSet::new();
    if let Some(out) = git_stdout(
        wt,
        &["status", "--porcelain", "-z", "--untracked-files=all"],
    ) {
        for entry in out.split('\0').filter(|s| !s.is_empty()) {
            // -z 格式：`XY <path>`；rename 目标与来源各占一段，来源段无状态前缀。
            let bytes = entry.as_bytes();
            if bytes.len() > 3 && bytes[2] == b' ' {
                changed.insert(entry[3..].to_string());
            } else {
                changed.insert(entry.to_string());
            }
        }
    }
    // 已提交改动：merge-base(HEAD, 主根分支)..HEAD（worktree 为 detached HEAD）。
    if let Some(branch) = git_stdout(project_root, &["rev-parse", "--abbrev-ref", "HEAD"]) {
        let branch = branch.trim();
        if let Some(base) = git_stdout(wt, &["merge-base", "HEAD", branch]) {
            let range = format!("{}..HEAD", base.trim());
            if let Some(out) = git_stdout(wt, &["diff", "--name-only", "-z", &range]) {
                for path in out.split('\0').filter(|s| !s.is_empty()) {
                    changed.insert(path.to_string());
                }
            }
        }
    }
    changed.into_iter().collect()
}

#[derive(Debug, serde::Serialize)]
struct WorktreeMergeReport {
    merged: Vec<String>,
    skipped: Vec<Value>,
    conflicts: Vec<Value>,
    /// 合并前项目根 shadow 快照 tree（§10.3 回滚原语；同一分片可 restore）。
    snapshot_tree: Option<String>,
}

/// 把受管 worktree 改动合入项目主根：project == base 直接覆写，否则三方合并；
/// 冲突文件不落盘并随报告返回三方内容（§8.6 合并预览数据）。合并前对项目根取
/// shadow 快照（§10.3 回滚原语）。
fn merge_worktree_into_root(
    snapshots_root: &std::path::Path,
    project_id: &str,
    max_untracked_mb: u64,
    project_root: &std::path::Path,
    wt: &std::path::Path,
    dirty_paths: &std::collections::BTreeSet<String>,
) -> Result<WorktreeMergeReport, String> {
    let changed = collect_worktree_changes(wt, project_root);
    let mut pending: Vec<String> = Vec::new();
    let mut skipped = Vec::new();
    for rel in &changed {
        if dirty_paths.contains(rel) {
            skipped.push(json!({"path": rel, "reason": "dirty_buffer"}));
        } else {
            pending.push(rel.clone());
        }
    }
    let mut snapshot_tree = None;
    if !pending.is_empty() {
        // 先对项目根取 shadow 快照（§10.3 回滚原语），再动用户工作区。
        let root_store =
            SnapshotStore::open(snapshots_root, project_id, project_root, max_untracked_mb)
                .map_err(|e| format!("快照库: {e}"))?;
        let tree = root_store
            .snapshot()
            .map_err(|e| format!("合并前快照失败: {e}"))?;
        snapshot_tree = Some(tree);
    }
    let head = git_stdout(wt, &["rev-parse", "HEAD"]).map(|s| s.trim().to_string());
    // worktree 为 detached HEAD：base = merge-base(HEAD, 主根分支)，取不到回退 HEAD。
    let base_commit = git_stdout(project_root, &["rev-parse", "--abbrev-ref", "HEAD"])
        .map(|b| b.trim().to_string())
        .and_then(|b| git_stdout(wt, &["merge-base", "HEAD", &b]).map(|s| s.trim().to_string()))
        .or_else(|| head.clone());
    let base_ref = base_commit.as_deref().unwrap_or("HEAD").to_string();

    // 阶段一：全量决策不写盘——任一冲突即整体返回（不静默部分合并）。
    enum Plan {
        Write(String),
        Delete,
        Skip(&'static str),
    }
    let mut plans: Vec<(String, Plan)> = Vec::new();
    let mut conflicts = Vec::new();
    for rel in &pending {
        let wt_path = wt.join(rel);
        let proj_path = project_root.join(rel);
        let base_spec = format!("{base_ref}:{rel}");
        let base_exists = git_stdout(wt, &["cat-file", "-e", &base_spec]).is_some();
        let base_content = if base_exists {
            git_stdout(wt, &["show", &base_spec])
        } else {
            None
        };
        let wt_exists = wt_path.exists();
        let wt_content = if wt_exists {
            std::fs::read_to_string(&wt_path).ok()
        } else {
            None
        };
        let proj_exists = proj_path.exists();
        let proj_content = if proj_exists {
            std::fs::read_to_string(&proj_path).ok()
        } else {
            None
        };
        // 二进制 / 不可读文件不参与自动合并（不静默覆盖）。
        if wt_exists && wt_content.is_none() {
            plans.push((rel.clone(), Plan::Skip("unreadable_worktree_file")));
            continue;
        }
        if proj_exists && proj_content.is_none() {
            plans.push((rel.clone(), Plan::Skip("unreadable_project_file")));
            continue;
        }
        match (wt_content, proj_content) {
            (None, None) => plans.push((rel.clone(), Plan::Skip("already_absent"))),
            (None, Some(proj)) => {
                // worktree 删除：主根与 base 一致才删；否则冲突（不静默覆盖）。
                if !base_exists {
                    plans.push((rel.clone(), Plan::Skip("already_absent")));
                } else if proj == base_content.clone().unwrap_or_default() {
                    plans.push((rel.clone(), Plan::Delete));
                } else {
                    conflicts.push(json!({
                        "path": rel,
                        "base": base_content.unwrap_or_default(),
                        "project": proj,
                        "worktree": String::new(),
                    }));
                }
            }
            (Some(wt_s), Some(proj)) => {
                if wt_s == proj {
                    plans.push((rel.clone(), Plan::Skip("already_applied")));
                } else if !base_exists {
                    // 双方各自新增同名文件：冲突交用户裁决。
                    conflicts.push(json!({
                        "path": rel,
                        "base": String::new(),
                        "project": proj,
                        "worktree": wt_s,
                    }));
                } else if proj == base_content.clone().unwrap_or_default() {
                    plans.push((rel.clone(), Plan::Write(wt_s)));
                } else {
                    match tenon_core::merge::merge_three_way(
                        &base_content.unwrap_or_default(),
                        &proj,
                        &wt_s,
                    ) {
                        Ok(m) => plans.push((rel.clone(), Plan::Write(m))),
                        Err(c) => conflicts.push(json!({
                            "path": rel,
                            "base": c.base,
                            "project": c.ours,
                            "worktree": c.theirs,
                        })),
                    }
                }
            }
            (Some(wt_s), None) => {
                // 主根缺失：base 也没有 → 落新文件；base 有 → 用户侧删除冲突。
                if !base_exists {
                    plans.push((rel.clone(), Plan::Write(wt_s)));
                } else {
                    conflicts.push(json!({
                        "path": rel,
                        "base": base_content.unwrap_or_default(),
                        "project": String::new(),
                        "worktree": wt_s,
                    }));
                }
            }
        }
    }

    // 阶段二：无冲突才应用写 / 删。
    let mut merged = Vec::new();
    if conflicts.is_empty() {
        for (rel, plan) in plans {
            match plan {
                Plan::Write(content) => {
                    if write_merged_file(&project_root.join(&rel), &content).is_ok() {
                        merged.push(rel);
                    } else {
                        skipped.push(json!({"path": rel, "reason": "write_failed"}));
                    }
                }
                Plan::Delete => {
                    if std::fs::remove_file(project_root.join(&rel)).is_ok() {
                        merged.push(rel);
                    } else {
                        skipped.push(json!({"path": rel, "reason": "delete_failed"}));
                    }
                }
                Plan::Skip(reason) => {
                    skipped.push(json!({"path": rel, "reason": reason}));
                }
            }
        }
    }
    Ok(WorktreeMergeReport {
        merged,
        skipped,
        conflicts,
        snapshot_tree,
    })
}

fn write_merged_file(path: &std::path::Path, content: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, content)
}

#[derive(Deserialize)]
struct WorktreeDiscardBody {
    /// 必须显式确认（设计 §9.7：丢弃不动用户根、不可默认）。
    confirm: bool,
}

type WorktreeParts = (String, std::path::PathBuf, std::path::PathBuf);

async fn managed_worktree_parts(
    state: &Arc<DaemonState>,
    id: &str,
) -> Result<WorktreeParts, (StatusCode, String)> {
    let sessions = state.sessions.lock().await;
    let Some(entry) = sessions.get(id) else {
        return Err((StatusCode::NOT_FOUND, "session not found".into()));
    };
    let Some(wt) = entry.managed_worktree.clone() else {
        return Err((StatusCode::CONFLICT, "NOT_MANAGED_WORKTREE".into()));
    };
    Ok((entry.project_id.clone(), entry.project_root.clone(), wt))
}

async fn merge_session_worktree(
    State(state): State<Arc<DaemonState>>,
    Path(id): Path<String>,
) -> Response {
    let (project_id, project_root, wt) = match managed_worktree_parts(&state, &id).await {
        Ok(parts) => parts,
        Err((status, message)) => return api_err(status, message),
    };
    let dirty = state
        .dirty_buffers_for(&project_id)
        .await
        .list()
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();
    let snapshots_root = state.snapshots_root.clone();
    let max_untracked_mb = state.config.checkpoint.max_untracked_mb;
    let report = tokio::task::spawn_blocking(move || {
        merge_worktree_into_root(
            &snapshots_root,
            &project_id,
            max_untracked_mb,
            &project_root,
            &wt,
            &dirty,
        )
    })
    .await;
    match report {
        Ok(Ok(report)) if report.conflicts.is_empty() => Json(json!(report)).into_response(),
        Ok(Ok(report)) => (
            StatusCode::CONFLICT,
            Json(json!({"error": "MERGE_CONFLICT", "report": report})),
        )
            .into_response(),
        Ok(Err(message)) => api_err(StatusCode::CONFLICT, message),
        Err(e) => api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn discard_session_worktree(
    State(state): State<Arc<DaemonState>>,
    Path(id): Path<String>,
    Json(body): Json<WorktreeDiscardBody>,
) -> Response {
    if !body.confirm {
        return api_err(StatusCode::BAD_REQUEST, "需要 confirm=true");
    }
    let (project_id, project_root, wt) = match managed_worktree_parts(&state, &id).await {
        Ok(parts) => parts,
        Err((status, message)) => return api_err(status, message),
    };
    let snapshots_root = state.snapshots_root.clone();
    let worktrees_root = state
        .worktrees_root
        .join(sanitize_worktree_component(&project_id));
    let result = tokio::task::spawn_blocking(move || {
        let pool = tenon_agent::subagents::WorktreePool::new(worktrees_root);
        pool.remove(&project_root, &id).map_err(|e| e.to_string())?;
        SnapshotStore::remove_worktree_shard(&snapshots_root, &project_id, &wt)
            .map_err(|e| e.to_string())?;
        Ok::<(), String>(())
    })
    .await;
    match result {
        Ok(Ok(())) => Json(json!({"discarded": true})).into_response(),
        Ok(Err(message)) => api_err(StatusCode::CONFLICT, message),
        Err(e) => api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn session_trace(
    State(state): State<Arc<DaemonState>>,
    Path(id): Path<String>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Response {
    let after: i64 = q.get("after").and_then(|s| s.parse().ok()).unwrap_or(0);
    let mut store = state.store.lock().await;
    let latest = store.latest_seq(&id).unwrap_or(0);
    match store.events_since(&id, after) {
        Ok(events) => Json(json!({"events": events, "latest_seq": latest})).into_response(),
        Err(e) => api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn session_checkpoints(
    State(state): State<Arc<DaemonState>>,
    Path(id): Path<String>,
) -> Response {
    let mut store = state.store.lock().await;
    match store.checkpoints(&id) {
        Ok(cps) => Json(json!({"checkpoints": cps})).into_response(),
        Err(e) => api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

#[derive(Deserialize)]
struct RollbackBody {}

async fn checkpoint_rollback(
    State(state): State<Arc<DaemonState>>,
    Path(checkpoint_id): Path<String>,
    Json(_body): Json<RollbackBody>,
) -> Response {
    let target = {
        let mut store = state.store.lock().await;
        match store.checkpoint(&checkpoint_id) {
            Ok(Some(cp)) => cp,
            _ => return api_err(StatusCode::NOT_FOUND, "checkpoint not found"),
        }
    };
    let session = {
        let sessions = state.sessions.lock().await;
        sessions.get(&target.session_id).map(|e| e.session.clone())
    };
    let Some(session) = session else {
        return api_err(StatusCode::NOT_FOUND, "session not found");
    };
    match session.rollback_last().await {
        Ok(files) => Json(json!({"rolled_back": files})).into_response(),
        Err(e) => api_err(StatusCode::CONFLICT, e.to_string()),
    }
}

// ---------- 模型路由（§11） ----------

#[derive(Deserialize)]
struct ModelSuggestBody {
    text: String,
}

/// 路由建议（§11 轻量启发式 + Laya 集成点 #4）：纯读任务建议轻模型；
/// 展示层一键采纳，不自动改派（§5 非目标 8）。
async fn model_suggest(
    State(state): State<Arc<DaemonState>>,
    Json(body): Json<ModelSuggestBody>,
) -> Response {
    // Laya 优先（本地 0 token）；不可用回退规则引擎（§9.8 整体回退）
    let (pure_read, source) = match state.laya.route_suggest_light(&body.text).await {
        tenon_laya::LayaOutcome::Success {
            value, duration_ms, ..
        } => (
            value,
            serde_json::json!({"laya": true, "duration_ms": duration_ms}),
        ),
        _ => (
            tenon_models::routing::is_pure_read_task(&body.text),
            serde_json::json!({"rule_engine": true}),
        ),
    };
    Json(json!({
        "pure_read": pure_read,
        "source": source,
        "default": state.default_provider.read().unwrap().clone(),
    }))
    .into_response()
}

#[derive(Deserialize)]
struct SwitchModelBody {
    provider: String,
}

/// 会话切换模型（§11：上下文随迁；model_fallback 事件入 Trace）。
async fn switch_session_model(
    State(state): State<Arc<DaemonState>>,
    Path(id): Path<String>,
    Json(body): Json<SwitchModelBody>,
) -> Response {
    let new_provider = match state.provider_or_default(&body.provider).await {
        Ok(p) => p,
        Err(e) => return api_err(StatusCode::BAD_REQUEST, e),
    };
    let session = {
        let sessions = state.sessions.lock().await;
        sessions.get(&id).map(|e| e.session.clone())
    };
    let Some(session) = session else {
        return api_err(StatusCode::NOT_FOUND, "session not found");
    };
    session.switch_provider(new_provider).await;
    Json(json!({
        "session_id": id,
        "model": session.current_model().await,
    }))
    .into_response()
}

// ---------- 编辑器与文件 ----------

#[derive(Deserialize)]
struct TreeQuery {
    /// 懒加载目录；空 = 项目根（§8.1）。
    #[serde(default)]
    path: Option<String>,
}

#[derive(Deserialize)]
struct FuzzyFilesQuery {
    #[serde(default)]
    q: String,
    #[serde(default = "default_fuzzy_limit")]
    limit: usize,
}

fn default_fuzzy_limit() -> usize {
    100
}

/// fuzzy 打开（§7.4）：遍历 gitignore-aware 项目树后本地打分排序。
async fn fuzzy_files(
    State(state): State<Arc<DaemonState>>,
    Path(id): Path<String>,
    Query(query): Query<FuzzyFilesQuery>,
) -> Response {
    let Some(project_path) = state.ensure_project_runtime(&id).await else {
        return api_err(StatusCode::NOT_FOUND, "project not found");
    };
    let limit = query.limit.clamp(1, 500);
    let statuses = tenon_fs::git::status_map(&project_path);
    let result = tokio::task::spawn_blocking(move || -> Result<Value, String> {
        let tree = tenon_fs::tree::full_tree(&project_path).map_err(|e| e.to_string())?;
        let candidates: Vec<&str> = tree
            .iter()
            .filter(|entry| entry.kind == tenon_fs::tree::EntryKind::File)
            .map(|entry| entry.path.as_str())
            .collect();
        let hits = tenon_fs::fuzzy::fuzzy_filter(candidates, &query.q)
            .into_iter()
            .take(limit)
            .map(|hit| {
                let entry = tree
                    .iter()
                    .find(|entry| entry.path == hit.text)
                    .expect("candidate comes from tree");
                let status = statuses
                    .get(&entry.path)
                    .copied()
                    .unwrap_or(tenon_fs::tree::GitStatus::Clean);
                json!({
                    "path": entry.path,
                    "name": entry.name,
                    "kind": entry.kind,
                    "git_status": status,
                    "score": hit.score,
                })
            })
            .collect::<Vec<_>>();
        Ok(json!({"hits": hits, "query": query.q}))
    })
    .await;
    match result {
        Ok(Ok(value)) => Json(value).into_response(),
        Ok(Err(e)) => api_err(StatusCode::INTERNAL_SERVER_ERROR, e),
        Err(e) => api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn project_tree(
    State(state): State<Arc<DaemonState>>,
    Path(id): Path<String>,
    Query(query): Query<TreeQuery>,
) -> Response {
    let Some(project_path) = state.ensure_project_runtime(&id).await else {
        return api_err(StatusCode::NOT_FOUND, "project not found");
    };
    let rel = match query.path.as_deref() {
        None | Some("") | Some(".") => String::new(),
        Some(path) => {
            let joined = project_path.join(path);
            let canonical = match std::fs::canonicalize(&joined) {
                Ok(p) => p,
                Err(e) => return api_err(StatusCode::BAD_REQUEST, e.to_string()),
            };
            if !canonical.starts_with(&project_path) {
                return api_err(StatusCode::BAD_REQUEST, "PATH_ESCAPE");
            }
            match canonical.strip_prefix(&project_path) {
                Ok(rel) => rel.to_string_lossy().into_owned(),
                Err(_) => return api_err(StatusCode::BAD_REQUEST, "PATH_ESCAPE"),
            }
        }
    };
    let statuses = tenon_fs::git::status_map(&project_path);
    match tenon_fs::tree::list_dir(&project_path, &rel, &statuses) {
        Ok(entries) => Json(json!({"entries": entries})).into_response(),
        Err(e) => api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

#[derive(Deserialize)]
struct FilePathQuery {
    path: String,
}

async fn read_file(
    State(state): State<Arc<DaemonState>>,
    Path(project_id): Path<String>,
    Query(q): Query<FilePathQuery>,
) -> Response {
    let Some(root) = state.ensure_project_runtime(&project_id).await else {
        return api_err(StatusCode::NOT_FOUND, "project not found");
    };
    // 全量读取放入阻塞线程（§8.1 v1.69）：任意大小文件可打开，IO 不阻塞异步运行时。
    let path = q.path.clone();
    let view = tokio::task::spawn_blocking(move || {
        tenon_fs::FileService::new(&root).read_file_view(&path)
    })
    .await;
    let view = match view {
        Ok(Ok(view)) => view,
        Ok(Err(e)) => return api_err(StatusCode::BAD_REQUEST, e.to_string()),
        Err(e) => return api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    Json(json!({
        "path": q.path,
        "content": view.content,
        "total_bytes": view.total_bytes,
    }))
    .into_response()
}

#[derive(Deserialize)]
struct WriteFileBody {
    path: String,
    content: String,
}

async fn write_file(
    State(state): State<Arc<DaemonState>>,
    Path(project_id): Path<String>,
    Json(body): Json<WriteFileBody>,
) -> Response {
    let Some(root) = state.ensure_project_runtime(&project_id).await else {
        return api_err(StatusCode::NOT_FOUND, "project not found");
    };
    let fs = tenon_fs::FileService::new(root);
    match fs.write_file(&body.path, &body.content) {
        Ok(created) => Json(json!({"ok": true, "created": created})).into_response(),
        Err(e) => api_err(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

/// daemon 侧 tree-sitter 基础高亮 token 流（§8.2 / §16）。
/// 当前 Rust grammar 已接入；其它语言回退 Monaco 基础高亮。
async fn syntax_highlight(
    State(state): State<Arc<DaemonState>>,
    Path(project_id): Path<String>,
    Query(q): Query<FilePathQuery>,
) -> Response {
    let Some(root) = state.ensure_project_runtime(&project_id).await else {
        return api_err(StatusCode::NOT_FOUND, "project not open");
    };
    // tree-sitter 解析同步且无增量缓存：先按大小挡下超帽文件（不做全量读取），
    // 读取与解析都在阻塞线程执行，大文件不拖累其它请求（§8.7）。
    let path = q.path.clone();
    let source = tokio::task::spawn_blocking(move || {
        let fs = tenon_fs::FileService::new(&root);
        match fs.file_size(&path) {
            Ok(size) if size > 2 * 1024 * 1024 => Ok(None),
            Ok(_) => fs
                .read_file_view(&path)
                .map(|view| Some(view.content))
                .map_err(|e| e.to_string()),
            Err(e) => Err(e.to_string()),
        }
    })
    .await;
    let source = match source {
        Ok(Ok(Some(source))) => source,
        Ok(Ok(None)) => {
            return api_err(
                StatusCode::PAYLOAD_TOO_LARGE,
                "syntax highlight supports files up to 2MB",
            )
        }
        Ok(Err(e)) => return api_err(StatusCode::BAD_REQUEST, e),
        Err(e) => return api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let worker_path = q.path.clone();
    let worker_source = source.clone();
    let tokens =
        tokio::task::spawn_blocking(move || tenon_fs::highlight_file(&worker_path, &worker_source))
            .await;
    let token_to_value = |source: &str, token: tenon_fs::HighlightToken| {
        let lines: Vec<&str> = source.split_inclusive('\n').collect();
        let utf16 = |line_index: usize, byte_column: usize| -> usize {
            lines
                .get(line_index)
                .map(|line| {
                    let mut boundary = byte_column.min(line.len());
                    while boundary > 0 && !line.is_char_boundary(boundary) {
                        boundary -= 1;
                    }
                    let boundary = if line.is_char_boundary(boundary) {
                        boundary
                    } else {
                        (0..boundary)
                            .rev()
                            .find(|offset| line.is_char_boundary(*offset))
                            .unwrap_or(0)
                    };
                    line[..boundary].chars().map(char::len_utf16).sum()
                })
                .unwrap_or(0)
        };
        json!({
            "kind": token.kind,
            "start_line": token.start_row,
            "start_column": utf16(token.start_row, token.start_col),
            "end_line": token.end_row,
            "end_column": utf16(token.end_row, token.end_col),
        })
    };
    match tokens {
        Ok(Some(tokens)) => Json(json!({
            "path": q.path,
            "tokens": tokens
                .iter()
                .map(|token| token_to_value(&source, token.clone()))
                .collect::<Vec<_>>(),
            "language": "rust",
        }))
        .into_response(),
        Ok(None) => Json(json!({
            "path": q.path,
            "tokens": [],
            "language": null,
            "fallback": true,
        }))
        .into_response(),
        Err(e) => api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

#[derive(Deserialize)]
struct GitSourceViewQuery {
    /// 活动文件路径；提供时返回 inline blame。
    #[serde(default)]
    path: Option<String>,
}

async fn git_source_view(
    State(state): State<Arc<DaemonState>>,
    Path(project_id): Path<String>,
    Query(query): Query<GitSourceViewQuery>,
) -> Response {
    let Some(root) = state.ensure_project_runtime(&project_id).await else {
        return api_err(StatusCode::NOT_FOUND, "project not open");
    };
    let blame_path = query.path.filter(|path| !path.is_empty());
    let result: Result<Result<serde_json::Value, String>, tokio::task::JoinError> =
        tokio::task::spawn_blocking(move || {
        if !tenon_fs::git::is_repository(&root) {
            return Ok(json!({"repository": false, "branch": null, "branches": [], "changes": [], "commits": [], "blame": null}));
        }
        let blame = match blame_path {
            Some(path) => {
                let fs = tenon_fs::FileService::new(root.clone());
                let _validated = fs.resolve(&path).map_err(|e| e.to_string())?;
                Some(json!({
                    "path": path,
                    "lines": tenon_fs::git::blame_file(&root, &path).unwrap_or_default(),
                }))
            }
            None => None,
        };
        Ok(json!({
            "repository": true,
            "branch": tenon_fs::git::current_branch(&root),
            "branches": tenon_fs::git::list_branches(&root),
            "changes": tenon_fs::git::changed_files(&root),
            "commits": tenon_fs::git::recent_commits(&root, 20),
            "blame": blame,
        }))
    })
    .await;
    match result {
        Ok(Ok(value)) => Json(value).into_response(),
        Ok(Err(message)) => api_err(StatusCode::BAD_REQUEST, message),
        Err(e) => api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

#[derive(Deserialize)]
struct FileOpsBody {
    ops: Vec<tenon_fs::FileOp>,
}

async fn file_ops(
    State(state): State<Arc<DaemonState>>,
    Path(project_id): Path<String>,
    Json(body): Json<FileOpsBody>,
) -> Response {
    let Some(root) = state.ensure_project_runtime(&project_id).await else {
        return api_err(StatusCode::NOT_FOUND, "project not found");
    };
    let ops = tenon_fs::FileOps::new(root);
    let mut results = Vec::new();
    for op in &body.ops {
        match ops.apply(op) {
            Ok(o) => results.push(json!({"ok": true, "outcome": o})),
            Err(e) => results.push(json!({"ok": false, "error": e.to_string()})),
        }
    }
    Json(json!({"results": results})).into_response()
}

#[derive(Deserialize)]
struct SearchQuery {
    q: String,
    #[serde(default)]
    replace: Option<String>,
}

async fn search(
    State(state): State<Arc<DaemonState>>,
    Path(project_id): Path<String>,
    Query(q): Query<SearchQuery>,
) -> Response {
    let Some(root) = state.ensure_project_runtime(&project_id).await else {
        return api_err(StatusCode::NOT_FOUND, "project not found");
    };
    let opts = tenon_fs::SearchOptions::default();
    match q.replace {
        Some(replacement) => {
            match tenon_fs::search::replace_preview(&root, &q.q, &replacement, &opts) {
                Ok(previews) => {
                    let items: Vec<Value> = previews
                        .into_iter()
                        .map(|(path, diff)| json!({"path": path, "diff": diff}))
                        .collect();
                    Json(json!({"previews": items})).into_response()
                }
                Err(e) => api_err(StatusCode::BAD_REQUEST, e.to_string()),
            }
        }
        None => match tenon_fs::search::search(&root, &q.q, &opts) {
            Ok(hits) => Json(json!({"hits": hits})).into_response(),
            Err(e) => api_err(StatusCode::BAD_REQUEST, e.to_string()),
        },
    }
}

#[derive(Deserialize)]
struct SearchReplaceBody {
    q: String,
    replacement: String,
    paths: Vec<String>,
}

async fn apply_search_replace(
    State(state): State<Arc<DaemonState>>,
    Path(project_id): Path<String>,
    Json(body): Json<SearchReplaceBody>,
) -> Response {
    let Some(root) = state.ensure_project_runtime(&project_id).await else {
        return api_err(StatusCode::NOT_FOUND, "project not found");
    };
    if body.paths.is_empty() {
        return api_err(StatusCode::BAD_REQUEST, "未选择要替换的文件");
    }
    // §8.6：未保存缓冲是用户工作区的一部分；全局替换不得静默覆盖。
    let buffers = state.dirty_buffers_for(&project_id).await;
    let mut clean = Vec::new();
    let mut skipped = Vec::new();
    for path in body.paths {
        if buffers.get(&path).is_none() {
            clean.push(path);
        } else {
            skipped.push(json!({"path": path, "reason": "dirty buffer"}));
        }
    }
    let report = tokio::task::spawn_blocking(move || {
        tenon_fs::search::replace_selected(
            &root,
            &body.q,
            &body.replacement,
            &clean,
            &tenon_fs::SearchOptions::default(),
        )
    })
    .await;
    let report = match report {
        Ok(Ok(report)) => report,
        Ok(Err(e)) => return api_err(StatusCode::BAD_REQUEST, e.to_string()),
        Err(e) => return api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    for (path, error) in report.failed {
        skipped.push(json!({"path": path, "reason": error}));
    }
    Json(json!({"applied": report.applied, "skipped": skipped})).into_response()
}

#[derive(Deserialize)]
struct LspBody {
    path: String,
    /// completion | hover | definition | references | diagnostics | format
    action: String,
    #[serde(default)]
    line: u32,
    #[serde(default)]
    character: u32,
    /// rename → new_name；workspace_symbol → query
    #[serde(default)]
    extra: Option<String>,
}

#[derive(Deserialize)]
struct ApplyLspEditBody {
    /// LSP WorkspaceEdit（changes / documentChanges 均支持）。
    workspace_edit: Value,
    /// 关联会话用于 checkpoint 时间轴；必须属于当前 project。
    session_id: Option<String>,
}

async fn apply_lsp_workspace_edit(
    State(state): State<Arc<DaemonState>>,
    Path(project_id): Path<String>,
    Json(body): Json<ApplyLspEditBody>,
) -> Response {
    let Some(root) = state.ensure_project_runtime(&project_id).await else {
        return api_err(StatusCode::NOT_FOUND, "project not open");
    };

    // LSP workspace edit 是 B 级写：未保存缓冲不得被服务器结果静默覆盖。
    let dirty = state.dirty_buffers_for(&project_id).await;
    let planned = match crate::lsp_edit::plan_workspace_edit(&root, &body.workspace_edit) {
        Ok(files) => files,
        Err(e) => return api_err(StatusCode::BAD_REQUEST, e),
    };
    let conflicts: Vec<String> = planned
        .iter()
        .filter(|file| dirty.is_dirty(&file.path))
        .map(|file| file.path.clone())
        .collect();
    if !conflicts.is_empty() {
        return api_err(
            StatusCode::CONFLICT,
            format!("dirty buffer conflict: {}", conflicts.join(", ")),
        );
    }

    let session_id = if let Some(session_id) = body.session_id.as_deref() {
        let mut store = state.store.lock().await;
        match store.session(session_id) {
            Ok(Some(session)) if session.project_id == project_id => session_id.to_string(),
            _ => return api_err(StatusCode::CONFLICT, "session does not belong to project"),
        }
    } else {
        return api_err(
            StatusCode::BAD_REQUEST,
            "session_id is required for checkpoint",
        );
    };

    // 独立 shadow 快照（§10.3）：先记 before，原子写全量；任一失败 restore(before)。
    let snapshots = SnapshotStore::open(
        &state.snapshots_root,
        &project_id,
        &root,
        state.config.checkpoint.max_untracked_mb,
    );
    let snapshots = match snapshots {
        Ok(snapshots) => snapshots,
        Err(e) => return api_err(StatusCode::INTERNAL_SERVER_ERROR, format!("快照库: {e}")),
    };
    let before_tree = match tokio::task::spawn_blocking(move || snapshots.snapshot()).await {
        Ok(Ok(tree)) => tree,
        Ok(Err(e)) => return api_err(StatusCode::CONFLICT, format!("快照失败: {e}")),
        Err(e) => return api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };

    let mut write_error: Option<String> = None;
    for file in &planned {
        let absolute = root.join(&file.path);
        if let Some(parent) = absolute.parent() {
            if let Err(e) = tokio::fs::create_dir_all(parent).await {
                write_error = Some(format!("创建目录 {}: {e}", parent.display()));
                break;
            }
        }
        if let Err(e) = tokio::fs::write(&absolute, &file.after).await {
            write_error = Some(format!("写入 {}: {e}", file.path));
            break;
        }
    }
    if let Some(error) = write_error {
        let restore_snapshots = SnapshotStore::open(
            &state.snapshots_root,
            &project_id,
            &root,
            state.config.checkpoint.max_untracked_mb,
        );
        if let Ok(snapshots) = restore_snapshots {
            let _ = tokio::task::spawn_blocking(move || snapshots.restore(&before_tree)).await;
        }
        return api_err(StatusCode::CONFLICT, format!("LSP edit 写入失败: {error}"));
    }

    let after_snapshots = SnapshotStore::open(
        &state.snapshots_root,
        &project_id,
        &root,
        state.config.checkpoint.max_untracked_mb,
    );
    let after_tree = match after_snapshots {
        Ok(snapshots) => match tokio::task::spawn_blocking(move || snapshots.snapshot()).await {
            Ok(Ok(tree)) => tree,
            Ok(Err(e)) => {
                let restore = SnapshotStore::open(
                    &state.snapshots_root,
                    &project_id,
                    &root,
                    state.config.checkpoint.max_untracked_mb,
                );
                if let Ok(snapshots) = restore {
                    let _ =
                        tokio::task::spawn_blocking(move || snapshots.restore(&before_tree)).await;
                }
                return api_err(StatusCode::CONFLICT, format!("快照失败: {e}"));
            }
            Err(e) => return api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        },
        Err(e) => return api_err(StatusCode::INTERNAL_SERVER_ERROR, format!("快照库: {e}")),
    };

    let checkpoint = {
        let mut store = state.store.lock().await;
        store.insert_checkpoint(
            &session_id,
            &before_tree,
            &planned
                .iter()
                .map(|file| file.path.clone())
                .collect::<Vec<_>>(),
            // 负 seq 标记编辑器原子操作；既有 rollback_last 可选中 LSP checkpoint。
            Some(-1),
        )
    };
    let checkpoint = match checkpoint {
        Ok(checkpoint) => checkpoint,
        Err(e) => return api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };

    Json(json!({
        "applied": true,
        "checkpoint_id": checkpoint.id,
        "before_tree": before_tree,
        "after_tree": after_tree,
        "files": planned.iter().map(|file| json!({
            "path": file.path,
            "diff": file.diff,
        })).collect::<Vec<_>>(),
    }))
    .into_response()
}

/// LSP 代理（project-scoped，§8.5 / §15）：路径参数为唯一项目来源（v1.92 修复
/// 假作用域——此前 Path 参数被忽略、实际依赖 body 传项目）。
async fn project_lsp_proxy(
    State(state): State<Arc<DaemonState>>,
    Path(project_id): Path<String>,
    Json(body): Json<LspBody>,
) -> Response {
    let Some(project_root) = state.ensure_project_runtime(&project_id).await else {
        return api_err(StatusCode::NOT_FOUND, "project not found");
    };
    // §8.5 / §15：共享 LSP 宿主语义端点
    match state
        .lsp
        .request(
            &project_root,
            &body.path,
            &body.action,
            body.line,
            body.character,
            body.extra.as_deref(),
        )
        .await
    {
        Ok(result) => Json(json!({ "result": result })).into_response(),
        Err(tenon_lsp::LspManagerError::BadPath(p)) => {
            api_err(StatusCode::BAD_REQUEST, format!("路径越界: {p}"))
        }
        Err(e) => api_err(StatusCode::SERVICE_UNAVAILABLE, e.to_string()),
    }
}

#[derive(Deserialize)]
struct InlineCompleteBody {
    session_id: String,
    path: String,
    #[serde(default)]
    language: String,
    prefix: String,
    #[serde(default)]
    suffix: String,
}

/// AI ghost text（P2 实验；UI 默认关闭）：单轮、只读、不进入工具循环。
async fn inline_complete(
    State(state): State<Arc<DaemonState>>,
    Path(project_id): Path<String>,
    Json(body): Json<InlineCompleteBody>,
) -> Response {
    let session = {
        let sessions = state.sessions.lock().await;
        match sessions.get(&body.session_id) {
            Some(entry) if entry.project_id == project_id => Some(entry.session.clone()),
            _ => None,
        }
    };
    let Some(session) = session else {
        return api_err(StatusCode::CONFLICT, "session does not belong to project");
    };
    // 只发送光标附近窗口，避免把大文件上下文整份送出。
    const WINDOW: usize = 24_000;
    let prefix: String = body
        .prefix
        .chars()
        .rev()
        .take(WINDOW)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let suffix: String = body.suffix.chars().take(WINDOW).collect();
    let language = if body.language.trim().is_empty() {
        std::path::Path::new(&body.path)
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("text")
            .to_string()
    } else {
        body.language
    };
    match session.inline_complete(&language, &prefix, &suffix).await {
        Ok(completion) => Json(json!({
            "completion": completion,
            "provider": "active-session",
        }))
        .into_response(),
        Err(e) => api_err(StatusCode::SERVICE_UNAVAILABLE, e.to_string()),
    }
}

// ---------- 插件 registry（§13 / M2） ----------

/// 已装插件列表（store plugins 表，§14.2）。
async fn list_plugins(State(state): State<Arc<DaemonState>>) -> Response {
    let mut store = state.store.lock().await;
    let installed: Vec<Value> = store
        .list_plugins()
        .unwrap_or_default()
        .into_iter()
        .map(|p| {
            json!({
                "id": p.id, "version": p.version,
                "permissions": p.permissions, "signature": p.signature,
                "installed_at": p.installed_at,
            })
        })
        .collect();
    Json(json!({ "installed": installed })).into_response()
}

#[derive(Deserialize)]
struct RegistrySearchBody {
    query: String,
    #[serde(default)]
    registry_url: Option<String>,
}

/// registry 检索（§13.2 检索）。
async fn search_registry(
    State(_state): State<Arc<DaemonState>>,
    Json(body): Json<RegistrySearchBody>,
) -> Response {
    let index = match tenon_registry::fetch_index(body.registry_url.as_deref()).await {
        Ok(i) => i,
        Err(e) => return api_err(StatusCode::BAD_GATEWAY, format!("registry 不可达: {e}")),
    };
    let hits: Vec<Value> = tenon_registry::search_index(&index, &body.query)
        .into_iter()
        .map(|e| json!(e))
        .collect();
    Json(json!({ "hits": hits })).into_response()
}

#[derive(Deserialize)]
struct InstallPluginBody {
    /// registry 条目（客户端从检索结果取得）；或直接给 manifest YAML
    entry: tenon_registry::RegistryEntry,
    /// 已装版本权限（客户端从 GET /plugins 取；用于权限 diff 展示）
    #[serde(default)]
    installed_permissions: Vec<String>,
    #[serde(default)]
    public_key: Option<String>,
}

async fn install_plugin(
    State(state): State<Arc<DaemonState>>,
    Json(body): Json<InstallPluginBody>,
) -> Response {
    use tenon_registry as treg;
    // 签名校验（官方条目走发布公钥解析链；社区条目须带公钥）
    let pk = body
        .public_key
        .clone()
        .or_else(treg::load_public_key)
        .unwrap_or_else(|| "0".repeat(64));
    if let Err(e) = treg::verify_entry(&body.entry, &pk) {
        eprintln!(
            "[plugins-debug] verify_entry failed pk_prefix={} entry_sig_empty={} sha_len={}",
            &pk[..8.min(pk.len())],
            body.entry.signature.is_empty(),
            body.entry.sha256.len()
        );
        return api_err(StatusCode::BAD_GATEWAY, format!("签名校验失败: {e}"));
    }
    // 下载 + SHA-256 校验
    let pkg_bytes = match treg::download_entry(&treg::InstallPlan {
        version: 0,
        sha256: body.entry.sha256.clone(),
        signature: body.entry.signature.clone(),
        url: body.entry.url.clone(),
        size_bytes: None,
    })
    .await
    {
        Ok(b) => b,
        Err(e) => return api_err(StatusCode::BAD_GATEWAY, format!("下载失败: {e}")),
    };
    let manifest = match treg::parse_manifest(&String::from_utf8_lossy(pkg_bytes.as_slice())) {
        Ok(m) => m,
        Err(e) => return api_err(StatusCode::BAD_GATEWAY, format!("manifest 解析失败: {e}")),
    };
    // 保留字安装期拦截（§12.5）
    if let Err(e) = treg::pre_install_check(&manifest, body.entry.id.starts_with("official.")) {
        return api_err(StatusCode::FORBIDDEN, e.to_string());
    }

    // 权限 diff（§13.2：新增权限高亮）
    let diff = treg::permission_diff(&body.installed_permissions, &manifest.permissions);

    // 版本锁定 + 入库（安装到 ~/.tenon/plugins/，§14.1）
    let dir = tenon_config::Config::data_dir().join("plugins");
    std::fs::create_dir_all(&dir).ok();
    let pkg_path = dir.join(format!(
        "{}-{}.yaml",
        manifest.id.replace('/', "_"),
        manifest.version
    ));
    std::fs::write(&pkg_path, pkg_bytes.as_slice()).ok();
    let mut st = state.store.lock().await;
    st.insert_plugin(
        &manifest.id,
        &manifest.version,
        &manifest.permissions,
        &body.entry.signature,
    )
    .ok();
    Json(json!({
        "installed": true,
        "id": manifest.id,
        "version": manifest.version,
        "permission_diff": diff,
    }))
    .into_response()
}

// ---------- 脏缓冲（§8.6 人机共编） ----------

#[derive(Deserialize)]
struct DirtyBufferBody {
    path: String,
    dirty: String,
}

async fn set_dirty_buffer(
    State(state): State<Arc<DaemonState>>,
    Path(id): Path<String>,
    Json(body): Json<DirtyBufferBody>,
) -> Response {
    let Some(project_root) = project_root_by_id(&state, &id).await else {
        return api_err(StatusCode::NOT_FOUND, "project not found");
    };
    let buffers = state.dirty_buffers_for(&id).await;
    let base = std::fs::read_to_string(project_root.join(&body.path)).unwrap_or_default();
    buffers.set(&body.path, &body.dirty, &base);
    Json(json!({"ok": true, "dirty": true})).into_response()
}

#[derive(Deserialize)]
struct BufferQuery {
    path: String,
}

async fn clear_dirty_buffer(
    State(state): State<Arc<DaemonState>>,
    Path(id): Path<String>,
    Query(q): Query<BufferQuery>,
) -> Response {
    let buffers = state.dirty_buffers_for(&id).await;
    buffers.clear(&q.path);
    Json(json!({ "ok": true, "dirty": false })).into_response()
}

async fn project_root_by_id(state: &Arc<DaemonState>, id: &str) -> Option<std::path::PathBuf> {
    state.ensure_project_runtime(id).await
}

// ---------- 语言包向导（§8.4） ----------

async fn detect_language_packs(
    State(state): State<Arc<DaemonState>>,
    Path(id): Path<String>,
) -> Response {
    let Some(project_root) = project_root_by_id(&state, &id).await else {
        return api_err(StatusCode::NOT_FOUND, "project not found");
    };
    let packs: Vec<Value> = tenon_lsp::pack::builtin_packs()
        .into_iter()
        .map(|p| {
            let detected = p
                .detect_files
                .iter()
                .any(|f| project_root.join(f).exists());
            let server_installed = tenon_lsp::manager::command_on_path(&p.command);
            let runtime_hint = if p.language == "typescript" && !server_installed {
                Some("官方指引：npm install -g typescript-language-server typescript@5.8.3（或一键安装）")
            } else if p.language == "python" && !server_installed {
                Some("官方指引：npm install -g pyright（或一键安装）")
            } else {
                None
            };
            json!({
                "language": p.language,
                "command": p.command,
                "detected": detected,
                "server_installed": server_installed,
                "runtime_hint": runtime_hint,
            })
        })
        .filter(|p| p["detected"].as_bool().unwrap_or(false))
        .collect();
    Json(json!({ "packs": packs })).into_response()
}

#[derive(Deserialize)]
struct InstallPackBody {
    /// typescript | python
    pack: String,
}

async fn install_language_pack(
    State(state): State<Arc<DaemonState>>,
    Path(id): Path<String>,
    Json(body): Json<InstallPackBody>,
) -> Response {
    let Some(_project_root) = project_root_by_id(&state, &id).await else {
        return api_err(StatusCode::NOT_FOUND, "project not found");
    };
    let command = match body.pack.as_str() {
        "typescript" => "npm install -g typescript-language-server typescript@5.8.3",
        "python" => "npm install -g pyright",
        other => return api_err(StatusCode::BAD_REQUEST, format!("未知语言包: {other}")),
    };
    let out = tenon_sandbox::exec_command(
        command,
        &std::env::temp_dir(),
        std::time::Duration::from_secs(300),
        &tenon_sandbox::SandboxSpec::None, // 全局安装：v1.89 直执，安装路径固定
    );
    match out {
        Ok(o) if o.success() => Json(json!({"installed": true, "pack": body.pack})).into_response(),
        Ok(o) => api_err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("安装失败: {} {}", o.stdout.trim(), o.stderr.trim()),
        ),
        Err(e) => api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

// ---------- 管理 ----------

async fn list_projects(State(state): State<Arc<DaemonState>>) -> Response {
    let projects_with_sessions = {
        let state = state.clone();
        async move {
            let mut store = state.store.lock().await;
            let projects = store.list_projects().ok()?;
            let mut with_sessions = Vec::new();
            for project in &projects {
                with_sessions.push((
                    project.clone(),
                    store.list_sessions(&project.id).unwrap_or_default(),
                    store.project_usage_totals(&project.id).unwrap_or_default(),
                ));
            }
            Some(with_sessions)
        }
    }
    .await;
    let Some(projects) = projects_with_sessions else {
        return api_err(StatusCode::INTERNAL_SERVER_ERROR, "项目清单读取失败");
    };
    // 活跃 runtime 会话（§6.2：runtime 不跨重启）；UI 由此区分可直接
    // 操作的会话与仅存 DB 的历史会话，避免死会话操作 404。
    let session_runtimes: Vec<String> = state.sessions.lock().await.keys().cloned().collect();
    let summaries: Vec<Value> = projects
        .into_iter()
        .map(|(project, sessions, usage)| {
            let active = sessions
                .iter()
                .filter(|s| {
                    matches!(
                        s.status,
                        SessionStatus::Executing | SessionStatus::Verifying | SessionStatus::Fixing
                    )
                })
                .count();
            json!({
                "id": project.id,
                "path": project.path,
                "display_name": project_display_name(&project),
                "trusted": project.trusted,
                "language_packs": project.language_packs,
                "dirty_buffers": state
                    .dirty_buffers
                    .try_lock()
                    .map(|buffers| {
                        buffers
                            .get(&project.id)
                            .map(|registry| registry.list().len())
                            .unwrap_or(0)
                    })
                    .unwrap_or(0),
                "usage": {
                    "input_tokens": usage.0,
                    "output_tokens": usage.1,
                    "cost_usd": usage.2,
                },
                "sessions": sessions.iter().map(|s| json!({
                    "id": s.id,
                    "status": s.status.as_str(),
                    "model": s.model,
                    "title": s.title,
                    "worktree_path": s.worktree_path,
                    "updated_at": s.updated_at,
                })).collect::<Vec<_>>(),
                "session_runtimes": session_runtimes,
                "active_sessions": active,
            })
        })
        .collect();
    Json(json!({"projects": summaries, "config": state.config.projects})).into_response()
}

#[derive(Deserialize)]
struct TrustBody {
    project_id: String,
    trusted: bool,
}

async fn set_project_trust(
    State(state): State<Arc<DaemonState>>,
    Json(body): Json<TrustBody>,
) -> Response {
    let mut store = state.store.lock().await;
    match store.set_project_trusted(&body.project_id, body.trusted) {
        Ok(()) => Json(json!({"ok": true})).into_response(),
        Err(e) => api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

// ---------- 项目级 UI 状态（§7.2 / §7.5：布局按项目记忆） ----------

async fn get_project_ui_state(
    State(state): State<Arc<DaemonState>>,
    Path(project_id): Path<String>,
) -> Response {
    let mut store = state.store.lock().await;
    match store.project_ui_state(&project_id) {
        Ok(Some(raw)) => match serde_json::from_str::<Value>(&raw) {
            Ok(value) => Json(value).into_response(),
            Err(e) => api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        },
        Ok(None) => Json(json!({})).into_response(),
        Err(e) => api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn put_project_ui_state(
    State(state): State<Arc<DaemonState>>,
    Path(project_id): Path<String>,
    Json(body): Json<Value>,
) -> Response {
    if !body.is_object() {
        return api_err(StatusCode::BAD_REQUEST, "UI state must be an object");
    }
    if serde_json::to_string(&body)
        .map(|raw| raw.len() > 256 * 1024)
        .unwrap_or(true)
    {
        return api_err(StatusCode::PAYLOAD_TOO_LARGE, "UI state too large");
    }
    let mut store = state.store.lock().await;
    match store.project(&project_id) {
        Ok(Some(_)) => {}
        _ => return api_err(StatusCode::NOT_FOUND, "project not found"),
    }
    let raw = serde_json::to_string(&body).unwrap_or_default();
    match store.set_project_ui_state(&project_id, &raw) {
        Ok(()) => Json(json!({"ok": true})).into_response(),
        Err(e) => api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// 本机浏览器访问配对入口（§12.6 / §15）：
/// 仅本机回环请求可达（Host 校验由中间件保证）；
/// 返回完整握手（port/token）供同源 UI 自发现（无需 query 传参）。
async fn pairing_info(State(state): State<Arc<DaemonState>>) -> Response {
    let ws_ticket = state.tickets.issue();
    Json(json!({
        "local_only": true,
        "self_hosted": true,
        "port": state.port.load(std::sync::atomic::Ordering::Relaxed),
        "token": state.token,
        "ws_ticket": ws_ticket,
        "lan_enabled": false,
        // 启动时注册的项目根：浏览器自发现 UI 据此打开同一项目（而非 cwd）
        "project": state.default_project,
    }))
    .into_response()
}

/// AI Evals 报告列表（§18.3 / M3 可视化数据源）。
async fn list_evals(State(state): State<Arc<DaemonState>>) -> Response {
    let mut store = state.store.lock().await;
    match store.eval_runs() {
        Ok(runs) => Json(json!({ "runs": runs })).into_response(),
        Err(e) => api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn list_models(State(state): State<Arc<DaemonState>>) -> Response {
    // provider 表为读写锁（v1.40）：PUT /settings 模型键即时重建
    let (providers, default) = {
        let map = state.providers.read().unwrap();
        let map = map
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect::<Vec<_>>();
        let default = state.default_provider.read().unwrap().clone();
        (map, default)
    };
    let models: Vec<Value> = providers
        .iter()
        .map(|(name, p)| {
            json!({
                "name": name,
                "default_model": p.default_model(),
                "local": p.is_local(),
                "is_default": *name == default,
            })
        })
        .collect();
    // Laya 状态（§15：版本 / 已下载 / 加载 / 设备，§9.8）
    let loaded = state.laya.is_loaded().await;
    let version = state.laya.version().await;
    let laya = json!({
        "enabled": state.config.models.laya.enabled,
        "downloaded": loaded,
        "version": version,
        "device": state.config.models.laya.device,
    });
    Json(json!({
        "models": models,
        "default": default,
        "laya": laya,
    }))
    .into_response()
}

// ---------- 局域网配对（M3 §12.6） ----------

#[derive(Deserialize)]
struct LanPairBody {
    device: String,
    code: String,
}

/// 显式开启局域网访问：生成一次性配对码（返回给已登录的本机 UI）。
async fn lan_enable(State(state): State<Arc<DaemonState>>) -> Response {
    let _ = &state;
    let code = state.lan_pairing.enable();
    Json(json!({ "enabled": true, "code": code, "ttl_s": 300 })).into_response()
}

/// 局域网设备凭一次性码配对：成功返回设备令牌（= daemon token，可吊销）。
async fn lan_pair(
    State(state): State<Arc<DaemonState>>,
    Json(body): Json<LanPairBody>,
) -> Response {
    // 配对码换发主 token（吊销即 remove → 校验失败）
    // PairingStore 签发独立设备令牌（非主 token；吊销即失效）
    match state.lan_pairing.pair(&body.device, &body.code) {
        Some(device_token) => Json(json!({
            "paired": true,
            "device": body.device,
            "paired_token_header": "X-Tenon-Paired",
            "paired_token": device_token,
        }))
        .into_response(),
        None => api_err(StatusCode::FORBIDDEN, "配对码无效或已过期"),
    }
}

#[derive(Deserialize)]
struct LanRevokeBody {
    device: String,
}

async fn lan_revoke(
    State(state): State<Arc<DaemonState>>,
    Json(body): Json<LanRevokeBody>,
) -> Response {
    let revoked = state.lan_pairing.revoke(&body.device);
    Json(json!({ "revoked": revoked })).into_response()
}

/// L4 索引统计（§10.1）：UI / 诊断用。
async fn l4_stats(
    State(state): State<Arc<DaemonState>>,
    Path(project_id): Path<String>,
) -> Response {
    let mut store = state.store.lock().await;
    let chunks = store.l4_chunk_count(&project_id);
    drop(store);
    let status = state
        .l4_status
        .lock()
        .expect("l4 status lock")
        .get(&project_id)
        .cloned();
    match chunks {
        Ok(chunks) => Json(json!({
            "project_id": project_id,
            "chunks": chunks,
            "status": status,
        }))
        .into_response(),
        Err(e) => api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// 手动重建 L4 全量索引（§10.1）：异步入队，状态经 `/l4/stats` 查询。
async fn l4_rebuild(
    State(state): State<Arc<DaemonState>>,
    Path(project_id): Path<String>,
) -> Response {
    let Some(root) = state.ensure_project_runtime(&project_id).await else {
        return api_err(StatusCode::NOT_FOUND, "project not open");
    };
    state.set_l4_status(&project_id, "queued", None);
    match state
        .l4_index_tx
        .send(L4IndexRequest::Project {
            project_id: project_id.clone(),
            root,
        })
        .await
    {
        Ok(()) => Json(json!({"queued": true, "project_id": project_id})).into_response(),
        Err(e) => {
            state.set_l4_status(&project_id, "failed", Some(e.to_string()));
            api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
        }
    }
}

/// 成本归因（§15 v1.92 收敛）：会话级查询，`?session=` 必带（月度聚合无消费方）。
async fn costs(
    State(state): State<Arc<DaemonState>>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Response {
    let Some(session_id) = q.get("session") else {
        return api_err(StatusCode::BAD_REQUEST, "session 参数必填（?session=<id>）");
    };
    let mut store = state.store.lock().await;
    let (inp, out, cost) = store.session_usage_totals(session_id).unwrap_or_default();
    Json(
        json!({"session": session_id, "input_tokens": inp, "output_tokens": out, "cost_usd": cost}),
    )
    .into_response()
}

async fn get_settings(State(state): State<Arc<DaemonState>>) -> Response {
    // 设置面板覆盖叠加在配置默认值上（§15 /settings 合并视图）
    let ov = state.settings_overrides.lock().unwrap().clone();
    let mut session = serde_json::to_value(&state.config.session).unwrap_or_default();
    if let Some(obj) = session.as_object_mut() {
        if let Some(v) = ov.first_edit_buffer_ms {
            obj.insert("first_edit_buffer_ms".into(), serde_json::json!(v));
        }
        if let Some(v) = ov.command_timeout_s {
            obj.insert("command_timeout_s".into(), serde_json::json!(v));
        }
    }
    let mut update = serde_json::to_value(&state.config.update).unwrap_or_default();
    if let Some(obj) = update.as_object_mut() {
        if let Some(v) = &ov.update_channel {
            obj.insert("channel".into(), serde_json::Value::String(v.clone()));
        }
    }
    // models 合并视图（v1.40）：覆盖叠加后回显；api_key 明文永不回显（§11 密钥存储）
    let mut models = {
        let mut m = state.config.models.clone();
        ov.apply_models_to(&mut m);
        serde_json::to_value(&m).unwrap_or_default()
    };
    if let Some(providers) = models.get_mut("providers").and_then(|p| p.as_object_mut()) {
        for (name, entry) in providers.iter_mut() {
            if let Some(e) = entry.as_object_mut() {
                e.remove("api_key");
                // 覆盖表条目可从设置删除；纯配置文件条目只能编辑（v1.40）
                let overridden = ov.models_providers.contains_key(name);
                e.insert("overridden".into(), serde_json::Value::Bool(overridden));
            }
        }
    }
    let mut exec = serde_json::to_value(&state.config.agent.exec).unwrap_or_default();
    if let Some(obj) = exec.as_object_mut() {
        if let Some(v) = ov.command_timeout_s {
            obj.insert("command_timeout_s".into(), serde_json::json!(v));
        }
    }
    Json(json!({
        "session": session,
        "exec": exec,
        "models": models,
        "agent": state.config.agent,
        "checkpoint": state.config.checkpoint,
        "update": update,
        "team_policy": state.team_policy.read().expect("team policy lock").clone(),
    }))
    .into_response()
}

async fn put_settings(State(state): State<Arc<DaemonState>>, Json(body): Json<Value>) -> Response {
    // 设置面板（§7.2 / §15）：校验 → 更新运行时覆盖 → 持久化 settings.json
    //（0600）→ 新会话即时生效（既有会话保持各自配置）
    let models_changed = body.get("models").is_some();
    {
        let mut ov = state.settings_overrides.lock().unwrap();
        if let Err(e) = ov.merge_json(&body) {
            return api_err(StatusCode::BAD_REQUEST, e);
        }
        // models.default 须指向合并后已配置的 provider（v1.40）
        if let Some(d) = &ov.models_default {
            let mut models = state.config.models.clone();
            ov.apply_models_to(&mut models);
            if !models.providers.contains_key(d) {
                return api_err(
                    StatusCode::BAD_REQUEST,
                    format!("models.default 指向未配置 provider：{d}"),
                );
            }
        }
        ov.persist_to(&state.settings_path);
    }
    if models_changed {
        state.rebuild_providers();
    }
    get_settings(State(state)).await
}

async fn put_team_policy(
    State(state): State<Arc<DaemonState>>,
    Json(body): Json<Value>,
) -> Response {
    // 高级策略是全量原子配置：先完整校验，再运行时切换并持久化；
    // 失败时不污染旧策略，避免出现“半收窄”配置。
    let policy = match validate_team_policy(&body) {
        Ok(p) => p,
        Err(e) => return api_err(StatusCode::BAD_REQUEST, e),
    };
    if let Err(e) = persist_team_policy(&policy, &state.policy_path) {
        return api_err(StatusCode::INTERNAL_SERVER_ERROR, e);
    }
    *state.team_policy.write().expect("team policy lock") = policy;
    let policy = state.team_policy.read().expect("team policy lock").clone();
    Json(json!({
        "denied_tools": policy.denied_tools,
        "max_cost_usd": policy.max_cost_usd,
    }))
    .into_response()
}

fn updates_status(state: &DaemonState) -> Response {
    let staged = find_staged_update(
        &state.updates_staging_dir,
        &current_target(),
        env!("CARGO_PKG_VERSION"),
    )
    .map_err(|e| e.to_string())
    .ok()
    .flatten();
    Json(serde_json::json!({
        "current_version": env!("CARGO_PKG_VERSION"),
        "channel": state.effective_update_channel(),
        "last_check_at": state.update_last_check.lock().expect("update check lock").clone(),
        "last_error": state.update_last_error.lock().expect("update error lock").clone(),
        "staged": staged,
    }))
    .into_response()
}

async fn get_updates(State(state): State<Arc<DaemonState>>) -> Response {
    updates_status(&state)
}

async fn check_updates(State(state): State<Arc<DaemonState>>) -> Response {
    let result = stage_update(
        &state.config.update.manifest_url,
        &state.config.update.public_key_hex,
        env!("CARGO_PKG_VERSION"),
        &current_target(),
        &state.updates_staging_dir,
    )
    .await;
    match result {
        Ok(_) => {
            state.record_update_check(None);
            updates_status(&state)
        }
        Err(e) => {
            state.record_update_check(Some(&e.to_string()));
            let status = if matches!(
                e,
                crate::updates::UpdateError::NotNewer
                    | crate::updates::UpdateError::NoPlatform
                    | crate::updates::UpdateError::PublicKeyMissing
            ) {
                StatusCode::OK
            } else {
                StatusCode::BAD_GATEWAY
            };
            api_err(status, e.to_string())
        }
    }
}

async fn apply_updates(State(state): State<Arc<DaemonState>>) -> Response {
    match find_staged_update(
        &state.updates_staging_dir,
        &current_target(),
        env!("CARGO_PKG_VERSION"),
    ) {
        Ok(Some(staged)) => match mark_staged_update(&staged, &state.updates_staging_dir) {
            Ok(()) => Json(serde_json::json!({
                "accepted": true,
                "restart_required": true,
                "staged": staged,
            }))
            .into_response(),
            Err(e) => api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        },
        Ok(None) => api_err(StatusCode::CONFLICT, "没有 staged 更新"),
        Err(e) => api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// UI 偏好读取（§7.5）：全部键值对。
async fn get_ui_prefs(State(state): State<Arc<DaemonState>>) -> Response {
    let mut store = state.store.lock().await;
    match store.ui_prefs() {
        Ok(pairs) => {
            let map: serde_json::Map<String, Value> = pairs
                .into_iter()
                .map(|(k, v)| (k, Value::String(v)))
                .collect();
            Json(Value::Object(map)).into_response()
        }
        Err(e) => api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// UI 偏好写入（§7.5）：逐键 upsert；值须为字符串。
async fn put_ui_prefs(State(state): State<Arc<DaemonState>>, Json(body): Json<Value>) -> Response {
    let Some(obj) = body.as_object() else {
        return api_err(StatusCode::BAD_REQUEST, "须为 JSON 对象");
    };
    let mut store = state.store.lock().await;
    for (k, v) in obj {
        let Some(s) = v.as_str() else {
            return api_err(StatusCode::BAD_REQUEST, format!("偏好 {k} 值须为字符串"));
        };
        if let Err(e) = store.set_ui_pref(k, s) {
            return api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
        }
    }
    Json(json!({"ok": true})).into_response()
}

// ---------- WS（ADR-10） ----------

async fn ws_ticket(State(state): State<Arc<DaemonState>>) -> Response {
    let ticket = state.tickets.issue();
    Json(json!({"ticket": ticket, "expires_in_s": 60})).into_response()
}

async fn ws_upgrade(
    State(state): State<Arc<DaemonState>>,
    Query(query): Query<std::collections::HashMap<String, String>>,
    upgrade: WebSocketUpgrade,
) -> Response {
    let project_id = query.get("project_id").cloned();
    upgrade.on_upgrade(move |socket| ws_first_frame_auth(state, project_id, socket))
}

/// WS 鉴权：连接首帧必须携带一次性 ticket；重放即拒。
async fn ws_first_frame_auth(
    state: Arc<DaemonState>,
    project_filter: Option<String>,
    mut socket: WebSocket,
) {
    let first = tokio::time::timeout(std::time::Duration::from_secs(5), socket.recv()).await;
    let ticket = match first {
        Ok(Some(Ok(Message::Text(t)))) => t,
        _ => {
            let _ = socket.close().await;
            return;
        }
    };
    if !state.tickets.consume(ticket.trim()) {
        let _ = socket.send(Message::text("auth failed".to_string())).await;
        let _ = socket.close().await;
        return;
    }
    let _ = socket.send(Message::text("auth ok".to_string())).await;
    let mut file_events = state.file_events.subscribe();
    let mut l4_status_events = state.l4_status_events.subscribe();
    // 事件流：轮询 store 推送（M1 换进程内广播）；断线续传按 seq 由 /trace 拉取
    let mut interval = tokio::time::interval(std::time::Duration::from_millis(300));
    let mut last_seen: i64 = 0;
    loop {
        tokio::select! {
            _ = interval.tick() => {
                let events = {
                    let mut store = state.store.lock().await;
                    store.recent_events(last_seen, 100).unwrap_or_default()
                };
                for ev in events {
                    last_seen = last_seen.max(ev.id);
                    if socket
                        .send(Message::text(
                            serde_json::to_string(&ev).unwrap_or_default(),
                        ))
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
            }
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return,
                    _ => {}
                }
            }
            event = file_events.recv() => {
                let Ok(event) = event else { continue };
                if project_filter.as_ref().is_some_and(|filter| *filter != event.project_id) {
                    continue;
                }
                if socket
                    .send(Message::text(
                        serde_json::to_string(&event).unwrap_or_default(),
                    ))
                    .await
                    .is_err()
                {
                    return;
                }
            }
            event = l4_status_events.recv() => {
                let Ok(event) = event else { continue };
                if project_filter
                    .as_ref()
                    .is_some_and(|filter| *filter != event.project_id)
                {
                    continue;
                }
                if socket
                    .send(Message::text(
                        serde_json::to_string(&event).unwrap_or_default(),
                    ))
                    .await
                    .is_err()
                {
                    return;
                }
            }
        }
    }
}
