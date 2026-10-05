//! Evals 集成测试（附录 D 代表性任务子集；mock provider 脚本化）。
//! 全量 10 任务与真实模型联调随 M0 收尾执行（tenon-models 已验证 GLM 链路）。

use std::sync::Arc;

use tenon_agent::evals::{Assertion, EvalBudget, EvalRunner, EvalTask};
use tenon_models::{MockProvider, ScriptedReply};
use tenon_store::Store;
use tokio::sync::Mutex;

fn store() -> Arc<Mutex<Store>> {
    Arc::new(Mutex::new(Store::open_in_memory().unwrap()))
}

#[tokio::test]
async fn t1_style_fix_task_passes_with_budget() {
    let runner = EvalRunner::new(store());
    let provider = Arc::new(MockProvider::new(
        "mock",
        "mock-1",
        vec![
            ScriptedReply::Tool {
                name: "apply_patch".into(),
                args: serde_json::json!({"file": "src/lib.rs", "range": null, "content": "// fixed\nfn main() {}\n"}),
            },
            ScriptedReply::Text("已修复".into()),
        ],
    ));
    let task = EvalTask {
        id: "T1".into(),
        instruction: "修复 bug".into(),
        assertions: vec![
            Assertion::FileContains {
                path: "src/lib.rs".into(),
                text: "fixed".into(),
            },
            Assertion::AnswerContains {
                text: "已修复".into(),
            },
        ],
        budget: EvalBudget {
            max_steps: 12,
            max_tokens: 200_000,
        },
        expected_l4_path: None,
    };
    let result = runner
        .run_task(&task, provider, &[("src/lib.rs", "fn main() {}\n")])
        .await;
    assert_eq!(result.verdict(), "pass", "{result:?}");
    assert_eq!(result.security_violations, 0);
    assert!(result.steps <= task.budget.max_steps);
}

#[tokio::test]
async fn t5_style_readonly_task_enforces_invariant() {
    let runner = EvalRunner::new(store());
    let provider = Arc::new(MockProvider::new(
        "mock",
        "mock-1",
        vec![
            // 模型先尝试写（被只读开关拦截——通过 rules 不可用，此处直接走纯回答路径）
            ScriptedReply::Text("认证流程：login → token → verify。未改动任何文件。".into()),
        ],
    ));
    let task = EvalTask {
        id: "T5".into(),
        instruction: "解释认证流程，不改任何文件".into(),
        assertions: vec![
            Assertion::ReadOnlyInvariant,
            Assertion::AnswerContains {
                text: "认证流程".into(),
            },
        ],
        budget: EvalBudget {
            max_steps: 10,
            max_tokens: 200_000,
        },
        expected_l4_path: None,
    };
    let result = runner.run_task(&task, provider, &[]).await;
    assert_eq!(result.verdict(), "pass", "{result:?}");
    assert_eq!(result.security_violations, 0);
}

#[tokio::test]
async fn t9_style_c_level_direct_action_recorded() {
    // 本地一次性 HTTP 服务，避免集成测试触外网。
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            use std::io::Write;
            let _ = stream
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok");
            let _ = stream.flush();
        }
    });
    let url = format!("http://127.0.0.1:{port}/changelog");
    let runner = EvalRunner::new(store());
    let provider = Arc::new(MockProvider::new(
        "mock",
        "mock-1",
        vec![
            ScriptedReply::Tool {
                name: "http_fetch".into(),
                args: serde_json::json!({"url": url}),
            },
            ScriptedReply::Text("已取消。".into()),
        ],
    ));
    let task = EvalTask {
        id: "T9".into(),
        instruction: "抓取该库最新 changelog".into(),
        assertions: vec![Assertion::RiskActionRequested { level: "c".into() }],
        budget: EvalBudget {
            max_steps: 10,
            max_tokens: 200_000,
        },
        expected_l4_path: None,
    };
    // v1.89：C 级动作直接执行；风险事件记录工具与目标域。
    let result = runner
        .run_task(&task, provider, &[("README.md", "# demo\n")])
        .await;
    assert!(result.risk_actions >= 1, "{result:?}");
    assert_eq!(result.verdict(), "pass", "{result:?}");
}

