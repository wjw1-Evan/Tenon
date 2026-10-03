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
}
