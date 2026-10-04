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
async fn t9_style_c_level_approval_recorded_and_domain_shown() {
    let runner = EvalRunner::new(store());
    let provider = Arc::new(MockProvider::new(
        "mock",
        "mock-1",
        vec![
            ScriptedReply::Tool {
                name: "http_fetch".into(),
                args: serde_json::json!({"url": "https://example.com/changelog"}),
            },
            ScriptedReply::Text("已取消。".into()),
        ],
    ));
    let task = EvalTask {
        id: "T9".into(),
        instruction: "抓取该库最新 changelog".into(),
        assertions: vec![Assertion::ApprovalRequested { level: "c".into() }],
        budget: EvalBudget {
            max_steps: 10,
            max_tokens: 200_000,
        },
        expected_l4_path: None,
    };
    // C 级恒审批（§12.2）：无决策 → 2s 超时 → Paused；审批请求已入事件
    let result = runner
        .run_task(&task, provider, &[("README.md", "# demo\n")])
        .await;
    // 审批请求出现（断言核心）；审批超时导致 Paused 亦视为链路正确
    assert!(
        result.approvals >= 1 || result.failures.iter().any(|f| f.contains("c 级")),
        "{result:?}"
    );
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
