//! L4 本地持久索引构建（§10.1）：源码切片 + 确定性本地 embedding。
//!
//! 索引完全本地：不联网、不调用云模型。embedding 是可复现的 hash-based
//! bag-of-words 向量，用于小/中型仓库的粗召回；真实语义嵌入后续可替换，
//! 表结构与调用边界保持不变。

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// 一个 L4 切片；行号为 1-based inclusive。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct L4Chunk {
    pub symbol: Option<String>,
    pub start_line: usize,
    pub end_line: usize,
    pub text: String,
}

/// 一个待入库文件的切片集合。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct L4Document {
    pub path: String,
    pub chunks: Vec<L4Chunk>,
}

const EMBEDDING_DIM: usize = 1024;
const MAX_CHUNK_LINES: usize = 160;
const OVERLAP_LINES: usize = 20;
const MAX_FILE_BYTES: usize = 512 * 1024;

fn symbol_regex() -> &'static Regex {
    static RE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
        Regex::new(concat!(
            r"^\s*(?:",
            r"((?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?(?:fn|struct|enum|trait|impl|mod)\s+[\w<>, ]+)|",
            r"((?:export\s+)?(?:default\s+)?(?:async\s+)?(?:function|class|interface|type|const|let)\s+\w+)|",
            r"((?:public|private|protected|internal|static|final|abstract)?\s*(?:class|interface|record|enum)\s+\w+)|",
            r"((?:async\s+)?def\s+\w+)|",
            r"(#{1,3}\s+\S.*)",
            r")"
        ))
        .expect("l4 symbol regex")
    });
    &RE
}

/// 从源码切片；symbol 行提前开块，长文件硬性分段并保留重叠上下文。
pub fn chunk_source(source: &str) -> Vec<L4Chunk> {
    let lines: Vec<&str> = source.lines().collect();
    if lines.is_empty() {
        return Vec::new();
    }
    let re = symbol_regex();
    let mut chunks = Vec::new();
    let mut start = 0usize;
    let mut symbol: Option<String> = None;

    let mut push = |start: usize, end: usize, symbol: Option<String>| {
        let text = lines[start..=end].join("\n");
        if !text.trim().is_empty() {
            chunks.push(L4Chunk {
                symbol,
                start_line: start + 1,
                end_line: end + 1,
                text,
            });
        }
    };

    for (index, line) in lines.iter().enumerate() {
        if index > start {
            let captured = re.captures(line).and_then(|caps| {
                caps.iter()
                    .skip(1)
                    .flatten()
                    .next()
                    .map(|m| m.as_str().trim().to_string())
            });
            let hard_limit = index - start >= MAX_CHUNK_LINES;
            if captured.is_some() || hard_limit {
                let end = index.saturating_sub(1);
                if end >= start {
                    push(start, end, symbol.take());
                }
                start = if hard_limit && index + OVERLAP_LINES < lines.len() {
                    index.saturating_sub(OVERLAP_LINES)
                } else {
                    index
                };
                if start > index {
                    start = index;
                }
            }
            if captured.is_some() {
                symbol = captured;
            }
        } else if index == 0 {
            symbol = re.captures(line).and_then(|caps| {
                caps.iter()
                    .skip(1)
                    .flatten()
                    .next()
                    .map(|m| m.as_str().trim().to_string())
            });
        }
    }
    push(start, lines.len() - 1, symbol);
    chunks
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn tokenize(text: &str) -> Vec<String> {
    static TOKEN: std::sync::LazyLock<Regex> =
        std::sync::LazyLock::new(|| Regex::new(r"\p{Alphabetic}+|\p{Nd}+").expect("l4 tokenizer"));
    let mut out = Vec::new();
    for word in TOKEN.find_iter(text) {
        let word = word.as_str().to_lowercase();
        for part in word.split('_') {
            if part.is_empty() {
                continue;
            }
            out.push(part.to_string());
            // CJK / camel-free identifiers may not word-split; short character
            // n-grams keep local substring recall without a cloud embedder.
            let chars: Vec<char> = part.chars().collect();
            for window in chars.windows(3.min(chars.len())) {
                if window.len() == 3 {
                    out.push(window.iter().collect::<String>());
                }
            }
        }
    }
    out
}

fn should_index(path: &str) -> bool {
    static IGNORED: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
        Regex::new(r"(^|/)(\.gitignore|\.gitattributes|package-lock\.json|pnpm-lock\.yaml|Cargo\.lock|yarn\.lock)$")
            .expect("l4 ignore regex")
    });
    !IGNORED.is_match(path)
}

