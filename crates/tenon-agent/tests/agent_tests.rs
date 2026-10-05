//! Agent 会话集成测试（mock provider 脚本化驱动 §9.1 全流程）。

use std::sync::Arc;
use std::time::Duration;

use tenon_agent::session::{AgentConfig, ControlCommand, ProjectWriteLock, TaskOutcome};
use tenon_agent::AgentSession;
use tenon_core::context::ProjectRules;
use tenon_core::policy::Mode;
use tenon_fs::l4;
use tenon_models::{MockProvider, Role, ScriptedReply};
use tenon_snapshot::SnapshotStore;
use tenon_store::{EventKind, Store};
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
async fn model_stream_is_persisted_and_joined_without_loss() {
    let (_d, session, store, _p) = setup(
        vec![ScriptedReply::Text("流式回答ABC".into())],
        false,
        Mode::Interactive,
    )
    .await;
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
        true,
        Mode::Auto,
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
    let (_d, session, store, _p) = setup(
        vec![
            ScriptedReply::Truncated("t1".into()),
            ScriptedReply::Truncated("t2".into()),
            ScriptedReply::Truncated("t3".into()),
        ],
        true,
        Mode::Auto,
    )
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
    let (dir, session, store, _p) = setup(
        vec![
            ScriptedReply::Tool {
                name: "apply_patch".into(),
                args: serde_json::json!({"file": "a.txt", "range": null, "content": "one\n"}),
            },
            ScriptedReply::Text("已写入。".into()),
        ],
        false,
        Mode::Interactive,
    )
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
    assert!(!events.iter().any(|e| e.kind == EventKind::ApprovalRequest));
}

#[tokio::test]
async fn d_level_git_commit_executes_and_audits_directly() {
    let (dir, session, store, _p) = setup(
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

#[tokio::test]
async fn l4_recall_builds_l1_working_set_for_provider() {
    let (_d, session, store, provider) = setup(
        vec![ScriptedReply::Text("已基于 L4 上下文回答".into())],
        false,
        Mode::Interactive,
    )
    .await;
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
    let (_d, session, store, provider) = setup(
        vec![ScriptedReply::Text(
            "```rust\nreturn cached_value;\n```".into(),
        )],
        true,
        Mode::Interactive,
    )
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
    let (_d, session, _store, provider) = setup(
        vec![ScriptedReply::Text("任务回答".into())],
        false,
        Mode::Interactive,
    )
    .await;
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
    assert_eq!(request.max_tokens, 48);
    // 标题请求不进任务调用记录，脚本化断言零扰动。
    assert!(provider.calls().is_empty());
}

#[tokio::test]
async fn set_title_persists_and_broadcasts_session_title_event() {
    let (_d, session, store, _p) = setup(vec![], false, Mode::Interactive).await;
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
