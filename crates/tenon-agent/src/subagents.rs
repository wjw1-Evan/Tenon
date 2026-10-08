//! 并行子代理（设计方案 §9.5）：
//! - **worktree 隔离**：每个子代理独立 git worktree（内核托管于
//!   `~/.tenon/worktrees/<task-id>/`，§9.5；不触碰用户工作区）；
//! - **文件集不相交调度**：静态检查任务文件集——相交 → 拒绝 / 转串行；
//! - **并发 ≤3 × 单代理 token 预算硬上限**；冲突 = 失败回传用户处置；
//!   （逐条列明、一次批准）。

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::session::TaskOutcome;

#[derive(Debug, Error)]
pub enum SubAgentError {
    #[error("文件集相交（§9.5）：{a} ∩ {b}")]
    FileSetsIntersect { a: String, b: String },
    #[error("worktree 操作失败: {0}")]
    Worktree(String),
    #[error("目标不是 git 仓库（worktree 隔离要求 git）")]
    NotGitRepo,
}

pub type Result<T> = std::result::Result<T, SubAgentError>;

/// 子代理任务定义。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubTask {
    pub id: String,
    pub instruction: String,
    /// 计划触碰的文件集（不相交调度的静态依据）
    pub files: Vec<String>,
}

/// 调度计划：不相交的分组并行（每批 ≤ max_concurrency），相交者拒绝。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Schedule {
    /// 并行批次（批内文件集两两不相交）
    pub batches: Vec<Vec<SubTask>>,
    /// 被拒绝的任务（与已调度文件集相交，§9.5：拒绝 / 转串行）
    pub rejected: Vec<(String, String)>,
}

/// 文件集不相交检查。
pub fn disjoint(a: &[String], b: &[String]) -> bool {
    a.iter().all(|f| !b.contains(f))
}

/// 静态调度：贪心按序分组，文件集与同批任一任务相交即拒绝（§9.5）。
pub fn plan_parallel(tasks: &[SubTask], max_concurrency: usize) -> Schedule {
    let max = max_concurrency.clamp(1, 3); // §9.5：并发 ≤3
    let mut schedule = Schedule::default();
    let mut current: Vec<SubTask> = Vec::new();
    let mut current_files: Vec<String> = Vec::new();

    for task in tasks {
        // 与当前批内任务比较
        if let Some(clash) = current.iter().find(|c| !disjoint(&c.files, &task.files)) {
            schedule
                .rejected
                .push((task.id.clone(), format!("与 {} 文件集相交", clash.id)));
            continue;
        }
        // 与已定批（更早批次）也要求不相交——跨批写同文件仍有竞态
        if schedule
            .batches
            .iter()
            .flatten()
            .any(|c: &SubTask| !disjoint(&c.files, &task.files))
        {
            schedule
                .rejected
                .push((task.id.clone(), "与已完成调度的任务文件集相交".into()));
            continue;
        }
        if current.len() >= max {
            schedule.batches.push(std::mem::take(&mut current));
            current_files.clear();
        }
        current_files.extend(task.files.iter().cloned());
        current.push(task.clone());
    }
    if !current.is_empty() {
        schedule.batches.push(current);
    }
    schedule
}

/// 内核托管 worktree 池（§9.5：`~/.tenon/worktrees/<task-id>/`）。
pub struct WorktreePool {
    root: PathBuf,
}

impl WorktreePool {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// 为子任务创建独立 worktree（基于当前 HEAD）。
    pub fn create(&self, repo_root: &Path, task_id: &str) -> Result<PathBuf> {
        if !repo_root.join(".git").exists() {
            return Err(SubAgentError::NotGitRepo);
        }
        std::fs::create_dir_all(&self.root).map_err(|e| SubAgentError::Worktree(e.to_string()))?;
        let wt = self.root.join(sanitize(task_id));
        if wt.exists() {
            // 复用已存在的 worktree（幂等）
            return Ok(wt);
        }
        let out = Command::new("git")
            .arg("-C")
            .arg(repo_root)
            .args(["worktree", "add"])
            .arg(&wt)
            .arg("HEAD")
            .output()
            .map_err(|e| SubAgentError::Worktree(e.to_string()))?;
        if !out.status.success() {
            return Err(SubAgentError::Worktree(
                String::from_utf8_lossy(&out.stderr).into_owned(),
            ));
        }
        Ok(wt)
    }

