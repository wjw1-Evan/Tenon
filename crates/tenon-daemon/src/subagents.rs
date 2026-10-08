//! daemon 侧子代理编排器（§9.5 v1.190）：worktree 池 + 子会话登记 + 批内并发。
//!
//! 语义：子代理运行于独立受管 worktree（§9.7 机制同款），完成后**保留
//! worktree**——合并 / 丢弃由用户在子会话行处置（复用 /session/{id}/worktree/
//! merge|discard 既有收尾端点，子会话已登记进 state.sessions 因此可达）。
//! 与 v1.187 前的 run_parallel 不同：批末不清理 worktree（清了子改动即丢）。
//! 递归护栏：子会话不再注入编排器（depth=0 才注入），spawn 深度恒为 1。

use std::path::PathBuf;
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
                results.push(SubagentResult {
                    task_id,
                    session_id,
                    status: status.into(),
                    answer,
                    worktree: wt_path,
                });
            }
        }
        Ok(results)
    }
}
