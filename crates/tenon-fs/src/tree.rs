//! 文件树（设计方案 §8.1 / §15 `GET /project/:id/tree`）。

use crate::{FsError, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    Dir,
    File,
}

/// git 状态装饰（§8.1）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GitStatus {
    Modified,
    Added,
    Deleted,
    Renamed,
    Untracked,
    Ignored,
    Clean,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreeEntry {
    /// 项目相对路径（POSIX 风格）。
    pub path: String,
    pub name: String,
    pub kind: EntryKind,
    pub git_status: GitStatus,
}

/// 列出某目录一层内容（排序：目录在前，名称升序）。
pub fn list_dir(
    root: &Path,
    rel: &str,
    statuses: &HashMap<String, GitStatus>,
) -> Result<Vec<TreeEntry>> {
    let dir = if rel.is_empty() || rel == "." {
        root.to_path_buf()
    } else {
        root.join(rel)
    };
    if !dir.is_dir() {
        return Err(FsError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("not a directory: {rel}"),
        )));
    }
    let mut entries = Vec::new();
    for item in std::fs::read_dir(&dir)? {
        let item = item?;
        let name = item.file_name().to_string_lossy().into_owned();
        if name == ".git" || name == ".DS_Store" {
            continue;
        }
        let ft = item.file_type()?;
        let rel_path = if rel.is_empty() || rel == "." {
            name.clone()
        } else {
            format!("{rel}/{name}")
        };
        let git_status = statuses.get(&rel_path).copied().unwrap_or(GitStatus::Clean);
        entries.push(TreeEntry {
            path: rel_path,
            name,
            kind: if ft.is_dir() {
                EntryKind::Dir
            } else {
                EntryKind::File
            },
            git_status,
        });
    }
    entries.sort_by(|a, b| {
        let ka = if a.kind == EntryKind::Dir { 0 } else { 1 };
        let kb = if b.kind == EntryKind::Dir { 0 } else { 1 };
        ka.cmp(&kb)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(entries)
}

/// 列出某目录的直接子目录（§15 `GET /fs/dirs`，v1.133 目录选择器数据源）：
/// 只含目录——符号链接经 metadata 跟随判定（指向目录的链接入选，断链跳过），
/// 跳过 `.git` / `.DS_Store`，名称不区分大小写升序。
pub fn list_subdirs(dir: &Path) -> Result<Vec<String>> {
    if !dir.is_dir() {
        return Err(FsError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("not a directory: {}", dir.display()),
        )));
    }
    let mut names = Vec::new();
    for item in std::fs::read_dir(dir)? {
        let item = item?;
        let name = item.file_name().to_string_lossy().into_owned();
        if name == ".git" || name == ".DS_Store" {
            continue;
        }
        // DirEntry::metadata 是 lstat 语义（不跟随符号链接），这里须用
        // std::fs::metadata 跟随判定——目录选择器需要看见指向目录的符号链接。
        if std::fs::metadata(item.path())
            .map(|m| m.is_dir())
            .unwrap_or(false)
        {
            names.push(name);
        }
    }
    names.sort_by_key(|a| a.to_lowercase());
    Ok(names)
}

/// 递归全树（忽略 .gitignore 与 .git；UI 侧也可按层懒加载）。
pub fn full_tree(root: &Path) -> Result<Vec<TreeEntry>> {
    let mut out = Vec::new();
    let walker = ignore::WalkBuilder::new(root)
        .hidden(false)
        .require_git(false)
        .filter_entry(|e| !crate::search::within_dot_git(e.path()))
        .build();
    for item in walker {
        let entry = item.map_err(|e| FsError::Io(std::io::Error::other(e.to_string())))?;
        let path = entry.path();
        if path == root {
            continue;
        }
        let rel = path
            .strip_prefix(root)
            .map_err(|_| FsError::BadPath(path.to_string_lossy().into_owned()))?
            .to_string_lossy()
            .replace('\\', "/");
        let name = entry.file_name().to_string_lossy().into_owned();
        let ft = entry.file_type();
        out.push(TreeEntry {
            path: rel,
            name,
            kind: if ft.map(|f| f.is_dir()).unwrap_or(false) {
                EntryKind::Dir
            } else {
                EntryKind::File
            },
            git_status: GitStatus::Clean,
        });
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (
        tempfile::TempDir,
        std::collections::HashMap<String, GitStatus>,
    ) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/main.rs"), "fn main() {}\n").unwrap();
        std::fs::write(dir.path().join("README.md"), "# t\n").unwrap();
        (dir, std::collections::HashMap::new())
    }

    #[test]
    fn list_dir_sorts_dirs_first() {
        let (d, statuses) = setup();
        let entries = list_dir(d.path(), "", &statuses).unwrap();
        assert_eq!(entries[0].kind, EntryKind::Dir);
        assert_eq!(entries[0].name, "src");
        assert!(entries
            .iter()
            .any(|e| e.name == "README.md" && e.kind == EntryKind::File));
    }

    #[test]
    fn list_dir_decorates_git_status() {
        let (d, _) = setup();
        let mut statuses = std::collections::HashMap::new();
        statuses.insert("src/main.rs".to_string(), GitStatus::Modified);
        let entries = list_dir(d.path(), "src", &statuses).unwrap();
        assert_eq!(entries[0].git_status, GitStatus::Modified);
    }

    #[test]
    fn list_subdirs_only_dirs_following_symlinks() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("zz")).unwrap();
        std::fs::create_dir_all(d.path().join("AA")).unwrap();
        std::fs::create_dir_all(d.path().join(".git")).unwrap();
        std::fs::write(d.path().join("file.txt"), "x").unwrap();
        std::fs::write(d.path().join(".DS_Store"), "x").unwrap();
        let target = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(d.path().join("zz"), d.path().join("dir_link")).unwrap();
            std::os::unix::fs::symlink(d.path().join("nope"), d.path().join("broken")).unwrap();
        }
        let names = list_subdirs(d.path()).unwrap();
        // 名称不区分大小写升序；文件 / .git / .DS_Store / 断链不进，指向目录的符号链接进
        #[cfg(unix)]
        assert_eq!(
            names,
            vec!["AA".to_string(), "dir_link".to_string(), "zz".to_string()]
        );
        #[cfg(not(unix))]
        assert_eq!(names, vec!["AA".to_string(), "zz".to_string()]);
        let _ = target; // 保活符号链接目标
    }

    #[test]
    fn list_subdirs_rejects_non_directory() {
        let d = tempfile::tempdir().unwrap();
        let file = d.path().join("f.txt");
        std::fs::write(&file, "x").unwrap();
        assert!(list_subdirs(&file).is_err());
        assert!(list_subdirs(&d.path().join("missing")).is_err());
    }

    #[test]
    fn full_tree_respects_gitignore_and_skips_git_dir() {
        let (d, _) = setup();
        std::fs::write(d.path().join(".gitignore"), "*.log\n").unwrap();
        std::fs::write(d.path().join("noise.log"), "x").unwrap();
        std::fs::create_dir_all(d.path().join(".git")).unwrap();
        let tree = full_tree(d.path()).unwrap();
        let paths: Vec<&str> = tree.iter().map(|e| e.path.as_str()).collect();
        assert!(paths.contains(&".gitignore"));
        assert!(paths.contains(&"src/main.rs"));
        assert!(!paths.contains(&"noise.log"), ".gitignore 生效");
        assert!(
            !paths.iter().any(|p| *p == ".git" || p.starts_with(".git/")),
            ".git 不进树"
        );
    }
}
