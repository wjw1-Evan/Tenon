//! daemon 侧子代理编排器（§9.5 v1.190）：worktree 池 + 子会话登记 + 批内并发。
//!
//! 语义：子代理运行于独立受管 worktree（§9.7 机制同款），完成后**保留
//! worktree**——合并 / 丢弃由用户在子会话行处置（复用 /session/{id}/worktree/
//! merge|discard 既有收尾端点，子会话已登记进 state.sessions 因此可达）。
//! 与 v1.187 前的 run_parallel 不同：批末不清理 worktree（清了子改动即丢）。
//! 递归护栏：子会话不再注入编排器（depth=0 才注入），spawn 深度恒为 1。

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use tokio::sync::Mutex;

use tenon_agent::subagents::{
    SubTask, SubagentOrchestrator, SubagentResult, SubagentSpawn, WorktreePool,
};
use tenon_models::ModelProvider;

use crate::state::{DaemonState, MessageQueue, SessionEntry};

pub struct DaemonSubagents {
    pub state: Arc<DaemonState>,
    pub project: tenon_store::Project,
    pub provider: Arc<dyn ModelProvider>,
}

/// 生成子任务 id（sanitize 安全：hex 短串）。
fn child_task_id(parent_session_id: &str, index: usize) -> String {
    let tail: String = parent_session_id
        .chars()
        .rev()
        .take(6)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("sub-{tail}-{index}")
}

/// §9.5 v1.211：子 worktree 验收——探测测试命令并在沙箱内运行
/// （Offline 断网，120s 超时；与 §9.4 验证同规）。返回证据文本。
fn verify_child_worktree(wt: &Path) -> String {
    let Some(cmd) = tenon_agent::executor::detect_test_command(wt) else {
        return "无测试命令，未验收".to_string();
    };
    let spec = tenon_sandbox::SandboxSpec::Offline {
        project_root: wt.to_path_buf(),
    };
    match tenon_sandbox::exec_command(&cmd, wt, std::time::Duration::from_secs(120), &spec) {
        Ok(out) => {
            if out.success() {
                format!("tests exit=0 ({cmd})")
            } else {
                let tail: String = out.stdout.lines().last().unwrap_or("").to_string();
                format!(
                    "tests FAILED (exit={}): {tail}",
                    out.exit_code.unwrap_or(-1)
                )
            }
        }
        Err(e) => format!("测试运行失败: {e}"),
    }
}

#[async_trait::async_trait]
impl SubagentOrchestrator for DaemonSubagents {
    async fn run_batch(
        &self,
        parent_session_id: &str,
        tasks: Vec<SubagentSpawn>,
    ) -> Result<Vec<SubagentResult>, String> {
        let subtasks: Vec<SubTask> = tasks
            .iter()
            .enumerate()
            .map(|(i, t)| SubTask {
                id: child_task_id(parent_session_id, i),
                instruction: t.instruction.clone(),
                files: t.files.clone(),
            })
            .collect();
        // §9.5：不相交调度（批 ≤3 并发；相交任务拒绝并回传）
        let schedule = tenon_agent::subagents::plan_parallel(&subtasks, 3);
        let pool = WorktreePool::new(self.state.worktrees_root.join(&self.project.id));

        let mut results: Vec<SubagentResult> = Vec::new();
        for (id, reason) in &schedule.rejected {
            results.push(SubagentResult {
                task_id: id.clone(),
                session_id: String::new(),
                status: "rejected".into(),
                answer: reason.clone(),
                worktree: String::new(),
                verification: String::new(),
            });
        }

        for batch in &schedule.batches {
            // 建 worktree + 子会话 + 登记（合并 / 丢弃端点由此可达）
            let mut handles = Vec::new();
            for task in batch {
                let child_sid = uuid::Uuid::now_v7().to_string();
                let wt = match pool.create(std::path::Path::new(&self.project.path), &child_sid) {
                    Ok(wt) => wt,
                    Err(e) => {
                        results.push(SubagentResult {
                            task_id: task.id.clone(),
                            session_id: String::new(),
                            status: "error".into(),
                            answer: format!("受管 worktree 创建失败: {e}"),
                            worktree: String::new(),
                            verification: String::new(),
                        });
                        continue;
                    }
                };
                let child = match crate::routes::create_agent_session(
                    &self.state,
                    &self.project,
                    self.provider.clone(),
                    None,
                    Some(child_sid),
                    Some(wt.clone()),
                    // v2.0（design-v2.md §4.1）：子代理为无人值守派生会话，无交互面
                    // 确认——Approval 收窄 never（D 级动作由主会话执行更可控）
                    Some(tenon_core::gates::ApprovalGear::Never),
                )
                .await
                {
                    Ok(s) => s,
                    Err((_, message)) => {
                        // 子会话创建失败：清孤儿 worktree（与 routes 同款纪律）
                        let _ = pool.remove(std::path::Path::new(&self.project.path), &task.id);
                        results.push(SubagentResult {
                            task_id: task.id.clone(),
                            session_id: String::new(),
                            status: "error".into(),
                            answer: format!("子会话创建失败: {message}"),
                            worktree: String::new(),
                            verification: String::new(),
                        });
                        continue;
                    }
                };
                {
                    let mut sessions = self.state.sessions.lock().await;
                    sessions.insert(
                        child.session_id.clone(),
                        SessionEntry {
                            session: child.clone(),
                            project_root: PathBuf::from(&self.project.path),
                            project_id: self.project.id.clone(),
                            managed_worktree: Some(wt.clone()),
                            last_outcome: Mutex::new(None),
                            queue: Mutex::new(MessageQueue::default()),
                            busy: AtomicBool::new(false),
                            last_seq: 0,
                        },
                    );
                }
                let instruction = task.instruction.clone();
                let task_id = task.id.clone();
                let wt_path = wt.to_string_lossy().into_owned();
                handles.push(tokio::spawn(async move {
                    let outcome = child.run_task(&instruction).await;
                    (task_id, child.session_id.clone(), wt_path, outcome)
                }));
            }
            for handle in handles {
                let (task_id, session_id, wt_path, outcome) =
                    handle.await.map_err(|e| format!("子代理 join 失败: {e}"))?;
                let (status, answer) = match outcome {
                    tenon_agent::session::TaskOutcome::Done(card) => ("done", card.answer),
                    tenon_agent::session::TaskOutcome::Paused { reason, .. } => ("paused", reason),
                    tenon_agent::session::TaskOutcome::Error(e) => ("error", e),
                };
                // §9.5 v1.211 验收：Done 子任务在其 worktree 内沙箱跑测试，
                // 证据随结果回传主对话（失败 → 主对话模型可再分发修复）
                let verification = if status == "done" {
                    verify_child_worktree(Path::new(&wt_path))
                } else {
                    "任务未完成，跳过验收".to_string()
                };
                results.push(SubagentResult {
                    task_id,
                    session_id,
                    status: status.into(),
                    answer,
                    worktree: wt_path,
                    verification,
                });
            }
        }
        Ok(results)
    }
}
