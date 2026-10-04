//! 文件服务（设计方案 §8.1 / §15 编辑器与文件 API 的内核侧实现）：
//! 文件树（git 状态装饰）、读写（写守卫收敛项目内）、CRUD ops、
//! ripgrep 全局搜索与替换预览、fuzzy 查找、变更监听。

pub mod dirty;
pub mod fuzzy;
pub mod git;
pub mod highlight;
pub mod l4;
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

/// 文件读取视图（§8.1）：任意大小文件全量读取（v1.69 移除只读分块）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct FileView {
    pub content: String,
    pub total_bytes: u64,
}

/// 文件服务：所有路径参数为项目相对路径（POSIX 风格），绝对路径仅当位于项目内才接受。
pub struct FileService {
    root: PathBuf,
    guard: tenon_sandbox::WriteGuard,
}

impl FileService {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        Self {
            guard: tenon_sandbox::WriteGuard::new(&root),
            root,
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

    fn decode_text(rel: &str, bytes: Vec<u8>) -> Result<String> {
        if bytes.iter().take(8192).any(|b| *b == 0) {
            return Err(FsError::Binary(rel.to_string()));
        }
        String::from_utf8(bytes).map_err(|_| FsError::Binary(rel.to_string()))
    }

    /// 读取文本文件（UTF-8；二进制报错；无大小上限，§8.1）。
    pub fn read_file(&self, rel: &str) -> Result<String> {
        let path = self.resolve(rel)?;
        let bytes = std::fs::read(&path)?;
        Self::decode_text(rel, bytes)
    }

    /// 文件字节大小（路径经写守卫校验）。
    pub fn file_size(&self, rel: &str) -> Result<u64> {
        let path = self.resolve(rel)?;
        Ok(std::fs::metadata(&path)?.len())
    }

    /// 全量读取文本文件并返回字节总数（§8.1：任意大小可打开可编辑）。
    pub fn read_file_view(&self, rel: &str) -> Result<FileView> {
        let path = self.resolve(rel)?;
        let total_bytes = std::fs::metadata(&path)?.len();
        let bytes = std::fs::read(&path)?;
        let content = Self::decode_text(rel, bytes)?;
        Ok(FileView {
            content,
            total_bytes,
        })
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
    fn binary_rejected() {
        let (d, s) = svc();
        // 二进制（含 NUL）
        std::fs::write(d.path().join("bin.dat"), [0u8, 1, 2, 3]).unwrap();
        assert!(matches!(s.read_file("bin.dat"), Err(FsError::Binary(_))));
    }

    #[test]
    fn large_file_full_read_and_editable() {
        let (d, s) = svc();
        // 10 MiB + 5：越过旧只读阈值，v1.69 起必须全量可读且可写。
        let large = format!("{}next\n", "A".repeat(10 * 1024 * 1024));
        std::fs::write(d.path().join("big.txt"), &large).unwrap();
        let view = s.read_file_view("big.txt").unwrap();
        assert_eq!(view.total_bytes, 10 * 1024 * 1024 + 5);
        assert_eq!(view.content.len(), 10 * 1024 * 1024 + 5);
        // 旧文件超过 10MB 也允许写盘（v1.69 移除拒写）。
        assert!(!s.write_file("big.txt", "replaced\n").unwrap());
        assert_eq!(s.read_file("big.txt").unwrap(), "replaced\n");
    }

    #[test]
    fn multibyte_content_full_read() {
        let (d, s) = svc();
        let source = "abc漢xyz\n".to_string();
        std::fs::write(d.path().join("utf8.txt"), &source).unwrap();
        let view = s.read_file_view("utf8.txt").unwrap();
        assert_eq!(view.content, source);
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
