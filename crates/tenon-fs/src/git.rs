//! git 状态装饰（设计方案 §8.1：文件树 git 状态装饰；只读 git 查询）。

use crate::tree::GitStatus;
use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use std::process::Stdio;

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
        // 重命名/复制是两条记录：`XY <new>\0<orig>\0`——entry 余量即新路径，
        // 下一条记录是原路径（消费掉，否则后续条目整体错位）
        if xy.contains('R') || xy.contains('C') {
            let _orig = parts.next();
        }
        let path = rest.trim_start().to_string();
        let status = match xy.trim() {
            "M" | "AM" | "MM" => GitStatus::Modified,
            "A" => GitStatus::Added,
            "D" | "AD" => GitStatus::Deleted,
            "R" | "C" => GitStatus::Renamed,
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

fn git_output(root: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[derive(Debug, Clone, Serialize)]
pub struct GitBranch {
    pub name: String,
    pub current: bool,
    pub commit: String,
    pub upstream: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GitChangedFile {
    pub path: String,
    pub old_path: Option<String>,
    pub index_status: String,
    pub worktree_status: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct GitCommit {
    pub id: String,
    pub short_id: String,
    pub summary: String,
    pub author: String,
    pub email: String,
    pub timestamp: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct GitBlameLine {
    pub line: u32,
    pub commit: String,
    pub author: String,
    pub email: String,
    pub timestamp: i64,
    pub summary: String,
    pub content: String,
}

pub fn is_repository(root: &Path) -> bool {
    git_output(root, &["rev-parse", "--is-inside-work-tree"])
        .map(|value| value.trim() == "true")
        .unwrap_or(false)
}

pub fn current_branch(root: &Path) -> String {
    git_output(root, &["symbolic-ref", "--short", "HEAD"])
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "HEAD".to_string())
}

pub fn list_branches(root: &Path) -> Vec<GitBranch> {
    let Some(text) = git_output(
        root,
        &[
            "for-each-ref",
            "--format=%(refname:short)%09%(objectname:short)%09%(upstream:short)%09%(HEAD)",
            "refs/heads",
        ],
    ) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            Some(GitBranch {
                name: fields.next()?.trim().to_string(),
                commit: fields.next().unwrap_or_default().trim().to_string(),
                upstream: fields
                    .next()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string),
                current: fields.next().map(str::trim) == Some("*"),
            })
        })
        .collect()
}

pub fn changed_files(root: &Path) -> Vec<GitChangedFile> {
    let Some(text) = git_output(
        root,
        &["status", "--porcelain=v1", "-z", "--untracked-files=normal"],
    ) else {
        return Vec::new();
    };
    let mut files = Vec::new();
    let mut parts = text.split('\0').filter(|entry| !entry.is_empty());
    while let Some(entry) = parts.next() {
        if entry.len() < 4 {
            continue;
        }
        let (statuses, remainder) = entry.split_at(2);
        let mut index_status = statuses.chars().next().unwrap_or(' ').to_string();
        let mut worktree_status = statuses.chars().nth(1).unwrap_or(' ').to_string();
        if index_status == "?" {
            index_status = "A".to_string();
            worktree_status = "A".to_string();
        }
        if statuses.contains('R') || statuses.contains('C') {
            // `XY <new>\0<orig>\0`：entry 余量为新路径，下一条记录是原路径；
            // 旧解析按三条记录消费会把下一条的状态行当路径，逐条错乱
            let old_path = parts.next().map(|p| p.trim().replace('\\', "/"));
            files.push(GitChangedFile {
                path: remainder.trim_start().replace('\\', "/"),
                old_path,
                index_status,
                worktree_status,
            });
        } else {
            files.push(GitChangedFile {
                path: remainder.trim_start().replace('\\', "/"),
                old_path: None,
                index_status,
                worktree_status,
            });
        }
    }
    files
}

pub fn recent_commits(root: &Path, limit: u32) -> Vec<GitCommit> {
    let Some(text) = git_output(
        root,
        &[
            "log",
            &format!("--max-count={limit}"),
            "--date-order",
            "--pretty=format:%H%x09%h%x09%s%x09%an%x09%ae%x09%ct",
        ],
    ) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split('\t').collect();
            Some(GitCommit {
                id: fields.first()?.to_string(),
                short_id: fields.get(1)?.to_string(),
                summary: fields.get(2)?.to_string(),
                author: fields.get(3)?.to_string(),
                email: fields.get(4)?.to_string(),
                timestamp: fields.get(5)?.parse().ok()?,
            })
        })
        .collect()
}

