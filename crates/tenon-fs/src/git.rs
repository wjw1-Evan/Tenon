//! git 状态装饰（设计方案 §8.1：文件树 git 状态装饰；只读 git 查询）。

use crate::tree::GitStatus;
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

/// `git status --porcelain=v1 -z` → 相对路径状态映射。
/// 非 git 项目返回空映射。
pub fn status_map(root: &Path) -> HashMap<String, GitStatus> {
    let mut map = HashMap::new();
    let out = match Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["status", "--porcelain=v1", "-z", "--untracked-files=normal"])
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return map,
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let mut parts = text.split('\0').filter(|s| !s.is_empty());
    while let Some(entry) = parts.next() {
        if entry.len() < 4 {
            continue;
        }
        let (xy, rest) = entry.split_at(2);
        // rename 记录为 "R  old\0new\0"
        let path = if xy.contains('R') {
            match parts.next() {
                Some(new) => new.trim_start().to_string(),
                None => rest.trim_start().to_string(),
            }
        } else {
            rest.trim_start().to_string()
        };
        let status = match xy.trim() {
            "M" | "AM" | "MM" => GitStatus::Modified,
            "A" => GitStatus::Added,
            "D" | "AD" => GitStatus::Deleted,
            "R" => GitStatus::Renamed,
            "??" => GitStatus::Untracked,
            "!!" => GitStatus::Ignored,
            _ => GitStatus::Modified,
        };
        map.insert(path.replace('\\', "/"), status);
    }
    map
}

/// 只读 git 查询：status / log / diff（代理 A 级 `git_read` 的底层）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitReadKind {
    Status,
    Log,
    Diff,
}

/// 执行只读 git 查询（不带任何写参数；输出经调用方脱敏）。
pub fn git_read(root: &Path, kind: GitReadKind, extra: &[&str]) -> std::io::Result<String> {
    let sub = match kind {
        GitReadKind::Status => vec!["status".to_string(), "--porcelain=v1".to_string()],
        GitReadKind::Log => vec![
            "log".to_string(),
            "--oneline".to_string(),
            "-n".into(),
            "20".into(),
        ],
        GitReadKind::Diff => vec!["diff".to_string(), "HEAD".to_string()],
    };
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(&sub)
        .args(extra)
        .output()?;
    if !out.status.success() {
        return Ok(String::new());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git_repo() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        run(&root, &["init", "-q", "."]);
        run(&root, &["config", "user.email", "t@t"]);
        run(&root, &["config", "user.name", "t"]);
        (dir, root)
    }

    fn run(root: &Path, args: &[&str]) {
        let _ = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .unwrap();
    }

    #[test]
    fn status_map_detects_modifications_and_untracked() {
        let (_d, root) = git_repo();
        std::fs::write(root.join("tracked.txt"), "v1\n").unwrap();
        run(&root, &["add", "."]);
        run(&root, &["commit", "-qm", "init"]);

        std::fs::write(root.join("tracked.txt"), "v2\n").unwrap();
        std::fs::write(root.join("new.txt"), "x\n").unwrap();

        let map = status_map(&root);
        assert_eq!(map.get("tracked.txt"), Some(&GitStatus::Modified));
        assert_eq!(map.get("new.txt"), Some(&GitStatus::Untracked));
    }

    #[test]
    fn non_git_dir_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert!(status_map(dir.path()).is_empty());
    }

    #[test]
    fn git_read_kinds() {
        let (_d, root) = git_repo();
        std::fs::write(root.join("a.txt"), "hello\n").unwrap();
        run(&root, &["add", "."]);
        run(&root, &["commit", "-qm", "init"]);
        std::fs::write(root.join("a.txt"), "changed\n").unwrap();

        let status = git_read(&root, GitReadKind::Status, &[]).unwrap();
        assert!(status.contains("a.txt"));
        let log = git_read(&root, GitReadKind::Log, &[]).unwrap();
        assert!(log.contains("init"));
        let diff = git_read(&root, GitReadKind::Diff, &[]).unwrap();
        assert!(diff.contains("changed"));
    }
}
