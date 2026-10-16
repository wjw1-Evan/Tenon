//! 全局搜索与替换预览（设计方案 §8.1：ripgrep 驱动；替换前返回 diff 预览）。

use crate::{FsError, Result};
use grep_regex::RegexMatcher;
use grep_searcher::{BinaryDetection, Searcher, SearcherBuilder, Sink, SinkMatch};
use ignore::WalkState;
use regex::Regex;
use serde::{Deserialize, Serialize};
use similar::TextDiff;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchHit {
    pub path: String,
    pub line: u64,
    pub column: u64,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct SearchOptions {
    /// 结果上限（防超大盘点）。
    pub max_hits: usize,
    /// 单文件字节上限（跳过大文件）。
    pub max_file_bytes: u64,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            max_hits: 1000,
            max_file_bytes: 4 * 1024 * 1024,
        }
    }
}

/// 单文件命中收集（路径由外层提供，规避 SinkMatch 路径借用）。
struct LineSink<'a> {
    regex: &'a Regex,
    hits: &'a mut Vec<(u64, u64, String)>,
    cap: usize,
}

impl Sink for LineSink<'_> {
    type Error = std::io::Error;

    fn matched(
        &mut self,
        _s: &Searcher,
        m: &SinkMatch<'_>,
    ) -> std::result::Result<bool, Self::Error> {
        if self.hits.len() >= self.cap {
            return Ok(false);
        }
        let text = String::from_utf8_lossy(m.bytes())
            .trim_end_matches(['\n', '\r'])
            .to_string();
        let column = self
            .regex
            .find(&text)
            .map(|span| text[..span.start()].chars().count() as u64 + 1)
            .unwrap_or(1);
        self.hits.push((m.line_number().unwrap_or(0), column, text));
        Ok(self.hits.len() < self.cap)
    }
}

/// 正则全局搜索（尊重 .gitignore；跳过二进制 / 大文件）。
///
/// 并行遍历（rg 同款 `WalkParallel`）：文件扫描本就是各文件独立的 CPU +
/// IO 混合负载，单线程 walker 在多核机上把搜索时长钉死在核数之外。命中
/// 达上限即全局退出。结果按（路径，行，列）稳定排序：组内行序升序是
/// UI「打开首个命中」跳转的语义前提，全局顺序从目录枚举序收敛为路径
/// 字典序（并行下枚举序本就不可依赖，排序后跨平台可复现）。
pub fn search(root: &Path, pattern: &str, opts: &SearchOptions) -> Result<Vec<SearchHit>> {
    let matcher = Arc::new(
        RegexMatcher::new(pattern)
            .map_err(|e| FsError::Io(std::io::Error::other(e.to_string())))?,
    );
    let line_regex = Arc::new(
        Regex::new(pattern)
            .ok()
            .unwrap_or_else(|| NEVER_REGEX.clone()),
    );
    let hits = Mutex::new(Vec::new());
    let matched = AtomicUsize::new(0);
    // 外层 FnMut 会被多次调用：共享状态以引用形式进闭包（move 内层只复制引用）
    let hits_shared = &hits;
    let matched_shared = &matched;

    ignore::WalkBuilder::new(root)
        .hidden(false)
        .require_git(false)
        .filter_entry(|e| !within_dot_git(e.path()))
        .max_filesize(Some(opts.max_file_bytes))
        .build_parallel()
        .run(|| {
            let matcher = matcher.clone();
            let line_regex = line_regex.clone();
            let mut searcher = SearcherBuilder::new()
                .binary_detection(BinaryDetection::quit(0))
                .line_number(true)
                .build();
            Box::new(move |entry| {
                if matched_shared.load(Ordering::Relaxed) >= opts.max_hits {
                    return WalkState::Quit;
                }
                // 逐条目错误（权限等）跳过不中断：并行 walker 的错误本就是
                // 单条目粒度，整场搜索不因单个不可读目录报废。
                let Ok(entry) = entry else {
                    return WalkState::Continue;
                };
                if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                    return WalkState::Continue;
                }
                let path = entry.path();
                let mut file_hits: Vec<(u64, u64, String)> = Vec::new();
                {
                    let mut sink = LineSink {
                        regex: &line_regex,
                        hits: &mut file_hits,
                        // 单文件上限给足（LineSink 内部到量即停）；全局上限在
                        // 合并时统一收敛，单文件不可能贡献超过全局上限的有效命中
                        cap: opts.max_hits,
                    };
                    let _ = searcher.search_path(&*matcher, path, &mut sink);
                }
                if file_hits.is_empty() {
                    return WalkState::Continue;
                }
                let rel = path
                    .strip_prefix(root)
                    .map(|p| p.to_string_lossy().replace('\\', "/"))
                    .unwrap_or_else(|_| path.to_string_lossy().into_owned());
                let mut all = hits_shared.lock().expect("hits lock");
                if all.len() >= opts.max_hits {
                    return WalkState::Quit;
                }
                let remaining = opts.max_hits - all.len();
                all.extend(
                    file_hits
                        .into_iter()
                        .take(remaining)
                        .map(|(line, column, text)| SearchHit {
                            path: rel.clone(),
                            line,
                            column,
                            text,
                        }),
                );
                let total = all.len();
                drop(all);
                matched_shared.store(total, Ordering::Relaxed);
                if total >= opts.max_hits {
                    WalkState::Quit
                } else {
                    WalkState::Continue
                }
            })
        });

    let mut hits = hits.into_inner().expect("hits lock");
    hits.sort_by(|a, b| {
        a.path
            .cmp(&b.path)
            .then(a.line.cmp(&b.line))
            .then(a.column.cmp(&b.column))
    });
    hits.truncate(opts.max_hits);
    Ok(hits)
}

