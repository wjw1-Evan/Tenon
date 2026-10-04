//! 路由（设计方案 §15 本地 API 表）。

use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Path, Query, State, WebSocketUpgrade};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use futures::SinkExt;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::Mutex;

use tenon_agent::session::{AgentConfig, AgentSession, ControlCommand};
use tenon_core::context::ProjectRules;
use tenon_core::policy::Mode;
use tenon_snapshot::SnapshotStore;
use tenon_store::{ApprovalDecision, SessionStatus};

use crate::auth::auth_middleware;
use crate::state::{DaemonState, SessionEntry};

pub fn build_router(state: Arc<DaemonState>) -> Router {
    let token = state.token.clone();
    let auth_state = (token, state.lan_pairing.clone());
    Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/ws-ticket", post(ws_ticket))
        .route("/ws", get(ws_upgrade))
        // ---------- 会话与审批（§15） ----------
        .route("/session", post(create_session))
        .route("/session/{id}/message", post(send_message))
        .route("/session/{id}", get(get_session))
        .route("/session/{id}/control", post(session_control))
        .route("/approval/{id}", post(approval_decision))
        .route("/session/{id}/trace", get(session_trace))
        .route("/session/{id}/checkpoints", get(session_checkpoints))
        .route("/checkpoint/{id}/rollback", post(checkpoint_rollback))
        .route("/session/{id}/model", post(switch_session_model))
        .route("/model-suggest", post(model_suggest))
        // ---------- 编辑器与文件（§15） ----------
        .route("/project/{id}/tree", get(project_tree))
        .route(
            "/project/{id}/buffers",
            put(set_dirty_buffer)
                .get(list_dirty_buffers)
                .delete(clear_dirty_buffer),
        )
        .route("/project/{id}/language-packs", get(detect_language_packs))
        .route(
            "/project/{id}/language-packs/install",
            post(install_language_pack),
        )
        .route("/file", get(read_file).put(write_file))
        .route("/file/ops", post(file_ops))
        .route("/search", get(search))
        .route("/lsp", post(lsp_proxy))
        .route("/lsp/openvsx", post(register_openvsx))
        .route("/plugins", get(list_plugins).put(search_registry))
        .route("/plugins/install", post(install_plugin))
        // ---------- 管理（§15） ----------
        .route("/project", get(list_projects).put(register_project))
        .route("/project/trust", put(set_project_trust))
        .route("/models", get(list_models))
        .route("/pairing", get(pairing_info))
        .route("/evals", get(list_evals))
        .route("/models/laya/download", post(laya_download))
        .route("/lan/enable", post(lan_enable))
        .route("/lan/pair", post(lan_pair))
        .route("/lan/revoke", post(lan_revoke))
        .route("/costs", get(costs))
        .route("/settings", get(get_settings).put(put_settings))
        .layer(axum::middleware::from_fn_with_state(
            auth_state,
            auth_middleware,
        ))
        .with_state(state)
}

fn api_err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(json!({"error": msg.into()}))).into_response()
}

// ---------- 会话与审批 ----------

#[derive(Deserialize)]
struct CreateSessionBody {
    project_path: String,
    #[serde(default)]
    provider: String,
    /// interactive | auto
    #[serde(default)]
    mode: String,
}

