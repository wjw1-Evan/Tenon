//! 文件服务（设计方案 §8.1 / §15 编辑器与文件 API 的内核侧实现）：
//! 文件树（git 状态装饰）、读写（写守卫收敛项目内）、CRUD ops、
//! ripgrep 全局搜索与替换预览、fuzzy 查找、变更监听。

pub mod dirty;
pub mod fuzzy;
pub mod git;
pub mod highlight;
pub mod ops;
pub mod search;
pub mod tree;
pub mod watcher;

pub use dirty::{DirtyBuffer, DirtyBufferRegistry};
pub use highlight::{highlight_file, HighlightToken};
pub use ops::{FileOp, FileOps};
pub use search::{SearchHit, SearchOptions};
pub use tree::TreeEntry;
pub use watcher::FileWatcher;

use std::path::{Path, PathBuf};
use tenon_sandbox::SandboxError;

#[derive(Debug, thiserror::Error)]
pub enum FsError {
    #[error("文件过大（>{max_mb}MB，大文件只读分块随 M1 落地，§8.1）")]
    TooLarge { max_mb: u64 },
    #[error("二进制文件不支持文本读取: {0}")]
    Binary(String),
    #[error("路径越界或非法: {0}")]
    BadPath(String),
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),
    #[error("沙箱拒绝: {0}")]
    Sandbox(#[from] SandboxError),
}

pub type Result<T> = std::result::Result<T, FsError>;

/// 文件服务：所有路径参数为项目相对路径（POSIX 风格），绝对路径仅当位于项目内才接受。
pub struct FileService {
    root: PathBuf,
    guard: tenon_sandbox::WriteGuard,
    /// 大文件阈值（§4.2：10MB）。
    pub max_read_bytes: u64,
}

impl FileService {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        Self {
            guard: tenon_sandbox::WriteGuard::new(&root),
            root,
            max_read_bytes: 10 * 1024 * 1024,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 相对路径 → 绝对路径（过写守卫边界检查）。
    pub fn resolve(&self, rel: &str) -> std::result::Result<PathBuf, FsError> {
        let normalized = self.guard.check_write_rel(rel).map_err(|e| match e {
            SandboxError::PathEscape(p) => FsError::BadPath(p),
            SandboxError::SymlinkEscape(p) => FsError::BadPath(p),
            SandboxError::GitInternal(p) => FsError::BadPath(p),
            SandboxError::Io(io) => FsError::Io(io),
        })?;
        Ok(self.root.join(normalized))
    }

    /// 读取文本文件（UTF-8；二进制 / 超限报错）。
    pub fn read_file(&self, rel: &str) -> Result<String> {
        let path = self.resolve(rel)?;
        let meta = std::fs::metadata(&path)?;
        if meta.len() > self.max_read_bytes {
            return Err(FsError::TooLarge {
                max_mb: self.max_read_bytes / 1024 / 1024,
            });
        }
        let bytes = std::fs::read(&path)?;
        if bytes.iter().take(8192).any(|b| *b == 0) {
            return Err(FsError::Binary(rel.to_string()));
        }
        String::from_utf8(bytes).map_err(|_| FsError::Binary(rel.to_string()))
    }

    /// 写文件（经写守卫；自动建父目录）。返回是否为新建。
    pub fn write_file(&self, rel: &str, content: &str) -> Result<bool> {
        let path = self.resolve(rel)?;
        let existed = path.exists();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, content)?;
        Ok(!existed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn svc() -> (tempfile::TempDir, FileService) {
        let dir = tempfile::tempdir().unwrap();
        let s = FileService::new(dir.path());
        (dir, s)
    }

    #[test]
    fn read_write_roundtrip() {
        let (_d, s) = svc();
        let created = s.write_file("src/a.txt", "hello\n").unwrap();
        assert!(created);
        assert_eq!(s.read_file("src/a.txt").unwrap(), "hello\n");
        // 覆盖写
        assert!(!s.write_file("src/a.txt", "world\n").unwrap());
        assert_eq!(s.read_file("src/a.txt").unwrap(), "world\n");
    }

    #[test]
    fn binary_and_oversize_rejected() {
        let (d, mut s) = svc();
        // 二进制（含 NUL）
        std::fs::write(d.path().join("bin.dat"), [0u8, 1, 2, 3]).unwrap();
        assert!(matches!(s.read_file("bin.dat"), Err(FsError::Binary(_))));
        // 超限
        s.max_read_bytes = 8;
        std::fs::write(d.path().join("big.txt"), "123456789").unwrap();
        assert!(matches!(
            s.read_file("big.txt"),
            Err(FsError::TooLarge { .. })
        ));
    }

    #[test]
    fn path_escape_rejected() {
        let (_d, s) = svc();
        assert!(matches!(
            s.read_file("../etc/passwd"),
            Err(FsError::BadPath(_))
        ));
        assert!(matches!(
            s.write_file("/absolute/path", "x"),
            Err(FsError::BadPath(_))
        ));
    }
}
