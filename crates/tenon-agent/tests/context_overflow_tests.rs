//! 上下文溢出自动恢复集成测试（§10.2 v1.191，DSH「overflow 恢复路径」借鉴项）：
//! provider 报上下文超限（400 非瞬时）→ 同回合强制省略一次陈旧工具输出 → 重试。

use std::sync::Arc;

use tenon_agent::session::{AgentConfig, ProjectWriteLock, TaskOutcome};
use tenon_agent::AgentSession;
use tenon_core::context::ProjectRules;
use tenon_models::{MockProvider, ScriptedReply};
use tenon_snapshot::SnapshotStore;
use tenon_store::{EventKind, Store};
use tokio::sync::Mutex;

type StdStore = Arc<Mutex<Store>>;

async fn setup(
    script: Vec<ScriptedReply>,
) -> (
    tempfile::TempDir,
    Arc<AgentSession>,
    StdStore,
    Arc<MockProvider>,
) {
    let dir = tempfile::tempdir().unwrap();
    let store: StdStore = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
    let project_id = {
        let mut st = store.lock().await;
        st.upsert_project(dir.path().to_str().unwrap()).unwrap().id
    };
    let snapshots_root = dir.path().join(".tenon-snapshots");
    let snapshots =
        Arc::new(SnapshotStore::open(&snapshots_root, &project_id, dir.path(), 2).unwrap());
    let provider = Arc::new(MockProvider::new("mock", "mock-1", script));
    let mut config = AgentConfig::for_project(dir.path().to_path_buf(), &project_id);
    config.first_edit_buffer_ms = 20;
    let session = AgentSession::create(
        store.clone(),
        snapshots,
        provider.clone(),
        config,
        ProjectWriteLock::new(),
        ProjectRules::default(),
    )
    .await
    .unwrap();
    (dir, session, store, provider)
}

/// 5 连工具回合（KEEP_RECENT_TOOL_RESULTS=4，须 >4 才产生可省略的陈旧输出）。
fn tool_rounds(n: usize) -> Vec<ScriptedReply> {
    (0..n)
        .map(|_| ScriptedReply::Tool {
            name: "list_dir".into(),
            args: serde_json::json!({"path": "."}),
        })
        .collect()
}

#[tokio::test]
async fn overflow_triggers_compaction_and_retry_to_done() {
    // 前 5 回合正常工具调用 → 上下文超限 → 压缩重试 → 文本回答收尾 Done。
    // 模型调用恰 7 次（5 工具 / 溢出 / 压缩后重试）。
    let mut script = tool_rounds(5);
    script.push(ScriptedReply::ContextOverflow);
    script.push(ScriptedReply::Text("溢出已恢复，任务完成".into()));
    let (_d, session, store, provider) = setup(script).await;
    let outcome = session.run_task("列出目录后总结").await;
    assert!(
        matches!(outcome, TaskOutcome::Done(_)),
        "溢出恢复后应 Done，实际 {outcome:?}"
    );
    // 恰 7 次：5 工具 / 溢出（非瞬时不重试不 fallback）/ 压缩后重试
    assert_eq!(provider.calls().len(), 7);
    // 溢出压缩事件入 Trace，且带 overflow 标记与省略计数
    let mut st = store.lock().await;
    let events = st.events(&session.session_id).unwrap();
    let overflow_compactions: Vec<_> = events
        .iter()
        .filter(|e| {
            e.kind == EventKind::Compaction && e.payload["overflow"].as_bool() == Some(true)
        })
        .collect();
    assert_eq!(overflow_compactions.len(), 1, "恰一次溢出压缩事件");
    assert!(
        overflow_compactions[0].payload["elided_tool_results"]
            .as_u64()
            .unwrap_or(0)
            >= 1,
        "省略了早期工具输出"
    );
}

#[tokio::test]
async fn double_overflow_exhausts_to_error() {
    // 压缩重试一次后再溢出 → 落 ERROR（单回合至多恢复一次，防循环）。
    let mut script = tool_rounds(5);
    script.push(ScriptedReply::ContextOverflow);
    script.push(ScriptedReply::ContextOverflow);
    let (_d, session, store, provider) = setup(script).await;
    let outcome = session.run_task("列出目录后总结").await;
    assert!(
        matches!(outcome, TaskOutcome::Error(_)),
        "二次溢出应落 ERROR，实际 {outcome:?}"
    );
    // 恰 7 次：5 工具 / 首次溢出 / 压缩后重试再溢出（不再第三试）
    assert_eq!(provider.calls().len(), 7);
    let mut st = store.lock().await;
    let events = st.events(&session.session_id).unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|e| {
                e.kind == EventKind::Compaction && e.payload["overflow"].as_bool() == Some(true)
            })
            .count(),
        1,
        "溢出压缩事件恰一次"
    );
}

#[tokio::test]
async fn overflow_without_elidable_history_lands_error() {
    // 首轮即溢出（无工具输出可省略）→ 压缩不可行 → 直接 ERROR，不空转重试。
    let (_d, session, _store, provider) = setup(vec![ScriptedReply::ContextOverflow]).await;
    let outcome = session.run_task("纯溢出").await;
    assert!(matches!(outcome, TaskOutcome::Error(_)));
    assert_eq!(provider.calls().len(), 1, "无恢复路径不重试");
}