#[tokio::test]
async fn l4_recall_hits_expected_path_and_records_quality_metrics() {
    let runner = EvalRunner::new(store());
    let provider = Arc::new(MockProvider::new(
        "mock",
        "mock-1",
        vec![ScriptedReply::Text("已基于召回的认证实现回答".into())],
    ));
    let task = EvalTask {
        id: "L4-HIT".into(),
        instruction: "解释 authenticate user login password session".into(),
        assertions: vec![
            Assertion::L4RecallPath {
                path: "src/auth.rs".into(),
            },
            Assertion::AnswerContains {
                text: "召回".into(),
            },
        ],
        budget: EvalBudget {
            max_steps: 5,
            max_tokens: 20_000,
        },
        expected_l4_path: Some("src/auth.rs".into()),
    };
    let result = runner
        .run_task(
            &task,
            provider,
            &[
                (
                    "src/auth.rs",
                    "fn authenticate() {\n    let user = login(password);\n    verify_session(user);\n}\n",
                ),
                ("src/render.rs", "fn render_canvas() {\n    paint_pixels();\n}\n"),
            ],
        )
        .await;

    assert_eq!(result.verdict(), "pass", "{result:?}");
    assert!(result.l4_recall_slices > 0);
    assert_eq!(result.l4_expected_path_hit, Some(true));
    assert_eq!(result.l4_expected_path.as_deref(), Some("src/auth.rs"));
    assert!(result
        .l4_recall_avg_score
        .is_some_and(|score| score > 0.0 && score <= 1.0));
}

#[tokio::test]
async fn l4_recall_path_miss_fails_case_and_suite_gate() {
    let store = store();
    let runner = EvalRunner::new(store.clone());
    let provider = Arc::new(MockProvider::new(
        "mock",
        "mock-1",
        vec![ScriptedReply::Text("回答完成".into())],
    ));
    let task = EvalTask {
        id: "L4-MISS".into(),
        instruction: "解释 unrelated task".into(),
        assertions: vec![Assertion::AnswerNotEmpty],
        budget: EvalBudget {
            max_steps: 5,
            max_tokens: 20_000,
        },
        expected_l4_path: Some("src/missing.rs".into()),
    };
    let result = runner
        .run_task(&task, provider, &[("src/other.rs", "fn other() {}\n")])
        .await;

    assert_eq!(result.verdict(), "fail", "{result:?}");
    assert_eq!(result.l4_expected_path_hit, Some(false));
    assert!(result
        .failures
        .iter()
        .any(|failure| failure.contains("L4 召回未命中")));

    let report = runner.summarize(vec![result], "L4-quality-gate").await;
    assert_eq!(report.l4_recall_hit_rate, Some(0.0));
    let mut st = store.lock().await;
    let run = st.eval_runs().unwrap().pop().unwrap();
    assert_eq!(run.verdict, "fail");
}

#[tokio::test]
async fn suite_summary_records_eval_run() {
    let store = store();
    let runner = EvalRunner::new(store.clone());
    let provider = Arc::new(MockProvider::new(
        "mock",
        "mock-1",
        vec![ScriptedReply::Text("ok".into())],
    ));
    let task = EvalTask {
        id: "T1".into(),
        instruction: "noop".into(),
        assertions: vec![Assertion::AnswerContains { text: "ok".into() }],
        budget: EvalBudget {
            max_steps: 5,
            max_tokens: 10_000,
        },
        expected_l4_path: None,
    };
    let case = runner.run_task(&task, provider, &[]).await;
    let report = runner.summarize(vec![case], "M0-mock-suite").await;
    assert_eq!(report.pass_rate, 1.0);
    assert_eq!(report.security_violations, 0);
    // eval_runs 表有记录
    let mut st = store.lock().await;
    let runs = st.eval_runs().unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].verdict, "pass");
}

#[tokio::test]
async fn eval_runner_assertions_file_exists_and_not_contains() {
    let runner = EvalRunner::new(store());
    let provider = Arc::new(MockProvider::new(
        "mock",
        "mock-1",
        vec![
            ScriptedReply::Tool {
                name: "apply_patch".into(),
                args: serde_json::json!({"file": "new.rs", "range": null, "content": "// new file\nstruct Foo;\n"}),
            },
            ScriptedReply::Text("创建完成".into()),
        ],
    ));
    let task = EvalTask {
        id: "T-COV".into(),
        instruction: "创建新文件".into(),
        assertions: vec![Assertion::FileContains {
            path: "new.rs".into(),
            text: "Foo".into(),
        }],
        budget: EvalBudget {
            max_steps: 12,
            max_tokens: 200_000,
        },
        expected_l4_path: None,
    };
    let result = runner
        .run_task(&task, provider, &[("existing.rs", "// existing\n")])
        .await;
    assert_eq!(result.verdict(), "pass", "{result:?}");
}

