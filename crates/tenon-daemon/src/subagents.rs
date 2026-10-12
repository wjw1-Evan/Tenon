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

use crate::state::{DaemonState, SessionEntry};

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

impl DaemonSubagents {
    /// v2.24 ACP 轨单任务执行（design-v2.md §8①）。
    async fn run_acp_task(
        &self,
        pool: &WorktreePool,
        task: &SubTask,
    ) -> tenon_agent::subagents::SubagentResult {
        let mut registered_session: Option<String> = None;
        let err_result = |answer: String| tenon_agent::subagents::SubagentResult {
            task_id: task.id.clone(),
            session_id: String::new(),
            status: "error".into(),
            answer,
            worktree: String::new(),
            verification: "任务未执行，跳过验收".into(),
        };
        // settings 取首个配置 agent
        let agent_cfg = {
            let ov = self.state.settings_overrides.lock().unwrap().clone();
            let picked = ov.acp_agents.unwrap_or_default().into_iter().next();
            match picked {
                Some(a) => a,
                None => {
                    return err_result("ACP 轨不可用：settings acp.agents 未配置任何 agent".into())
                }
            }
        };
        // 受管 worktree（与 in-process 轨同池）
        let wt = match pool.create(std::path::Path::new(&self.project.path), &task.id) {
            Ok(wt) => wt,
            Err(e) => return err_result(format!("受管 worktree 创建失败: {e}")),
        };
        // v2.26 子会话行登记（收口 v2.24 余项）：登记型 Tenon 会话——合并 /
        // 丢弃端点与全局活动视图对 ACP 轨子任务可达；标题 [ACP] 前缀可辨识。
        let child_sid = uuid::Uuid::now_v7().to_string();
        let registered = crate::routes::create_agent_session(
            &self.state,
            &self.project,
            self.provider.clone(),
            None,
            Some(child_sid.clone()),
            Some(wt.clone()),
        )
        .await;
        match registered {
            Ok(child) => {
                let mut sessions = self.state.sessions.lock().await;
                sessions.insert(
                    child.session_id.clone(),
                    crate::state::SessionEntry {
                        session: child.clone(),
                        project_root: PathBuf::from(&self.project.path),
                        project_id: self.project.id.clone(),
                        managed_worktree: Some(wt.clone()),
                        last_outcome: Mutex::new(None),
                        busy: AtomicBool::new(false),
                        last_seq: 0,
                    },
                );
                drop(sessions);
                // 登记型会话标题与终态回写（不跑任务——执行在外部 ACP agent）
                let title = format!(
                    "[ACP] {}",
                    task.instruction.chars().take(40).collect::<String>()
                );
                {
                    let mut st = self.state.store.lock().await;
                    let _ = st.set_session_title(&child.session_id, &title);
                }
                registered_session = Some(child.session_id.clone());
            }
            Err((_, message)) => {
                // 登记失败不阻断执行——worktree 路径仍随结果回传（降级语义）
                let _ = message;
            }
        }
        // 沙箱包装外部 agent 进程（写根 = worktree；网络 offline）
        let spec = tenon_sandbox::external::ExternalSpec {
            write_roots: vec![wt.clone()],
            network: tenon_sandbox::external::ExternalNetwork::Offline,
        };
        let args: Vec<&str> = agent_cfg.args.iter().map(String::as_str).collect();
        let spawned = match tenon_sandbox::external::spawn_external(
            &agent_cfg.command,
            &args,
            &wt,
            &spec,
            &[],
        ) {
            Ok(sp) => sp,
            Err(e) => {
                let _ = pool.remove(std::path::Path::new(&self.project.path), &task.id);
                return err_result(format!("ACP agent 进程启动失败: {e}"));
            }
        };
        // 协议链（阻塞 IO 在 spawn_blocking——stdin/stdout 是同步管道）
        let instruction = task.instruction.clone();
        let wt_path = wt.clone();
        let proto = tokio::task::spawn_blocking(move || {
            let mut agent = match tenon_acp::AcpAgent::attach(spawned.child) {
                Ok(a) => a,
                Err(e) => return Err(format!("ACP attach 失败: {e}")),
            };
            let run = (|| -> Result<(String, String), String> {
                agent.initialize().map_err(|e| format!("initialize: {e}"))?;
                let sid = agent
                    .new_session(&wt_path)
                    .map_err(|e| format!("session/new: {e}"))?;
                let result = agent
                    .prompt(&sid, &instruction)
                    .map_err(|e| format!("prompt: {e}"))?;
                Ok((result.answer, result.stop_reason))
            })();
            agent.shutdown();
            run
        })
        .await;
        let answer = match proto {
            Ok(Ok((answer, stop_reason))) => format!("{answer}（stopReason: {stop_reason}）"),
            Ok(Err(e)) => {
                let _ = pool.remove(std::path::Path::new(&self.project.path), &task.id);
                return err_result(format!("ACP 协议失败: {e}"));
            }
            Err(e) => {
                let _ = pool.remove(std::path::Path::new(&self.project.path), &task.id);
                return err_result(format!("ACP 任务 join 失败: {e}"));
            }
        };
        // 验收同规（v1.211：Done 任务在 worktree 内沙箱跑测试）
        let verification = verify_child_worktree(&wt);
        // 登记型会话终态回写（done）——全局活动视图呈现
        if let Some(sid) = &registered_session {
            let mut st = self.state.store.lock().await;
            let _ = st.set_session_status(sid, tenon_store::SessionStatus::Done);
        }
        tenon_agent::subagents::SubagentResult {
            task_id: task.id.clone(),
            session_id: registered_session.unwrap_or_default(),
            status: "done".into(),
            answer,
            worktree: wt.to_string_lossy().into_owned(),
            verification,
        }
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
                provider: t.provider.clone(),
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

        // v2.24 ACP 提供方轨（design-v2.md §8①）：provider=acp 的任务在受管
        // worktree 内由外部 ACP agent 执行——进程经 sandbox external 包装（写根
        // = worktree + 缓存目录、网络 offline：外部 agent 全部工具执行受进程级
        // 沙箱约束）；协议链 initialize → session/new → prompt；验收同规；无
        // Tenon 子会话（session_id 空串，合并经 worktree 路径——登记形态列后续）。
        for task in &subtasks {
            if task.provider.as_deref() != Some("acp") {
                continue;
            }
            let pool_acp = WorktreePool::new(self.state.worktrees_root.join(&self.project.id));
            let result = self.run_acp_task(&pool_acp, task).await;
            results.push(result);
        }

        let in_process: Vec<SubTask> = subtasks
            .iter()
            .filter(|t| t.provider.as_deref() != Some("acp"))
            .cloned()
            .collect();
        // §9.5：不相交调度重算（仅 in-process 集）
        let schedule = tenon_agent::subagents::plan_parallel(&in_process, 3);

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
