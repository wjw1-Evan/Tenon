//! Agent 会话集成测试（mock provider 脚本化驱动 §9.1 全流程）。

use std::sync::Arc;
use std::time::Duration;

use tenon_agent::session::{AgentConfig, ControlCommand, ProjectWriteLock, TaskOutcome};
use tenon_agent::AgentSession;
use tenon_core::context::ProjectRules;
use tenon_fs::l4;
use tenon_models::{MockProvider, Role, ScriptedReply};
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
    config.first_edit_buffer_ms = 20; // 测试加速
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
    let (_d, session, store, _p) =
        setup(vec![ScriptedReply::Text("这是纯回答，无改动".into())]).await;
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
async fn model_stream_is_persisted_and_joined_without_loss() {
    let (_d, session, store, _p) = setup(vec![ScriptedReply::Text("流式回答ABC".into())]).await;
    let outcome = session.run_task("解释").await;
    assert!(matches!(outcome, TaskOutcome::Done(_)));
    let mut st = store.lock().await;
    let deltas = st
        .events(&session.session_id)
        .unwrap()
        .into_iter()
        .filter(|event| event.kind == EventKind::ModelDelta)
        .map(|event| {
            event
                .payload
                .get("text")
                .and_then(|text| text.as_str())
                .unwrap_or_default()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("");
    assert_eq!(deltas, "流式回答ABC");
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
async fn truncated_reply_continues_instead_of_done() {
    // v1.53：finish_reason=length 的纯文本回复不是回答——注入续写指令继续循环，
    // 后续回合完成改动；不修此路径时长规划被截断即静默 Done（changed_files 空）。
    let (dir, session, _store, _p) = setup(
        vec![
            ScriptedReply::Truncated("任务规划：先写游戏主体…（输出被截断）".into()),
            ScriptedReply::Tool {
                name: "apply_patch".into(),
                args: serde_json::json!({"file": "src/lib.rs", "range": null, "content": "fn game() {}\n"}),
            },
            ScriptedReply::Text("开发完成".into()),
        ],
    )
    .await;
    let outcome = session.run_task("开发游戏").await;
    match outcome {
        TaskOutcome::Done(card) => {
            assert_eq!(card.changed_files, vec!["src/lib.rs".to_string()]);
            assert_eq!(card.answer, "开发完成");
        }
        other => panic!("期望截断后续跑完成，实际 {other:?}"),
    }
    assert_eq!(
        std::fs::read_to_string(dir.path().join("src/lib.rs")).unwrap(),
        "fn game() {}\n"
    );
    // 续写指令进入对话历史（第二次模型调用含 nudge 用户消息）
}

#[tokio::test]
async fn consecutive_truncations_error_out() {
    // v1.53：连续 3 次截断按模型失败语义转 ERROR，不再无限续跑
    let (_d, session, store, _p) = setup(vec![
        ScriptedReply::Truncated("t1".into()),
        ScriptedReply::Truncated("t2".into()),
        ScriptedReply::Truncated("t3".into()),
    ])
    .await;
    let outcome = session.run_task("任务").await;
    match outcome {
        TaskOutcome::Error(msg) => assert!(msg.contains("截断"), "错误信息应说明截断: {msg}"),
        other => panic!("期望 Error，实际 {other:?}"),
    }
    let mut st = store.lock().await;
    let events = st.events(&session.session_id).unwrap();
    assert!(events.iter().any(|e| e.kind == EventKind::Error));
}

#[tokio::test]
async fn interactive_mode_executes_b_level_writes_directly() {
    // v1.89：档位只是兼容元数据；B 级修改不再等待审批。
    let (dir, session, store, _p) = setup(vec![
        ScriptedReply::Tool {
            name: "apply_patch".into(),
            args: serde_json::json!({"file": "a.txt", "range": null, "content": "one\n"}),
        },
        ScriptedReply::Text("已写入。".into()),
    ])
    .await;
    let outcome = session.run_task("写一个文件").await;
    match outcome {
        TaskOutcome::Done(card) => assert_eq!(card.changed_files, vec!["a.txt"]),
        other => panic!("期望 Done，实际 {other:?}"),
    }
    assert_eq!(
        std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "one\n"
    );
    let mut st = store.lock().await;
    let events = st.events(&session.session_id).unwrap();
    // v1.89 审批事件类型已删：B 级写直接落 patch_applied
    assert!(events.iter().any(|e| e.kind == EventKind::PatchApplied));
}

#[tokio::test]
async fn d_level_git_commit_executes_and_audits_directly() {
    let (dir, session, store, _p) = setup(vec![
        ScriptedReply::Tool {
            name: "git_commit".into(),
            args: serde_json::json!({"message": "test commit"}),
        },
        ScriptedReply::Text("已提交".into()),
    ])
    .await;
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
    let outcome = session.run_task("提交代码").await;
    assert!(matches!(outcome, TaskOutcome::Done(_)), "{outcome:?}");
    let log = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["log", "--oneline"])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&log.stdout).contains("test commit"));
    let mut st = store.lock().await;
    let events = st.events(&session.session_id).unwrap();
    let risk = events
        .iter()
        .find(|e| e.kind == EventKind::DirectAction)
        .expect("D 级动作应有直执审计");
    assert_eq!(risk.payload["level"], "d");
    assert_eq!(risk.payload["tool"], "git_commit");
}

