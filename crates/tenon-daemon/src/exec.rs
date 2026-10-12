//! headless 一次性执行（§6.2 v1.181）：`tenon-daemon --exec "任务"` 非交互
//! 跑完即退——不取单实例锁、不起 HTTP；复用全量会话链路（工具目录 / 降级
//! 隔离 / 防护 / 证据链），事件以人类可读行打 stdout（日志走 stderr）。
//! 会话与事件照常落库——脚本任务在 UI 会话列表可见、可审计、可回滚。

use std::sync::Arc;

use tenon_agent::session::TaskOutcome;
use tenon_store::EventKind;

use crate::state::DaemonState;

/// 一次 exec 的结果（main 据此决定进程退出码）。
pub struct ExecOutcome {
    pub session_id: String,
    pub status: &'static str,
    pub answer: Option<String>,
    pub exit_code: i32,
}

/// 以一次性模式运行任务：登记项目 → 建会话（全量链路）→ 流式打印事件 →
/// 跑到终态。`print` 回调承载 stdout 行（main 侧决定格式与刷写）。
pub async fn run_exec(
    state: Arc<DaemonState>,
    project_path: &str,
    provider_name: &str,
    task: &str,
    mut print: impl FnMut(String),
) -> anyhow::Result<ExecOutcome> {
    let canonical = std::fs::canonicalize(project_path)
        .map_err(|e| anyhow::anyhow!("无法打开项目路径 {project_path}: {e}"))?;
    let project = {
        let mut store = state.store.lock().await;
        store.upsert_project(&canonical.to_string_lossy())?
    };
    let provider = state
        .provider_or_default(provider_name)
        .await
        .map_err(|e| anyhow::anyhow!(e))?;
    let session = crate::routes::create_agent_session(&state, &project, provider, None, None, None)
        .await
        .map_err(|(_, e)| anyhow::anyhow!(e))?;
    let session_id = session.session_id.clone();

    // 订阅先于启动：任务事件不丢首帧。任务完成即收尾（session 持有发送端
    // 不 drop，recv 不会自然关流——用 select 竞赛 + try_recv 排空残余事件）。
    let mut rx = session.subscribe();
    let mut runner = Box::pin(tokio::spawn({
        let session = session.clone();
        let task = task.to_string();
        async move { session.run_task(&task).await }
    }));
    let mut joined: Option<std::result::Result<TaskOutcome, tokio::task::JoinError>> = None;
    while joined.is_none() {
        tokio::select! {
            ev = rx.recv() => {
                if let Ok(ev) = ev {
                    if let Some(line) = format_event(&ev.kind, &ev.payload) {
                        print(line);
                    }
                }
            }
            res = &mut runner => {
                joined = Some(res);
            }
        }
    }
    while let Ok(ev) = rx.try_recv() {
        if let Some(line) = format_event(&ev.kind, &ev.payload) {
            print(line);
        }
    }
    let outcome = joined
        .unwrap()
        .map_err(|e| anyhow::anyhow!("任务线程失败: {e}"))?;

    let (status, answer, exit_code) = match &outcome {
        TaskOutcome::Done(card) => ("done", Some(card.answer.clone()), 0),
        TaskOutcome::Paused { reason, .. } => {
            print(format!("── 暂停：{reason}（计划待批准 / 熔断 / 用户暂停）"));
            ("paused", None, 1)
        }
        TaskOutcome::Error(e) => {
            print(format!("── 失败：{e}"));
            ("error", None, 1)
        }
    };
    if let TaskOutcome::Done(card) = &outcome {
        print("── 完成 ──".to_string());
        print(card.answer.clone());
    }
    Ok(ExecOutcome {
        session_id,
        status,
        answer,
        exit_code,
    })
}

/// 事件 → 人类可读行（None = 不打印的内部事件：sensing / model_delta / 事件链簿记）。
fn format_event(kind: &EventKind, payload: &serde_json::Value) -> Option<String> {
    Some(match kind {
        EventKind::Decision => format!("▸ {}", payload["intent"].as_str().unwrap_or("")),
        EventKind::CommandRun => format!("● {}", payload["tool"].as_str().unwrap_or("")),
        EventKind::PatchApplied => format!("✎ {}", payload["tool"].as_str().unwrap_or("")),
        EventKind::DirectAction => format!("⚡ {}", payload["tool"].as_str().unwrap_or("")),
        EventKind::ModelRetry => {
            format!("↻ 自动重试（{}）", payload["error"].as_str().unwrap_or(""))
        }
        EventKind::ModelFallback => {
            format!("⇄ 切换模型 → {}", payload["to"].as_str().unwrap_or(""))
        }
        EventKind::PlanSubmitted => format!(
            "☰ 计划已提交（{} 项），等待批准",
            payload["items"].as_array().map(|a| a.len()).unwrap_or(0)
        ),
        EventKind::HookRun => format!(
            "⚙ hook {} {}",
            payload["event"].as_str().unwrap_or(""),
            payload["action"].as_str().unwrap_or("")
        ),
        EventKind::Error => format!("✗ {}", payload["error"].as_str().unwrap_or("")),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DaemonOptions, DaemonState};
    use tenon_models::{MockProvider, ScriptedReply};

    /// 注入 mock provider 的内存态 daemon + 临时项目目录。
    async fn state_with_mock(script: Vec<ScriptedReply>) -> (Arc<DaemonState>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let mut options = DaemonOptions::in_memory();
        options.project = Some(dir.path().to_str().unwrap().to_string());
        options.providers = vec![Arc::new(MockProvider::new("mock", "mock-1", script))];
        options.default_provider = "mock".into();
        let state = Arc::new(DaemonState::new(options).await);
        (state, dir)
    }

    /// 成功路径：答案输出 + 退出码 0 + 事件行可读。
    #[tokio::test]
    async fn exec_runs_task_to_done() {
        let (state, dir) =
            state_with_mock(vec![ScriptedReply::Text("一次性执行的回答".into())]).await;
        let lines = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = lines.clone();
        let outcome = run_exec(
            state,
            dir.path().to_str().unwrap(),
            "mock",
            "解释这段代码",
            move |line| sink.lock().unwrap().push(line),
        )
        .await
        .unwrap();
        assert_eq!(outcome.exit_code, 0);
        assert_eq!(outcome.status, "done");
        assert_eq!(outcome.answer.as_deref(), Some("一次性执行的回答"));
        let lines = lines.lock().unwrap();
        assert!(
            lines.iter().any(|l| l.contains("一次性执行的回答")),
            "答案行输出: {lines:?}"
        );
    }

    /// 失败路径：非瞬时模型错误 → 退出码 1（无 fallback 链配置）。
    #[tokio::test]
    async fn exec_reports_failure_with_nonzero_exit() {
        let (state, dir) =
            state_with_mock(vec![ScriptedReply::NonTransient("bad key".into())]).await;
        let outcome = run_exec(state, dir.path().to_str().unwrap(), "mock", "任务", |_| {})
            .await
            .unwrap();
        assert_eq!(outcome.exit_code, 1);
        assert_eq!(outcome.status, "error");
        assert!(outcome.answer.is_none());
    }
}
