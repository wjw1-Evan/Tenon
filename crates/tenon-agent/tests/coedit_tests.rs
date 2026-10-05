//! 人机共编集成测试（设计方案 §8.6）：
//! 代理写盘前脏缓冲检查 → 不相交自动三方合并 / 同区冲突阻断 + 三栏事件。

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

use tenon_agent::session::{AgentConfig, AgentSession, ProjectWriteLock};
use tenon_agent::subagents::{plan_parallel, run_parallel, SubTask, WorktreePool};
use tenon_core::context::ProjectRules;
use tenon_models::{MockProvider, ScriptedReply};
use tenon_snapshot::SnapshotStore;
use tenon_store::Store;

use tenon_agent::executor::{execute_tool, ToolContext};
use tenon_core::merge::merge_three_way;
use tenon_fs::DirtyBufferRegistry;

#[tokio::test]
async fn dirty_buffer_disjoint_changes_auto_merge() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("f.txt"),
        "l1\nl2\nl3\nl4\nl5\nl6\nl7\nl8\nl9\nl10\n",
    )
    .unwrap();
    let mut ctx = ToolContext::new(dir.path(), Duration::from_secs(10));
    let registry = Arc::new(DirtyBufferRegistry::new());
    ctx.dirty = Some(registry.clone());

    // UI 在 l8 追加一行（脏缓冲，base=磁盘原内容）
    let base = std::fs::read_to_string(dir.path().join("f.txt")).unwrap();
    registry.set("f.txt", &format!("{base}USER-APPEND\n"), &base);

    // 代理改 l2（与 UI 改动不相交）
    let out = execute_tool(
        &ctx,
        "apply_patch",
        &serde_json::json!({"file": "f.txt", "range": [2, 2], "content": "AI-L2"}),
    );
    assert!(out.ok, "{}", out.content);
    assert_eq!(out.dirty_merged, Some(true));
    let content = std::fs::read_to_string(dir.path().join("f.txt")).unwrap();
    assert!(content.contains("AI-L2"), "代理改动保留: {content}");
    assert!(content.contains("USER-APPEND"), "用户改动保留: {content}");
    // 合并后 base 更新为新内容（后续继续编辑基于新基线）
    let buf = registry.get("f.txt").unwrap();
    assert_eq!(buf.base, content);
    assert!(buf.dirty.contains("USER-APPEND"));
}

#[tokio::test]
async fn dirty_buffer_same_region_conflict_blocks_write() {
    let dir = tempfile::tempdir().unwrap();
    let original = "l1\nl2\nl3\nl4\nl5\nl6\nl7\nl8\nl9\nl10\n";
    std::fs::write(dir.path().join("f.txt"), original).unwrap();
    let mut ctx = ToolContext::new(dir.path(), Duration::from_secs(10));
    let registry = Arc::new(DirtyBufferRegistry::new());
    ctx.dirty = Some(registry.clone());

    // UI 改 l5
    let base = std::fs::read_to_string(dir.path().join("f.txt")).unwrap();
    registry.set("f.txt", &base.replace("l5", "USER5"), &base);

    // 代理也改 l5（同区不同内容）→ 冲突阻断
    let out = execute_tool(
        &ctx,
        "apply_patch",
        &serde_json::json!({"file": "f.txt", "range": [5, 5], "content": "AI5"}),
    );
    assert!(!out.ok, "冲突必须阻断写入");
    let view = match out.dirty_conflict {
        Some(v) => v,
        None => panic!("应有三栏冲突视图: {out:?}"),
    };
    assert_eq!(view.path, "f.txt");
    assert!(view.ours.contains("AI5"), "ours=代理改动: {view:?}");
    assert!(view.theirs.contains("USER5"), "theirs=用户改动: {view:?}");
    // 磁盘未被改写（l5 保持原值）
    let disk = std::fs::read_to_string(dir.path().join("f.txt")).unwrap();
    assert!(disk.contains("l5"), "冲突时磁盘不动: {disk}");
}

#[tokio::test]
async fn no_dirty_buffer_writes_normally() {
    let dir = tempfile::tempdir().unwrap();
    let mut ctx = ToolContext::new(dir.path(), Duration::from_secs(10));
    let registry = Arc::new(DirtyBufferRegistry::new());
    ctx.dirty = Some(registry.clone());
    let out = execute_tool(
        &ctx,
        "apply_patch",
        &serde_json::json!({"file": "new.txt", "range": null, "content": "hi\n"}),
    );
    assert!(out.ok);
    assert!(out.dirty_merged.is_none());
    assert!(out.dirty_conflict.is_none());
}

#[test]
fn merge_engine_matches_e2e_expectations() {
    let base = "l1\nl2\nl3\nl4\nl5\nl6\nl7\nl8\nl9\nl10\n";
    let ours = base.replace("l2", "AI-L2");
    let theirs = format!("{base}USER-APPEND\n");
    let merged = merge_three_way(base, &ours, &theirs).unwrap();
    assert!(merged.contains("AI-L2") && merged.contains("USER-APPEND"));
}

