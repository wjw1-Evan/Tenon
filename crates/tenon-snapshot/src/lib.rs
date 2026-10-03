//! 独立 shadow git 快照库（设计方案 §10.3，源码级参考 opencode `src/snapshot/`）。
//!
//! 设计要点：
//! - 快照库位于 `~/.tenon/snapshots/<project-id>/<worktree-hash>/`，与用户仓库完全隔离
//!   ——不写用户 `.git` 任何 refs / objects / index，不触发用户 hooks 与 gc（ADR-7）；
//! - git / 非 git 项目统一单路径：git 项目享受种子加速（`objects/info/alternates`
//!   指向用户对象库并复制其 index），非 git 项目首次全量 add；
//! - 快照点 = `git write-tree` 的 tree oid：无 ref 链、无 commit、天然去重；
//! - 尊重用户 `.gitignore`（`check-ignore --no-index` 动态判定，新增忽略项自动从
//!   快照索引移除）；未跟踪大文件（默认 >2MB）排除；
//! - 回滚双原语：按事件撤销 revert（三方合并保留用户手改）/ 整体恢复 restore；
//! - unrevert 由调用方实现：每次回滚前自动追加快照即可（本 crate 提供原语）。

use sha1::Digest;
use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use tempfile::tempdir;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SnapshotError {
    #[error("git 不可用或命令失败: {0}")]
    Git(String),
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),
    #[error("非法 UTF-8 输出")]
    Utf8,
}

pub type Result<T> = std::result::Result<T, SnapshotError>;

/// 按事件撤销时单文件结果（§10.3 / §8.6）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileRevertOutcome {
    /// 直接恢复到事件前版本。
    Restored,
    /// 三方合并：保留用户手改并撤销 AI 补丁。
    Merged,
    /// AI 新建文件随撤销删除。
    Deleted,
    /// 三方合并冲突——保持原样待用户处置。
    Conflict,
    /// 工作区与目标状态一致，无需处理。
    Unchanged,
}

#[derive(Debug, Clone, Default)]
pub struct RevertReport {
    pub per_file: Vec<(String, FileRevertOutcome)>,
}

impl RevertReport {
    pub fn has_conflicts(&self) -> bool {
        self.per_file
            .iter()
            .any(|(_, o)| *o == FileRevertOutcome::Conflict)
    }
}

/// 一个项目 × worktree 的 shadow git 快照库。同库操作串行（互斥锁，§10.3）。
pub struct SnapshotStore {
    /// shadow 仓库 git 目录（`<root>/.git`）。
    git_dir: PathBuf,
    /// shadow index 文件。
    index_file: PathBuf,
    /// shadow 目录（info/exclude 所在）。
    shadow_root: PathBuf,
    /// 被快照的工作区。
    work_tree: PathBuf,
    /// 未跟踪大文件排除阈值（字节）。
    max_untracked_bytes: u64,
    lock: Mutex<()>,
    empty_tree: std::sync::OnceLock<String>,
}

fn worktree_hash(workspace: &Path) -> String {
    // 路径别名防御：/var → /private/var、符号链接等统一为真实路径，
    // 保证「同一工作区」无论以何种路径形式打开都命中同一 shadow 库
    let canonical = std::fs::canonicalize(workspace).unwrap_or_else(|_| workspace.to_path_buf());
    let mut hasher = sha1::Sha1::new();
    hasher.update(canonical.to_string_lossy().as_bytes());
    hex::encode(hasher.finalize())[..16].to_string()
}

/// 用户仓库 git 目录（支持 worktree/submodule 的 `.git` 文件形式）。
fn user_git_dir(workspace: &Path) -> Option<PathBuf> {
    let dot_git = workspace.join(".git");
    if dot_git.is_dir() {
        return Some(dot_git);
    }
    if dot_git.is_file() {
        let content = std::fs::read_to_string(&dot_git).ok()?;
        let target = content.trim().strip_prefix("gitdir:")?.trim();
        let p = PathBuf::from(target);
        return Some(if p.is_absolute() {
            p
        } else {
            workspace.join(p)
        });
    }
    None
}

