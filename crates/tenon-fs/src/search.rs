//! 全局搜索与替换预览（设计方案 §8.1：ripgrep 驱动；替换前返回 diff 预览）。

use crate::{FsError, Result};
use grep_regex::RegexMatcher;
use grep_searcher::{BinaryDetection, Searcher, SearcherBuilder, Sink, SinkMatch};
use regex::Regex;
use serde::{Deserialize, Serialize};
use similar::TextDiff;
use std::path::Path;

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
pub fn search(root: &Path, pattern: &str, opts: &SearchOptions) -> Result<Vec<SearchHit>> {
    let matcher = RegexMatcher::new(pattern)
        .map_err(|e| FsError::Io(std::io::Error::other(e.to_string())))?;
    let line_regex = Regex::new(pattern).ok();
    let mut hits: Vec<SearchHit> = Vec::new();
    let mut searcher = SearcherBuilder::new()
        .binary_detection(BinaryDetection::quit(0))
        .line_number(true)
        .build();

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
        if let Ok(meta) = entry.metadata() {
            if meta.len() > opts.max_file_bytes {
                continue;
            }
        }
        let path = entry.path();
        let rel = path
            .strip_prefix(root)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| path.to_string_lossy().into_owned());
        let mut file_hits: Vec<(u64, u64, String)> = Vec::new();
        {
            let mut sink = LineSink {
                regex: line_regex.as_ref().unwrap_or(&NEVER_REGEX),
                hits: &mut file_hits,
                cap: opts.max_hits - hits.len(),
            };
            let _ = searcher.search_path(&matcher, path, &mut sink);
        }
        for (line, column, text) in file_hits {
            hits.push(SearchHit {
                path: rel.clone(),
                line,
                column,
                text,
            });
        }
        if hits.len() >= opts.max_hits {
            break;
        }
    }
    Ok(hits)
}

/// 路径是否位于 .git 内（含 .git 自身）。
pub fn within_dot_git(path: &Path) -> bool {
    path.components().any(|c| c.as_os_str() == ".git")
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
        let Ok(content) = std::fs::read_to_string(path) else {
            continue; // 二进制 / 非 UTF-8 跳过
        };
        if content.len() > opts.max_file_bytes as usize {
            continue;
        }
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
        let Ok(content) = std::fs::read_to_string(&abs) else {
            failed.push((rel.clone(), "binary, missing, or non-UTF-8".into()));
            continue;
        };
        if content.len() > opts.max_file_bytes as usize {
            failed.push((rel.clone(), "file exceeds replace size limit".into()));
            continue;
        }
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