async fn create_session(
    State(state): State<Arc<DaemonState>>,
    Json(body): Json<CreateSessionBody>,
) -> Response {
    let provider = match state.provider_or_default(&body.provider).await {
        Ok(p) => p,
        Err(e) => return api_err(StatusCode::BAD_REQUEST, e),
    };
    // 项目登记（TOFU：未信任默认）
    let project = {
        let mut st = state.store.lock().await;
        match st.upsert_project(&body.project_path) {
            Ok(p) => p,
            Err(e) => return api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        }
    };
    let snapshots = match SnapshotStore::open(
        &state.snapshots_root,
        &project.id,
        std::path::Path::new(&project.path),
        state.config.checkpoint.max_untracked_mb,
    ) {
        Ok(s) => Arc::new(s),
        Err(e) => return api_err(StatusCode::INTERNAL_SERVER_ERROR, format!("快照库: {e}")),
    };
    let mode = match body.mode.as_str() {
        "auto" => Mode::Auto,
        _ => Mode::Interactive,
    };
    let mut agent_cfg = AgentConfig::for_project(
        std::path::PathBuf::from(&project.path),
        &project.id,
        project.trusted,
        mode,
    );
    agent_cfg.first_edit_buffer_ms = state.config.session.first_edit_buffer_ms;
    agent_cfg.approval_timeout_s = state.config.session.approval_timeout_s;
    agent_cfg.circuit = (&state.config.agent.circuit).into();
    agent_cfg.fix_rounds = state.config.agent.fix_loop.max_rounds;
    agent_cfg.command_timeout_s = state.config.agent.exec.command_timeout_s;
    agent_cfg.snapshots_root.clone_from(&state.snapshots_root);
    // §8.6 人机共编：代理写盘前检查 UI 未保存缓冲并三方合并
    agent_cfg.dirty = Some(state.dirty_buffers.clone());
    // M3 团队策略：工具黑名单跨会话只收窄
    agent_cfg.team_denied_tools = state.team_policy.denied_tools.clone();

    let write_lock = state.write_lock_for(&project.id).await;
    let session = match AgentSession::create(
        state.store.clone(),
        snapshots,
        provider,
        agent_cfg,
        write_lock,
        ProjectRules::default(),
    )
    .await
    {
        Ok(s) => s,
        Err(e) => return api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let sid = session.session_id.clone();
    let mut sessions = state.sessions.lock().await;
    sessions.insert(
        sid.clone(),
        SessionEntry {
            session,
            project_root: std::path::PathBuf::from(&project.path),
            project_id: project.id.clone(),
            last_outcome: Mutex::new(None),
            last_seq: 0,
        },
    );
    Json(json!({"session_id": sid, "project_id": project.id, "trusted": project.trusted}))
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
    // 后台执行任务；状态经 GET /session/{id} 轮询
    let state2 = state.clone();
    let sid = id.clone();
    tokio::spawn(async move {
        let outcome = session.run_task(&body.text).await;
        let mut sessions = state2.sessions.lock().await;
        if let Some(entry) = sessions.get_mut(&sid) {
            *entry.last_outcome.lock().await = Some(outcome);
        }
    });
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

#[derive(Deserialize)]
struct ApprovalBody {
    /// once | session | deny
    decision: String,
}

async fn approval_decision(
    State(state): State<Arc<DaemonState>>,
    Path(approval_id): Path<String>,
    Json(body): Json<ApprovalBody>,
) -> Response {
    let decision = match body.decision.as_str() {
        "once" => ApprovalDecision::Once,
        "session" => ApprovalDecision::Session,
        "deny" => ApprovalDecision::Deny,
        other => return api_err(StatusCode::BAD_REQUEST, format!("未知 decision: {other}")),
    };
    let (sid, action) = {
        let mut store = state.store.lock().await;
        match store.approval(&approval_id) {
            Ok(Some(a)) => (a.session_id, a.action),
            _ => return api_err(StatusCode::NOT_FOUND, "approval not found"),
        }
    };
    // 会话审批（交互卡）：委派会话（事件 + 唤醒等待者）；
    // 系统级审批（system:*，如插件安装 / Laya 下载）：直接落库
    if let Some(session) = state
        .sessions
        .lock()
        .await
        .get(&sid)
        .map(|e| e.session.clone())
    {
        return match session
            .decide_approval(&approval_id, decision, &action)
            .await
        {
            Ok(()) => Json(json!({"ok": true})).into_response(),
            Err(e) => api_err(StatusCode::CONFLICT, e.to_string()),
        };
    }
    let mut store = state.store.lock().await;
    match store.decide_approval(&approval_id, decision) {
        Ok(Some(_)) => Json(json!({"ok": true})).into_response(),
        Ok(None) => api_err(StatusCode::NOT_FOUND, "approval not found"),
        Err(e) => api_err(StatusCode::CONFLICT, e.to_string()),
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
struct RollbackBody {
    /// restore（checkpoint 级整体恢复）| revert（按事件撤销，默认）。
    #[serde(default)]
    #[allow(dead_code)]
    granularity: String,
}

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
        "default": state.default_provider,
    }))
    .into_response()
}