    /// 任务结束后清理 worktree（工作区文件已由用户处置：合并/丢弃决策在主代理侧）。
    pub fn remove(&self, repo_root: &Path, task_id: &str) -> Result<()> {
        let wt = self.root.join(sanitize(task_id));
        let out = Command::new("git")
            .arg("-C")
            .arg(repo_root)
            .args(["worktree", "remove", "--force"])
            .arg(&wt)
            .output()
            .map_err(|e| SubAgentError::Worktree(e.to_string()))?;
        if !out.status.success() {
            // 目录不存在 → 幂等成功
            if wt.exists() {
                return Err(SubAgentError::Worktree(
                    String::from_utf8_lossy(&out.stderr).into_owned(),
                ));
            }
        }
        Ok(())
    }
}

/// v1.190 §9.5：子代理任务入参（模型经 spawn_subagents 工具提交）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentSpawn {
    pub instruction: String,
    /// 计划触碰的文件集（不相交调度依据；§9.5）
    pub files: Vec<String>,
}

/// v1.190 §9.5：单个子代理运行结果（子会话已登记，worktree 留待用户合并/丢弃）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentResult {
    pub task_id: String,
    pub session_id: String,
    /// done | paused | error
    pub status: String,
    /// 收尾回答摘要（截断由调用方定）
    pub answer: String,
    /// 受管 worktree 路径（合并 / 丢弃走子会话既有收尾端点）
    pub worktree: String,
}

/// v1.190 §9.5：子代理编排器——daemon 侧实现（worktree 池 + 子会话登记 +
/// 并发运行）；session 工具面经此委托，不直接触碰 daemon 状态。
#[async_trait::async_trait]
pub trait SubagentOrchestrator: Send + Sync {
    /// 运行一批子代理：不相交调度（≤3 并发）、独立受管 worktree、
    /// 子会话登记并可合并；实现不得在返回前清理 worktree。
    async fn run_batch(
        &self,
        parent_session_id: &str,
        tasks: Vec<SubagentSpawn>,
    ) -> std::result::Result<Vec<SubagentResult>, String>;
}

/// 批量任务提交摘要：逐条列明，用于共享日志展示。
pub fn composite_commit_summary(messages: &[String]) -> String {
    // 计数只替换头部占位符：全局 replace 会把 commit message 里的「N」一并改写
    let mut s = format!("批量提交（直执审计，共 {} 个 commit）：", messages.len());
    for (i, m) in messages.iter().enumerate() {
        s.push_str(&format!("\n  {}. {m}", i + 1));
    }
    s
}

/// 并行运行结果。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ParallelOutcome {
    /// 成功完成的子任务（id → worktree 中改动文件集由调用方经 diff 获取）
    pub completed: Vec<String>,
    /// 失败（冲突 / 任务失败）：回传用户处置（§9.5）
    pub failed: Vec<(String, String)>,
}

