//! 同项目并行执行集成测试（design v1.87 §9.7 并行写锁）：
//! 主根会话与受管 worktree 会话共享同一 ProjectWriteLock——
//! 写锁键为 `(project_id, worktree_scope)`，不同作用域必须能同时推进任务。
//! 若锁仍为项目级互斥，后到的 run_task 无法进入模型回合，barrier 等待超时即失败。

use std::sync::Arc;
use std::time::Duration;

use tenon_agent::session::{AgentConfig, AgentSession, ProjectWriteLock};
use tenon_agent::subagents::WorktreePool;
use tenon_core::context::ProjectRules;
use tenon_core::policy::Mode;
use tenon_models::{
    ChatRequest, ChatResponse, MockProvider, ModelProvider, ProviderResult, ScriptedReply,
};
use tenon_snapshot::SnapshotStore;
use tenon_store::Store;

/// 双会话都到达模型回合才放行：并行的直接证据。
struct BarrierProvider {
    barrier: Arc<tokio::sync::Barrier>,
    script: MockProvider,
}

#[async_trait::async_trait]
impl ModelProvider for BarrierProvider {
    fn name(&self) -> &str {
        "barrier-mock"
    }

    async fn chat(&self, req: &ChatRequest) -> ProviderResult<ChatResponse> {
        self.barrier.wait().await;
        self.script.chat(req).await
    }

    fn default_model(&self) -> String {
        self.script.default_model()
    }
}

fn git(repo: &std::path::Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} 失败: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[tokio::test]
async fn root_and_worktree_sessions_execute_in_parallel() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    git(&repo, &["config", "user.email", "t@tenon.dev"]);
    git(&repo, &["config", "user.name", "t"]);
    std::fs::write(repo.join("a.txt"), "a\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-qm", "init"]);

    let store = Arc::new(tokio::sync::Mutex::new(Store::open_in_memory().unwrap()));
    let project_id = {
        let mut st = store.lock().await;
        st.upsert_project(repo.to_str().unwrap()).unwrap().id
    };
    let snapshots_root = dir.path().join("snaps");
    // 与 daemon 同语义：同项目共享一个 ProjectWriteLock（§9.7）。
    let write_lock = ProjectWriteLock::new();
    let barrier = Arc::new(tokio::sync::Barrier::new(2));

    // 会话 A：项目主根（write_scope = "root"）
    let provider_a = Arc::new(BarrierProvider {
        barrier: barrier.clone(),
        script: MockProvider::new(
            "mock",
            "mock-1",
            vec![ScriptedReply::Text("answer root".into())],
        ),
    });
    let config_a = AgentConfig::for_project(repo.clone(), &project_id, true, Mode::Interactive);
    let session_a = AgentSession::create(
        store.clone(),
        Arc::new(SnapshotStore::open(&snapshots_root, &project_id, &repo, 2).unwrap()),
        provider_a,
        config_a,
        write_lock.clone(),
        ProjectRules::default(),
    )
    .await
    .unwrap();

    // 会话 B：受管 worktree（write_scope = worktree 路径；写边界 / 快照随 worktree）
    let pool = WorktreePool::new(dir.path().join("worktrees/proj"));
    let wt = pool.create(&repo, "wt-session-b").unwrap();
    let provider_b = Arc::new(BarrierProvider {
        barrier,
        script: MockProvider::new(
            "mock",
            "mock-1",
            vec![ScriptedReply::Text("answer worktree".into())],
        ),
    });
    let mut config_b = AgentConfig::for_project(repo.clone(), &project_id, true, Mode::Interactive);
    config_b.session_id = Some("session-wt-b".to_string());
    config_b.managed_worktree = Some(wt.clone());
    config_b.write_scope = wt.to_string_lossy().into_owned();
    let session_b = AgentSession::create(
        store.clone(),
        Arc::new(SnapshotStore::open(&snapshots_root, &project_id, &wt, 2).unwrap()),
        provider_b,
        config_b,
        write_lock,
        ProjectRules::default(),
    )
    .await
    .unwrap();

    // 关键断言：两个任务同时进入模型回合（barrier 双方到齐）；项目级互斥锁会在此超时。
    let (outcome_a, outcome_b) = tokio::time::timeout(Duration::from_secs(15), async move {
        tokio::join!(
            session_a.run_task("task root"),
            session_b.run_task("task worktree")
        )
    })
    .await
    .expect("不同 worktree 作用域必须并行：barrier 超时说明写锁仍互斥");

    assert!(
        matches!(outcome_a, tenon_agent::session::TaskOutcome::Done(_)),
        "{outcome_a:?}"
    );
    assert!(
        matches!(outcome_b, tenon_agent::session::TaskOutcome::Done(_)),
        "{outcome_b:?}"
    );

    // B 的写入边界在 worktree（会话行受管标记随 store 落库由 daemon 层测试覆盖）。
    assert!(wt.join("a.txt").exists(), "worktree 内容就绪");
}
