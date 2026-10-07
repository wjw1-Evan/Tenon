//! 传输层抽象：帧读写半分离（宿主读线程与写互斥不互相阻塞）。
//! 进程 stdio（真实语言服务器）与通道（测试用假服务器）两种实现。

use std::io::{self, BufReader};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex};

/// 帧读端。
pub trait FrameRead: Send {
    /// 读一帧 JSON 体（阻塞）。
    fn read_frame(&mut self) -> io::Result<String>;
    /// 关闭（终止阻塞读）。
    fn shutdown(&mut self);
}

/// 帧写端。
pub trait FrameWrite: Send {
    fn write_frame(&mut self, body: &str) -> io::Result<()>;
    fn shutdown(&mut self);
}

// ---------- 进程传输 ----------

/// 语言服务器子进程连接（拆分读写半）。
pub struct ProcessConnection {
    pub reader: ProcessReader,
    pub writer: ProcessWriter,
}

pub struct ProcessReader {
    stdout: BufReader<ChildStdout>,
}

pub struct ProcessWriter {
    stdin: ChildStdin,
    child: Arc<Mutex<Child>>,
}

impl ProcessConnection {
    /// 启动语言服务器进程。
    pub fn spawn(program: &str, args: &[&str], cwd: &std::path::Path) -> io::Result<Self> {
        let mut child = Command::new(program)
            .args(args)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = BufReader::new(child.stdout.take().expect("piped stdout"));
        let child = Arc::new(Mutex::new(child));
        Ok(Self {
            reader: ProcessReader { stdout },
            writer: ProcessWriter { stdin, child },
        })
    }
}

impl ProcessConnection {
    /// String 参数版本的 spawn（语言包 args 为 Vec<String>）。
    pub fn spawn_str(program: &str, args: &[String], cwd: &std::path::Path) -> io::Result<Self> {
        let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        Self::spawn(program, &arg_refs, cwd)
    }
}

impl FrameRead for ProcessReader {
    fn read_frame(&mut self) -> io::Result<String> {
        crate::codec::read_message(&mut self.stdout).map_err(|e| io::Error::other(e.to_string()))
    }

    fn shutdown(&mut self) {
        // 读端无进程句柄；终止在 writer 侧统一做
    }
}

impl FrameWrite for ProcessWriter {
    fn write_frame(&mut self, body: &str) -> io::Result<()> {
        crate::codec::write_message(&mut self.stdin, body)
            .map_err(|e| io::Error::other(e.to_string()))
    }

    fn shutdown(&mut self) {
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for ProcessWriter {
    fn drop(&mut self) {
        // initialize 失败 / 宿主未 shutdown 即丢弃时，Child 的 kill+wait 只
        // 存在于 shutdown()——不兜底则语言服务器成孤儿进程、退出后变僵尸，
        // daemon 长期运行 + 多项目反复开关会稳步泄漏
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

// ---------- 通道传输（测试 / 假服务器） ----------

struct SharedEnd {
    inbox: Mutex<std::sync::mpsc::Receiver<String>>,
    outbox: std::sync::mpsc::Sender<String>,
    closed: std::sync::atomic::AtomicBool,
}

/// 一对互通的（reader, writer）× 2。
pub fn channel_pair() -> (
    (ChannelEndReader, ChannelEndWriter),
    (ChannelEndReader, ChannelEndWriter),
) {
    let (a_tx, a_rx) = std::sync::mpsc::channel::<String>();
    let (b_tx, b_rx) = std::sync::mpsc::channel::<String>();
    let mk = |rx: std::sync::mpsc::Receiver<String>, tx: std::sync::mpsc::Sender<String>| {
        let shared = Arc::new(SharedEnd {
            inbox: Mutex::new(rx),
            outbox: tx,
            closed: std::sync::atomic::AtomicBool::new(false),
        });
        (
            ChannelEndReader {
                shared: shared.clone(),
            },
            ChannelEndWriter { shared },
        )
    };
    (mk(a_rx, b_tx), mk(b_rx, a_tx))
}

pub struct ChannelEndReader {
    shared: Arc<SharedEnd>,
}

pub struct ChannelEndWriter {
    shared: Arc<SharedEnd>,
}

impl FrameRead for ChannelEndReader {
    fn read_frame(&mut self) -> io::Result<String> {
        loop {
            if self.shared.closed.load(std::sync::atomic::Ordering::SeqCst) {
                return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "closed"));
            }
            match self
                .shared
                .inbox
                .lock()
                .expect("inbox lock")
                .recv_timeout(std::time::Duration::from_millis(25))
            {
                Ok(v) => return Ok(v),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "disconnected"));
                }
            }
        }
    }

    fn shutdown(&mut self) {
        self.shared
            .closed
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

impl FrameWrite for ChannelEndWriter {
    fn write_frame(&mut self, body: &str) -> io::Result<()> {
        if self.shared.closed.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(io::Error::new(io::ErrorKind::BrokenPipe, "closed"));
        }
        self.shared
            .outbox
            .send(body.to_string())
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "peer gone"))
    }

    fn shutdown(&mut self) {
        self.shared
            .closed
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }
}
