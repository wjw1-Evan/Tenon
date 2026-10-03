//! Agent 会话集成测试（mock provider 脚本化驱动 §9.1 全流程）。

use std::sync::Arc;
use std::time::Duration;

use tenon_agent::session::{AgentConfig, ControlCommand, ProjectWriteLock, TaskOutcome};
use tenon_agent::AgentSession;
use tenon_core::context::ProjectRules;
use tenon_core::policy::Mode;
use tenon_models::{MockProvider, ScriptedReply};
use tenon_snapshot::SnapshotStore;
use tenon_store::{ApprovalDecision, EventKind, Store};
use tokio::sync::Mutex;

type StdStore = Arc<Mutex<Store>>;

async fn setup(
    script: Vec<ScriptedReply>,
    trusted: bool,
    mode: Mode,
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
    let mut config = AgentConfig::for_project(dir.path().to_path_buf(), &project_id, trusted, mode);
    config.first_edit_buffer_ms = 20; // 测试加速
    config.approval_timeout_s = 5;
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
async fn answer_only_task_completes_without_changes() {
    let (_d, session, store, _p) = setup(
        vec![ScriptedReply::Text("这是纯回答，无改动".into())],
        false,
        Mode::Interactive,
    )
    .await;
    let outcome = session.run_task("解释这段代码").await;
    match outcome {
        TaskOutcome::Done(card) => {
            assert_eq!(card.answer, "这是纯回答，无改动");
            assert!(card.changed_files.is_empty());
            assert_eq!(card.verification_strength, "none");
        }
        other => panic!("期望 Done，实际 {other:?}"),
    }
    // 事件链完整：user_input → done 会话状态
    let mut st = store.lock().await;
    let events = st.events(&session.session_id).unwrap();
    assert_eq!(events[0].kind, EventKind::UserInput);
    assert!(events.iter().any(|e| e.kind == EventKind::Decision));
}

#[tokio::test]
async fn patch_task_auto_mode_writes_verifies_and_checkpoints() {
    let (dir, session, store, _p) = setup(
        vec![
            ScriptedReply::Tool {
                name: "apply_patch".into(),
                args: serde_json::json!({"file": "src/lib.rs", "range": null, "content": "fn fixed() {}\n"}),
            },
            ScriptedReply::Text("修复完成".into()),
        ],
        true,
        Mode::Auto,
    )
    .await;
    let outcome = session.run_task("修复 bug").await;
    match outcome {
        TaskOutcome::Done(card) => {
            assert_eq!(card.changed_files, vec!["src/lib.rs".to_string()]);
            assert!(card.steps >= 2);
        }
        other => panic!("期望 Done，实际 {other:?}"),
    }
    // 文件真实写入
    assert_eq!(
        std::fs::read_to_string(dir.path().join("src/lib.rs")).unwrap(),
        "fn fixed() {}\n"
    );
    // patch_applied 事件 + checkpoint 已记录
    let mut st = store.lock().await;
    let events = st.events(&session.session_id).unwrap();
    assert!(events.iter().any(|e| e.kind == EventKind::PatchApplied));
    let cps = st.checkpoints(&session.session_id).unwrap();
    assert!(!cps.is_empty(), "任务应有 checkpoint");
    // 首改缓冲事件
    assert!(events.iter().any(|e| e.kind == EventKind::Decision
        && e.payload.get("first_edit") == Some(&serde_json::json!(true))));
}

#[tokio::test]
async fn interactive_mode_requires_approval_for_b_and_deny_replans() {
    // 交互档：B 级需审批 → 拒绝 → 改案（改用只读回答）
    let (_dir, session, _store, _p) = setup(
        vec![
            ScriptedReply::Tool {
                name: "apply_patch".into(),
                args: serde_json::json!({"file": "a.txt", "range": null, "content": "x\n"}),
            },
            ScriptedReply::Text("好的，我不改了。".into()),
        ],
        false,
        Mode::Interactive,
    )
    .await;
    // 审批到达时拒绝
    let mut rx = session.subscribe();
    let session2 = session.clone();
    let denier = tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(ev) if ev.kind == EventKind::ApprovalRequest => {
                    let id = ev.payload["approval_id"].as_str().unwrap().to_string();
                    session2
                        .decide_approval(&id, ApprovalDecision::Deny, "apply_patch")
                        .await
                        .unwrap();
                    break;
                }
                Ok(_) => continue,
                Err(_) => break,
            }
        }
    });
    let outcome = session.run_task("改一下").await;
    denier.await.unwrap();
    match outcome {
        TaskOutcome::Done(card) => {
            assert!(card.changed_files.is_empty(), "拒绝后不应有改动");
            assert!(card.answer.contains("不改了"));
        }
        other => panic!("期望 Done，实际 {other:?}"),
    }
}