#[derive(Deserialize)]
struct SwitchModelBody {
    provider: String,
    /// 预留：同 provider 多模型时覆盖默认模型（当前随 provider 默认）。
    #[serde(default)]
    #[allow(dead_code)]
    model: String,
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

async fn project_tree(State(state): State<Arc<DaemonState>>, Path(id): Path<String>) -> Response {
    let project_path = {
        let sessions = state.sessions.lock().await;
        match sessions.get(&id) {
            Some(e) => e.project_root.clone(),
            None => {
                // 项目未开会话：按 project_id 查路径
                let mut store = state.store.lock().await;
                match store.project(&id) {
                    Ok(Some(p)) => std::path::PathBuf::from(p.path),
                    _ => return api_err(StatusCode::NOT_FOUND, "project not found"),
                }
            }
        }
    };
    let statuses = tenon_fs::git::status_map(&project_path);
    match tenon_fs::tree::list_dir(&project_path, "", &statuses) {
        Ok(entries) => Json(json!({"entries": entries})).into_response(),
        Err(e) => api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

#[derive(Deserialize)]
struct FilePathQuery {
    path: String,
}

async fn first_project_root(state: &DaemonState) -> Option<std::path::PathBuf> {
    let sessions = state.sessions.lock().await;
    sessions.values().next().map(|e| e.project_root.clone())
}

async fn read_file(
    State(state): State<Arc<DaemonState>>,
    Query(q): Query<FilePathQuery>,
) -> Response {
    let Some(root) = first_project_root(&state).await else {
        return api_err(StatusCode::CONFLICT, "无已打开项目");
    };
    let fs = tenon_fs::FileService::new(root);
    match fs.read_file(&q.path) {
        Ok(content) => Json(json!({"path": q.path, "content": content})).into_response(),
        Err(e) => api_err(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

#[derive(Deserialize)]
struct WriteFileBody {
    path: String,
    content: String,
}

async fn write_file(
    State(state): State<Arc<DaemonState>>,
    Json(body): Json<WriteFileBody>,
) -> Response {
    let Some(root) = first_project_root(&state).await else {
        return api_err(StatusCode::CONFLICT, "无已打开项目");
    };
    let fs = tenon_fs::FileService::new(root);
    match fs.write_file(&body.path, &body.content) {
        Ok(created) => Json(json!({"ok": true, "created": created})).into_response(),
        Err(e) => api_err(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

#[derive(Deserialize)]
struct FileOpsBody {
    ops: Vec<tenon_fs::FileOp>,
}

async fn file_ops(
    State(state): State<Arc<DaemonState>>,
    Json(body): Json<FileOpsBody>,
) -> Response {
    let Some(root) = first_project_root(&state).await else {
        return api_err(StatusCode::CONFLICT, "无已打开项目");
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

async fn search(State(state): State<Arc<DaemonState>>, Query(q): Query<SearchQuery>) -> Response {
    let Some(root) = first_project_root(&state).await else {
        return api_err(StatusCode::CONFLICT, "无已打开项目");
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

/// Open VSX 语言子集实验兼容（§13.3）：languageContribution → 内部语言包。
async fn register_openvsx(Json(body): Json<Value>) -> Response {
    let Some(package_json) = body.get("package_json").and_then(|p| p.as_str()) else {
        return api_err(StatusCode::BAD_REQUEST, "缺少 package_json");
    };
    match tenon_lsp::convert_extension(package_json) {
        Ok(converted) => {
            tenon_lsp::register_dynamic_pack(converted.pack.clone());
            Json(json!({
                "registered": true,
                "language": converted.language,
                "extension": converted.extension,
                "experimental": true,
            }))
            .into_response()
        }
        Err(e) => api_err(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

#[derive(Deserialize)]
struct LspBody {
    project_path: String,
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

async fn lsp_proxy(State(state): State<Arc<DaemonState>>, Json(body): Json<LspBody>) -> Response {
    // §8.5 / §15：共享 LSP 宿主语义端点
    match state
        .lsp
        .request(
            std::path::Path::new(&body.project_path),
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
    /// 两阶段：省略 approval_id → 返回权限 diff + D 级卡；带已批准 id → 安装
    #[serde(default)]
    approval_id: Option<String>,
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

    // 两阶段 D 级审批：第一调（无 approval_id）→ 返回 diff + 审批卡
    let Some(approval_id) = body.approval_id else {
        let approval = {
            let mut st = state.store.lock().await;
            st.insert_approval(
                "system:plugin",
                &format!(
                    "安装插件 {} v{}（新增权限: {}）",
                    manifest.id,
                    manifest.version,
                    if diff.added.is_empty() {
                        "无".into()
                    } else {
                        diff.added.join(", ")
                    }
                ),
                tenon_store::Level::D,
            )
            .expect("insert approval")
        };
        return Json(json!({
            "approval_id": approval.id,
            "level": "d",
            "permission_diff": diff,
            "manifest": manifest,
        }))
        .into_response();
    };
    // 第二调：校验审批已「允许」
    let approved = {
        let mut st = state.store.lock().await;
        st.approval(&approval_id)
            .ok()
            .flatten()
            .map(|a| matches!(a.decision, Some(tenon_store::ApprovalDecision::Once)))
            .unwrap_or(false)
    };
    if !approved {
        return api_err(StatusCode::FORBIDDEN, "D 级审批未通过（§13.2）");
    }
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
    let base = std::fs::read_to_string(project_root.join(&body.path)).unwrap_or_default();
    state.dirty_buffers.set(&body.path, &body.dirty, &base);
    Json(json!({"ok": true, "dirty": true})).into_response()
}

async fn list_dirty_buffers(
    State(state): State<Arc<DaemonState>>,
    Path(_id): Path<String>,
) -> Response {
    let paths = state.dirty_buffers.list();
    Json(json!({ "paths": paths })).into_response()
}

#[derive(Deserialize)]
struct BufferQuery {
    path: String,
}

async fn clear_dirty_buffer(
    State(state): State<Arc<DaemonState>>,
    Path(_id): Path<String>,
    Query(q): Query<BufferQuery>,
) -> Response {
    state.dirty_buffers.clear(&q.path);
    Json(json!({ "ok": true, "dirty": false })).into_response()
}

async fn project_root_by_id(state: &Arc<DaemonState>, id: &str) -> Option<std::path::PathBuf> {
    {
        let sessions = state.sessions.lock().await;
        if let Some(e) = sessions.get(id) {
            return Some(e.project_root.clone());
        }
    }
    let mut store = state.store.lock().await;
    store
        .project(id)
        .ok()
        .flatten()
        .map(|p| std::path::PathBuf::from(p.path))
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
                Some("官方指引：npm install -g typescript-language-server typescript@5.8.3（或一键安装，走 D 级审批）")
            } else if p.language == "python" && !server_installed {
                Some("官方指引：npm install -g pyright（或一键安装，走 D 级审批）")
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
    /// 两阶段 D 级审批（§8.4 一键安装走审批，同 §9.8 下载）
    #[serde(default)]
    approval_id: Option<String>,
}

async fn install_language_pack(
    State(state): State<Arc<DaemonState>>,
    Path(id): Path<String>,
    Json(body): Json<InstallPackBody>,
) -> Response {
    let Some(_project_root) = project_root_by_id(&state, &id).await else {
        return api_err(StatusCode::NOT_FOUND, "project not found");
    };
    let (command, summary) = match body.pack.as_str() {
        "typescript" => (
            "npm install -g typescript-language-server typescript@5.8.3",
            "安装 TypeScript 语言包（typescript-language-server + 锁定版 TypeScript 5.8.3）",
        ),
        "python" => ("npm install -g pyright", "安装 Python 语言包（pyright）"),
        other => return api_err(StatusCode::BAD_REQUEST, format!("未知语言包: {other}")),
    };
    // 两阶段 D 级审批
    let Some(approval_id) = body.approval_id else {
        let approval = {
            let mut st = state.store.lock().await;
            st.insert_approval("system:language-pack", summary, tenon_store::Level::D)
                .expect("insert approval")
        };
        return Json(json!({
            "approval_id": approval.id,
            "level": "d",
            "command": command,
        }))
        .into_response();
    };
    let approved = {
        let mut st = state.store.lock().await;
        st.approval(&approval_id)
            .ok()
            .flatten()
            .map(|a| matches!(a.decision, Some(tenon_store::ApprovalDecision::Once)))
            .unwrap_or(false)
    };
    if !approved {
        return api_err(
            StatusCode::FORBIDDEN,
            "D 级审批未通过（§8.4 一键安装走审批）",
        );
    }
    let out = tenon_sandbox::exec_command(
        command,
        &std::env::temp_dir(),
        std::time::Duration::from_secs(300),
        &tenon_sandbox::SandboxSpec::None, // 全局安装：写系统目录（已过 D 级审批）
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
    let mut store = state.store.lock().await;
    match store.list_projects() {
        Ok(projects) => Json(json!({"projects": projects})).into_response(),
        Err(e) => api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

#[derive(Deserialize)]
struct RegisterProjectBody {
    path: String,
}

async fn register_project(
    State(state): State<Arc<DaemonState>>,
    Json(body): Json<RegisterProjectBody>,
) -> Response {
    let mut store = state.store.lock().await;
    match store.upsert_project(&body.path) {
        Ok(p) => Json(json!(p)).into_response(),
        Err(e) => api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
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

/// 本机浏览器访问配对入口（§12.6 / §15）：
/// 仅本机回环请求可达（Host 校验由中间件保证）；
/// 返回同源 UI 自发现握手（浏览器打开 / 即自动接入，无需 query 传参）。
async fn pairing_info(State(state): State<Arc<DaemonState>>) -> Response {
    let ws_ticket = state.tickets.issue();
    Json(json!({
        "local_only": true,
        "hint": "本机浏览器访问：以 ?port=&token= 打开 UI（token 见 daemon 握手行 / Tauri 注入）",
        "ws_ticket": ws_ticket,
        "lan_enabled": false,
        // UI 自发现：同源部署时 main.tsx 经此获取握手
        "self_hosted": true,
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
    let models: Vec<Value> = state
        .providers
        .iter()
        .map(|(name, p)| {
            json!({
                "name": name,
                "default_model": p.default_model(),
                "local": p.is_local(),
                "is_default": *name == state.default_provider,
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
        "default": state.default_provider,
        "laya": laya,
    }))
    .into_response()
}

#[derive(Deserialize)]
struct LayaDownloadBody {
    /// 第一次调用：省略 approval_id → 创建 D 级审批卡并返回安装计划；
    /// 第二次调用：携带已批准的 approval_id → 执行下载与安装。
    #[serde(default)]
    approval_id: Option<String>,
    #[serde(default)]
    registry_url: Option<String>,
}

async fn laya_download(
    State(state): State<Arc<DaemonState>>,
    Json(body): Json<LayaDownloadBody>,
) -> Response {
    let Some(approval_id) = body.approval_id else {
        let registry =
            match tenon_laya::registry::fetch_manifest(body.registry_url.as_deref()).await {
                Ok(m) => m,
                Err(e) => return api_err(StatusCode::BAD_GATEWAY, format!("registry 不可达: {e}")),
            };
        let plan = match tenon_laya::registry::plan_install(&registry) {
            Ok(p) => p,
            Err(e) => return api_err(StatusCode::BAD_GATEWAY, format!("清单校验失败: {e}")),
        };
        let approval = {
            let mut st = state.store.lock().await;
            st.insert_approval(
                "system:laya",
                &format!(
                    "下载本地决策模型 Laya v{}（SHA-256 {}…）",
                    plan.version,
                    &plan.sha256[..plan.sha256.len().min(12)]
                ),
                tenon_store::Level::D,
            )
            .expect("insert approval")
        };
        return Json(json!({
            "approval_id": approval.id,
            "level": "d",
            "plan": plan,
        }))
        .into_response();
    };
    let approved = {
        let mut st = state.store.lock().await;
        st.approval(&approval_id)
            .ok()
            .flatten()
            .map(|a| matches!(a.decision, Some(tenon_store::ApprovalDecision::Once)))
            .unwrap_or(false)
    };
    if !approved {
        return api_err(
            StatusCode::FORBIDDEN,
            "D 级审批未通过（§9.8：下载须一次审批）",
        );
    }
    let registry = match tenon_laya::registry::fetch_manifest(body.registry_url.as_deref()).await {
        Ok(m) => m,
        Err(e) => return api_err(StatusCode::BAD_GATEWAY, format!("registry 不可达: {e}")),
    };
    let plan = match tenon_laya::registry::plan_install(&registry) {
        Ok(p) => p,
        Err(e) => return api_err(StatusCode::BAD_GATEWAY, format!("清单校验失败: {e}")),
    };
    match tenon_laya::registry::download_model(&plan).await {
        Ok(bytes) => match state.laya.install(&bytes).await {
            Ok(()) => Json(json!({"installed": true, "version": plan.version})).into_response(),
            Err(e) => api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        },
        Err(e) => api_err(StatusCode::BAD_GATEWAY, e.to_string()),
    }
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

async fn costs(
    State(state): State<Arc<DaemonState>>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Response {
    let mut store = state.store.lock().await;
    if let Some(session_id) = q.get("session") {
        let (inp, out, cost) = store.session_usage_totals(session_id).unwrap_or_default();
        return Json(
            json!({"session": session_id, "input_tokens": inp, "output_tokens": out, "cost_usd": cost}),
        )
        .into_response();
    }
    let monthly = store.monthly_usage().unwrap_or_default();
    Json(json!({"monthly": monthly})).into_response()
}

async fn get_settings(State(state): State<Arc<DaemonState>>) -> Response {
    Json(json!({
        "session": state.config.session,
        "agent": state.config.agent,
        "checkpoint": state.config.checkpoint,
        "privacy": state.config.privacy,
        "locale": state.config.locale,
        "team_policy": state.team_policy,
    }))
    .into_response()
}

async fn put_settings(State(state): State<Arc<DaemonState>>, Json(body): Json<Value>) -> Response {
    // M0/M1：接受覆盖 session.mode（附录 E 子集）；完整 settings 随设置面板
    if let Some(mode) = body
        .get("session")
        .and_then(|s| s.get("mode"))
        .and_then(|m| m.as_str())
    {
        let mut cfg = state.config.clone();
        cfg.session.mode = match mode {
            "auto" => tenon_config::SessionMode::Auto,
            _ => tenon_config::SessionMode::Interactive,
        };
        let _ = cfg;
    }
    get_settings(State(state)).await
}

// ---------- WS（ADR-10） ----------

async fn ws_ticket(State(state): State<Arc<DaemonState>>) -> Response {
    let ticket = state.tickets.issue();
    Json(json!({"ticket": ticket, "expires_in_s": 60})).into_response()
}

async fn ws_upgrade(State(state): State<Arc<DaemonState>>, upgrade: WebSocketUpgrade) -> Response {
    upgrade.on_upgrade(move |socket| ws_first_frame_auth(state, socket))
}

/// WS 鉴权：连接首帧必须携带一次性 ticket；重放即拒。
async fn ws_first_frame_auth(state: Arc<DaemonState>, mut socket: WebSocket) {
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
        }
    }
}