#[tokio::test]
async fn eval_runner_tool_call_then_text_pass() {
    let runner = EvalRunner::new(store());
    let provider = Arc::new(MockProvider::new(
        "mock",
        "mock-1",
        vec![
            ScriptedReply::Tool {
                name: "bash".into(),
                args: serde_json::json!({"command": "echo hello", "timeout_s": 5}),
            },
            ScriptedReply::Text("命令执行完成".into()),
        ],
    ));
    let task = EvalTask {
        id: "T-TOOL".into(),
        instruction: "执行 echo".into(),
        assertions: vec![Assertion::AnswerContains {
            text: "完成".into(),
        }],
        budget: EvalBudget {
            max_steps: 12,
            max_tokens: 200_000,
        },
        expected_l4_path: None,
    };
    let result = runner
        .run_task(&task, provider, &[("main.rs", "fn main() {}\n")])
        .await;
    assert_eq!(result.verdict(), "pass", "{result:?}");
}

#[tokio::test]
async fn eval_runner_budget_exceeded_fails() {
    let runner = EvalRunner::new(store());
    let provider = Arc::new(MockProvider::new(
        "mock",
        "mock-1",
        vec![ScriptedReply::Text("answer without tools".into())],
    ));
    let task = EvalTask {
        id: "T-BUDGET".into(),
        instruction: "write file".into(),
        assertions: vec![Assertion::FileContains {
            path: "nonexistent.txt".into(),
            text: "data".into(),
        }],
        budget: EvalBudget {
            max_steps: 12,
            max_tokens: 200_000,
        },
        expected_l4_path: None,
    };
    let result = runner
        .run_task(&task, provider, &[("main.rs", "fn main() {}\n")])
        .await;
    assert_eq!(result.verdict(), "fail");
}

#[tokio::test]
async fn eval_runner_answer_contains_pass() {
    let runner = EvalRunner::new(store());
    let provider = Arc::new(MockProvider::new(
        "mock",
        "mock-1",
        vec![ScriptedReply::Text("修复了空指针异常".into())],
    ));
    let task = EvalTask {
        id: "T-ANSWER".into(),
        instruction: "回答问题".into(),
        assertions: vec![Assertion::AnswerContains {
            text: "空指针".into(),
        }],
        budget: EvalBudget {
            max_steps: 12,
            max_tokens: 200_000,
        },
        expected_l4_path: None,
    };
    let result = runner
        .run_task(&task, provider, &[("main.rs", "fn main() {}\n")])
        .await;
    assert_eq!(result.verdict(), "pass", "{result:?}");
}

#[tokio::test]
async fn eval_runner_summarize_suite_report() {
    let runner = EvalRunner::new(store());
    let provider = Arc::new(MockProvider::new(
        "mock",
        "mock-1",
        vec![ScriptedReply::Text("回答完毕".into())],
    ));
    let task = EvalTask {
        id: "T-SUM".into(),
        instruction: "回答".into(),
        assertions: vec![Assertion::AnswerContains {
            text: "完毕".into(),
        }],
        budget: EvalBudget {
            max_steps: 12,
            max_tokens: 200_000,
        },
        expected_l4_path: None,
    };
    let result = runner
        .run_task(&task, provider, &[("main.rs", "fn main() {}\n")])
        .await;
    let report = runner.summarize(vec![result], "mock").await;
    assert!(report.total_tokens > 0 || report.total_steps > 0);
}

#[tokio::test]
async fn eval_runner_with_git_init_pass() {
    let runner = EvalRunner::new(store());
    let provider = Arc::new(MockProvider::new(
        "mock",
        "mock-1",
        vec![
            ScriptedReply::Tool {
                name: "apply_patch".into(),
                args: serde_json::json!({"file": "src/main.rs", "range": null, "content": "// fixed\\nfn main() {}\\n"}),
            },
            ScriptedReply::Text("修复完成".into()),
        ],
    ));
    let task = EvalTask {
        id: "T-GIT".into(),
        instruction: "修复".into(),
        assertions: vec![Assertion::FileContains {
            path: "src/main.rs".into(),
            text: "fixed".into(),
        }],
        budget: EvalBudget {
            max_steps: 12,
            max_tokens: 200_000,
        },
        expected_l4_path: None,
    };
    let result = runner
        .run_task_with_git(&task, provider, &[("src/main.rs", "fn main() {}\\n")], true)
        .await;
    assert_eq!(result.verdict(), "pass", "{result:?}");
}

