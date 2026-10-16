//! 后台任务注册表（§9.2 v2.64，Claude Code run_in_background 对标）：
//! `run_command {background: true}` 启动的长命令（dev server / 长构建 /
//! 长测试）不阻塞回合——任务号、输出缓冲、状态由本注册表托管；`task_output`
//! 查询（可阻塞等待完成）、`task_kill` 终止。
//!
//! 生命周期：注册表挂 ToolContext（会话作用域）；stdout/stderr 读取线程与
//! waiter 线程随任务存活，`Drop` 兜底清杀全部存活子进程（会话结束无孤儿）。

use std::io::Read;
use std::process::Child;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tenon_sandbox::spawn_command_background;

/// 单任务输出缓冲上限（超出截留，读取面注明）。
const BUFFER_CAP: usize = 512 * 1024;
/// waiter 轮询周期。
const POLL_INTERVAL: Duration = Duration::from_millis(100);
/// 输出排空宽限（v2.81）：进程退出后 stdout/stderr 读取线程可能仍在排空管道，
/// 查询须等两路归零再取快照，否则丢尾部输出（CI 实测 stderr 尾行缺失）。
const DRAIN_GRACE: Duration = Duration::from_millis(500);

/// 任务终态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExitState {
    /// 进程退出码（被信号杀死时为 None）。
    pub code: Option<i32>,
    /// task_kill 终止。
    pub killed: bool,
    /// 运行超时被杀。
    pub timed_out: bool,
    pub finished: Instant,
}

/// 一个后台任务：共享给读取 / waiter 线程与查询面。
pub struct BgTask {
    pub id: u32,
    pub cmd: String,
    pub started: Instant,
    /// stdout+stderr 合并原始字节（cap BUFFER_CAP，超出截留）。
    buffer: Mutex<Vec<u8>>,
    truncated: AtomicBool,
    killed: AtomicBool,
    /// 尚在排空管道的读取线程数（stdout / stderr 各一，v2.81）。
    readers: AtomicU32,
    exit: Mutex<Option<ExitState>>,
    child: Mutex<Option<Child>>,
}

impl BgTask {
    /// 任务是否已结束（含被杀 / 超时）。
    pub fn is_done(&self) -> bool {
        self.exit.lock().expect("bg exit lock").is_some()
    }

    /// stdout / stderr 读取线程是否都已排空退出（v2.81）。
    fn readers_done(&self) -> bool {
        self.readers.load(Ordering::SeqCst) == 0
    }

    /// task_kill：已结束返回 None；否则发终止信号返回 true。
    fn kill(&self) -> Option<bool> {
        let mut guard = self.child.lock().expect("bg child lock");
        match guard.as_mut() {
            None => None, // 已被 waiter 收割
            Some(child) => {
                self.killed.store(true, Ordering::SeqCst);
                let _ = child.kill();
                Some(true)
            }
        }
    }
}

/// 会话作用域后台任务注册表。
#[derive(Default)]
pub struct BgRegistry {
    next_id: AtomicU32,
    tasks: Mutex<Vec<Arc<BgTask>>>,
}

