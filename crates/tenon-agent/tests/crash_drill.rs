//! 崩溃恢复演练（设计方案 §10.3 / §4.2）：
//! EXECUTING 中崩溃 → daemon 重启 → 自动回滚最近快照 + 会话转 ROLLED_BACK +
//! 恢复事件入 Trace（依据事件日志可重建上下文）。

use std::sync::Arc;

use tenon_agent::recovery::recover_stale_sessions;
use tenon_agent::session::{AgentConfig, AgentSession, ProjectWriteLock, TaskOutcome};
use tenon_core::context::ProjectRules;
use tenon_models::{MockProvider, ScriptedReply};
use tenon_snapshot::SnapshotStore;
use tenon_store::{EventKind, SessionStatus, Store};
use tokio::sync::Mutex;

type StdStore = Arc<Mutex<Store>>;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn crash_mid_executing_recovers_to_last_snapshot() {
    #[allow(deprecated)]
    let dir = tempfile::tempdir().unwrap().into_path(); // 保留现场供失败排查
    let project = dir.join("proj");
    std::fs::create_dir_all(&project).unwrap();
    let project = std::fs::canonicalize(&project).unwrap();
    let store: StdStore = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
    let project_id = {
        let mut st = store.lock().await;
        st.upsert_project(project.to_str().unwrap()).unwrap().id
    };
    let snapshots_root = dir.join("snaps");
    let snapshots =
        Arc::new(SnapshotStore::open(&snapshots_root, &project_id, &project, 2).unwrap());
    let provider = Arc::new(MockProvider::new(
        "mock",
        "mock-1",
        vec![
            ScriptedReply::Tool {
                name: "apply_patch".into(),
                args: serde_json::json!({"file": "code.txt", "range": null, "content": "committed change\n"}),
            },
            ScriptedReply::Text("done".into()),
        ],
    ));
    let mut config = AgentConfig::for_project(project.clone(), &project_id);
    config.first_edit_buffer_ms = 5;
    let session = AgentSession::create(
        store.clone(),
        snapshots.clone(),
        provider,
        config,
        ProjectWriteLock::new(),
        ProjectRules::default(),
    )
    .await
    .unwrap();

    // 任务完成（产生补丁 + 事件级 checkpoint + 终点 checkpoint）
    let outcome = session.run_task("改一下 code.txt").await;
    assert!(matches!(outcome, TaskOutcome::Done(_)));
    assert!(project.join("code.txt").exists());

    // ---- 模拟 EXECUTING 中崩溃（忠实重建现场）----
    // 崩溃时进程死亡，留下的状态 = 新事务语义下的现场：
    // 会话停在 Executing + 写前 checkpoint 行已落库（先快照后写入）+ 半成品写入（事件未发生）
    let (session2_id, _pre_tree) = {
        let mut st = store.lock().await;
        let s2 = st.create_session(&project_id, "mock").unwrap();
        st.set_session_status(&s2.id, SessionStatus::Executing)
            .unwrap();
        // 写前快照（同事务第一步）
        let pre_tree = snapshots.snapshot().unwrap();
        st.insert_checkpoint(&s2.id, &pre_tree, &["code.txt".to_string()], None)
            .unwrap();
        (s2.id, pre_tree)
    };
    // 崩溃：半成品写盘完成、执行事件未发生（daemon 死亡）
    std::fs::write(project.join("code.txt"), "HALF-WRITTEN garbage\n").unwrap();

    // ---- daemon 重启：恢复扫描 ----
    let report = recover_stale_sessions(store.clone(), &snapshots_root)
        .await
        .unwrap();
    assert!(
        report.errors.is_empty(),
        "恢复不应有错误: {:?}",
        report.errors
    );

    // 断言 1：陈旧会话被处理
    assert!(
        report.sessions.contains(&session2_id),
        "崩溃会话应被恢复: {:?}",
        report.sessions
    );

    // 断言 2：半成品回滚——文件恢复到最近快照（上一任务完成态）
    let content = std::fs::read_to_string(project.join("code.txt")).unwrap();
    assert!(
        content.contains("committed change"),
        "回滚到最近快照: {content} ||| errors={:?} ||| shadows={:?}",
        report.errors,
        {
            // 物理检查：05bc 对象是否存在于 shadow objects
            let mut found = Vec::new();
            let shadow_base = snapshots_root.join(sanitize_id(&project_id));
            for e in walk_snapshot_objects(&shadow_base) {
                found.push(e);
            }
            found
        }
    );
    assert!(
        !content.contains("HALF-WRITTEN"),
        "半成品不保留（§10.3 崩溃语义）: {content}"
    );

    // 断言 3：会话状态转 ROLLED_BACK + 恢复事件入 Trace
    {
        let mut st = store.lock().await;
        let s = st.session(&session2_id).unwrap().unwrap();
        assert_eq!(s.status, SessionStatus::RolledBack);
        let events = st.events(&session2_id).unwrap();
        assert!(
            events.iter().any(|e| e.kind == EventKind::Rollback
                && e.payload.get("recovery") == Some(&serde_json::json!(true))),
            "恢复事件入 Trace"
        );
    }
}

fn sanitize_id(s: &str) -> String {
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

fn walk_snapshot_objects(root: &std::path::Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                    if name.starts_with("05bc09c") {
                        out.push(p.to_string_lossy().into_owned());
                    }
                }
            }
        }
    }
    out
}