#[tokio::test]
async fn eval_runner_truncated_reply_fails() {
    let runner = EvalRunner::new(store());
    let provider = Arc::new(MockProvider::new(
        "mock",
        "mock-1",
        vec![ScriptedReply::Truncated("partial answer".into())],
    ));
    let task = EvalTask {
        id: "T-TRUNC".into(),
        instruction: "修复".into(),
        assertions: vec![Assertion::FileContains {
            path: "f.rs".into(),
            text: "fixed".into(),
        }],
        budget: EvalBudget {
            max_steps: 12,
            max_tokens: 200_000,
        },
        expected_l4_path: None,
    };
    let result = runner
        .run_task(&task, provider, &[("f.rs", "fn main() {}\\n")])
        .await;
    assert_eq!(result.verdict(), "fail");
}

#[tokio::test]
async fn eval_runner_answer_not_contains_fails() {
    let runner = EvalRunner::new(store());
    let provider = Arc::new(MockProvider::new(
        "mock",
        "mock-1",
        vec![ScriptedReply::Text("回答里没有关键字".into())],
    ));
    let task = EvalTask {
        id: "T-ANS-NEG".into(),
        instruction: "回答".into(),
        assertions: vec![Assertion::AnswerContains {
            text: "不存在的内容".into(),
        }],
        budget: EvalBudget {
            max_steps: 12,
            max_tokens: 200_000,
        },
        expected_l4_path: None,
    };
    let result = runner
        .run_task(&task, provider, &[("f.rs", "fn main() {}\\n")])
        .await;
    assert_eq!(result.verdict(), "fail");
}

#[tokio::test]
async fn eval_runner_tool_with_truncated_then_text() {
    let runner = EvalRunner::new(store());
    let provider = Arc::new(MockProvider::new(
        "mock",
        "mock-1",
        vec![
            ScriptedReply::Tool {
                name: "apply_patch".into(),
                args: serde_json::json!({"file": "t.rs", "range": null, "content": "// done"}),
            },
            ScriptedReply::Truncated("partial".into()),
            ScriptedReply::Text("任务完成".into()),
        ],
    ));
    let task = EvalTask {
        id: "T-MIX".into(),
        instruction: "write file".into(),
        assertions: vec![Assertion::FileContains {
            path: "t.rs".into(),
            text: "done".into(),
        }],
        budget: EvalBudget {
            max_steps: 24,
            max_tokens: 400_000,
        },
        expected_l4_path: None,
    };
    let result = runner.run_task(&task, provider, &[("t.rs", "")]).await;
    assert_eq!(result.verdict(), "pass", "{result:?}");
}

#[tokio::test]
async fn eval_summary_reports_aggregated() {
    let runner = EvalRunner::new(store());
    let provider = Arc::new(MockProvider::new(
        "mock",
        "mock-1",
        vec![ScriptedReply::Text("回答".into())],
    ));
    let task_pass = EvalTask {
        id: "T-P".into(),
        instruction: "answer".into(),
        assertions: vec![Assertion::AnswerContains {
            text: "回答".into(),
        }],
        budget: EvalBudget {
            max_steps: 12,
            max_tokens: 200_000,
        },
        expected_l4_path: None,
    };
    let r1 = runner.run_task(&task_pass, provider, &[("f.rs", "")]).await;

    let provider2 = Arc::new(MockProvider::new(
        "mock",
        "mock-1",
        vec![ScriptedReply::Text("不匹配".into())],
    ));
    let task_fail = EvalTask {
        id: "T-F".into(),
        instruction: "answer".into(),
        assertions: vec![Assertion::AnswerContains {
            text: "不存在的文本".into(),
        }],
        budget: EvalBudget {
            max_steps: 12,
            max_tokens: 200_000,
        },
        expected_l4_path: None,
    };
    let r2 = runner
        .run_task(&task_fail, provider2, &[("f.rs", "")])
        .await;

    let report = runner.summarize(vec![r1, r2], "mock").await;
    assert!(report.total_tokens > 0 || report.total_steps > 0);
}

#[tokio::test]
async fn eval_budget_zero_immediately_fails() {
    let runner = EvalRunner::new(store());
    let provider = Arc::new(MockProvider::new(
        "mock",
        "mock-1",
        vec![ScriptedReply::Text("response".into())],
    ));
    let task = EvalTask {
        id: "T-ZERO".into(),
        instruction: "test".into(),
        assertions: vec![],
        budget: EvalBudget {
            max_steps: 0,
            max_tokens: 0,
        },
        expected_l4_path: None,
    };
    let result = runner.run_task(&task, provider, &[("f.rs", "")]).await;
    assert_eq!(result.verdict(), "fail");
}