impl BgRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 后台启动命令（execpolicy 判定由调用方前置）。返回任务号。
    pub fn spawn(
        &self,
        cmd: &str,
        cwd: &std::path::Path,
        timeout: Duration,
    ) -> Result<u32, String> {
        let mut child =
            spawn_command_background(cmd, cwd).map_err(|e| format!("后台启动失败: {e}"))?;
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        let id = self.next_id.fetch_add(1, Ordering::SeqCst) + 1;
        let task = Arc::new(BgTask {
            id,
            cmd: cmd.to_string(),
            started: Instant::now(),
            buffer: Mutex::new(Vec::new()),
            truncated: AtomicBool::new(false),
            killed: AtomicBool::new(false),
            readers: AtomicU32::new(2),
            exit: Mutex::new(None),
            child: Mutex::new(Some(child)),
        });

        // stdout / stderr 读取线程：合并写入共享缓冲（超上限截留）
        for stream in [Box::new(stdout) as Box<dyn Read + Send>, Box::new(stderr)] {
            let task = Arc::clone(&task);
            std::thread::spawn(move || {
                let mut stream = stream;
                let mut chunk = [0u8; 8192];
                loop {
                    match stream.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            let mut buf = task.buffer.lock().expect("bg buffer lock");
                            if buf.len() < BUFFER_CAP {
                                let remain = BUFFER_CAP - buf.len();
                                buf.extend_from_slice(&chunk[..n.min(remain)]);
                                if n > remain {
                                    task.truncated.store(true, Ordering::SeqCst);
                                }
                            } else {
                                task.truncated.store(true, Ordering::SeqCst);
                            }
                        }
                    }
                }
                // v2.81：排空完毕（EOF / 出错）才计数归零——查询端据此等齐输出
                task.readers.fetch_sub(1, Ordering::SeqCst);
            });
        }

        // waiter 线程：轮询退出 + 超时杀
        let waiter_task = Arc::clone(&task);
        std::thread::spawn(move || loop {
            {
                let mut guard = waiter_task.child.lock().expect("bg child lock");
                if let Some(child) = guard.as_mut() {
                    if let Ok(Some(status)) = child.try_wait() {
                        let killed = waiter_task.killed.load(Ordering::SeqCst);
                        *guard = None;
                        drop(guard);
                        *waiter_task.exit.lock().expect("bg exit lock") = Some(ExitState {
                            code: status.code(),
                            killed,
                            timed_out: false,
                            finished: Instant::now(),
                        });
                        break;
                    }
                } else {
                    break;
                }
            }
            if waiter_task.started.elapsed() >= timeout {
                let mut guard = waiter_task.child.lock().expect("bg child lock");
                if let Some(child) = guard.as_mut() {
                    let _ = child.kill();
                    let _ = child.wait();
                    *guard = None;
                    drop(guard);
                    *waiter_task.exit.lock().expect("bg exit lock") = Some(ExitState {
                        code: None,
                        killed: false,
                        timed_out: true,
                        finished: Instant::now(),
                    });
                }
                break;
            }
            std::thread::sleep(POLL_INTERVAL);
        });

        self.tasks.lock().expect("bg tasks lock").push(task);
        Ok(id)
    }

    /// 按任务号查询（阻塞等待完成至多 `wait`）。
    pub fn wait_output(&self, id: u32, wait: Duration) -> Option<TaskSnapshot> {
        let task = self.get(id)?;
        let wait_deadline = Instant::now() + wait;
        let drain_deadline = Instant::now() + DRAIN_GRACE;
        // v2.81：终态后仍等读取线程排空（宽限内），保证快照输出完整——
        // 此前 exit 置位即返回，管道未读尽时尾行丢失（CI 实测 stderr 尾行缺失）
        loop {
            if task.is_done() {
                if task.readers_done() || Instant::now() >= drain_deadline {
                    break;
                }
            } else if Instant::now() >= wait_deadline {
                break;
            }
            std::thread::sleep(POLL_INTERVAL);
        }
        let buffer =
            String::from_utf8_lossy(&task.buffer.lock().expect("bg buffer lock")).into_owned();
        let exit = *task.exit.lock().expect("bg exit lock");
        Some(TaskSnapshot {
            id: task.id,
            cmd: task.cmd.clone(),
            running: !task.is_done(),
            elapsed: task.started.elapsed(),
            buffer,
            truncated: task.truncated.load(Ordering::SeqCst),
            exit,
        })
    }

    /// 终止任务：Ok(()) = 已发送终止信号；Err = 未知任务号或任务已结束。
    pub fn kill(&self, id: u32) -> Result<(), String> {
        let task = self.get(id).ok_or_else(|| format!("未知后台任务 #{id}"))?;
        task.kill()
            .map(|_| ())
            .ok_or_else(|| format!("任务 #{id} 已结束"))
    }

    /// 全部任务摘要（v2.70 列表模式：跨任务找回后台任务的唯一口——
    /// 跨任务携带只带对话不带工具输出，任务号只在启动它的任务上下文里）。
    pub fn list(&self) -> Vec<TaskSnapshot> {
        self.tasks
            .lock()
            .expect("bg tasks lock")
            .iter()
            .map(|task| TaskSnapshot {
                id: task.id,
                cmd: task.cmd.clone(),
                running: !task.is_done(),
                elapsed: task.started.elapsed(),
                buffer: String::new(),
                truncated: false,
                exit: *task.exit.lock().expect("bg exit lock"),
            })
            .collect()
    }

    fn get(&self, id: u32) -> Option<Arc<BgTask>> {
        self.tasks
            .lock()
            .expect("bg tasks lock")
            .iter()
            .find(|t| t.id == id)
            .map(Arc::clone)
    }
}

impl Drop for BgRegistry {
    fn drop(&mut self) {
        // 会话结束兜底：清杀全部存活子进程（读取线程随进程退出自行收尾）
        for task in self.tasks.lock().expect("bg tasks lock").iter() {
            task.kill();
        }
    }
}