impl SnapshotStore {
    /// 打开（必要时初始化）shadow 快照库。
    pub fn open(
        snapshots_root: &Path,
        project_id: &str,
        workspace: &Path,
        max_untracked_mb: u64,
    ) -> Result<Self> {
        let shadow_root = snapshots_root
            .join(sanitize(project_id))
            .join(worktree_hash(workspace));
        let git_dir = shadow_root.join(".git");
        if !git_dir.exists() {
            std::fs::create_dir_all(&shadow_root)?;
            let root_str = shadow_root.to_string_lossy().into_owned();
            run_in(&shadow_root, &[], &["init", "-q", &root_str])?;
        }
        let index_file = shadow_root.join("index");
        let store = Self {
            git_dir,
            index_file,
            shadow_root,
            work_tree: workspace.to_path_buf(),
            max_untracked_bytes: max_untracked_mb * 1024 * 1024,
            lock: Mutex::new(()),
            empty_tree: std::sync::OnceLock::new(),
        };
        store.configure_and_seed()?;
        Ok(store)
    }

    pub fn work_tree(&self) -> &Path {
        &self.work_tree
    }

    fn configure_and_seed(&self) -> Result<()> {
        // 大仓库调优（§10.3，参考 opencode）
        let _ = self.git(&["config", "core.untrackedCache", "true"]);
        let _ = self.git(&["config", "index.version", "4"]);
        let _ = self.git(&["config", "gc.pruneExpire", "7.days.ago"]);
        let _ = self.git(&["config", "user.email", "tenon@local"]);
        let _ = self.git(&["config", "user.name", "tenon-snapshot"]);

        // 种子加速：git 项目 → alternates 指向用户对象库（含其 alternates 链）+ 复制 index
        if let Some(user_git) = user_git_dir(&self.work_tree) {
            let alt_path = self.git_dir.join("objects/info/alternates");
            if !alt_path.exists() {
                let user_objects = user_git.join("objects");
                if user_objects.exists() {
                    let mut lines = vec![user_objects.to_string_lossy().into_owned()];
                    let user_alts = user_objects.join("info/alternates");
                    if let Ok(content) = std::fs::read_to_string(&user_alts) {
                        for line in content.lines() {
                            let l = line.trim();
                            if l.is_empty() || l.starts_with('#') {
                                continue;
                            }
                            let p = PathBuf::from(l);
                            let abs = if p.is_absolute() {
                                p
                            } else {
                                user_objects.join(p)
                            };
                            lines.push(abs.to_string_lossy().into_owned());
                        }
                    }
                    if let Some(parent) = alt_path.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    std::fs::write(&alt_path, lines.join("\n") + "\n")?;
                }
            }
            // 复制用户 index（已哈希对象直接复用，首次快照近零成本，§10.3）
            let user_index = user_git.join("index");
            if user_index.exists() && !self.index_file.exists() {
                std::fs::copy(&user_index, &self.index_file)?;
            }
        }
        Ok(())
    }

    fn git(&self, args: &[&str]) -> Result<String> {
        self.git_with(args, None)
    }

