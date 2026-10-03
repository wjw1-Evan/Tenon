//! A/B 写守卫（设计方案 §12.3 M0 交付）：所有代理写路径强制收敛在项目内。
//!
//! 规则：
//! - 目标路径（含尚不存在的目标）必须位于项目根内；
//! - 禁止写用户仓库 `.git` 内部（快照走独立 shadow 库，§10.3 / ADR-7；
//!   子代理 worktree 的 `.git` 元数据由内核托管路径写入，不经此守卫，§9.5）；
//! - 符号链接解析后逃逸项目根 → 拒绝。

use crate::SandboxError;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone)]
pub struct WriteGuard {
    root: PathBuf,
}

impl WriteGuard {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        // macOS /var → /private/var 等：以真实路径为基准
        let root = std::fs::canonicalize(&root).unwrap_or(root);
        Self { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 归一化相对路径：拒绝 `..` 逃逸与绝对路径注入。
    pub fn normalize_relative(&self, rel: &str) -> Result<PathBuf, SandboxError> {
        let p = Path::new(rel);
        if p.is_absolute() {
            // 绝对路径仅当位于项目内时接受
            return match self.check_abs(p) {
                Ok(()) => Ok(p.to_path_buf()),
                Err(e) => Err(e),
            };
        }
        let mut out = PathBuf::new();
        for comp in p.components() {
            match comp {
                Component::Normal(c) => out.push(c),
                Component::CurDir => {}
                Component::ParentDir => {
                    if !out.pop() {
                        return Err(SandboxError::PathEscape(rel.to_string()));
                    }
                }
                _ => return Err(SandboxError::PathEscape(rel.to_string())),
            }
        }
        Ok(out)
    }

    /// 校验一个写入目标（文件可能尚不存在）：父目录链不得逃逸项目根。
    pub fn check_write_rel(&self, rel: &str) -> Result<PathBuf, SandboxError> {
        let rel_path = self.normalize_relative(rel)?;
        let full = self.root.join(&rel_path);
        self.check_abs(&full)?;
        Ok(rel_path)
    }

    /// 校验绝对路径写入目标。
    pub fn check_abs(&self, target: &Path) -> Result<(), SandboxError> {
        // .git 内部保护
        for anc in target.ancestors() {
            if anc.file_name().map(|f| f == ".git").unwrap_or(false) && target.starts_with(anc) {
                return Err(SandboxError::GitInternal(
                    target.to_string_lossy().into_owned(),
                ));
            }
        }
        // 符号链接逃逸检测：从最深存在目录逐级 canonicalize
        let mut to_canon = target.to_path_buf();
        let mut suffix = Vec::new();
        loop {
            match std::fs::canonicalize(&to_canon) {
                Ok(real) => {
                    if !real.starts_with(&self.root) {
                        return Err(SandboxError::SymlinkEscape(
                            target.to_string_lossy().into_owned(),
                        ));
                    }
                    let mut real_full = real;
                    for part in suffix.iter().rev() {
                        real_full.push(part);
                    }
                    if !real_full.starts_with(&self.root) {
                        return Err(SandboxError::PathEscape(
                            target.to_string_lossy().into_owned(),
                        ));
                    }
                    return Ok(());
                }
                Err(_) => {
                    let name = to_canon.file_name().map(|n| n.to_os_string());
                    match name {
                        Some(n) => {
                            suffix.push(n);
                            if !to_canon.pop() {
                                return Err(SandboxError::PathEscape(
                                    target.to_string_lossy().into_owned(),
                                ));
                            }
                        }
                        None => {
                            return Err(SandboxError::PathEscape(
                                target.to_string_lossy().into_owned(),
                            ))
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guard() -> (tempfile::TempDir, WriteGuard) {
        let dir = tempfile::tempdir().unwrap();
        let g = WriteGuard::new(dir.path());
        (dir, g)
    }

    #[test]
    fn allows_writes_inside_project() {
        let (_d, g) = guard();
        assert!(g.check_write_rel("src/main.rs").is_ok());
        assert!(g.check_write_rel("./a/../b.txt").is_ok());
        assert_eq!(
            g.normalize_relative("./a/../b.txt").unwrap(),
            PathBuf::from("b.txt")
        );
    }

    #[test]
    fn rejects_dotdot_escape() {
        let (_d, g) = guard();
        assert!(g.check_write_rel("../outside.txt").is_err());
        assert!(g.check_write_rel("a/../../outside.txt").is_err());
        assert!(g.normalize_relative("..").is_err());
    }

    #[test]
    fn rejects_absolute_outside() {
        let (_d, g) = guard();
        assert!(g.check_write_rel("/etc/hosts").is_err());
        assert!(g.check_write_rel("/tmp/x").is_err());
    }

    #[test]
    fn rejects_git_internal_writes() {
        let (d, g) = guard();
        let root = d.path();
        // 用户仓库 .git 目录
        std::fs::create_dir_all(root.join(".git/objects")).unwrap();
        assert!(g.check_write_rel(".git/objects/ab/cdef").is_err());
        let _ = root;
    }

    #[test]
    fn rejects_symlink_escape() {
        let (d, g) = guard();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), d.path().join("link")).unwrap();
        assert!(g.check_write_rel("link/evil.txt").is_err());
    }

    #[test]
    fn nonexistent_parent_chain_resolved() {
        let (d, g) = guard();
        // 多级不存在目录内的写入（父目录全不存在）
        assert!(g.check_write_rel("deep/nested/dir/file.txt").is_ok());
        // 经软链接目录指向项目内 → 允许
        std::os::unix::fs::symlink(d.path().join("real"), d.path().join("alias")).unwrap();
        std::fs::create_dir_all(d.path().join("real")).unwrap();
        assert!(g.check_write_rel("alias/file.txt").is_ok());
    }
}
