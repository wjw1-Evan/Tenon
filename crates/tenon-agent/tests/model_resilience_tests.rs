//! 模型调用韧性集成测试（§9.1 自动恢复 / §11 fallback 链，v1.171）：
//! 瞬时错误自动重试 → 主 provider 穷尽自动切备用链（上下文随迁）→ 全链穷尽
//! 才落 ERROR；非瞬时错误不重试直落 fallback；辅助调用（标题）瞬时重试 ≤1。

use std::sync::Arc;

use tokio::sync::Mutex;

use tenon_agent::session::{AgentConfig, AgentSession, ProjectWriteLock, TaskOutcome};
use tenon_core::context::ProjectRules;
use tenon_models::{MockProvider, ScriptedReply};
use tenon_snapshot::SnapshotStore;
use tenon_store::{EventKind, Store};

type StdStore = Arc<Mutex<Store>>;

/// 构造带 fallback 链的会话：primary 脚本 + 任意备用 provider。
async fn setup_with_fallback(
    script: Vec<ScriptedReply>,
    fallbacks: Vec<Arc<MockProvider>>,
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
    config.first_edit_buffer_ms = 20; // 测试加速
    config.fallback_providers = fallbacks
        .into_iter()
        .map(|m| m as Arc<dyn tenon_models::ModelProvider>)
        .collect();
    let rules = ProjectRules::default();
    let session = AgentSession::create(
        store.clone(),
        snapshots,
        provider.clone(),
        config,
        ProjectWriteLock::new(),
        rules,
    )
    .await
    .unwrap();
    (dir, session, store, provider)
}

#[tokio::test]
async fn transient_rate_limit_retries_then_succeeds_without_error() {
    // 429 ×2（Retry-After=10ms 压进亚秒）后成功：不进 ERROR，model_retry ×2。
    let (_d, session, store, provider) = setup_with_fallback(
        vec![
            ScriptedReply::RateLimited {
                retry_after_ms: Some(10),
            },
            ScriptedReply::RateLimited {
                retry_after_ms: Some(10),
            },
            ScriptedReply::Text("限流恢复后的回答".into()),
        ],
        vec![],
    )
    .await;
    let outcome = session.run_task("解释").await;
    assert!(
        matches!(outcome, TaskOutcome::Done(_)),
        "瞬时错误重试后应完成任务: {outcome:?}"
    );
    assert_eq!(provider.calls().len(), 3, "1 次初始 + 2 次自动重试");
    let mut st = store.lock().await;
    let events = st.events(&session.session_id).unwrap();
    let retries: Vec<_> = events
        .iter()
        .filter(|e| e.kind == EventKind::ModelRetry)
        .collect();
    assert_eq!(retries.len(), 2, "model_retry 事件 ×2: {retries:?}");
    assert_eq!(retries[0].payload["attempt"], 1);
    assert_eq!(
        retries[0].payload["delay_ms"], 10,
        "Retry-After 优先于默认退避"
    );
    assert!(
        !events.iter().any(|e| e.kind == EventKind::Error),
        "自动恢复成功不得发 Error 事件"
    );
}

#[tokio::test]
async fn non_transient_failure_goes_straight_to_fallback() {
    // 401 非瞬时：主 provider 不重试（恰好 1 次调用），直落备用链并固定。
    let fallback = Arc::new(MockProvider::new(
        "mock-fallback",
        "mock-fb-1",
        vec![ScriptedReply::Text("备用模型的回答".into())],
    ));
    let (_d, session, store, provider) = setup_with_fallback(
        vec![ScriptedReply::NonTransient("unauthorized".into())],
        vec![fallback.clone()],
    )
    .await;
    let outcome = session.run_task("解释").await;
    match outcome {
        TaskOutcome::Done(card) => assert_eq!(card.answer, "备用模型的回答"),
        other => panic!("应经 fallback 完成: {other:?}"),
    }
    assert_eq!(provider.calls().len(), 1, "非瞬时错误不重试");
    assert_eq!(fallback.calls().len(), 1);
    assert_eq!(
        session.current_model().await,
        "mock-fb-1",
        "切换后续回合固定用备用（上下文随迁）"
    );
    let mut st = store.lock().await;
    let events = st.events(&session.session_id).unwrap();
    let fallbacks: Vec<_> = events
        .iter()
        .filter(|e| e.kind == EventKind::ModelFallback)
        .collect();
    assert_eq!(fallbacks.len(), 1);
    assert_eq!(fallbacks[0].payload["origin"], "auto");
    assert_eq!(fallbacks[0].payload["to"], "mock-fb-1");
    assert_eq!(fallbacks[0].payload["context_migrated"], true);
}

#[tokio::test]
async fn exhausted_chain_enters_error_with_retries_recorded() {
    // 主 401 直落备用；备用 429 ×2（1 次 + ≤1 重试）穷尽 → ERROR。
    let fallback = Arc::new(MockProvider::new(
        "mock-fallback",
        "mock-fb-1",
        vec![ScriptedReply::RateLimited {
            retry_after_ms: Some(10),
        }],
    ));
    let (_d, session, store, provider) = setup_with_fallback(
        vec![ScriptedReply::NonTransient("unauthorized".into())],
        vec![fallback.clone()],
    )
    .await;
    let outcome = session.run_task("解释").await;
    match outcome {
        TaskOutcome::Error(msg) => assert!(msg.contains("限流"), "末次错误透传: {msg}"),
        other => panic!("全链穷尽应 ERROR: {other:?}"),
    }
    assert_eq!(provider.calls().len(), 1);
    // 备用：1 次初始 + 1 次瞬时重试（备用余量 ≤1）
    assert_eq!(fallback.calls().len(), 2);
    let mut st = store.lock().await;
    let events = st.events(&session.session_id).unwrap();
    assert!(
        events.iter().any(|e| e.kind == EventKind::Error),
        "穷尽后必须 ERROR"
    );
    assert!(
        events
            .iter()
            .any(|e| e.kind == EventKind::ModelRetry && e.payload["provider"] == "mock-fallback"),
        "备用上的瞬时重试入 Trace"
    );
}

#[tokio::test]
async fn title_generation_retries_on_transient_failure() {
    // 标题调用首次 429（测试钩子）→ 辅助韧性重试 ≤1 → 成功出标题。
    // （标题生成由 daemon 在任务完成后调用，这里直接驱动该公开入口。）
    let (_d, session, _store, provider) =
        setup_with_fallback(vec![ScriptedReply::Text("纯回答".into())], vec![]).await;
    provider.fail_first_title_calls(1);
    let title = session.generate_title("给这个任务起个标题场景").await;
    assert!(
        matches!(title.as_deref(), Ok("Mock 会话标题")),
        "瞬时失败重试后成功: {title:?}"
    );
    assert_eq!(
        provider.title_calls().len(),
        1,
        "首次失败不记录，重试成功后恰 1 次"
    );
}

#[tokio::test]
async fn no_fallback_config_keeps_legacy_error_path() {
    // 无备用链（默认）：非瞬时错误一次即 ERROR（v1.170 前行为不变）。
    let (_d, session, _store, provider) =
        setup_with_fallback(vec![ScriptedReply::NonTransient("bad key".into())], vec![]).await;
    let outcome = session.run_task("解释").await;
    assert!(matches!(outcome, TaskOutcome::Error(_)));
    assert_eq!(provider.calls().len(), 1);
}
