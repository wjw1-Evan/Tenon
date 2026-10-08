//! 项目终端（§7.2 / §15 v1.197）：每项目一个用户自有 shell 的 PTY 会话。
//!
//! 信任模型：终端是用户自己的 shell（与用户在系统终端里开一个等价）——
//! 全信任、不过沙箱、不进 Trace（§12.3 注记：这是用户的交互通道而非代理
//! 动作面）；项目级单实例，项目运行时关闭时回收。传输复用 WS 票据鉴权。

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use tokio::sync::{broadcast, Mutex as AsyncMutex};

/// 单终端输出环形缓冲订阅上限（慢消费者丢帧可接受——交互终端）。
const OUTPUT_CHANNEL_CAPACITY: usize = 256;

pub struct TerminalSession {
    /// 输出流（读者线程 → WS）。
    pub output_tx: broadcast::Sender<Vec<u8>>,
    writer: Mutex<Box<dyn Write + Send>>,
    pty_master: Mutex<Box<dyn MasterPty + Send>>,
    /// kill 句柄（独立锁——wait 由看护线程独占 child，避免互等死锁）。
    killer: Mutex<Box<dyn portable_pty::ChildKiller + Send + Sync>>,
    /// shell 路径（诊断展示）。
    pub shell: String,
}

impl TerminalSession {
    fn spawn(cwd: PathBuf) -> Result<Self, String> {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| format!("PTY 分配失败: {e}"))?;

        // shell 选择：$SHELL 优先，缺省平台兜底（§7.2 v1.197）
        let shell = std::env::var("SHELL").unwrap_or_else(|_| default_shell());
        let mut cmd = CommandBuilder::new(&shell);
        cmd.cwd(cwd);
        cmd.env("TERM", "xterm-256color");

        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| format!("shell 启动失败（{shell}）: {e}"))?;

        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| format!("PTY 读者克隆失败: {e}"))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|e| format!("PTY 写者获取失败: {e}"))?;

        let (output_tx, _) = broadcast::channel(OUTPUT_CHANNEL_CAPACITY);
        {
            let output_tx = output_tx.clone();
            std::thread::spawn(move || {
                let mut buf = [0_u8; 4096];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            if output_tx.send(buf[..n].to_vec()).is_err() {
                                break; // 无订阅者（终端关闭）
                            }
                        }
                        Err(_) => break,
                    }
                }
            });
        }

        // 子进程看护线程：独占 child 做 wait（防僵尸）；退出即关输出通道
        // （WS 侧感知 EOF）。kill 经独立 killer 句柄——不与 wait 抢锁。
        let killer = child.clone_killer();
        {
            let mut child = child; // 移交看护线程独占
            let output_tx = output_tx.clone();
            std::thread::spawn(move || {
                let _ = child.wait();
                let _ = output_tx.send(Vec::new());
            });
        }

        Ok(Self {
            output_tx,
            writer: Mutex::new(writer),
            pty_master: Mutex::new(pair.master),
            killer: Mutex::new(killer),
            shell,
        })
    }

    /// 写入用户键入（字节透传，终端自身处理回显 / 行编辑）。
    fn write(&self, data: &[u8]) -> Result<(), String> {
        self.writer
            .lock()
            .expect("terminal writer lock")
            .write_all(data)
            .map_err(|e| format!("PTY 写入失败: {e}"))
    }

    /// 尺寸同步（xterm fit 后调用）。
    fn resize(&self, rows: u16, cols: u16) -> Result<(), String> {
        self.pty_master
            .lock()
            .expect("pty master lock")
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| format!("PTY resize 失败: {e}"))
    }

    fn kill(&self) {
        if let Ok(mut killer) = self.killer.lock() {
            let _ = killer.kill();
        }
    }
}

fn default_shell() -> String {
    #[cfg(target_os = "windows")]
    {
        "cmd.exe".into()
    }
    #[cfg(not(target_os = "windows"))]
    {
        "/bin/zsh".into()
    }
}

/// 项目级终端管理器：`project_id → 终端会话`（单实例；项目运行时关闭时回收）。
#[derive(Default)]
pub struct TerminalManager {
    sessions: AsyncMutex<HashMap<String, Arc<TerminalSession>>>,
}