/// 路径是否位于 .git 内（含 .git 自身）。
pub fn within_dot_git(path: &Path) -> bool {
    path.components().any(|c| c.as_os_str() == ".git")
}

/// 文件名模式定位（§9.2 v2.61）：globset 全路径匹配，walker 与 `search`
/// 同一配置（尊重 .gitignore、滤 .git、含隐藏文件）——grep 与 glob 看到同一
/// 文件宇宙。返回相对 root 的路径（正斜杠、排序去重）。
pub fn glob_files(root: &Path, pattern: &str) -> Result<Vec<String>> {
    if pattern.trim().is_empty() {
        return Err(FsError::Io(std::io::Error::other("pattern 不可为空")));
    }
    let mut builder = globset::GlobSetBuilder::new();
    builder.add(
        globset::Glob::new(pattern.trim())
            .map_err(|e| FsError::Io(std::io::Error::other(format!("非法 glob 模式: {e}"))))?,
    );
    let globs = builder
        .build()
        .map_err(|e| FsError::Io(std::io::Error::other(format!("非法 glob 模式: {e}"))))?;
    let walker = ignore::WalkBuilder::new(root)
        .hidden(false)
        .require_git(false)
        .filter_entry(|e| !within_dot_git(e.path()))
        .build();
    let mut paths: Vec<String> = Vec::new();
    for item in walker {
        let entry = item.map_err(|e| FsError::Io(std::io::Error::other(e.to_string())))?;
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        let path = entry.path();
        let rel = path
            .strip_prefix(root)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| path.to_string_lossy().into_owned());
        if globs.is_match(&rel) {
            paths.push(rel);
        }
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

/// 列号计算兜底用的永不匹配正则。
static NEVER_REGEX: std::sync::LazyLock<Regex> =
    std::sync::LazyLock::new(|| Regex::new(r"[^\s\S]").expect("never regex"));

/// 替换预览：返回每文件的统一 diff（不落盘，§15 `GET /search` 替换预览）。
pub fn replace_preview(
    root: &Path,
    pattern: &str,
    replacement: &str,
    opts: &SearchOptions,
) -> Result<Vec<(String, String)>> {
    let re = Regex::new(pattern).map_err(|e| FsError::Io(std::io::Error::other(e.to_string())))?;
    let mut previews = Vec::new();
    let walker = ignore::WalkBuilder::new(root)
        .hidden(false)
        .require_git(false)
        .filter_entry(|e| !within_dot_git(e.path()))
        .build();
    for item in walker {
        let entry = item.map_err(|e| FsError::Io(std::io::Error::other(e.to_string())))?;
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        let path = entry.path();
        // 读前先按 metadata 过滤大小：整读后再丢上限对内存爆无防护
        //（项目里的 GB 级数据文件会让每次替换预览全量进内存）
        let over_size = std::fs::metadata(path)
            .map(|m| m.len() > opts.max_file_bytes)
            .unwrap_or(true);
        if over_size {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(path) else {
            continue; // 二进制 / 非 UTF-8 跳过
        };
        if !re.is_match(&content) {
            continue;
        }
        let replaced = re.replace_all(&content, replacement).into_owned();
        let diff = TextDiff::from_lines(&content, &replaced);
        let unified = diff
            .unified_diff()
            .context_radius(2)
            .header("a", "b")
            .to_string();
        let rel = path
            .strip_prefix(root)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| path.to_string_lossy().into_owned());
        previews.push((rel, unified));
    }
    Ok(previews)
}

#[derive(Debug, Clone, Serialize)]
pub struct ReplaceOutcome {
    pub path: String,
    pub replacements: usize,
    pub diff: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReplaceApplyReport {
    pub applied: Vec<ReplaceOutcome>,
    pub failed: Vec<(String, String)>,
}

/// 选定文件替换：先全量物化结果，再落盘；任一物化失败都不会写入。
/// 返回逐文件成功 / 失败，调用方可将失败展示给用户（部分写入会明确报告）。
pub fn replace_selected(
    root: &Path,
    pattern: &str,
    replacement: &str,
    paths: &[String],
    opts: &SearchOptions,
) -> Result<ReplaceApplyReport> {
    let re = Regex::new(pattern).map_err(|e| FsError::Io(std::io::Error::other(e.to_string())))?;
    let guard = tenon_sandbox::WriteGuard::new(root);
    let mut staged = Vec::new();
    let mut failed = Vec::new();
    for rel in paths {
        let normalized = match guard.check_write_rel(rel) {
            Ok(path) => path,
            Err(e) => {
                failed.push((rel.clone(), e.to_string()));
                continue;
            }
        };
        let abs = root.join(normalized);
        // 读前按 metadata 拦超限（同 replace_preview：整读后判上限不防内存爆）
        match std::fs::metadata(&abs) {
            Ok(m) if m.len() > opts.max_file_bytes => {
                failed.push((rel.clone(), "file exceeds replace size limit".into()));
                continue;
            }
            Err(e) => {
                failed.push((rel.clone(), e.to_string()));
                continue;
            }
            Ok(_) => {}
        }
        let Ok(content) = std::fs::read_to_string(&abs) else {
            failed.push((rel.clone(), "binary, missing, or non-UTF-8".into()));
            continue;
        };
        if !re.is_match(&content) {
            failed.push((rel.clone(), "no matches".into()));
            continue;
        }
        let replacements = re.find_iter(&content).count();
        let replaced = re.replace_all(&content, replacement).into_owned();
        let diff = TextDiff::from_lines(&content, &replaced)
            .unified_diff()
            .context_radius(2)
            .header("a", "b")
            .to_string();
        staged.push((rel.clone(), abs, replaced, replacements, diff));
    }

    let mut applied = Vec::new();
    for (rel, abs, replaced, replacements, diff) in staged {
        match std::fs::write(&abs, &replaced) {
            Ok(()) => applied.push(ReplaceOutcome {
                path: rel,
                replacements,
                diff,
            }),
            Err(e) => failed.push((rel, e.to_string())),
        }
    }
    Ok(ReplaceApplyReport { applied, failed })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "alpha beta\ngamma\n").unwrap();
        std::fs::create_dir_all(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub/b.txt"), "beta again\n").unwrap();
        std::fs::write(dir.path().join(".gitignore"), "ignored*\n").unwrap();
        std::fs::write(dir.path().join("ignored.txt"), "beta hidden\n").unwrap();
        dir
    }

    #[test]
    fn finds_matches_respecting_gitignore() {
        let d = setup();
        let hits = search(d.path(), "beta", &SearchOptions::default()).unwrap();
        let paths: Vec<&str> = hits.iter().map(|h| h.path.as_str()).collect();
        assert!(paths.contains(&"a.txt"));
        assert!(paths.contains(&"sub/b.txt"));
        assert!(
            !paths.iter().any(|p| p.contains("ignored")),
            ".gitignore 生效"
        );
        assert_eq!(hits[0].line, 1);
    }

    #[test]
    fn regex_search() {
        let d = setup();
        let hits = search(d.path(), r"\bgamma\b", &SearchOptions::default()).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "a.txt");
        assert_eq!(hits[0].line, 2);
    }

    #[test]
    fn replace_preview_produces_diff_without_touching_files() {
        let d = setup();
        let previews =
            replace_preview(d.path(), "beta", "BETA", &SearchOptions::default()).unwrap();
        assert_eq!(previews.len(), 2, "两个文件命中");
        for (path, diff) in &previews {
            assert!(
                diff.contains("-alpha beta")
                    || diff.contains("+alpha BETA")
                    || diff.contains("BETA")
            );
            assert!(!path.contains("ignored"));
        }
        // 文件未被修改
        let content = std::fs::read_to_string(d.path().join("a.txt")).unwrap();
        assert_eq!(content, "alpha beta\ngamma\n");
    }

    #[test]
    fn hit_cap_enforced() {
        let d = setup();
        let opts = SearchOptions {
            max_hits: 1,
            ..Default::default()
        };
        let hits = search(d.path(), "beta", &opts).unwrap();
        assert_eq!(hits.len(), 1);
    }

    /// 并行遍历下枚举序不可依赖：结果必须收敛为（路径，行，列）稳定序。
    /// 组内行序升序是 UI「打开首个命中」跳转的语义前提。
    #[test]
    fn hits_are_stable_sorted_by_path_line_column() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("z")).unwrap();
        std::fs::write(dir.path().join("z/second.txt"), "hit\nfiller\nhit\nhit\n").unwrap();
        std::fs::write(dir.path().join("a_first.txt"), "hit\n").unwrap();
        let hits = search(dir.path(), "hit", &SearchOptions::default()).unwrap();
        let paths: Vec<&str> = hits.iter().map(|h| h.path.as_str()).collect();
        assert_eq!(
            paths,
            vec![
                "a_first.txt",
                "z/second.txt",
                "z/second.txt",
                "z/second.txt"
            ]
        );
        let lines: Vec<u64> = hits.iter().map(|h| h.line).collect();
        assert_eq!(lines, vec![1, 1, 3, 4]);
    }

    /// v2.61 §9.2：glob_files——模式匹配、.gitignore 生效、排序、空模式拒绝。
    #[test]
    fn glob_files_matches_respecting_gitignore() {
        let d = setup();
        let files = glob_files(d.path(), "**/*.txt").unwrap();
        assert_eq!(
            files,
            vec!["a.txt", "sub/b.txt"],
            "匹配 + 排序 + gitignore 生效"
        );

        let all = glob_files(d.path(), "*").unwrap();
        assert!(all.contains(&"a.txt".to_string()), "根层文件命中");
        assert!(
            !all.iter().any(|p| p.contains("ignored")),
            ".gitignore 文件不被列出"
        );

        assert!(glob_files(d.path(), "**/*.nope").unwrap().is_empty());
        assert!(glob_files(d.path(), "  ").is_err(), "空模式拒绝");
        assert!(glob_files(d.path(), "[").is_err(), "非法模式拒绝");
    }

    #[test]
    fn replace_selected_applies_only_selected_files() {
        let d = setup();
        let report = replace_selected(
            d.path(),
            "beta",
            "BETA",
            &["a.txt".into(), "sub/b.txt".into()],
            &SearchOptions::default(),
        )
        .unwrap();
        assert_eq!(report.applied.len(), 2);
        assert!(report.failed.is_empty());
        assert_eq!(report.applied[0].replacements, 1);
        assert_eq!(report.applied[1].replacements, 1);
        assert_eq!(
            std::fs::read_to_string(d.path().join("a.txt")).unwrap(),
            "alpha BETA\ngamma\n"
        );
        assert_eq!(
            std::fs::read_to_string(d.path().join("sub/b.txt")).unwrap(),
            "BETA again\n"
        );
    }

    #[test]
    fn replace_selected_confines_paths_and_reports_misses() {
        let d = setup();
        let report = replace_selected(
            d.path(),
            "beta",
            "BETA",
            &[
                "../outside.txt".into(),
                "missing.txt".into(),
                "a.txt".into(),
            ],
            &SearchOptions::default(),
        )
        .unwrap();
        assert_eq!(report.applied.len(), 1);
        assert_eq!(report.applied[0].path, "a.txt");
        assert_eq!(report.failed.len(), 2);
    }
}