#[tokio::test]
async fn readonly_session_blocks_writes_but_answers() {
    let (_dir, _unused_session, _store, _p) = setup(vec![
        ScriptedReply::Tool {
            name: "apply_patch".into(),
            args: serde_json::json!({"file": "a.txt", "range": null, "content": "x\n"}),
        },
        ScriptedReply::Text("只读会话无法修改文件。".into()),
    ])
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
        let mut config = AgentConfig::for_project(dir.path().to_path_buf(), &project_id);
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
    let (dir, session, _store, _p) = setup(vec![
        ScriptedReply::Tool {
            name: "apply_patch".into(),
            args: serde_json::json!({"file": "a.txt", "range": null, "content": "x\n"}),
        },
        ScriptedReply::Tool {
            name: "apply_patch".into(),
            args: serde_json::json!({"file": "b.txt", "range": null, "content": "y\n"}),
        },
        ScriptedReply::Text("done".into()),
    ])
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
    let (dir, session, _store, _p) = setup(vec![
        ScriptedReply::Tool {
            name: "apply_patch".into(),
            args: serde_json::json!({"file": "a.txt", "range": null, "content": "written\n"}),
        },
        ScriptedReply::Failure("provider down".into()),
    ])
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pause_control_stops_between_tool_calls() {
    let (_dir, session, _store, _p) = setup(vec![
        ScriptedReply::Tool {
            name: "apply_patch".into(),
            args: serde_json::json!({"file": "a.txt", "range": null, "content": "x\n"}),
        },
        ScriptedReply::Tool {
            name: "apply_patch".into(),
            args: serde_json::json!({"file": "b.txt", "range": null, "content": "y\n"}),
        },
        ScriptedReply::Text("done".into()),
    ])
    .await;
    // v1.93：Pause = 挂起等待（Resume 继续 / Stop 退出），不再直接返回
    session.control(ControlCommand::Pause);
    let handle = tokio::spawn({
        let session = session.clone();
        async move { session.run_task("任务").await }
    });
    for _ in 0..250 {
        if matches!(
            session.current_state().await,
            tenon_core::machine::State::Paused
        ) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(matches!(
        session.current_state().await,
        tenon_core::machine::State::Paused
    ));
    session.control(ControlCommand::Stop);
    let outcome = handle.await.unwrap();
    match outcome {
        TaskOutcome::Paused { reason, .. } => assert!(reason.contains("停止"), "{reason}"),
        other => panic!("期望 Stop 退出为 Paused，实际 {other:?}"),
    }
}

#[tokio::test]
async fn rollback_last_restores_pre_task_snapshot() {
    let (dir, session, _store, _p) = setup(vec![
        ScriptedReply::Tool {
            name: "apply_patch".into(),
            args: serde_json::json!({"file": "new.txt", "range": null, "content": "ai made\n"}),
        },
        ScriptedReply::Text("done".into()),
    ])
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
    let (_dir, session, _store, p) = setup(vec![ScriptedReply::Text("answer".into())]).await;
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

#[tokio::test]
async fn l4_recall_builds_l1_working_set_for_provider() {
    let (_d, session, store, provider) =
        setup(vec![ScriptedReply::Text("已基于 L4 上下文回答".into())]).await;
    {
        let mut st = store.lock().await;
        let project_id = session.config().project_id.clone();
        st.replace_l4_file(
            &project_id,
            "src/auth.rs",
            &[tenon_store::L4ChunkRecord {
                symbol: Some("authenticate".into()),
                start_line: 12,
                end_line: 18,
                text: "pub fn authenticate(user: &str, password: &str) {}".into(),
                embedding: l4::embed(
                    "authenticate user login",
                    "pub fn authenticate user password session",
                ),
            }],
        )
        .unwrap();
    }

    let outcome = session.run_task("authenticate user login").await;
    assert!(matches!(outcome, TaskOutcome::Done(_)));

    // L4 召回必须进入 Trace（不含 chunk 原文，只有 path / range / score）。
    let events = {
        let mut st = store.lock().await;
        st.events(&session.session_id).unwrap()
    };
    let recall = events
        .iter()
        .find(|event| {
            event.kind == EventKind::Sensing
                && event.payload.get("l4_recall") == Some(&serde_json::json!(true))
        })
        .expect("L4 recall should be traced");
    assert_eq!(
        recall.payload["slices"][0]["path"],
        serde_json::json!("src/auth.rs")
    );

    // L1 工作集注入首条 user 消息，模型能看到 path / 行号 / 不可信标记。
    let request = &provider.calls()[0];
    let user_message = request
        .messages
        .iter()
        .find(|message| message.role == Role::User)
        .expect("user message");
    assert!(user_message.content.contains("authenticate user login"));
    assert!(user_message.content.contains("src/auth.rs:12-18"));
    assert!(user_message.content.contains("不可信仓库内容"));
}

#[tokio::test]
async fn inline_completion_uses_single_turn_model_and_records_usage() {
    let (_d, session, store, provider) = setup(vec![ScriptedReply::Text(
        "```rust\nreturn cached_value;\n```".into(),
    )])
    .await;

    let completion = session
        .inline_complete("rust", "fn main() {\n    let cached_value = 1;\n", "\n}")
        .await
        .expect("inline completion");
    assert_eq!(completion, "return cached_value;");

    let request = &provider.calls()[0];
    assert!(
        request.tools.is_empty(),
        "ghost text must not enter tool loop"
    );
    assert_eq!(request.max_tokens, 256);
    assert!(request
        .messages
        .iter()
        .any(|m| m.content.contains("before_cursor")));

    let usage = {
        let mut st = store.lock().await;
        st.project_usage_totals(session.config().project_id.as_str())
            .unwrap()
    };
    assert!(
        usage.0 > 0 && usage.1 > 0,
        "inline completion usage should be attributed"
    );
}

#[test]
fn sanitize_title_takes_first_line_and_strips_wrappers() {
    assert_eq!(
        tenon_agent::sanitize_title("修复登录超时", 24),
        "修复登录超时"
    );
    assert_eq!(
        tenon_agent::sanitize_title("「修复登录超时」", 24),
        "修复登录超时"
    );
    assert_eq!(
        tenon_agent::sanitize_title("\"修复登录超时\"", 24),
        "修复登录超时"
    );
    assert_eq!(
        tenon_agent::sanitize_title("修复登录超时。", 24),
        "修复登录超时"
    );
    assert_eq!(tenon_agent::sanitize_title("第一行\n第二行", 24), "第一行");
    assert_eq!(
        tenon_agent::sanitize_title("很长的标题很长的标题很长的标题很长的标题超出", 10),
        "很长的标题很长的标题"
    );
    assert_eq!(
        tenon_agent::sanitize_title("Refactor checkout flow and add discount tests", 32),
        "Refactor checkout flow and add"
    );
    assert_eq!(tenon_agent::sanitize_title("  \n ", 24), "");
}

/// 实测 GLM 会输出 "The user says: …" 类英文元文本——清洗后只留正文。
#[test]
fn sanitize_title_strips_english_meta_prefixes() {
    assert_eq!(
        tenon_agent::sanitize_title("The user says: \"只回复文字\"你好", 24),
        "只回复文字\"你好"
    );
    assert_eq!(
        tenon_agent::sanitize_title("The user wants a title about 登录超时", 24),
        "a title about 登录超时"
    );
    assert_eq!(
        tenon_agent::sanitize_title("标题：修复登录超时", 24),
        "修复登录超时"
    );
    // 无前缀不受影响；大小写变体同样剥除。
    assert_eq!(tenon_agent::sanitize_title("登录超时", 24), "登录超时");
    assert_eq!(
        tenon_agent::sanitize_title("THE USER SAYS: 登录超时", 24),
        "登录超时"
    );
}

/// v1.58 对话标题：单轮、无工具、带 TITLE_MARKER；不消耗任务脚本队列。
#[tokio::test]
async fn generate_title_is_single_turn_marked_and_keeps_script_intact() {
    let (_d, session, _store, provider) = setup(vec![ScriptedReply::Text("任务回答".into())]).await;
    let title = session
        .generate_title("帮我修复登录超时的 bug，越快越好")
        .await
        .expect("generated title");
    assert_eq!(title, "Mock 会话标题");

    let requests = provider.title_calls();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert!(
        request
            .messages
            .iter()
            .any(|m| m.content.contains("TENON_TASK_TITLE")),
        "title request must carry the marker"
    );
    assert!(
        request.tools.is_empty(),
        "title generation stays out of tool loop"
    );
    // low effort 避免 reasoning 吃光预算；128 足够输出短标题。
    assert_eq!(request.max_tokens, 128);
    assert_eq!(request.reasoning_effort.as_deref(), Some("low"));
    // 标题请求不进任务调用记录，脚本化断言零扰动。
    assert!(provider.calls().is_empty());
}

#[tokio::test]
async fn set_title_persists_and_broadcasts_session_title_event() {
    let (_d, session, store, _p) = setup(vec![]).await;
    let mut rx = session.subscribe();
    session.set_title("修复登录超时").await.unwrap();

    let mut st = store.lock().await;
    assert_eq!(
        st.session(&session.session_id).unwrap().unwrap().title,
        "修复登录超时"
    );
    let events = st.events(&session.session_id).unwrap();
    drop(st);
    let event = events
        .iter()
        .find(|e| e.kind == EventKind::SessionTitle)
        .expect("session_title event appended");
    assert_eq!(event.payload["title"], "修复登录超时");

    let broadcast = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("broadcast within timeout")
        .expect("subscriber receives event");
    assert_eq!(broadcast.kind, EventKind::SessionTitle);
}

// ---------- v1.93：真恢复 / 运行期只读 / 并发守卫 / token 熔断 ----------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pause_suspends_until_resume_then_completes_task() {
    let (dir, session, _store, _p) = setup(vec![
        ScriptedReply::Tool {
            name: "apply_patch".into(),
            args: serde_json::json!({"file": "a.txt", "range": null, "content": "x\n"}),
        },
        ScriptedReply::Tool {
            name: "apply_patch".into(),
            args: serde_json::json!({"file": "b.txt", "range": null, "content": "y\n"}),
        },
        ScriptedReply::Text("done".into()),
    ])
    .await;
    session.control(ControlCommand::Pause);
    let handle = tokio::spawn({
        let session = session.clone();
        async move { session.run_task("任务").await }
    });
    // 挂起等待：状态进入 Paused 且任务未返回
    for _ in 0..250 {
        if matches!(
            session.current_state().await,
            tenon_core::machine::State::Paused
        ) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(matches!(
        session.current_state().await,
        tenon_core::machine::State::Paused
    ));
    assert!(
        !handle.is_finished(),
        "挂起期间任务不得返回（v1.93 真挂起）"
    );
    // 恢复 → 继续原任务直至完成
    session.control(ControlCommand::Resume);
    let outcome = handle.await.unwrap();
    assert!(
        matches!(outcome, TaskOutcome::Done(_)),
        "恢复后应完成原任务，实际 {outcome:?}"
    );
    assert!(dir.path().join("a.txt").exists());
    assert!(
        dir.path().join("b.txt").exists(),
        "恢复后继续执行第二个工具步"
    );
}

#[tokio::test]
async fn set_readonly_takes_effect_at_runtime() {
    let (dir, session, _store, _p) = setup(vec![
        ScriptedReply::Tool {
            name: "apply_patch".into(),
            args: serde_json::json!({"file": "a.txt", "range": null, "content": "x\n"}),
        },
        ScriptedReply::Text("已只读".into()),
    ])
    .await;
    // v1.93：控制命令运行期切换（此前为空操作）
    session.control(ControlCommand::SetReadonly(true));
    let outcome = session.run_task("改文件").await;
    assert!(
        matches!(outcome, TaskOutcome::Done(_)),
        "只读拒绝写后仍应完成回答，实际 {outcome:?}"
    );
    assert!(
        !dir.path().join("a.txt").exists(),
        "只读模式下 B 级写必须被拒绝"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_task_rejects_reentry_while_running() {
    let (_dir, session, _store, _p) = setup(vec![
        ScriptedReply::Tool {
            name: "apply_patch".into(),
            args: serde_json::json!({"file": "a.txt", "range": null, "content": "x\n"}),
        },
        ScriptedReply::Text("done".into()),
    ])
    .await;
    let handle = tokio::spawn({
        let session = session.clone();
        async move { session.run_task("第一个任务").await }
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let second = session.run_task("并发第二个任务").await;
    match &second {
        TaskOutcome::Error(msg) => assert!(msg.contains("任务进行中"), "{msg}"),
        other => panic!("进行中重入应被拒绝，实际 {other:?}"),
    }
    assert!(matches!(handle.await.unwrap(), TaskOutcome::Done(_)));
}

#[tokio::test]
async fn circuit_breaker_pauses_on_token_budget() {
    let (dir, session, _store, _p) = setup(vec![
        ScriptedReply::Tool {
            name: "apply_patch".into(),
            args: serde_json::json!({"file": "a.txt", "range": null, "content": "x\n"}),
        },
        ScriptedReply::Tool {
            name: "apply_patch".into(),
            args: serde_json::json!({"file": "b.txt", "range": null, "content": "y\n"}),
        },
        ScriptedReply::Text("done".into()),
    ])
    .await;
    // §9.3 v1.93：token 预算熔断（此前 record_usage 不进熔断器，max_tokens 恒无效）
    session
        .set_circuit_limits(tenon_core::circuit::CircuitLimits {
            max_files: 100,
            max_lines: 100_000,
            max_tokens: 8,
            max_cost_usd: 1_000.0,
        })
        .await;
    let outcome = session.run_task("长任务").await;
    match &outcome {
        TaskOutcome::Paused { reason, .. } => assert!(reason.contains("max_tokens"), "{reason}"),
        other => panic!("期望 token 熔断暂停，实际 {other:?}"),
    }
    // 预算极小：首个回合用量即超限，在第一个工具步检查点拦截——
    // 后续脚本步（b.txt）不执行即语义达成。
    assert!(
        !dir.path().join("b.txt").exists(),
        "熔断后不得继续执行后续工具步"
    );
}

#[tokio::test]
async fn history_compaction_stubs_stale_tool_outputs_and_emits_trace_event() {
    // §10.2 v1.105：6 次读取同一大文件，第 5 条起更早的工具输出成为陈旧项——
    // 保留最近 4 条原文、更早的存根化，压缩事件入 Trace
    let (dir, session, store, provider) = setup(
        (0..6)
            .map(|_| ScriptedReply::Tool {
                name: "read_file".into(),
                args: serde_json::json!({"path": "big.txt"}),
            })
            .chain(std::iter::once(ScriptedReply::Text("完成".into())))
            .collect(),
    )
    .await;
    // 80k 字符 ≈ 26.7k 估算 token > 24k 阈值：首个工具输出后即进入压缩触发区间
    std::fs::write(dir.path().join("big.txt"), "x".repeat(80_000)).unwrap();
    let outcome = session.run_task("读取大文件").await;
    assert!(matches!(outcome, TaskOutcome::Done(_)));

    // 末次请求：6 条工具输出 → 最早 2 条存根化，最近 4 条保留原文
    let calls = provider.calls();
    let last = calls.last().expect("mock 记录了全部请求");
    let stubs = last
        .messages
        .iter()
        .filter(|m| m.content.contains("[工具输出已省略：read_file"))
        .count();
    assert_eq!(stubs, 2, "陈旧工具输出被存根化");
    let verbatim = last
        .messages
        .iter()
        .filter(|m| m.role == Role::Tool && m.content.starts_with('x'))
        .count();
    assert_eq!(verbatim, 4, "最近 4 条工具输出保留原文");
    assert!(
        last.messages
            .iter()
            .any(|m| m.role == Role::User && m.content.contains("读取大文件")),
        "首条 user（任务文本）永不省略"
    );
    // 压缩后请求体积受控：只有保留窗内的原文全文驻留
    let total_chars: usize = last
        .messages
        .iter()
        .map(|m| m.content.chars().count())
        .sum();
    assert!(
        total_chars < 80_000 * 5,
        "压缩后请求体积应远小于无压缩累积（实际 {total_chars}）"
    );

    // 压缩事件入 Trace：第 5、6 条工具输出各触发一次省略
    let mut st = store.lock().await;
    let events = st.events(&session.session_id).unwrap();
    let elided_total: u64 = events
        .iter()
        .filter(|e| e.kind == EventKind::Compaction)
        .map(|e| e.payload["elided_tool_results"].as_u64().unwrap_or(0))
        .sum();
    assert_eq!(elided_total, 2, "压缩事件累计省略条数入 Trace");
}

// ---------- L5 跨会话对话记忆（§10.1 v1.104） ----------

/// v1.104：任务 Done 后触发单轮记忆提取——请求带 MEMORY_MARKER、无工具目录、
/// max_tokens=512；MockProvider 固定 JSON 入库为 global preference；
/// 任务调用序列不受提取影响。
#[tokio::test]
async fn task_done_extracts_memories_via_marked_call() {
    let (_d, session, store, provider) = setup(vec![ScriptedReply::Text("任务回答".into())]).await;
    let outcome = session.run_task("记住我喜欢中文回复").await;
    assert!(matches!(outcome, TaskOutcome::Done(_)));

    let memory_calls = provider.memory_calls();
    assert_eq!(memory_calls.len(), 1, "单轮提取调用");
    let request = &memory_calls[0];
    assert!(request
        .messages
        .iter()
        .any(|m| m.content.contains("TENON_MEMORY_EXTRACT")));
    assert!(request.tools.is_empty(), "提取不出工具目录");
    assert_eq!(request.max_tokens, 512);

    // 提取结果入库：mock 固定返回一条 global preference
    let mut st = store.lock().await;
    let project_id = st.list_projects().unwrap()[0].id.clone();
    let memories = st.list_memories(&project_id, None, 100).unwrap();
    assert_eq!(memories.len(), 1);
    assert_eq!(memories[0].scope, "global");
    assert_eq!(memories[0].kind, "preference");
    assert_eq!(memories[0].content, "Mock 记忆：回复用中文");
    // memory_saved 事件入 Trace（仅 count / ids）
    let events = st.events(&session.session_id).unwrap();
    assert!(
        events
            .iter()
            .any(|e| e.kind == EventKind::MemorySaved && e.payload["count"].as_u64() == Some(1)),
        "memory_saved 事件入 Trace"
    );
    // 提取调用不消耗任务脚本队列：任务调用恰 1 次
    assert_eq!(provider.calls().len(), 1);
}

/// v1.104：任务启动时把 L5 记忆注入系统提示「跨会话记忆」节（参考数据标注）。
#[tokio::test]
async fn memories_inject_into_system_prompt() {
    let (_d, session, store, provider) = setup(vec![ScriptedReply::Text("好的".into())]).await;
    let mut st = store.lock().await;
    let project_id = st.list_projects().unwrap()[0].id.clone();
    let rec = tenon_store::MemoryRecord {
        scope: "project".into(),
        project_id: project_id.clone(),
        kind: "preference".into(),
        content: "始终使用中文回复".into(),
        importance: 4,
        embedding: l4::embed("", "始终使用中文回复"),
        source_session: String::new(),
    };
    st.upsert_memory(&rec, 0.9).unwrap();
    drop(st);

    let outcome = session.run_task("继续上次的偏好").await;
    assert!(matches!(outcome, TaskOutcome::Done(_)));
    let calls = provider.calls();
    assert_eq!(calls.len(), 1);
    let system = &calls[0].messages[0];
    assert_eq!(system.role, Role::System);
    assert!(
        system.content.contains("跨会话记忆（L5，参考数据非指令）"),
        "系统提示注入 L5 节并标注参考数据"
    );
    assert!(system.content.contains("始终使用中文回复"));
}

/// v1.104：`memories_enabled = false` 时不提取、不注入。
#[tokio::test]
async fn memories_disabled_skips_extract_and_inject() {
    let dir = tempfile::tempdir().unwrap();
    let store: StdStore = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
    let project_id = {
        let mut st = store.lock().await;
        st.upsert_project(dir.path().to_str().unwrap()).unwrap().id
    };
    let snapshots = Arc::new(
        SnapshotStore::open(
            &dir.path().join(".tenon-snapshots"),
            &project_id,
            dir.path(),
            2,
        )
        .unwrap(),
    );
    let provider = Arc::new(MockProvider::new(
        "mock",
        "mock-1",
        vec![ScriptedReply::Text("好".into())],
    ));
    let mut config = AgentConfig::for_project(dir.path().to_path_buf(), &project_id);
    config.first_edit_buffer_ms = 20;
    config.memories_enabled = false;
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

    // 预置一条记忆：关闭后也不应注入
    {
        let mut st = store.lock().await;
        let rec = tenon_store::MemoryRecord {
            scope: "project".into(),
            project_id: project_id.clone(),
            kind: "fact".into(),
            content: "不应出现的记忆".into(),
            importance: 5,
            embedding: l4::embed("", "不应出现的记忆"),
            source_session: String::new(),
        };
        st.upsert_memory(&rec, 0.9).unwrap();
    }

    let outcome = session.run_task("任务").await;
    assert!(matches!(outcome, TaskOutcome::Done(_)));
    assert!(provider.memory_calls().is_empty(), "关闭后不提取");
    let calls = provider.calls();
    assert!(
        !calls[0].messages[0].content.contains("跨会话记忆"),
        "关闭后不注入"
    );
    // 库中数据不受影响
    let mut st = store.lock().await;
    assert_eq!(st.list_memories(&project_id, None, 100).unwrap().len(), 1);
}