// ---------- 并行子代理（§9.5）：worktree 隔离 + 不相交调度 ----------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn parallel_subagents_worktree_isolation() {
    use std::process::Command;

    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::write(repo.join("a.rs"), "pub fn a() {}\n").unwrap();
    std::fs::write(repo.join("b.rs"), "pub fn b() {}\n").unwrap();
    for args in [
        vec!["init", "-q", "."],
        vec!["config", "user.email", "t@t"],
        vec!["config", "user.name", "t"],
        vec!["add", "-A"],
        vec!["commit", "-qm", "init"],
    ] {
        Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(&args)
            .output()
            .unwrap();
    }

    // 文件集不相交的两任务
    let subtasks = vec![
        SubTask {
            id: "t1".into(),
            instruction: "改 a.rs".into(),
            files: vec!["a.rs".into()],
        },
        SubTask {
            id: "t2".into(),
            instruction: "改 b.rs".into(),
            files: vec!["b.rs".into()],
        },
    ];
    let schedule = plan_parallel(&subtasks, 3);
    assert_eq!(schedule.batches.len(), 1, "不相交 → 同批并行");
    assert!(schedule.rejected.is_empty());

    // worktree 隔离执行
    let pool = WorktreePool::new(dir.path().join("pool"));
    let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
    let project_id = {
        let mut st = store.lock().await;
        st.upsert_project(repo.to_str().unwrap()).unwrap().id
    };
    let snapshots_root = dir.path().join("snaps");

    let outcome = run_parallel(&schedule, &pool, &repo, |task: &SubTask, wt: &Path| {
        let task = task.clone();
        let wt = wt.to_path_buf();
        let store = store.clone();
        let snapshots_root = snapshots_root.clone();
        let project_id = project_id.clone();
        let file = task.files[0].clone();
        let provider = Arc::new(MockProvider::new(
            "mock",
            "mock-1",
            vec![
                ScriptedReply::Tool {
                    name: "apply_patch".into(),
                    args: serde_json::json!({"file": file, "range": null, "content": "SUB-EDIT\n"}),
                },
                ScriptedReply::Text("子任务完成".into()),
            ],
        ));
        async move {
            let mut config = AgentConfig::for_project(wt.clone(), &project_id);
            config.first_edit_buffer_ms = 5;
            let snapshots = SnapshotStore::open(&snapshots_root, &project_id, &wt, 2).unwrap();
            let session = AgentSession::create(
                store,
                Arc::new(snapshots),
                provider,
                config,
                ProjectWriteLock::new(),
                ProjectRules::default(),
            )
            .await
            .unwrap();
            session.run_task(&task.instruction).await
        }
    })
    .await
    .unwrap();

    assert_eq!(outcome.completed.len(), 2, "{outcome:?}");
    assert!(outcome.failed.is_empty());
    // worktree 改动不落主工作区（§9.5 隔离）
    assert_eq!(
        std::fs::read_to_string(repo.join("a.rs")).unwrap(),
        "pub fn a() {}\n"
    );
    assert_eq!(
        std::fs::read_to_string(repo.join("b.rs")).unwrap(),
        "pub fn b() {}\n"
    );
    // worktree 已清理
    assert!(!dir
        .path()
        .join("pool")
        .read_dir()
        .map(|mut i| i.next().is_some())
        .unwrap_or(false));
}

#[tokio::test]
async fn intersecting_subtasks_rejected_not_raced() {
    use tenon_agent::subagents::{plan_parallel, SubTask};
    let subtasks = vec![
        SubTask {
            id: "t1".into(),
            instruction: "改共享文件".into(),
            files: vec!["shared.rs".into()],
        },
        SubTask {
            id: "t2".into(),
            instruction: "也改共享文件".into(),
            files: vec!["shared.rs".into()],
        },
    ];
    let schedule = plan_parallel(&subtasks, 3);
    assert!(
        schedule.rejected.len() == 1,
        "文件集相交 → 拒绝（§9.5）: {schedule:?}"
    );
}

#[tokio::test]
async fn team_policy_denied_tools_enforced_in_executor() {
    // M3 团队策略：denied_tools 在执行器强制（跨会话只收窄，优先级最高）
    let dir = tempfile::tempdir().unwrap();
    let mut ctx = tenon_agent::executor::ToolContext::new(dir.path(), Duration::from_secs(10));
    ctx.team_denied_tools = vec!["apply_patch".to_string()];
    let out = tenon_agent::executor::execute_tool(
        &ctx,
        "apply_patch",
        &serde_json::json!({"file": "x.txt", "range": null, "content": "y"}),
    );
    assert!(!out.ok);
    assert!(out.content.contains("团队策略禁用"), "{out:?}");
    assert!(!dir.path().join("x.txt").exists());
}