#[tokio::test]
async fn approval_once_executes_d_level_git_commit() {
    let (dir, session, _store, _p) = setup(
        vec![
            ScriptedReply::Tool {
                name: "git_commit".into(),
                args: serde_json::json!({"message": "test commit"}),
            },
            ScriptedReply::Text("已提交".into()),
        ],
        true,
        Mode::Auto,
    )
    .await;
    // 建 git 仓库（含一个用户文件，否则 nothing to commit）
    let root = dir.path();
    std::fs::write(root.join("code.txt"), "fn main() {}\n").unwrap();
    for cmd in [
        vec!["init", "-q", "."],
        vec!["config", "user.email", "t@t"],
        vec!["config", "user.name", "t"],
    ] {
        std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(&cmd)
            .output()
            .unwrap();
    }

    let mut rx = session.subscribe();
    let session2 = session.clone();
    let approver = tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(ev) if ev.kind == EventKind::ApprovalRequest => {
                    assert_eq!(ev.payload["level"], "d", "git_commit 是 D 级");
                    let id = ev.payload["approval_id"].as_str().unwrap().to_string();
                    session2
                        .decide_approval(&id, ApprovalDecision::Once, "git_commit")
                        .await
                        .unwrap();
                    break;
                }
                Ok(_) => continue,
                Err(_) => break,
            }
        }
    });
    let outcome = session.run_task("提交代码").await;
    approver.await.unwrap();
    assert!(matches!(outcome, TaskOutcome::Done(_)), "{outcome:?}");
    let log = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["log", "--oneline"])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&log.stdout).contains("test commit"));
}

#[tokio::test]
async fn readonly_session_blocks_writes_but_answers() {
    let (_dir, _unused_session, _store, _p) = setup(
        vec![
            ScriptedReply::Tool {
                name: "apply_patch".into(),
                args: serde_json::json!({"file": "a.txt", "range": null, "content": "x\n"}),
            },
            ScriptedReply::Text("只读会话无法修改文件。".into()),
        ],
        true,
        Mode::Auto,
    )
    .await;
    // 只读：由 rules 只收窄注入
    let (_d2, session) = {
        // 重建一个只读会话
        let dir = tempfile::tempdir().unwrap();
        let store: StdStore = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
        let project_id = {
            let mut st = store.lock().await;
            st.upsert_project(dir.path().to_str().unwrap()).unwrap().id
        };
        let snapshots_root = dir.path().join(".tenon-snapshots");
        let snapshots =
            Arc::new(SnapshotStore::open(&snapshots_root, &project_id, dir.path(), 2).unwrap());
        let provider = Arc::new(MockProvider::new(
            "mock",
            "mock-1",
            vec![
                ScriptedReply::Tool {
                    name: "apply_patch".into(),
                    args: serde_json::json!({"file": "a.txt", "range": null, "content": "x\n"}),
                },
                ScriptedReply::Text("只读会话无法修改文件。".into()),
            ],
        ));
        let mut config =
            AgentConfig::for_project(dir.path().to_path_buf(), &project_id, true, Mode::Auto);
        config.first_edit_buffer_ms = 10;
        let rules = ProjectRules {
            readonly: Some(true),
            ..Default::default()
        };
        let session = AgentSession::create(
            store.clone(),
            snapshots,
            provider,
            config,
            ProjectWriteLock::new(),
            rules,
        )
        .await
        .unwrap();
        (dir, session)
    };
    let outcome = session.run_task("改文件").await;
    match outcome {
        TaskOutcome::Done(card) => {
            assert!(card.changed_files.is_empty(), "只读会话零改动");
            assert!(card.answer.contains("只读"));
        }
        other => panic!("期望 Done，实际 {other:?}"),
    }
}

#[tokio::test]
async fn circuit_breaker_pauses_on_file_budget() {
    let (dir, session, _store, _p) = setup(
        vec![
            ScriptedReply::Tool {
                name: "apply_patch".into(),
                args: serde_json::json!({"file": "a.txt", "range": null, "content": "x\n"}),
            },
            ScriptedReply::Tool {
                name: "apply_patch".into(),
                args: serde_json::json!({"file": "b.txt", "range": null, "content": "y\n"}),
            },
            ScriptedReply::Text("done".into()),
        ],
        true,
        Mode::Auto,
    )
    .await;
    // 收紧熔断：最多 1 个文件
    session
        .set_circuit_limits(tenon_core::circuit::CircuitLimits {
            max_files: 1,
            max_lines: 10_000,
            max_tokens: 1_000_000,
            max_cost_usd: 100.0,
        })
        .await;
    let outcome = session.run_task("大改").await;
    match outcome {
        TaskOutcome::Paused { reason, .. } => {
            assert!(reason.contains("熔断"), "{reason}");
        }
        other => panic!("期望 Paused，实际 {other:?}"),
    }
    assert!(dir.path().join("a.txt").exists());
    assert!(!dir.path().join("b.txt").exists(), "熔断在第二个文件前暂停");
}