pub fn blame_file(root: &Path, path: &str) -> Option<Vec<GitBlameLine>> {
    if path.split('/').any(|part| part == "..") || path.starts_with('/') {
        return None;
    }
    let text = git_output(root, &["blame", "--line-porcelain", "--", path])?;
    let mut lines = Vec::new();
    let mut current: Option<GitBlameLine> = None;
    let mut seen_commit = std::collections::HashMap::new();
    for line in text.lines() {
        if let Some(content) = line.strip_prefix('\t') {
            if let Some(mut item) = current.take() {
                item.content = content.to_string();
                lines.push(item);
            }
            continue;
        }
        let mut fields = line.split_whitespace();
        let Some(commit) = fields.next() else {
            continue;
        };
        let looks_like_header = commit.len() == 40
            && commit.chars().all(|c| c.is_ascii_hexdigit())
            && fields
                .clone()
                .next()
                .and_then(|value| value.parse::<u32>().ok())
                .is_some();
        if !looks_like_header {
            if let Some(author) = line.strip_prefix("author ") {
                if let Some(item) = current.as_mut() {
                    item.author = author.to_string();
                }
            } else if let Some(email) = line.strip_prefix("author-mail ") {
                if let Some(item) = current.as_mut() {
                    item.email = email
                        .trim_start_matches('<')
                        .trim_end_matches('>')
                        .to_string();
                }
            } else if let Some(timestamp) = line.strip_prefix("author-time ") {
                if let Some(item) = current.as_mut() {
                    item.timestamp = timestamp.parse().unwrap_or(0);
                }
            } else if let Some(summary) = line.strip_prefix("summary ") {
                if let Some(item) = current.as_mut() {
                    item.summary = summary.to_string();
                }
            }
            continue;
        }
        let mut item = GitBlameLine {
            line: 0,
            commit: commit.to_string(),
            author: seen_commit
                .get(commit)
                .map(|meta: &(String, String, i64, String)| meta.0.clone())
                .unwrap_or_default(),
            email: seen_commit
                .get(commit)
                .map(|meta: &(String, String, i64, String)| meta.1.clone())
                .unwrap_or_default(),
            timestamp: seen_commit
                .get(commit)
                .map(|meta: &(String, String, i64, String)| meta.2)
                .unwrap_or_default(),
            summary: seen_commit
                .get(commit)
                .map(|meta: &(String, String, i64, String)| meta.3.clone())
                .unwrap_or_default(),
            content: String::new(),
        };
        if let Some(final_line) = fields.next().and_then(|value| value.parse().ok()) {
            item.line = final_line;
        }
        seen_commit.insert(
            commit.to_string(),
            (
                item.author.clone(),
                item.email.clone(),
                item.timestamp,
                item.summary.clone(),
            ),
        );
        current = Some(item);
    }
    Some(lines)
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
    fn rename_records_key_new_path_and_do_not_shift_later_entries() {
        let (_d, root) = git_repo();
        std::fs::write(root.join("old.txt"), "v1\n").unwrap();
        run(&root, &["add", "."]);
        run(&root, &["commit", "-qm", "init"]);
        // staged rename + 其后跟一条普通修改：旧解析把重命名按错边消费，
        // 会导致后续条目整体错位
        run(&root, &["mv", "old.txt", "new.txt"]);
        std::fs::write(root.join("later.txt"), "later\n").unwrap();

        let map = status_map(&root);
        assert_eq!(
            map.get("new.txt"),
            Some(&GitStatus::Renamed),
            "重命名后新路径应显示 Renamed，实际 {map:?}"
        );
        assert!(!map.contains_key("old.txt"), "原路径不应作为键");
        assert_eq!(map.get("later.txt"), Some(&GitStatus::Untracked));

        let files = changed_files(&root);
        let renamed = files
            .iter()
            .find(|f| f.path == "new.txt")
            .expect("changed_files 应含新路径");
        assert_eq!(renamed.old_path.as_deref(), Some("old.txt"));
        assert!(
            files.iter().any(|f| f.path == "later.txt"),
            "重命名其后的条目不得错位丢失：{files:?}"
        );
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

    #[test]
    fn source_view_branches_changes_commits_and_blame() {
        let (_d, root) = git_repo();
        run(&root, &["checkout", "-qb", "feature/source"]);
        std::fs::write(root.join("source.txt"), "first\nsecond\n").unwrap();
        run(&root, &["add", "."]);
        run(&root, &["commit", "-qm", "add source view fixture"]);

        assert!(is_repository(&root));
        let branches = list_branches(&root);
        assert!(branches
            .iter()
            .any(|branch| branch.name == "feature/source" && branch.current));

        std::fs::write(root.join("changed.txt"), "working\n").unwrap();
        let changes = changed_files(&root);
        assert!(changes
            .iter()
            .any(|file| file.path == "changed.txt" && file.worktree_status == "A"));

        let commits = recent_commits(&root, 10);
        assert!(commits
            .iter()
            .any(|commit| commit.summary == "add source view fixture"));

        let blame = blame_file(&root, "source.txt").expect("blame");
        assert_eq!(blame.len(), 2);
        assert!(
            blame.iter().all(|line| !line.author.is_empty()),
            "{blame:?}"
        );
        assert!(blame.iter().all(|line| line.author == "t"));
        assert!(blame
            .iter()
            .all(|line| line.summary == "add source view fixture"));
        assert_eq!(blame[1].content, "second");

        assert!(blame_file(&root, "../outside.txt").is_none());
    }
}