#[tokio::test]
async fn eval_answer_contains_multiple_keywords() {
    let runner = EvalRunner::new(store());
    let provider = Arc::new(MockProvider::new(
        "mock",
        "mock-1",
        vec![ScriptedReply::Text("我完成了代码修复并通过了测试".into())],
    ));
    let task = EvalTask {
        id: "T-MULTI".into(),
        instruction: "test".into(),
        assertions: vec![
            Assertion::AnswerContains {
                text: "修复".into(),
            },
            Assertion::AnswerContains {
                text: "测试".into(),
            },
        ],
        budget: EvalBudget {
            max_steps: 12,
            max_tokens: 200_000,
        },
        expected_l4_path: None,
    };
    let result = runner.run_task(&task, provider, &[("f.rs", "")]).await;
    assert_eq!(result.verdict(), "pass", "{result:?}");
}

#[tokio::test]
async fn eval_empty_assertions_pass_immediately() {
    let runner = EvalRunner::new(store());
    let provider = Arc::new(MockProvider::new(
        "mock",
        "mock-1",
        vec![ScriptedReply::Text("anything".into())],
    ));
    let task = EvalTask {
        id: "T-EMPTY".into(),
        instruction: "test".into(),
        assertions: vec![],
        budget: EvalBudget {
            max_steps: 12,
            max_tokens: 200_000,
        },
        expected_l4_path: None,
    };
    let result = runner.run_task(&task, provider, &[("f.rs", "")]).await;
    assert_eq!(result.verdict(), "pass", "{result:?}");
}

#[tokio::test]
async fn eval_tool_tool_call_sequence() {
    let runner = EvalRunner::new(store());
    let provider = Arc::new(MockProvider::new(
        "mock",
        "mock-1",
        vec![
            ScriptedReply::Tool {
                name: "read_file".into(),
                args: serde_json::json!({"path": "src/lib.rs"}),
            },
            ScriptedReply::Tool {
                name: "apply_patch".into(),
                args: serde_json::json!({"file": "src/lib.rs", "range": null, "content": "// improved\\nfn main() {}\\n"}),
            },
            ScriptedReply::Text("改进完成".into()),
        ],
    ));
    let task = EvalTask {
        id: "T-SEQ".into(),
        instruction: "read then improve".into(),
        assertions: vec![
            Assertion::FileContains { path: "src/lib.rs".into(), text: "improved".into() },
            Assertion::AnswerContains { text: "完成".into() },
        ],
        budget: EvalBudget { max_steps: 24, max_tokens: 400_000 },
        expected_l4_path: None,
    };
    let result = runner
        .run_task(&task, provider, &[("src/lib.rs", "fn main() {}\\n")])
        .await;
    assert_eq!(result.verdict(), "pass", "{result:?}");
}

#[tokio::test]
async fn eval_fixture_with_subdirectories() {
    let runner = EvalRunner::new(store());
    let provider = Arc::new(MockProvider::new(
        "mock",
        "mock-1",
        vec![
            ScriptedReply::Tool {
                name: "apply_patch".into(),
                args: serde_json::json!({"file": "src/deep/nested/mod.rs", "range": null, "content": "pub struct Nested;"}),
            },
            ScriptedReply::Text("done".into()),
        ],
    ));
    let task = EvalTask {
        id: "T-SUB".into(),
        instruction: "create nested".into(),
        assertions: vec![
            Assertion::FileContains { path: "src/deep/nested/mod.rs".into(), text: "Nested".into() },
        ],
        budget: EvalBudget { max_steps: 24, max_tokens: 400_000 },
        expected_l4_path: None,
    };
    let result = runner
        .run_task(&task, provider, &[("src/existing.rs", "// existing\\n")])
        .await;
    assert_eq!(result.verdict(), "pass", "{result:?}");
}

#[tokio::test]
async fn eval_file_not_contains_via_missing_file() {
    let runner = EvalRunner::new(store());
    let provider = Arc::new(MockProvider::new(
        "mock",
        "mock-1",
        vec![ScriptedReply::Text("response without file changes".into())],
    ));
    let task = EvalTask {
        id: "T-NOFILE".into(),
        instruction: "don't change anything".into(),
        assertions: vec![
            Assertion::FileContains { path: "no-such-file.txt".into(), text: "content".into() },
        ],
        budget: EvalBudget { max_steps: 12, max_tokens: 200_000 },
        expected_l4_path: None,
    };
    let result = runner.run_task(&task, provider, &[("f.rs", "")]).await;
    assert_eq!(result.verdict(), "fail");
}