impl TerminalManager {
    /// 取既有终端或新开（幂等；子进程退出后由下一次 get_or_spawn 懒重建）。
    pub async fn get_or_spawn(
        &self,
        project_id: &str,
        cwd: PathBuf,
    ) -> Result<Arc<TerminalSession>, String> {
        let mut sessions = self.sessions.lock().await;
        if let Some(existing) = sessions.get(project_id) {
            return Ok(existing.clone());
        }
        let session = Arc::new(TerminalSession::spawn(cwd)?);
        sessions.insert(project_id.to_string(), session.clone());
        Ok(session)
    }

    /// 项目运行时关闭时回收（§6.4 生命周期钩子）。
    pub async fn kill_project(&self, project_id: &str) {
        if let Some(session) = self.sessions.lock().await.remove(project_id) {
            session.kill();
        }
    }

    pub async fn kill_all(&self) {
        let ids: Vec<String> = self.sessions.lock().await.keys().cloned().collect();
        for id in ids {
            self.kill_project(&id).await;
        }
    }
}

/// WS 泵：输出广播 → socket；socket 文本帧 → PTY 写入（JSON 控制帧
/// `{"resize":[rows,cols]}` 例外）。运行于 WS 升级后的任务内。
pub async fn pump_terminal_ws(
    session: Arc<TerminalSession>,
    mut socket: axum::extract::ws::WebSocket,
) {
    let mut output_sub = session.output_tx.subscribe();

    // 双泵循环：socket 输入 → PTY；PTY 输出 → socket（UTF-8 文本帧优先）
    loop {
        tokio::select! {
            msg = socket.recv() => {
                match msg {
                    Some(Ok(axum::extract::ws::Message::Text(text))) => {
                        if let Ok(control) =
                            serde_json::from_str::<serde_json::Value>(&text)
                        {
                            let size = control
                                .get("resize")
                                .and_then(|r| r.as_array())
                                .and_then(|a| {
                                    if a.len() == 2 {
                                        Some((
                                            a[0].as_u64().and_then(|v| u16::try_from(v).ok()),
                                            a[1].as_u64().and_then(|v| u16::try_from(v).ok()),
                                        ))
                                    } else {
                                        None
                                    }
                                });
                            if let Some((Some(rows), Some(cols))) = size {
                                let _ = session.resize(rows, cols);
                                continue;
                            }
                        }
                        if session.write(text.as_bytes()).is_err() {
                            break;
                        }
                    }
                    Some(Ok(axum::extract::ws::Message::Binary(bytes))) => {
                        if session.write(&bytes).is_err() {
                            break;
                        }
                    }
                    Some(Ok(_)) => {}
                    _ => break,
                }
            }
            out = output_sub.recv() => {
                match out {
                    Ok(bytes) => {
                        let msg = match std::str::from_utf8(&bytes) {
                            Ok(text) => axum::extract::ws::Message::text(text.to_string()),
                            Err(_) => axum::extract::ws::Message::binary(bytes),
                        };
                        if socket.send(msg).await.is_err() {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn manager_lifecycle_idempotent_and_kill() {
        let manager = TerminalManager::default();
        let cwd = tempfile::tempdir().unwrap();
        let s1 = manager
            .get_or_spawn("p1", cwd.path().to_path_buf())
            .await
            .expect("spawn");
        let s2 = manager
            .get_or_spawn("p1", cwd.path().to_path_buf())
            .await
            .expect("get");
        assert!(Arc::ptr_eq(&s1, &s2), "同项目单实例（幂等复用）");
        manager.kill_project("p1").await;
        assert!(manager.sessions.lock().await.is_empty());
    }

    #[tokio::test]
    async fn pty_smoke_write_and_read() {
        // PTY 冒烟：写入一条 echo 命令，读回输出含命令回显（终端自回显语义）。
        let cwd = tempfile::tempdir().unwrap();
        let session = TerminalSession::spawn(cwd.path().to_path_buf()).expect("spawn");
        let mut sub = session.output_tx.subscribe();
        session.write(b"echo TERMINAL_SMOKE_1907\r\n").unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut seen = false;
        while std::time::Instant::now() < deadline {
            match tokio::time::timeout(std::time::Duration::from_millis(500), sub.recv()).await {
                Ok(Ok(bytes)) => {
                    if String::from_utf8_lossy(&bytes).contains("TERMINAL_SMOKE_1907") {
                        seen = true;
                        break;
                    }
                }
                _ => continue,
            }
        }
        assert!(seen, "5s 内未读回回显");
        session.kill();
    }
}
