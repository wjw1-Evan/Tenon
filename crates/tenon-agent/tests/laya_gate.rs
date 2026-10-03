//! Laya Evals 门（设计方案 §9.8 / §18.3）：
//! 集成点与判定阈值变更须过门——「基准通过率不降、token 消耗下降」。
//! 本测试以只读任务（T5 语义）+ prompt 规模计量的 mock provider 离线验证：
//! 意图预判收窄首轮工具目录 → 输入 token 下降且任务仍通过。

use std::sync::Arc;

use tenon_agent::session::{AgentConfig, AgentSession, ProjectWriteLock, TaskOutcome};
use tenon_core::context::ProjectRules;
use tenon_core::policy::Mode;
use tenon_laya::LayaRuntime;
use tenon_models::{MockProvider, ScriptedReply};
use tenon_snapshot::SnapshotStore;
use tenon_store::Store;
use tokio::sync::Mutex;

async fn run_read_only_task(laya: Option<Arc<LayaRuntime>>) -> (bool, u64) {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
    let project_id = {
        let mut st = store.lock().await;
        st.upsert_project(dir.path().to_str().unwrap()).unwrap().id
    };
    let snapshots = Arc::new(
        SnapshotStore::open(&dir.path().join("snaps"), &project_id, dir.path(), 2).unwrap(),
    );
    let provider = Arc::new(MockProvider::new(
        "mock",
        "mock-1",
        vec![ScriptedReply::Text(
            "认证流程：login → token → verify。".into(),
        )],
    ));
    let mut config =
        AgentConfig::for_project(dir.path().to_path_buf(), &project_id, true, Mode::Auto);
    config.first_edit_buffer_ms = 5;
    config.laya = laya;
    let session = AgentSession::create(
        store.clone(),
        snapshots,
        provider,
        config,
        ProjectWriteLock::new(),
        ProjectRules::default(),
    )
    .await
    .unwrap();
    let outcome = session
        .run_task("解释 src/auth.js 的认证流程，不要改任何文件")
        .await;
    let tokens = {
        let mut st = store.lock().await;
        st.session_usage_totals(&session.session_id).unwrap().0 as u64
    };
    (matches!(outcome, TaskOutcome::Done(_)), tokens)
}

fn laya_runtime() -> Arc<LayaRuntime> {
    // 按 §9.8：模型不进安装包——测试以「已下载」形态装到临时 models 目录
    let dir = std::env::temp_dir().join(format!("laya-gate-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let bytes = include_bytes!("../../tenon-laya/models/laya-starter-v1.json");
    std::fs::write(dir.join("model.json"), bytes).unwrap();
    Arc::new(LayaRuntime::open(
        &dir,
        &[
            "intent".to_string(),
            "risk".to_string(),
            "prefilter".to_string(),
            "routing".to_string(),
            "triage".to_string(),
        ],
    ))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn laya_gate_pass_rate_not_down_and_tokens_down() {
    let (base_pass, base_tokens) = run_read_only_task(None).await;
    let (laya_pass, laya_tokens) = run_read_only_task(Some(laya_runtime())).await;

    // 通过率不降
    assert!(base_pass, "基线任务应通过");
    assert!(laya_pass, "Laya 开启后任务应仍通过");

    // token 下降：只读先验收窄首轮工具目录（9 工具 → 4 工具）
    assert!(
        laya_tokens < base_tokens,
        "Laya 只读先验应降低输入 token：base={base_tokens} laya={laya_tokens}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn laya_needs_change_task_keeps_full_catalog() {
    // 需改动任务：意图预判 = needs_change → 不收窄目录，行为与基线一致
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
    let project_id = {
        let mut st = store.lock().await;
        st.upsert_project(dir.path().to_str().unwrap()).unwrap().id
    };
    let snapshots = Arc::new(
        SnapshotStore::open(&dir.path().join("snaps"), &project_id, dir.path(), 2).unwrap(),
    );
    let provider = Arc::new(MockProvider::new(
        "mock",
        "mock-1",
        vec![
            ScriptedReply::Tool {
                name: "apply_patch".into(),
                args: serde_json::json!({"file": "a.txt", "range": null, "content": "x\n"}),
            },
            ScriptedReply::Text("完成".into()),
        ],
    ));
    let mut config =
        AgentConfig::for_project(dir.path().to_path_buf(), &project_id, true, Mode::Auto);
    config.first_edit_buffer_ms = 5;
    config.laya = Some(laya_runtime());
    let session = AgentSession::create(
        store,
        snapshots,
        provider,
        config,
        ProjectWriteLock::new(),
        ProjectRules::default(),
    )
    .await
    .unwrap();
    let outcome = session.run_task("修复 bug 并写入 a.txt").await;
    assert!(matches!(outcome, TaskOutcome::Done(_)));
    assert!(
        dir.path().join("a.txt").exists(),
        "需改动任务不受只读先验影响"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn decider_call_event_recorded_without_input_text() {
    // §14.2：decider_call 入 Trace——类型/结果/耗时，不含输入原文
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
    let project_id = {
        let mut st = store.lock().await;
        st.upsert_project(dir.path().to_str().unwrap()).unwrap().id
    };
    let snapshots = Arc::new(
        SnapshotStore::open(&dir.path().join("snaps"), &project_id, dir.path(), 2).unwrap(),
    );
    let provider = Arc::new(MockProvider::new(
        "mock",
        "mock-1",
        vec![ScriptedReply::Text("回答".into())],
    ));
    let mut config =
        AgentConfig::for_project(dir.path().to_path_buf(), &project_id, true, Mode::Auto);
    config.first_edit_buffer_ms = 5;
    config.laya = Some(laya_runtime());
    let session = AgentSession::create(
        store.clone(),
        snapshots,
        provider,
        config,
        ProjectWriteLock::new(),
        ProjectRules::default(),
    )
    .await
    .unwrap();
    let marker = "ZDX_SECRET_MARKER_不要泄露这一句";
    let _ = session.run_task(marker).await;

    let mut st = store.lock().await;
    let events = st.events(&session.session_id).unwrap();
    let decider = events
        .iter()
        .find(|e| e.kind == tenon_store::EventKind::DeciderCall)
        .expect("应有 decider_call 事件");
    assert_eq!(decider.payload["feature"], "intent");
    assert_eq!(decider.payload["kind"], "choice");
    assert_eq!(decider.payload["fallback"], false);
    assert!(decider.payload["duration_ms"].is_u64(), "耗时入 Trace");
    // 不含输入原文
    let payload_text = decider.payload.to_string();
    assert!(
        !payload_text.contains(marker),
        "decider_call 不得包含输入原文"
    );
}