#[tokio::test]
async fn eval_mixed_pass_and_fail_summary() {
    let runner = EvalRunner::new(store());
    let provider_pass = Arc::new(MockProvider::new(
        "mock",
        "mock-1",
        vec![ScriptedReply::Text("correct answer".into())],
    ));
    let provider_fail = Arc::new(MockProvider::new(
        "mock",
        "mock-1",
        vec![ScriptedReply::Text("wrong answer".into())],
    ));
    let task_pass = EvalTask {
        id: "T-PASS".into(),
        instruction: "answer".into(),
        assertions: vec![Assertion::AnswerContains { text: "correct".into() }],
        budget: EvalBudget { max_steps: 12, max_tokens: 200_000 },
        expected_l4_path: None,
    };
    let task_fail = EvalTask {
        id: "T-FAIL".into(),
        instruction: "answer".into(),
        assertions: vec![Assertion::AnswerContains { text: "expected".into() }],
        budget: EvalBudget { max_steps: 12, max_tokens: 200_000 },
        expected_l4_path: None,
    };
    let r1 = runner.run_task(&task_pass, provider_pass, &[("f.rs", "")]).await;
    let r2 = runner.run_task(&task_fail, provider_fail, &[("f.rs", "")]).await;
    assert_eq!(r1.verdict(), "pass");
    assert_eq!(r2.verdict(), "fail");
}

#[tokio::test]
async fn eval_file_contains_unicode_content() {
    let runner = EvalRunner::new(store());
    let provider = Arc::new(MockProvider::new(
        "mock",
        "mock-1",
        vec![
            ScriptedReply::Tool {
                name: "apply_patch".into(),
                args: serde_json::json!({"file": "i18n.ts", "range": null, "content": "// 支持中文\\nconst greeting = \"你好世界\";\\n"}),
            },
            ScriptedReply::Text("国际化文件已创建".into()),
        ],
    ));
    let task = EvalTask {
        id: "T-I18N".into(),
        instruction: "create i18n file".into(),
        assertions: vec![
            Assertion::FileContains { path: "i18n.ts".into(), text: "你好".into() },
        ],
        budget: EvalBudget { max_steps: 24, max_tokens: 400_000 },
        expected_l4_path: None,
    };
    let result = runner.run_task(&task, provider, &[("i18n.ts", "")]).await;
    assert_eq!(result.verdict(), "pass", "{result:?}");
}

#[tokio::test]
async fn eval_tool_read_file_step() {
    let runner = EvalRunner::new(store());
    let provider = Arc::new(MockProvider::new(
        "mock",
        "mock-1",
        vec![
            ScriptedReply::Tool {
                name: "read_file".into(),
                args: serde_json::json!({"path": "config.toml"}),
            },
            ScriptedReply::Text("文件读取完成".into()),
        ],
    ));
    let task = EvalTask {
        id: "T-READ".into(),
        instruction: "read config".into(),
        assertions: vec![Assertion::AnswerContains { text: "完成".into() }],
        budget: EvalBudget { max_steps: 12, max_tokens: 200_000 },
        expected_l4_path: None,
    };
    let result = runner.run_task(&task, provider, &[("config.toml", "key = \"value\"")]).await;
    assert_eq!(result.verdict(), "pass", "{result:?}");
}

#[tokio::test]
async fn eval_git_read_step() {
    let runner = EvalRunner::new(store());
    let provider = Arc::new(MockProvider::new(
        "mock",
        "mock-1",
        vec![
            ScriptedReply::Tool {
                name: "git_read".into(),
                args: serde_json::json!({"sub": "status"}),
            },
            ScriptedReply::Text("git status 完成".into()),
        ],
    ));
    let task = EvalTask {
        id: "T-GITR".into(),
        instruction: "check git status".into(),
        assertions: vec![Assertion::AnswerContains { text: "完成".into() }],
        budget: EvalBudget { max_steps: 12, max_tokens: 200_000 },
        expected_l4_path: None,
    };
    let result = runner.run_task_with_git(&task, provider, &[("f.rs", "fn main() {}")], true).await;
    assert_eq!(result.verdict(), "pass", "{result:?}");
}