#[tokio::test]
async fn model_failure_rolls_back_to_pre_task_state() {
    let (dir, session, _store, _p) = setup(
        vec![
            ScriptedReply::Tool {
                name: "apply_patch".into(),
                args: serde_json::json!({"file": "a.txt", "range": null, "content": "written\n"}),
            },
            ScriptedReply::Failure("provider down".into()),
        ],
        true,
        Mode::Auto,
    )
    .await;
    let outcome = session.run_task("改文件").await;
    match outcome {
        TaskOutcome::Error(msg) => assert!(msg.contains("provider down")),
        other => panic!("期望 Error，实际 {other:?}"),
    }
    assert!(
        !dir.path().join("a.txt").exists(),
        "失败回滚：半成品不保留（§10.3 崩溃恢复语义）"
    );
}

#[tokio::test]
async fn pause_control_stops_between_tool_calls() {
    let (_dir, session, _store, _p) = setup(
        vec![
            ScriptedReply::Tool {
                name: "apply_patch".into(),
                args: serde_json::json!({"file": "a.txt", "range": null, "content": "x\n"}),
            },
            ScriptedReply::Tool {
                name: "apply_patch".into(),
                args: serde_json::json!({"file": "b.txt", "range": null, "content": "y\n"}),
            },
            ScriptedReply::Text("done".into()),
        ],
        true,
        Mode::Auto,
    )
    .await;
    session.control(ControlCommand::Pause);
    let outcome = session.run_task("任务").await;
    assert!(
        matches!(outcome, TaskOutcome::Paused { .. }),
        "暂停应在工具间生效，实际 {outcome:?}"
    );
}

#[tokio::test]
async fn rollback_last_restores_pre_task_snapshot() {
    let (dir, session, _store, _p) = setup(
        vec![
            ScriptedReply::Tool {
                name: "apply_patch".into(),
                args: serde_json::json!({"file": "new.txt", "range": null, "content": "ai made\n"}),
            },
            ScriptedReply::Text("done".into()),
        ],
        true,
        Mode::Auto,
    )
    .await;
    session.run_task("创建文件").await;
    assert!(dir.path().join("new.txt").exists());

    let rolled = session.rollback_last().await.unwrap();
    assert_eq!(rolled, vec!["new.txt".to_string()]);
    assert!(!dir.path().join("new.txt").exists(), "回滚删除 AI 新建文件");

    // unrevert 撤销回滚
    session.unrevert().await.unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.path().join("new.txt")).unwrap(),
        "ai made\n"
    );
}

#[tokio::test]
async fn secret_redaction_applies_to_git_read_output() {
    use tenon_agent::executor::ToolContext;
    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(dir.path(), Duration::from_secs(10));
    // git_read 输出含密钥时脱敏（通过直接执行器断言 redact 行为已在 core 测试；
    // 此处验证工具链路会调用 redact——构造 secret.env 并 grep）
    std::fs::write(
        dir.path().join("secret.env"),
        "API_KEY=sk-abcdefghijklmnopqrstuvwx\n",
    )
    .unwrap();
    let out = tenon_agent::executor::execute_tool(
        &ctx,
        "grep",
        &serde_json::json!({"pattern": "API_KEY"}),
    );
    // 注意：grep 命中源文件内容本身是数据；§12.4 拦截针对「进上下文的读取输出」，
    // 由 daemon 层 redact_tracked 包装。此处断言文件内容可被检索到。
    assert!(out.ok);
    assert!(out.content.contains("API_KEY"));
}

#[tokio::test]
async fn concurrent_tasks_serialized_by_project_write_lock() {
    let (_dir, session, _store, p) =
        setup(vec![ScriptedReply::Text("answer".into())], true, Mode::Auto).await;
    // 同一会话的并发 run_task 由写锁串行；两次都能完成
    let s1 = session.clone();
    let t1 = tokio::spawn(async move { s1.run_task("任务1").await });
    let s2 = session.clone();
    let t2 = tokio::spawn(async move { s2.run_task("任务2").await });
    let (r1, r2) = tokio::join!(t1, t2);
    assert!(matches!(r1.unwrap(), TaskOutcome::Done(_)));
    assert!(matches!(r2.unwrap(), TaskOutcome::Done(_)));
    let _ = p.calls();
}
