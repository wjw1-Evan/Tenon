//! 文件 CRUD 操作（设计方案 §8.1 / §15 `POST /file/ops`）：全部经写守卫。

use crate::{FsError, Result};
use serde::{Deserialize, Serialize};
use tenon_sandbox::WriteGuard;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "op")]
pub enum FileOp {
    CreateFile {
        path: String,
        #[serde(default)]
        content: String,
    },
    CreateDir {
        path: String,
    },
    Rename {
        from: String,
        to: String,
    },
    Move {
        from: String,
        to: String,
    },
    Delete {
        path: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum OpOutcome {
    Created,
    Renamed,
    Moved,
    Deleted,
}

pub struct FileOps {
    root: std::path::PathBuf,
    guard: WriteGuard,
}

impl FileOps {
    pub fn new(root: impl Into<std::path::PathBuf>) -> Self {
        let root = root.into();
        Self {
            guard: WriteGuard::new(&root),
            root,
        }
    }

    pub fn root(&self) -> &std::path::Path {
        &self.root
    }

    fn resolve(&self, rel: &str) -> Result<std::path::PathBuf> {
        let normalized = self.guard.check_write_rel(rel)?;
        Ok(self.root.join(normalized))
    }

    pub fn apply(&self, op: &FileOp) -> Result<OpOutcome> {
        match op {
            FileOp::CreateFile { path, content } => {
                let p = self.resolve(path)?;
                if p.exists() {
                    return Err(FsError::Io(std::io::Error::new(
                        std::io::ErrorKind::AlreadyExists,
                        format!("already exists: {path}"),
                    )));
                }
                if let Some(parent) = p.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&p, content)?;
                Ok(OpOutcome::Created)
            }
            FileOp::CreateDir { path } => {
                let p = self.resolve(path)?;
                std::fs::create_dir_all(&p)?;
                Ok(OpOutcome::Created)
            }
            FileOp::Rename { from, to } => {
                let src = self.resolve(from)?;
                let dst = self.resolve(to)?;
                if let Some(parent) = dst.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::rename(&src, &dst)?;
                Ok(OpOutcome::Renamed)
            }
            FileOp::Move { from, to } => self
                .apply(&FileOp::Rename {
                    from: from.clone(),
                    to: to.clone(),
                })
                .map(|_| OpOutcome::Moved),
            FileOp::Delete { path } => {
                let p = self.resolve(path)?;
                if p.is_dir() {
                    std::fs::remove_dir_all(&p)?;
                } else if p.exists() {
                    std::fs::remove_file(&p)?;
                }
                Ok(OpOutcome::Deleted)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ops() -> (tempfile::TempDir, FileOps) {
        let dir = tempfile::tempdir().unwrap();
        let ops = FileOps::new(dir.path());
        (dir, ops)
    }

    #[test]
    fn create_rename_delete_cycle() {
        let (_d, ops) = ops();
        let o = ops
            .apply(&FileOp::CreateFile {
                path: "a.txt".into(),
                content: "hi".into(),
            })
            .unwrap();
        assert_eq!(o, OpOutcome::Created);
        // 重复创建报错
        assert!(ops
            .apply(&FileOp::CreateFile {
                path: "a.txt".into(),
                content: "".into()
            })
            .is_err());

        let o = ops
            .apply(&FileOp::Rename {
                from: "a.txt".into(),
                to: "b/b.txt".into(),
            })
            .unwrap();
        assert_eq!(o, OpOutcome::Renamed);
        assert!(!ops.root().join("a.txt").exists());
        assert!(ops.root().join("b/b.txt").exists());

        let o = ops.apply(&FileOp::Delete { path: "b".into() }).unwrap();
        assert_eq!(o, OpOutcome::Deleted);
        assert!(!ops.root().join("b").exists());
    }

    #[test]
    fn ops_confined_to_project() {
        let (_d, ops) = ops();
        assert!(ops
            .apply(&FileOp::Delete {
                path: "../../etc".into()
            })
            .is_err());
        assert!(ops
            .apply(&FileOp::CreateFile {
                path: "/tmp/evil".into(),
                content: "".into()
            })
            .is_err());
        assert!(ops
            .apply(&FileOp::Rename {
                from: "x".into(),
                to: "../y".into()
            })
            .is_err());
    }

    #[test]
    fn create_dir_nested() {
        let (_d, ops) = ops();
        ops.apply(&FileOp::CreateDir {
            path: "a/b/c".into(),
        })
        .unwrap();
        assert!(ops.root().join("a/b/c").is_dir());
    }
}