/// 本地确定性 embedding：FNV hash bag-of-words + L2 normalize。
pub fn embed(path: &str, text: &str) -> Vec<f32> {
    let mut vector = vec![0.0f32; EMBEDDING_DIM];
    let mut counts: HashMap<String, usize> = HashMap::new();
    for token in tokenize(text) {
        *counts.entry(token).or_default() += 1;
    }
    for token in tokenize(path) {
        *counts.entry(format!("path:{token}")).or_default() += 2;
    }
    for (token, count) in counts {
        let weight = 1.0 + (count.min(16) as f32).ln().max(0.0);
        let primary = (fnv1a(token.as_bytes()) as usize) % EMBEDDING_DIM;
        vector[primary] += weight;
        // 两个辅助桶降低 hash collision 的偶然主导。
        let left = (fnv1a(token.as_bytes()) >> 17) as usize % EMBEDDING_DIM;
        let right = (fnv1a(token.as_bytes()) >> 41) as usize % EMBEDDING_DIM;
        if left != primary {
            vector[left] += weight * 0.5;
        }
        if right != primary && right != left {
            vector[right] += weight * 0.35;
        }
    }
    let norm = vector.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm > f32::EPSILON {
        for value in &mut vector {
            *value /= norm;
        }
    }
    vector
}

/// 构建单个文件的 L4 document；过大 / 非 UTF-8 返回 None。
pub fn index_file(path: &str, source: &str) -> Option<L4Document> {
    if source.len() > MAX_FILE_BYTES {
        return None;
    }
    let chunks = chunk_source(source);
    (!chunks.is_empty()).then_some(L4Document {
        path: path.to_string(),
        chunks,
    })
}

/// 构建单个项目相对 path 的 document。
pub fn build_document(root: &Path, path: &str) -> Option<L4Document> {
    let abs = root.join(path);
    let source = std::fs::read_to_string(abs).ok()?;
    index_file(path, &source)
}

/// gitignore-aware 全量扫描；供项目首次打开 / 手动 rebuild 使用。
pub fn scan_root(root: &Path) -> Vec<L4Document> {
    let mut documents = Vec::new();
    let walker = ignore::WalkBuilder::new(root)
        .hidden(false)
        .require_git(false)
        .filter_entry(|entry| {
            entry
                .path()
                .components()
                .all(|component| component.as_os_str() != ".git")
        })
        .build();
    for item in walker {
        let Ok(entry) = item else { continue };
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        if entry
            .metadata()
            .is_ok_and(|meta| meta.len() > MAX_FILE_BYTES as u64)
        {
            continue;
        }
        let Ok(rel) = entry.path().strip_prefix(root) else {
            continue;
        };
        let rel = rel.to_string_lossy().replace('\\', "/");
        if let Ok(source) = std::fs::read_to_string(entry.path()) {
            if should_index(&rel) {
                if let Some(document) = index_file(&rel, &source) {
                    documents.push(document);
                }
            }
        }
    }
    documents
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_symbols_and_ranges() {
        let source = "import x\n\nfn one() {\n    // one\n}\n\nfn two() {\n}\n";
        let chunks = chunk_source(source);
        assert!(chunks.len() >= 2, "{chunks:?}");
        assert!(chunks
            .iter()
            .any(|c| c.symbol.as_deref().is_some_and(|s| s.contains("fn one"))));
        assert!(chunks.iter().all(|c| c.start_line <= c.end_line));
        assert!(chunks.iter().all(|c| !c.text.trim().is_empty()));
    }

    #[test]
    fn embeddings_are_stable_and_normalized() {
        let a = embed("src/auth.rs", "pub fn login user password");
        let b = embed("src/auth.rs", "pub fn login user password");
        let different = embed("src/render.rs", "pub fn render canvas pixels");
        let dot = |x: &[f32], y: &[f32]| x.iter().zip(y).map(|(a, b)| a * b).sum::<f32>();
        assert_eq!(a, b);
        assert!((dot(&a, &a) - 1.0).abs() < 1e-5);
        assert!(dot(&a, &different) < 0.9);
    }

    #[test]
    fn scan_root_respects_gitignore_and_returns_documents() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".gitignore"), "ignored/\n").unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::create_dir_all(dir.path().join("ignored")).unwrap();
        std::fs::write(dir.path().join("src/lib.rs"), "fn indexed() {}\n").unwrap();
        std::fs::write(dir.path().join("ignored/hidden.rs"), "fn hidden() {}\n").unwrap();
        let docs = scan_root(dir.path());
        assert_eq!(docs.len(), 1, "{docs:?}");
        assert_eq!(docs[0].path, "src/lib.rs");
    }

    #[test]
    fn index_file_handles_empty_and_large() {
        assert!(index_file("empty.rs", "").is_none());
        let big = "x\n".repeat(MAX_FILE_BYTES + 1);
        assert!(index_file("big.txt", &big).is_none());
        assert!(index_file("ok.rs", "fn ok() {}\n").is_some());
    }
}