    fn git_with(&self, args: &[&str], stdin: Option<&[u8]>) -> Result<String> {
        let mut cmd = Command::new("git");
        cmd.arg("--git-dir").arg(&self.git_dir);
        cmd.arg("--work-tree").arg(&self.work_tree);
        cmd.env("GIT_INDEX_FILE", &self.index_file);
        // 隔离环境：不继承系统 git 配置
        cmd.env("GIT_CONFIG_NOSYSTEM", "1");
        cmd.env("GIT_TERMINAL_PROMPT", "0");
        cmd.env("HOME", &self.shadow_root); // 隔离全局配置（includeIf 等）
        cmd.args(args);
        cmd.stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });
        cmd.stderr(Stdio::piped());
        cmd.stdout(Stdio::piped());
        let mut child = cmd.spawn().map_err(|e| SnapshotError::Git(e.to_string()))?;
        if let Some(input) = stdin {
            child
                .stdin
                .take()
                .expect("piped stdin")
                .write_all(input)
                .map_err(|e| SnapshotError::Git(e.to_string()))?;
        }
        let out = child
            .wait_with_output()
            .map_err(|e| SnapshotError::Git(e.to_string()))?;
        if !out.status.success() {
            return Err(SnapshotError::Git(format!(
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        String::from_utf8(out.stdout).map_err(|_| SnapshotError::Utf8)
    }

    /// 当前索引中被 ignore 的路径（check-ignore --no-index --stdin，§10.3）。
    fn ignored_cached_paths(&self) -> Result<Vec<String>> {
        let cached = self.git(&["ls-files", "-z"])?;
        if cached.is_empty() {
            return Ok(Vec::new());
        }
        match self.git_with(
            &["check-ignore", "--no-index", "--stdin", "-z"],
            Some(cached.as_bytes()),
        ) {
            Ok(s) => Ok(s
                .split('\0')
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect()),
            // 无任何忽略项时 check-ignore 退出码 1
            Err(_) => Ok(Vec::new()),
        }
    }

    /// 增量 add：已跟踪变更 + 未跟踪新增（`add -A`），随后
    /// ① 移除新增忽略项 ② 排除未跟踪大文件（§10.3）。
    fn stage_all(&self) -> Result<()> {
        // 未跟踪清单必须在 add -A 之前采集（add 后不再属于 --others）
        let untracked = if self.max_untracked_bytes > 0 {
            self.git(&["ls-files", "--others", "--exclude-standard", "-z"])?
        } else {
            String::new()
        };
        // 排除内核托管目录（shadow 库可能位于工作区内：绝对不自我快照，§10.3）
        self.git(&[
            "add",
            "-A",
            "--",
            ":(exclude).tenon",
            ":(exclude).tenon-snapshots",
            ":(exclude).git",
        ])?;
        // 新增忽略项自动从快照索引移除（§10.3 check-ignore --no-index 动态判定）
        let ignored = self.ignored_cached_paths()?;
        if !ignored.is_empty() {
            let mut stdin = Vec::new();
            for p in &ignored {
                stdin.extend_from_slice(p.as_bytes());
                stdin.push(0);
            }
            let _ = self.git_with(
                &["update-index", "--force-remove", "-z", "--stdin"],
                Some(&stdin),
            );
        }
        // 未跟踪大文件排除（仅出索引，不动工作区）
        if self.max_untracked_bytes > 0 {
            let mut stdin = Vec::new();
            for p in untracked.split('\0').filter(|s| !s.is_empty()) {
                let full = self.work_tree.join(p);
                if let Ok(meta) = std::fs::metadata(&full) {
                    if meta.is_file() && meta.len() > self.max_untracked_bytes {
                        stdin.extend_from_slice(p.as_bytes());
                        stdin.push(0);
                    }
                }
            }
            if !stdin.is_empty() {
                let _ = self.git_with(
                    &["update-index", "--force-remove", "-z", "--stdin"],
                    Some(&stdin),
                );
            }
        }
        Ok(())
    }

    /// 取一次快照：返回 tree oid（§10.3 快照点）。
    pub fn snapshot(&self) -> Result<String> {
        let _guard = self.lock.lock().expect("snapshot lock");
        // 种子懒执行：仓库可能在项目打开后才 git init（幂等：已有 alternates/index 即跳过）
        self.configure_and_seed()?;
        self.stage_all()?;
        let tree = self.git(&["write-tree"])?;
        Ok(tree.trim().to_string())
    }

    fn empty_tree(&self) -> Result<String> {
        if let Some(t) = self.empty_tree.get() {
            return Ok(t.clone());
        }
        let t = self.git(&["mktree"])?.trim().to_string();
        let _ = self.empty_tree.set(t.clone());
        Ok(t)
    }

    /// 两棵树之间的差异（name-status）：`(status, path)`，status ∈ A/M/D。
    pub fn diff_trees(&self, a: &str, b: Option<&str>) -> Result<Vec<(char, String)>> {
        let empty = if b.is_none() {
            self.empty_tree()?
        } else {
            String::new()
        };
        let b_owned: &str = match b {
            Some(x) => x,
            None => &empty,
        };
        let out = self.git(&[
            "diff-tree",
            "-r",
            "-z",
            "--name-status",
            "--no-commit-id",
            a,
            b_owned,
        ])?;
        let mut result = Vec::new();
        let mut parts = out.split('\0').filter(|s| !s.is_empty());
        while let (Some(status), Some(path)) = (parts.next(), parts.next()) {
            if let Some(c) = status.chars().next() {
                result.push((c, path.to_string()));
            }
        }
        Ok(result)
    }

    fn blob_of(&self, tree: &str, path: &str) -> Result<Option<String>> {
        match self.git(&["cat-file", "blob", &format!("{tree}:{path}")]) {
            Ok(s) => Ok(Some(s)),
            Err(_) => Ok(None),
        }
    }

    fn read_workspace_file(&self, path: &str) -> Option<String> {
        std::fs::read_to_string(self.work_tree.join(path)).ok()
    }

    fn write_workspace_file(&self, path: &str, content: &str) -> Result<()> {
        let full = self.work_tree.join(path);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&full, content)?;
        Ok(())
    }

    fn remove_workspace_file(&self, path: &str) -> Result<()> {
        let full = self.work_tree.join(path);
        if full.exists() {
            std::fs::remove_file(&full)?;
        }
        // 清理空目录（体验细节，忽略失败）
        if let Some(parent) = full.parent() {
            let _ = std::fs::remove_dir(parent);
        }
        Ok(())
    }

    fn stage_file(&self, path: &str) -> Result<()> {
        self.git(&["update-index", "--add", "--", path])?;
        Ok(())
    }

    fn unstage_file(&self, path: &str) -> Result<()> {
        let _ = self.git_with(
            &["update-index", "--force-remove", "-z", "--stdin"],
            Some(format!("{path}\0").as_bytes()),
        );
        Ok(())
    }

    /// 按事件撤销（§10.3 revert）：把 `files` 恢复到 `tree_before` 状态。
    ///
    /// - `tree_after`：该事件后的快照（三方合并基）；缺省用 `tree_before`；
    /// - 工作区与合并基一致 → 直接恢复（Restored）；
    /// - 工作区含用户手改 → `git merge-file` 三方合并保留（Merged / Conflict）；
    /// - `tree_before` 中不存在的文件 = AI 新建 → 删除（Deleted，§7.3）。
    pub fn revert_files(
        &self,
        tree_before: &str,
        tree_after: Option<&str>,
        files: &[String],
    ) -> Result<RevertReport> {
        let _guard = self.lock.lock().expect("revert lock");
        let after_tree = tree_after.unwrap_or(tree_before).to_string();
        let mut report = RevertReport::default();
        for file in files {
            let pre = self.blob_of(tree_before, file)?;
            match pre {
                None => {
                    // AI 新建文件：随撤销移除（§7.3）
                    if self.read_workspace_file(file).is_some() {
                        self.remove_workspace_file(file)?;
                        self.unstage_file(file)?;
                        report
                            .per_file
                            .push((file.clone(), FileRevertOutcome::Deleted));
                    } else {
                        report
                            .per_file
                            .push((file.clone(), FileRevertOutcome::Unchanged));
                    }
                }
                Some(pre_content) => {
                    let base = self
                        .blob_of(&after_tree, file)?
                        .unwrap_or_else(|| pre_content.clone());
                    let ours = self.read_workspace_file(file);
                    match ours {
                        None => {
                            self.write_workspace_file(file, &pre_content)?;
                            self.stage_file(file)?;
                            report
                                .per_file
                                .push((file.clone(), FileRevertOutcome::Restored));
                        }
                        Some(ours) if ours == base => {
                            self.write_workspace_file(file, &pre_content)?;
                            self.stage_file(file)?;
                            report
                                .per_file
                                .push((file.clone(), FileRevertOutcome::Restored));
                        }
                        Some(ours) => {
                            // 三方合并：base=事件后内容，ours=工作区（含手改），theirs=事件前
                            match merge_three_way(&base, &ours, &pre_content) {
                                MergeResult::Clean(merged) => {
                                    self.write_workspace_file(file, &merged)?;
                                    self.stage_file(file)?;
                                    report
                                        .per_file
                                        .push((file.clone(), FileRevertOutcome::Merged));
                                }
                                MergeResult::Conflict => {
                                    report
                                        .per_file
                                        .push((file.clone(), FileRevertOutcome::Conflict));
                                }
                            }
                        }
                    }
                }
            }
        }
        Ok(report)
    }

    /// 整体恢复（§10.3 restore，高级粒度）：`read-tree` + `checkout-index -a -f`
    /// 回到快照全量状态（含用户手改，预览警示由调用方负责）。
    /// 目标树中不存在的文件从工作区移除。
    pub fn restore(&self, tree: &str) -> Result<()> {
        let _guard = self.lock.lock().expect("restore lock");
        let old_list: BTreeSet<String> = self
            .git(&["ls-files", "-z"])?
            .split('\0')
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect();
        self.git(&["read-tree", tree])?;
        let new_list: BTreeSet<String> = self
            .git(&["ls-files", "-z"])?
            .split('\0')
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect();
        self.git(&["checkout-index", "-a", "-f"])?;
        for gone in old_list.difference(&new_list) {
            self.remove_workspace_file(gone)?;
        }
        Ok(())
    }

    /// 目标树中是否存在某文件。
    pub fn tree_contains(&self, tree: &str, path: &str) -> Result<bool> {
        Ok(self.blob_of(tree, path)?.is_some())
    }

    /// shadow 库定时 gc（§10.3：每小时后台，默认 prune 7 天）。
    pub fn gc(&self, keep_days: u32) -> Result<()> {
        let _guard = self.lock.lock().expect("gc lock");
        self.git(&["gc", "--quiet", &format!("--prune={keep_days}.days.ago")])?;
        Ok(())
    }
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

enum MergeResult {
    Clean(String),
    Conflict,
}

/// `git merge-file` 三方合并（§8.6）：返回合并结果或冲突标记。
fn merge_three_way(base: &str, ours: &str, theirs: &str) -> MergeResult {
    let dir = match tempdir() {
        Ok(d) => d,
        Err(_) => return MergeResult::Conflict,
    };
    let (bp, op, tp) = (
        dir.path().join("b"),
        dir.path().join("o"),
        dir.path().join("t"),
    );
    if std::fs::write(&bp, base).is_err()
        || std::fs::write(&op, ours).is_err()
        || std::fs::write(&tp, theirs).is_err()
    {
        return MergeResult::Conflict;
    }
    Command::new("git")
        .args(["merge-file", "-p", "-q"])
        .arg(&op)
        .arg(&bp)
        .arg(&tp)
        .stderr(Stdio::null())
        .stdout(Stdio::piped())
        .output()
        .map(|o| {
            // 退出码 0 = 干净合并；>0 = 冲突数（保留工作区原样）
            if o.status.success() {
                String::from_utf8(o.stdout).map_or(MergeResult::Conflict, MergeResult::Clean)
            } else {
                MergeResult::Conflict
            }
        })
        .unwrap_or(MergeResult::Conflict)
}

fn run_in(dir: &Path, env: &[(&str, &str)], args: &[&str]) -> Result<String> {
    let mut cmd = Command::new("git");
    cmd.current_dir(dir).args(args);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = cmd
        .output()
        .map_err(|e| SnapshotError::Git(e.to_string()))?;
    if !out.status.success() {
        return Err(SnapshotError::Git(
            String::from_utf8_lossy(&out.stderr).into_owned(),
        ));
    }
    String::from_utf8(out.stdout).map_err(|_| SnapshotError::Utf8)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup(name: &str) -> (tempfile::TempDir, tempfile::TempDir, SnapshotStore) {
        let data = tempfile::tempdir().expect("workspace dir");
        let snaps = tempfile::tempdir().expect("snapshots dir");
        let ws = data.path().join(name);
        std::fs::create_dir_all(&ws).unwrap();
        let store = SnapshotStore::open(snaps.path(), "test-project", &ws, 2).unwrap();
        (data, snaps, store)
    }

    fn write(ws: &Path, rel: &str, content: &str) {
        let p = ws.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(p, content).unwrap();
    }

    fn read(ws: &Path, rel: &str) -> String {
        std::fs::read_to_string(ws.join(rel)).unwrap()
    }

    #[test]
    fn snapshot_returns_tree_oid_and_dedupes() {
        let (_d, _s, store) = setup("ws1");
        write(store.work_tree(), "a.txt", "hello\n");
        let t1 = store.snapshot().unwrap();
        assert_eq!(t1.len(), 40, "tree oid");
        assert!(store.tree_contains(&t1, "a.txt").unwrap());
        // 无变化 → 同一 tree oid（天然去重）
        let t2 = store.snapshot().unwrap();
        assert_eq!(t1, t2);
        // 变化 → 新 oid
        write(store.work_tree(), "a.txt", "changed\n");
        let t3 = store.snapshot().unwrap();
        assert_ne!(t1, t3);
    }

    #[test]
    fn restore_returns_workspace_to_full_snapshot() {
        let (_d, _s, store) = setup("ws2");
        write(store.work_tree(), "a.txt", "v1\n");
        write(store.work_tree(), "src/b.txt", "b1\n");
        let t1 = store.snapshot().unwrap();

        write(store.work_tree(), "a.txt", "v2\n");
        write(store.work_tree(), "new.txt", "created\n");
        std::fs::remove_file(store.work_tree().join("src/b.txt")).unwrap();
        let t2 = store.snapshot().unwrap();
        assert_ne!(t1, t2);

        store.restore(&t1).unwrap();
        assert_eq!(read(store.work_tree(), "a.txt"), "v1\n");
        assert_eq!(read(store.work_tree(), "src/b.txt"), "b1\n");
        assert!(
            !store.work_tree().join("new.txt").exists(),
            "快照中不存在的新文件被移除"
        );
    }

    #[test]
    fn diff_trees_lists_changes() {
        let (_d, _s, store) = setup("ws3");
        write(store.work_tree(), "a.txt", "1\n");
        let t1 = store.snapshot().unwrap();
        write(store.work_tree(), "a.txt", "2\n");
        write(store.work_tree(), "b.txt", "new\n");
        let t2 = store.snapshot().unwrap();
        let diffs = store.diff_trees(&t1, Some(&t2)).unwrap();
        assert!(diffs.contains(&('M', "a.txt".to_string())));
        assert!(diffs.contains(&('A', "b.txt".to_string())));
    }

    #[test]
    fn respects_gitignore_dynamically() {
        let (_d, _s, store) = setup("ws4");
        write(store.work_tree(), "keep.txt", "keep\n");
        write(store.work_tree(), "secret.log", "noise\n");
        write(store.work_tree(), ".gitignore", "*.log\n");
        let t = store.snapshot().unwrap();
        assert!(store.tree_contains(&t, "keep.txt").unwrap());
        assert!(
            !store.tree_contains(&t, "secret.log").unwrap(),
            ".gitignore 生效"
        );

        // 新增忽略项：之前已快照的文件自动从快照索引移除（§10.3）
        write(store.work_tree(), "tracked.txt", "was tracked\n");
        let t2 = store.snapshot().unwrap();
        assert!(store.tree_contains(&t2, "tracked.txt").unwrap());
        let gi = store.work_tree().join(".gitignore");
        let mut gi_content = std::fs::read_to_string(&gi).unwrap();
        gi_content.push_str("tracked.txt\n");
        std::fs::write(&gi, gi_content).unwrap();
        let t3 = store.snapshot().unwrap();
        assert!(
            !store.tree_contains(&t3, "tracked.txt").unwrap(),
            "新增忽略项自动从快照索引移除"
        );
    }

    #[test]
    fn big_untracked_files_excluded() {
        let (_d, _s, store) = setup("ws5");
        write(store.work_tree(), "small.txt", "ok\n");
        // 3MB > 默认 2MB 阈值
        let big = "x".repeat(3 * 1024 * 1024);
        write(store.work_tree(), "big.bin", &big);
        let t = store.snapshot().unwrap();
        assert!(store.tree_contains(&t, "small.txt").unwrap());
        assert!(
            !store.tree_contains(&t, "big.bin").unwrap(),
            "未跟踪大文件排除"
        );
        // 工作区文件不受影响
        assert!(store.work_tree().join("big.bin").exists());
    }

    #[test]
    fn revert_deletes_ai_created_files() {
        let (_d, _s, store) = setup("ws6");
        write(store.work_tree(), "base.txt", "base\n");
        let t_before = store.snapshot().unwrap();

        write(store.work_tree(), "ai_new.txt", "ai\n");
        let t_after = store.snapshot().unwrap();

        let report = store
            .revert_files(&t_before, Some(&t_after), &["ai_new.txt".to_string()])
            .unwrap();
        assert_eq!(report.per_file[0].1, FileRevertOutcome::Deleted);
        assert!(
            !store.work_tree().join("ai_new.txt").exists(),
            "AI 新建文件随撤销移除"
        );
        assert_eq!(read(store.work_tree(), "base.txt"), "base\n");
    }

    #[test]
    fn revert_restores_ai_edits_and_keeps_disjoint_user_edits() {
        let (_d, _s, store) = setup("ws7");
        let original = "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\n";
        write(store.work_tree(), "f.txt", original);
        let t_before = store.snapshot().unwrap();

        // AI 改第 2 行
        let ai = "line1\nAI-EDIT\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\n";
        write(store.work_tree(), "f.txt", ai);
        let t_after = store.snapshot().unwrap();

        // 用户随后在同一文件追加一行（与 AI 改动不相交）
        let with_user = format!("{ai}USER-APPEND\n");
        write(store.work_tree(), "f.txt", &with_user);

        let report = store
            .revert_files(&t_before, Some(&t_after), &["f.txt".to_string()])
            .unwrap();
        assert_eq!(
            report.per_file[0].1,
            FileRevertOutcome::Merged,
            "用户手改走三方合并保留"
        );
        let content = read(store.work_tree(), "f.txt");
        assert!(content.contains("line2\n"), "AI 改动被撤销回 line2");
        assert!(!content.contains("AI-EDIT"), "AI 编辑被移除");
        assert!(content.contains("USER-APPEND"), "用户手改被保留");
    }

    #[test]
    fn revert_conflict_leaves_file_untouched() {
        let (_d, _s, store) = setup("ws8");
        let original = "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\n";
        write(store.work_tree(), "f.txt", original);
        let t_before = store.snapshot().unwrap();

        write(
            store.work_tree(),
            "f.txt",
            "line1\nAI-EDIT\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\n",
        );
        let t_after = store.snapshot().unwrap();

        // 用户改了同一行 → 冲突
        write(
            store.work_tree(),
            "f.txt",
            "line1\nUSER-SAME-LINE\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\n",
        );

        let report = store
            .revert_files(&t_before, Some(&t_after), &["f.txt".to_string()])
            .unwrap();
        assert_eq!(report.per_file[0].1, FileRevertOutcome::Conflict);
        assert!(report.has_conflicts());
        assert_eq!(
            read(store.work_tree(), "f.txt"),
            "line1\nUSER-SAME-LINE\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\n",
            "冲突时工作区保持原样待用户处置"
        );
    }

    #[test]
    fn git_project_seeds_alternates() {
        let (_d, _s, store) = setup("ws9");
        let ws = store.work_tree();
        run_in(ws, &[], &["init", "-q", "."]).unwrap();
        write(ws, "code.rs", "fn main() {}\n");
        run_in(ws, &[], &["add", "."]).unwrap();
        run_in(
            ws,
            &[],
            &[
                "-c",
                "user.email=t@t",
                "-c",
                "user.name=t",
                "commit",
                "-qm",
                "init",
            ],
        )
        .unwrap();

        let t = store.snapshot().unwrap();
        assert!(store.tree_contains(&t, "code.rs").unwrap());

        let alt = store.shadow_root.join(".git/objects/info/alternates");
        let content = std::fs::read_to_string(&alt).unwrap();
        assert!(content.contains(ws.join(".git/objects").to_string_lossy().as_ref()));
    }

    #[test]
    fn snapshots_of_git_repo_reuse_seeded_index() {
        let (_d, _s, store) = setup("ws10");
        let ws = store.work_tree();
        run_in(ws, &[], &["init", "-q", "."]).unwrap();
        write(ws, "a.txt", "seeded\n");
        run_in(ws, &[], &["add", "."]).unwrap();
        run_in(
            ws,
            &[],
            &[
                "-c",
                "user.email=t@t",
                "-c",
                "user.name=t",
                "commit",
                "-qm",
                "init",
            ],
        )
        .unwrap();

        // 种子 index 生效：未修改时首快照应与用户 HEAD 树一致（近零成本）
        let user_tree = run_in(ws, &[], &["write-tree"]).unwrap().trim().to_string();
        let snap_tree = store.snapshot().unwrap();
        assert_eq!(user_tree, snap_tree, "种子 index + alternates 生效");
    }

    #[test]
    fn gc_runs_without_error() {
        let (_d, _s, store) = setup("ws11");
        write(store.work_tree(), "a.txt", "1\n");
        store.snapshot().unwrap();
        store.gc(7).unwrap();
    }

    #[test]
    fn unrevert_primitives_roundtrip() {
        // §10.3 unrevert：回滚前自动追加快照 → 可一键撤销最近回滚
        let (_d, _s, store) = setup("ws12");
        write(store.work_tree(), "f.txt", "v1\n");
        let t1 = store.snapshot().unwrap();
        write(store.work_tree(), "f.txt", "v2\n");
        let t2 = store.snapshot().unwrap();

        // 回滚前先快照（调用方约定）
        let safety = store.snapshot().unwrap();
        assert_eq!(safety, t2);
        store.restore(&t1).unwrap();
        assert_eq!(read(store.work_tree(), "f.txt"), "v1\n");
        // unrevert = restore 到回滚前快照
        store.restore(&safety).unwrap();
        assert_eq!(read(store.work_tree(), "f.txt"), "v2\n");
    }
}