/// 并行执行：按调度批次串行、批内并发（§9.5）。
/// `spawn_task` 由调用方提供（为子任务建独立 AgentSession，project_root =
/// worktree 路径）；worktree 由本函数创建并在批次结束后清理。
pub async fn run_parallel<F, Fut>(
    schedule: &Schedule,
    pool: &WorktreePool,
    repo_root: &Path,
    mut spawn_task: F,
) -> Result<ParallelOutcome>
where
    F: FnMut(&SubTask, &Path) -> Fut,
    Fut: std::future::Future<Output = TaskOutcome> + Send + 'static,
{
    let mut outcome = ParallelOutcome::default();
    for batch in &schedule.batches {
        // 创建 worktree
        let mut worktrees: Vec<(SubTask, PathBuf)> = Vec::new();
        for task in batch {
            let wt = pool.create(repo_root, &task.id)?;
            worktrees.push((task.clone(), wt));
        }
        // 批内并发（owned 数据进 spawned 任务）
        let mut handles = Vec::new();
        for (task, wt) in &worktrees {
            let fut = spawn_task(task, wt);
            let id = task.id.clone();
            handles.push(tokio::spawn(async move { (id, fut.await) }));
        }
        for handle in handles {
            let (id, result) = handle
                .await
                .map_err(|e| SubAgentError::Worktree(e.to_string()))?;
            match result {
                TaskOutcome::Done(_) => outcome.completed.push(id),
                TaskOutcome::Paused { reason, .. } => {
                    outcome.failed.push((id, format!("暂停: {reason}")))
                }
                TaskOutcome::Error(e) => outcome.failed.push((id, e)),
            }
        }
        // 批次结束清理 worktree
        for (task, _) in &worktrees {
            pool.remove(repo_root, &task.id)?;
        }
    }
    Ok(outcome)
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: &str, files: &[&str]) -> SubTask {
        SubTask {
            id: id.into(),
            instruction: format!("task {id}"),
            files: files.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn disjoint_tasks_batch_parallel() {
        let tasks = vec![
            task("t1", &["a.rs"]),
            task("t2", &["b.rs"]),
            task("t3", &["c.rs"]),
            task("t4", &["d.rs"]),
        ];
        let schedule = plan_parallel(&tasks, 3);
        // 并发 ≤3：批次 [t1,t2,t3] + [t4]
        assert_eq!(schedule.batches.len(), 2);
        assert_eq!(schedule.batches[0].len(), 3);
        assert_eq!(schedule.batches[1].len(), 1);
        assert!(schedule.rejected.is_empty());
    }

    #[test]
    fn intersecting_task_rejected() {
        let tasks = vec![
            task("t1", &["a.rs", "shared.rs"]),
            task("t2", &["shared.rs"]),
        ];
        let schedule = plan_parallel(&tasks, 3);
        assert_eq!(schedule.batches.len(), 1);
        assert_eq!(schedule.batches[0][0].id, "t1");
        assert_eq!(
            schedule.rejected,
            vec![("t2".into(), "与 t1 文件集相交".into())]
        );
    }

    #[test]
    fn cross_batch_file_conflict_rejected() {
        let tasks = vec![
            task("t1", &["a.rs"]),
            task("t2", &["b.rs"]),
            task("t3", &["c.rs"]),
            task("t4", &["a.rs"]), // 与第一批 t1 相交
        ];
        let schedule = plan_parallel(&tasks, 3);
        assert_eq!(schedule.rejected.len(), 1);
        assert_eq!(schedule.rejected[0].0, "t4");
    }

    #[test]
    fn composite_summary_lists_all() {
        let s = composite_commit_summary(&["fix a".into(), "fix b".into(), "fix c".into()]);
        assert!(s.contains("共 3 个 commit"));
        assert!(s.contains("1. fix a"));
        assert!(s.contains("3. fix c"));
    }

    #[test]
    fn worktree_requires_git_repo() {
        let dir = tempfile::tempdir().unwrap();
        let pool = WorktreePool::new(dir.path().join("pool"));
        let err = pool.create(dir.path(), "t1").unwrap_err();
        assert!(matches!(err, SubAgentError::NotGitRepo));
    }

    #[test]
    fn worktree_create_and_remove_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        for args in [
            vec!["init", "-q", "."],
            vec!["config", "user.email", "t@t"],
            vec!["config", "user.name", "t"],
        ] {
            std::process::Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(&args)
                .output()
                .unwrap();
        }
        std::fs::write(repo.join("a.txt"), "v1\n").unwrap();
        for args in [vec!["add", "-A"], vec!["commit", "-qm", "init"]] {
            std::process::Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(&args)
                .output()
                .unwrap();
        }
        let pool = WorktreePool::new(dir.path().join("pool"));
        let wt = pool.create(&repo, "task-1").unwrap();
        assert!(wt.join("a.txt").exists(), "worktree 基于当前 HEAD");
        assert!(wt.join(".git").exists());
        // 幂等复用
        let wt2 = pool.create(&repo, "task-1").unwrap();
        assert_eq!(wt, wt2);
        pool.remove(&repo, "task-1").unwrap();
        assert!(!wt.exists());
    }
}
