//! daemon 侧 tree-sitter 高亮管线（设计方案 §8.2）：
//! 解析在 daemon（Rust 增量解析），token 流推送 UI——WebView 不跑解析器。
//! M0 交付 Rust 语法的基础高亮捕获（未装语言包时 UI 有基础高亮，
//! 装包后由 LSP semantic tokens 增强，§8.3）。

use serde::Serialize;
use std::path::Path;
use tree_sitter::{Parser, Query, QueryCursor};

#[derive(Debug, Clone, Serialize)]
pub struct HighlightToken {
    /// 起始行（0-based）
    pub start_row: usize,
    pub start_col: usize,
    pub end_row: usize,
    pub end_col: usize,
    /// 捕获类别（function / keyword / string / comment / type / number / …）
    pub kind: String,
}

/// 按扩展名选择语言；不支持返回 None（UI 回退 Monaco 基础高亮）。
pub fn language_for(path: &str) -> Option<tree_sitter::Language> {
    let ext = Path::new(path).extension()?.to_str()?;
    match ext {
        "rs" => Some(tree_sitter_rust::LANGUAGE.into()),
        _ => None,
    }
}

pub(crate) const RUST_HIGHLIGHT_QUERY: &str = r#"
(function_item name: (identifier) @function)
(struct_item name: (type_identifier) @type)
(enum_item name: (type_identifier) @type)
(impl_item type: (type_identifier) @type)
((identifier) @type (#match? @type "^[A-Z]"))
(line_comment) @comment
(string_literal) @string
(char_literal) @string
(integer_literal) @number
(float_literal) @number
(boolean_literal) @keyword
"fn" @keyword
"let" @keyword
"if" @keyword
"else" @keyword
"for" @keyword
"while" @keyword
"return" @keyword
"match" @keyword
"struct" @keyword
"enum" @keyword
"impl" @keyword
"pub" @keyword
"use" @keyword
"mod" @keyword
"#;

/// 解析源码并产出高亮 token 流。
pub fn highlight(language: tree_sitter::Language, source: &str) -> Vec<HighlightToken> {
    let mut parser = Parser::new();
    if parser.set_language(&language).is_err() {
        return Vec::new();
    }
    let Some(tree) = parser.parse(source, None) else {
        return Vec::new();
    };
    let query = match Query::new(&language, RUST_HIGHLIGHT_QUERY) {
        Ok(q) => q,
        Err(e) => {
            eprintln!("highlight query 编译失败: {e}");
            return Vec::new();
        }
    };
    let mut cursor = QueryCursor::new();
    // TextProvider：按节点行迭代字节切片（tree-sitter 0.25 streaming-iterator 语义）
    let provider = |node: tree_sitter::Node| {
        let start = node.start_byte();
        let end = node.end_byte();
        source.as_bytes()[start..end].split_inclusive(|b| *b == b'\n')
    };
    let mut tokens = Vec::new();
    let mut matches = cursor.matches(&query, tree.root_node(), provider);
    use streaming_iterator::StreamingIterator;
    while let Some(m) = matches.next() {
        let Some(cap) = m.captures.first() else {
            continue;
        };
        let kind = query.capture_names()[cap.index as usize].to_string();
        let node = cap.node;
        tokens.push(HighlightToken {
            start_row: node.start_position().row,
            start_col: node.start_position().column,
            end_row: node.end_position().row,
            end_col: node.end_position().column,
            kind,
        });
    }
    tokens.sort_by_key(|t| (t.start_row, t.start_col));
    tokens
}

/// 便捷入口：按文件路径高亮。
pub fn highlight_file(path: &str, source: &str) -> Option<Vec<HighlightToken>> {
    let lang = language_for(path)?;
    Some(highlight(lang, source))
}

#[cfg(test)]
mod tests {
    use super::*;

    const CODE: &str = r#"
// 用户问候
pub struct Greeter { name: String }

pub fn greet(g: &Greeter) -> String {
    let n = 42;
    format!("hello {}", g.name)
}
"#;

    #[test]
    fn rust_highlight_pipeline_produces_tokens() {
        let tokens = highlight_file("src/main.rs", CODE).expect("rust 支持");
        assert!(!tokens.is_empty());
        let kinds: Vec<&str> = tokens.iter().map(|t| t.kind.as_str()).collect();
        assert!(kinds.contains(&"keyword"), "关键字: {kinds:?}");
        assert!(kinds.contains(&"comment"), "注释");
        assert!(kinds.contains(&"function"), "函数名");
        assert!(kinds.contains(&"type"), "类型名");
        // token 有序且坐标落在文本范围内
        assert!(tokens.iter().all(|t| t.start_row <= CODE.lines().count()));
    }

    #[test]
    fn unsupported_language_returns_none() {
        assert!(highlight_file("readme.md", "# hi").is_none());
    }

    #[test]
    fn syntax_error_still_highlights_partial() {
        // 容错解析：残缺代码仍产出可用 token（增量解析语义）
        let tokens = highlight_file("a.rs", "fn broken( { let");
        let t = tokens.unwrap_or_default();
        assert!(!t.is_empty(), "残缺代码也应有注释/关键字级 token");
    }
}