/// 查询面快照（task_output 渲染输入）。
pub struct TaskSnapshot {
    pub id: u32,
    pub cmd: String,
    pub running: bool,
    pub elapsed: Duration,
    pub buffer: String,
    pub truncated: bool,
    pub exit: Option<ExitState>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> BgRegistry {
        BgRegistry::new()
    }

    #[test]
    fn spawn_capture_and_exit() {
        let reg = registry();
        let id = reg
            .spawn(
                "echo hello-bg; echo err-bg 1>&2",
                &std::env::temp_dir(),
                Duration::from_secs(30),
            )
            .unwrap();
        let snap = reg.wait_output(id, Duration::from_secs(5)).unwrap();
        assert!(!snap.running, "5s 内完成");
        assert!(snap.buffer.contains("hello-bg"), "{}", snap.buffer);
        assert!(
            snap.buffer.contains("err-bg"),
            "stderr 合并: {}",
            snap.buffer
        );
        assert_eq!(snap.exit.unwrap().code, Some(0));
    }

    /// v2.81 回归：进程退出后读取线程可能仍在排空管道——查询须等排空再返回，
    /// 否则丢尾部输出（CI 实测 stderr 尾行缺失；`run_command background` 跑
    /// 起 dev server / 守护子进程时，父 shell 先退出而管道仍被子进程持有，
    /// 旧实现立即返回只拿到前半段输出）。
    #[test]
    fn output_complete_after_exit_waits_for_readers() {
        let reg = registry();
        // 父 shell 立即退出；后台子进程 0.3s 后才写 stderr 并释放管道
        let cmd = "( sleep 0.3; echo LATE-ERR 1>&2 ) & echo EARLY-OUT; exit 0";
        let id = reg
            .spawn(cmd, &std::env::temp_dir(), Duration::from_secs(30))
            .unwrap();
        let snap = reg.wait_output(id, Duration::from_secs(5)).unwrap();
        assert!(!snap.running, "应已结束");
        assert!(snap.buffer.contains("EARLY-OUT"), "前半段输出须在");
        assert!(
            snap.buffer.contains("LATE-ERR"),
            "退出后仍在写管道的子进程输出须在排空宽限内并入（本回归触发点）: {}",
            snap.buffer
        );
    }

    #[test]
    fn running_then_kill() {
        let reg = registry();
        let id = reg
            .spawn("sleep 30", &std::env::temp_dir(), Duration::from_secs(60))
            .unwrap();
        let snap = reg.wait_output(id, Duration::ZERO).unwrap();
        assert!(snap.running, "sleep 30 未结束");

        assert!(reg.kill(id).is_ok(), "终止成功");
        let snap = reg.wait_output(id, Duration::from_secs(5)).unwrap();
        assert!(!snap.running);
        assert!(snap.exit.unwrap().killed, "终态标 killed");

        // 已结束再杀 → 如实回报
        assert!(reg.kill(id).is_err());
    }

    #[test]
    fn timeout_kills_long_task() {
        let reg = registry();
        let id = reg
            .spawn(
                "sleep 30",
                &std::env::temp_dir(),
                Duration::from_millis(500),
            )
            .unwrap();
        let snap = reg.wait_output(id, Duration::from_secs(5)).unwrap();
        assert!(!snap.running);
        let exit = snap.exit.unwrap();
        assert!(exit.timed_out, "超时终止标注");
        assert!(!exit.killed);
    }

    #[test]
    fn unknown_id_rejected() {
        let reg = registry();
        assert!(reg.wait_output(999, Duration::ZERO).is_none());
        assert!(reg.kill(999).is_err());
    }

    #[test]
    fn buffer_cap_truncates() {
        let reg = registry();
        let id = reg
            .spawn(
                "yes 0123456789 | head -c 1048576",
                &std::env::temp_dir(),
                Duration::from_secs(30),
            )
            .unwrap();
        let snap = reg.wait_output(id, Duration::from_secs(10)).unwrap();
        assert!(!snap.running);
        assert!(snap.buffer.len() <= BUFFER_CAP, "缓冲封顶");
        assert!(snap.truncated, "截留标注");
    }

    #[test]
    fn drop_kills_surviving_children() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("marker");
        let reg = registry();
        let script = format!(
            "sleep 30; touch {}",
            marker.to_string_lossy().replace(' ', r"\ ")
        );
        let _id = reg
            .spawn(&script, &std::env::temp_dir(), Duration::from_secs(60))
            .unwrap();
        drop(reg);
        // Drop 杀子进程后 marker 不会出现（轮询等待足够覆盖 sleep 前的杀）
        std::thread::sleep(Duration::from_millis(500));
        assert!(!marker.exists(), "Drop 兜底清杀：sleep 被终止未执行 touch");
    }
}
