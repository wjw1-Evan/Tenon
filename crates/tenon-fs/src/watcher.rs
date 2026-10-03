//! 文件变更监听（设计方案 §8.6：代理写盘前检查用户未保存缓冲的基础设施）。
//!
//! M0 采用 PollWatcher（确定性、跨平台一致）；FSEvents/inotify 事件后端
//! 随性能打磨切换（接口不变）。

use notify::{PollWatcher, RecursiveMode, Watcher as _};
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
    _watcher: PollWatcher,
    rx: Receiver<ChangeEvent>,
}

impl FileWatcher {
    /// 监听目录（递归）。`.git` / target / node_modules / dist 忽略。
    pub fn watch(root: &Path) -> notify::Result<Self> {
        Self::watch_with_poll_interval(root, Duration::from_millis(100))
    }

    pub fn watch_with_poll_interval(root: &Path, interval: Duration) -> notify::Result<Self> {
        let (tx, rx) = std::sync::mpsc::channel();
        let root_owned: PathBuf = root.to_path_buf();
        let config = notify::Config::default()
            .with_poll_interval(interval)
            .with_compare_contents(true);
        let mut watcher = PollWatcher::new(
            move |res: notify::Result<notify::Event>| {
                if let Ok(event) = res {
                    for path in event.paths {
                        let rel = match path.strip_prefix(&root_owned) {
                            Ok(p) => p.to_string_lossy().replace('\\', "/"),
                            Err(_) => continue,
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
            },
            config,
        )?;
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
        let watcher = FileWatcher::watch(dir.path()).expect("watcher");
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
        let watcher = FileWatcher::watch(dir.path()).unwrap();
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
    fn timeout_returns_empty_batch() {
        let dir = tempfile::tempdir().unwrap();
        let watcher = FileWatcher::watch(dir.path()).unwrap();
        let events = watcher.next_batch(Duration::from_millis(200));
        assert!(events.is_empty());
    }
}
