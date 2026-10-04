//! 文件变更监听（设计方案 §8.6：代理写盘前检查用户未保存缓冲的基础设施）。
//!
//! v1.80 起默认使用平台原生事件后端（notify `recommended_watcher`：
//! macOS FSEvents / Linux inotify / Windows ReadDirectoryChangesW）——注册即
//! 返回。M0 的 PollWatcher + `compare_contents` 会在 `watch()` 同步全树扫描
//! 并对每个文件做内容哈希（忽略目录只滤事件不滤扫描），含 build 产物的仓库
//! 激活即分钟级阻塞。PollWatcher 保留为原生后端构建失败的回退与测试确定性
//! 通道（`watch_with_poll_interval`），对外接口不变。

use notify::{PollWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::Duration;

/// 归一化后的变更事件（项目相对路径）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeEvent {
    pub path: String,
    #[serde(rename = "type")]
    pub kind: ChangeKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Created,
    Modified,
    Removed,
}

/// 文件监听器：去抖合并后的事件流。
pub struct FileWatcher {
    _watcher: Box<dyn notify::Watcher + Send>,
    rx: Receiver<ChangeEvent>,
}

impl FileWatcher {
    /// 监听目录（递归）。`.git` / target / node_modules / dist 忽略。
    pub fn watch(root: &Path) -> notify::Result<Self> {
        let (tx, rx) = std::sync::mpsc::channel();
        let root_owned: PathBuf = root.to_path_buf();
        let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        let make_handler = move |tx: std::sync::mpsc::Sender<ChangeEvent>,
                                 root_owned: PathBuf,
                                 canonical_root: PathBuf| {
            move |res: notify::Result<notify::Event>| {
                if let Ok(event) = res {
                    for path in event.paths {
                        let rel = path
                            .strip_prefix(&root_owned)
                            .or_else(|_| path.strip_prefix(&canonical_root))
                            .ok()
                            .map(|p| p.to_string_lossy().replace('\\', "/"));
                        let Some(rel) = rel else { continue };
                        if should_ignore(&rel) {
                            continue;
                        }
                        let kind = if event.kind.is_create() {
                            ChangeKind::Created
                        } else if event.kind.is_remove() {
                            ChangeKind::Removed
                        } else if event.kind.is_modify() {
                            ChangeKind::Modified
                        } else {
                            continue;
                        };
                        let _ = tx.send(ChangeEvent { path: rel, kind });
                    }
                }
            }
        };
        // 平台后端注册必须不能阻塞 API：FSEvents / inotify 初始化交给 setup
        // 线程，2s 内未 ready 就显式失败，调用方可按“无 watcher”降级。
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let setup_root = root_owned.clone();
        std::thread::Builder::new()
            .name("tenon-watch-setup".into())
            .spawn(move || {
                let result =
                    notify::recommended_watcher(make_handler(tx, root_owned, canonical_root))
                        .and_then(|mut watcher| {
                            watcher
                                .watch(&setup_root, RecursiveMode::Recursive)
                                .map(|_| watcher)
                        });
                let _ = ready_tx.send(result);
            })?;

        match ready_rx.recv_timeout(Duration::from_secs(10)) {
            Ok(Ok(watcher)) => Ok(Self {
                _watcher: Box::new(watcher),
                rx,
            }),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(notify::Error::io(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "native watcher registration timed out",
            ))),
        }
    }

    /// 强制轮询后端（测试确定性通道）：指定轮询间隔 + 内容比对。
    pub fn watch_with_poll_interval(root: &Path, interval: Duration) -> notify::Result<Self> {
        Self::watch_impl(root, Some(interval))
    }

    fn watch_impl(root: &Path, force_poll: Option<Duration>) -> notify::Result<Self> {
        let (tx, rx) = std::sync::mpsc::channel();
        let root_owned: PathBuf = root.to_path_buf();
        // 原生后端（FSEvents）返回 canonical 路径（如 /tmp → /private/tmp）：
        // 原始根与 canonical 根都做前缀剥离，符号链接根不再静默丢事件。
        let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        // 事件回调工厂：各后端分支各自持有一份（Sender 可克隆）。
        let make_handler = move |tx: std::sync::mpsc::Sender<ChangeEvent>,
                                 root_owned: PathBuf,
                                 canonical_root: PathBuf| {
            move |res: notify::Result<notify::Event>| {
                if let Ok(event) = res {
                    for path in event.paths {
                        let rel = path
                            .strip_prefix(&root_owned)
                            .or_else(|_| path.strip_prefix(&canonical_root))
                            .ok()
                            .map(|p| p.to_string_lossy().replace('\\', "/"));
                        let rel = match rel {
                            Some(r) => r,
                            None => continue,
                        };
                        if should_ignore(&rel) {
                            continue;
                        }
                        let kind = if event.kind.is_create() {
                            ChangeKind::Created
                        } else if event.kind.is_remove() {
                            ChangeKind::Removed
                        } else if event.kind.is_modify() {
                            ChangeKind::Modified
                        } else {
                            continue;
                        };
                        let _ = tx.send(ChangeEvent { path: rel, kind });
                    }
                }
            }
        };
        let mut watcher: Box<dyn notify::Watcher + Send> = match force_poll {
            Some(interval) => Box::new(PollWatcher::new(
                make_handler(tx.clone(), root_owned.clone(), canonical_root.clone()),
                notify::Config::default()
                    .with_poll_interval(interval)
                    .with_compare_contents(true),
            )?),
            None => {
                match notify::recommended_watcher(make_handler(
                    tx.clone(),
                    root_owned.clone(),
                    canonical_root.clone(),
                )) {
                    Ok(native) => Box::new(native),
                    // 原生后端构建失败：回退轮询，间隔保守以防内容哈希风暴。
                    Err(_) => Box::new(PollWatcher::new(
                        make_handler(tx, root_owned, canonical_root),
                        notify::Config::default()
                            .with_poll_interval(Duration::from_secs(2))
                            .with_compare_contents(true),
                    )?),
                }
            }
        };
        watcher.watch(root, RecursiveMode::Recursive)?;
        Ok(Self {
            _watcher: watcher,
            rx,
        })
    }

    /// 阻塞等待一批事件（去抖：`window` 内的事件合并去重）。
    pub fn next_batch(&self, window: Duration) -> Vec<ChangeEvent> {
        let mut out = Vec::new();
        let first = match self.rx.recv_timeout(window) {
            Ok(e) => e,
            Err(RecvTimeoutError::Timeout) => return out,
            Err(RecvTimeoutError::Disconnected) => return out,
        };
        out.push(first);
        // 去抖窗口内继续收集
        loop {
            match self.rx.recv_timeout(Duration::from_millis(80)) {
                Ok(e) => out.push(e),
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        // 去重：同路径保留最后一个状态
        let mut dedup: Vec<ChangeEvent> = Vec::new();
        for e in out {
            if let Some(existing) = dedup.iter_mut().find(|x| x.path == e.path) {
                existing.kind = e.kind;
            } else {
                dedup.push(e);
            }
        }
        dedup
    }
}

fn should_ignore(rel: &str) -> bool {
    const IGNORED: [&str; 5] = [".git/", "target/", "node_modules/", "dist/", ".tenon/"];
    IGNORED.iter().any(|p| rel.starts_with(p)) || rel == ".git"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_file_creation_and_modification() {
        let dir = tempfile::tempdir().unwrap();
        // 确定性通道（v1.80）：轮询后端 + 内容比对，平台行为一致。
        let watcher = FileWatcher::watch_with_poll_interval(dir.path(), Duration::from_millis(100))
            .expect("watcher");
        std::fs::write(dir.path().join("new.txt"), "hello").unwrap();
        std::thread::sleep(Duration::from_millis(300));
        std::fs::write(dir.path().join("new.txt"), "changed").unwrap();

        let events = watcher.next_batch(Duration::from_secs(10));
        assert!(
            events.iter().any(|e| e.path == "new.txt"),
            "应收到 new.txt 事件，实际: {events:?}"
        );
    }

    #[test]
    fn ignores_target_and_git_dirs() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        std::fs::create_dir_all(dir.path().join("target/debug")).unwrap();
        let watcher =
            FileWatcher::watch_with_poll_interval(dir.path(), Duration::from_millis(100)).unwrap();
        std::fs::write(dir.path().join(".git/HEAD"), "ref: x").unwrap();
        std::fs::write(dir.path().join("target/debug/o.o"), "x").unwrap();
        std::fs::write(dir.path().join("src.rs"), "fn a() {}").unwrap();
        std::thread::sleep(Duration::from_millis(300));

        let events = watcher.next_batch(Duration::from_secs(10));
        assert!(events.iter().any(|e| e.path == "src.rs"));
        assert!(!events.iter().any(|e| e.path.starts_with(".git")));
        assert!(!events.iter().any(|e| e.path.starts_with("target")));
    }

    #[test]
    fn native_backend_delivers_events() {
        // 默认路径（v1.80 原生事件后端）：注册即返回、写入即有事件。
        let dir = tempfile::tempdir().unwrap();
        let watcher = FileWatcher::watch(dir.path()).expect("native watcher");
        std::fs::write(dir.path().join("native.txt"), "hi").unwrap();
        let events = watcher.next_batch(Duration::from_secs(10));
        assert!(
            events.iter().any(|e| e.path == "native.txt"),
            "原生后端应收到 native.txt 事件，实际: {events:?}"
        );
    }

    #[test]
    fn timeout_returns_empty_batch() {
        let dir = tempfile::tempdir().unwrap();
        let watcher = FileWatcher::watch(dir.path()).unwrap();
        let events = watcher.next_batch(Duration::from_millis(200));
        assert!(events.is_empty());
    }
}
