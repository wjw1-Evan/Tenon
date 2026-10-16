//! LSP 查询结果的模型可读格式化（§9.2 v2.82）。
//!
//! 共享 LSP 宿主（§8.5）返回的是原始 LSP JSON——`file://` URI、0-based range、
//! 数字枚举的符号 / 诊断级别。直接回传模型既冗长，又要求模型自行换算 URI 与
//! 行列坐标；本模块把它格式化为**可直接操作的** `路径:行:列` 形态：
//!
//! - 路径：项目内文件转为相对路径（与 `read_file` / `apply_patch` 同一坐标系），
//!   项目外（标准库 / 依赖）保留绝对路径；
//! - 行列：统一 1-based（与 `read_file` 输出的行号前缀一致，可直接用作
//!   `apply_patch` 的 range 锚点）；
//! - 命中位置附该行原文（可读文件 best-effort），模型无需再 `read_file` 复核。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// 列表类结果（references / workspace_symbol / diagnostics）行数上限。
const MAX_ROWS: usize = 100;
/// 命中行原文长度上限（字符）。
const EXCERPT_MAX_CHARS: usize = 200;
/// 为取原文而整读的文件字节上限（超限不附原文，避免大文件整读进内存）。
const EXCERPT_FILE_MAX_BYTES: u64 = 2 * 1024 * 1024;

/// 把一次 LSP 查询结果格式化为模型可读文本。
///
/// `path` 为本次查询的文件（相对项目根）；`diagnostics` 的条目本身不带 URI，
/// 故用查询文件回填。未知 action 如实回报（不静默丢弃）。
pub fn format_result(root: &Path, path: &str, action: &str, value: &serde_json::Value) -> String {
    match action {
        "definition" => format_locations(root, value, "未找到定义"),
        "references" => format_locations(root, value, "无引用"),
        "hover" => format_hover(value),
        "diagnostics" => format_diagnostics(root, path, value),
        "workspace_symbol" => format_symbols(root, value),
        other => format!("（未知 lsp action: {other}）"),
    }
}

/// 归一化后的一个位置。
struct Loc {
    /// 项目内相对路径；项目外为 None（显示用绝对路径）。
    rel: Option<String>,
    /// 绝对路径（读原文用）。
    abs: PathBuf,
    line0: u64,
    col0: u64,
}

impl Loc {
    /// 仅路径（项目内相对 / 项目外绝对）。
    fn path_str(&self) -> String {
        match &self.rel {
            Some(r) => r.clone(),
            None => self.abs.to_string_lossy().replace('\\', "/"),
        }
    }

    /// `路径:行:列`（1-based）。
    fn display(&self) -> String {
        format!("{}:{}:{}", self.path_str(), self.line0 + 1, self.col0 + 1)
    }
}

/// `file://` URI → (项目内相对路径, 绝对路径)；解析失败时原样保留 URI。
fn resolve(root: &Path, uri: &str) -> Loc {
    match tenon_lsp::guard::uri_to_path(uri) {
        Some(abs) => {
            let rel = abs
                .strip_prefix(root)
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .ok()
                .or_else(|| {
                    // 符号链接 / private 前缀差异：canonicalize 后再试一次
                    let rc = root.canonicalize().ok()?;
                    let ac = abs.canonicalize().ok()?;
                    ac.strip_prefix(&rc)
                        .ok()
                        .map(|p| p.to_string_lossy().replace('\\', "/"))
                });
            Loc {
                rel,
                abs,
                line0: 0,
                col0: 0,
            }
        }
        None => Loc {
            rel: None,
            abs: PathBuf::from(uri),
            line0: 0,
            col0: 0,
        },
    }
}

/// definition / references：Location[] | Location | LocationLink[] | null。
fn format_locations(root: &Path, value: &serde_json::Value, empty: &str) -> String {
    let mut locs = Vec::new();
    for item in as_items(value) {
        let uri = item
            .get("uri")
            .or_else(|| item.get("targetUri"))
            .and_then(|u| u.as_str());
        // LocationLink 优先取 targetSelectionRange（符号名本身），退回整区间
        let range = item
            .get("targetSelectionRange")
            .or_else(|| item.get("range"))
            .or_else(|| item.get("targetRange"));
        let (Some(uri), Some(range)) = (uri, range) else {
            continue;
        };
        let mut loc = resolve(root, uri);
        loc.line0 = range
            .pointer("/start/line")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        loc.col0 = range
            .pointer("/start/character")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        locs.push(loc);
    }
    if locs.is_empty() {
        return empty.to_string();
    }
    render_locs(locs)
}

fn render_locs(locs: Vec<Loc>) -> String {
    let total = locs.len();
    let mut cache: HashMap<PathBuf, Vec<String>> = HashMap::new();
    let mut out: Vec<String> = Vec::new();
    for loc in locs.into_iter().take(MAX_ROWS) {
        let mut line = loc.display();
        if let Some(text) = excerpt(&mut cache, &loc.abs, loc.line0) {
            if !text.is_empty() {
                line.push_str(": ");
                line.push_str(&text);
            }
        }
        out.push(line);
    }
    if total > MAX_ROWS {
        out.push(format!(
            "…（共 {total} 条，仅显示前 {MAX_ROWS} 条——请用更窄的查询 / 文件范围收窄）"
        ));
    }
    out.join("\n")
}

/// hover：Hover{contents} → 文本。
fn format_hover(value: &serde_json::Value) -> String {
    if value.is_null() {
        return "（无 hover 信息）".to_string();
    }
    let contents = value.get("contents").unwrap_or(value);
    let text = markup_text(contents);
    if text.trim().is_empty() {
        "（无 hover 信息）".to_string()
    } else {
        text
    }
}

/// MarkupContent{value} / MarkedString(string | {value}) / 数组 的统一取文。
fn markup_text(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(a) => a
            .iter()
            .map(markup_text)
            .filter(|s| !s.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
        serde_json::Value::Object(_) => v
            .get("value")
            .and_then(|s| s.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| v.to_string()),
        _ => v.to_string(),
    }
}

/// diagnostics：{items:[Diagnostic]}（条目无 URI，用查询文件回填）。
fn format_diagnostics(root: &Path, path: &str, value: &serde_json::Value) -> String {
    let items = value
        .get("items")
        .and_then(|i| i.as_array())
        .cloned()
        .unwrap_or_else(|| value.as_array().cloned().unwrap_or_default());
    if items.is_empty() {
        return "（无诊断）".to_string();
    }
    let base = resolve(root, &tenon_lsp::pack::file_uri(root, path));
    let total = items.len();
    let mut lines: Vec<String> = Vec::new();
    for d in items.into_iter().take(MAX_ROWS) {
        let (line0, col0) = range_start(&d);
        let severity = match d.get("severity").and_then(|s| s.as_u64()).unwrap_or(1) {
            1 => "Error",
            2 => "Warning",
            3 => "Info",
            _ => "Hint",
        };
        let message = d.get("message").and_then(|m| m.as_str()).unwrap_or("");
        let source = d.get("source").and_then(|s| s.as_str());
        let code = d.get("code").map(code_text).filter(|c| !c.is_empty());
        let mut head = format!(
            "{}:{}:{} [{severity}] {message}",
            base.path_str(),
            line0 + 1,
            col0 + 1
        );
        match (source, code) {
            (Some(s), Some(c)) => head.push_str(&format!(" ({s} {c})")),
            (Some(s), None) => head.push_str(&format!(" ({s})")),
            (None, Some(c)) => head.push_str(&format!(" ({c})")),
            (None, None) => {}
        }
        lines.push(head);
    }
    if total > MAX_ROWS {
        lines.push(format!("…（共 {total} 条诊断，仅显示前 {MAX_ROWS} 条）"));
    }
    lines.join("\n")
}

fn code_text(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// workspace_symbol：SymbolInformation[] / WorkspaceSymbol[]。
fn format_symbols(root: &Path, value: &serde_json::Value) -> String {
    let items = as_items(value);
    if items.is_empty() {
        return "（无匹配符号）".to_string();
    }
    let total = items.len();
    let mut out: Vec<String> = Vec::new();
    for it in items.into_iter().take(MAX_ROWS) {
        let name = it.get("name").and_then(|n| n.as_str()).unwrap_or("");
        let kind = it
            .get("kind")
            .and_then(|k| k.as_u64())
            .map(symbol_kind)
            .unwrap_or("Symbol");
        // WorkspaceSymbol 直接带 location / range；SymbolInformation 嵌 location
        let location = it.get("location").unwrap_or(it);
        let uri = location.get("uri").and_then(|u| u.as_str());
        let (line0, col0) = range_start(location);
        let where_ = match uri {
            Some(u) => {
                let mut loc = resolve(root, u);
                loc.line0 = line0;
                loc.col0 = col0;
                loc.display()
            }
            None => format!(":{}:{}", line0 + 1, col0 + 1),
        };
        out.push(format!("{name} ({kind}) {where_}"));
    }
    if total > MAX_ROWS {
        out.push(format!(
            "…（共 {total} 个符号，仅显示前 {MAX_ROWS} 个——请用更精确的查询名收窄）"
        ));
    }
    out.join("\n")
}

/// range.start → (line0, character0)。
fn range_start(v: &serde_json::Value) -> (u64, u64) {
    let range = v.get("range").or_else(|| v.get("targetSelectionRange"));
    let line = range
        .and_then(|r| r.pointer("/start/line"))
        .and_then(|l| l.as_u64())
        .unwrap_or(0);
    let ch = range
        .and_then(|r| r.pointer("/start/character"))
        .and_then(|c| c.as_u64())
        .unwrap_or(0);
    (line, ch)
}

/// null → 空；数组 → 逐项；单对象 → 一项（LSP 允许单值返回）。
fn as_items(value: &serde_json::Value) -> Vec<&serde_json::Value> {
    match value {
        serde_json::Value::Null => Vec::new(),
        serde_json::Value::Array(a) => a.iter().collect(),
        v => vec![v],
    }
}

/// 读取 `abs` 的第 `line0` 行原文（trim + 截断）；文件不可读 / 过大 → None。
fn excerpt(cache: &mut HashMap<PathBuf, Vec<String>>, abs: &Path, line0: u64) -> Option<String> {
    let lines = cache
        .entry(abs.to_path_buf())
        .or_insert_with(|| match std::fs::metadata(abs) {
            Ok(m) if m.len() <= EXCERPT_FILE_MAX_BYTES => std::fs::read_to_string(abs)
                .map(|c| c.lines().map(str::to_string).collect())
                .unwrap_or_default(),
            _ => Vec::new(),
        });
    lines.get(line0 as usize).map(|l| {
        let t = l.trim();
        if t.chars().count() > EXCERPT_MAX_CHARS {
            let head: String = t.chars().take(EXCERPT_MAX_CHARS).collect();
            format!("{head}…")
        } else {
            t.to_string()
        }
    })
}

/// LSP SymbolKind → 名字（1..26，见 LSP 规范）。
fn symbol_kind(k: u64) -> &'static str {
    match k {
        1 => "File",
        2 => "Module",
        3 => "Namespace",
        4 => "Package",
        5 => "Class",
        6 => "Method",
        7 => "Property",
        8 => "Field",
        9 => "Constructor",
        10 => "Enum",
        11 => "Interface",
        12 => "Function",
        13 => "Variable",
        14 => "Constant",
        15 => "String",
        16 => "Number",
        17 => "Boolean",
        18 => "Array",
        19 => "Object",
        20 => "Key",
        21 => "Null",
        22 => "EnumMember",
        23 => "Struct",
        24 => "Event",
        25 => "Operator",
        26 => "TypeParameter",
        _ => "Symbol",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn project() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("src/app.ts"),
            "import { x } from './x';\nfunction main() {\n  return x();\n}\n",
        )
        .unwrap();
        (dir, root)
    }

    fn uri(root: &Path, rel: &str) -> String {
        format!("file://{}", root.join(rel).to_string_lossy())
    }

    /// definition：Location[] → 项目相对路径 + 1-based 行列 + 命中行原文。
    #[test]
    fn definition_formats_relative_path_and_excerpt() {
        let (_d, root) = project();
        let value = json!([{ "uri": uri(&root, "src/app.ts"), "range": {"start": {"line": 1, "character": 9}, "end": {"line": 1, "character": 13}} }]);
        let out = format_result(&root, "src/app.ts", "definition", &value);
        assert_eq!(out, "src/app.ts:2:10: function main() {");
    }

    /// definition：LocationLink（targetUri / targetSelectionRange）与 null 空态。
    #[test]
    fn definition_handles_location_link_and_empty() {
        let (_d, root) = project();
        let link = json!({
            "targetUri": uri(&root, "src/app.ts"),
            "targetRange": {"start": {"line": 1, "character": 0}, "end": {"line": 3, "character": 1}},
            "targetSelectionRange": {"start": {"line": 1, "character": 9}, "end": {"line": 1, "character": 13}}
        });
        let out = format_result(&root, "src/app.ts", "definition", &link);
        assert!(out.starts_with("src/app.ts:2:10"), "{out}");
        assert_eq!(
            format_result(&root, "src/app.ts", "definition", &json!(null)),
            "未找到定义"
        );
    }

    /// 项目外文件（依赖 / 标准库）保留绝对路径，不误转相对。
    #[test]
    fn external_uri_keeps_absolute_path() {
        let (_d, root) = project();
        let value = json!([{ "uri": "file:///opt/sdk/lib.rs", "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}} }]);
        let out = format_result(&root, "src/app.ts", "definition", &value);
        assert_eq!(out, "/opt/sdk/lib.rs:1:1");
    }

    /// references：多命中列表 + 上限尾注；空态。
    #[test]
    fn references_lists_and_notes_truncation() {
        let (_d, root) = project();
        let one = json!([{ "uri": uri(&root, "src/app.ts"), "range": {"start": {"line": 2, "character": 9}, "end": {"line": 2, "character": 10}} }]);
        let out = format_result(&root, "src/app.ts", "references", &one);
        assert_eq!(out, "src/app.ts:3:10: return x();");
        assert_eq!(
            format_result(&root, "src/app.ts", "references", &json!([])),
            "无引用"
        );

        let many: Vec<serde_json::Value> = (0..(MAX_ROWS + 5))
            .map(|_| json!({"uri": uri(&root, "src/app.ts"), "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}}}))
            .collect();
        let out = format_result(&root, "src/app.ts", "references", &json!(many));
        assert!(out.contains(&format!("共 {} 条", MAX_ROWS + 5)), "{out}");
    }

    /// hover：MarkupContent / 数组 MarkedString / null 三形态。
    #[test]
    fn hover_extracts_markup_text() {
        let (_d, _root) = project();
        let root = PathBuf::from("/tmp");
        assert_eq!(
            format_result(
                &root,
                "a.ts",
                "hover",
                &json!({"contents": {"kind": "markdown", "value": "`x(): number`"}})
            ),
            "`x(): number`"
        );
        assert_eq!(
            format_result(
                &root,
                "a.ts",
                "hover",
                &json!({"contents": ["line1", {"value": "line2"}]})
            ),
            "line1\nline2"
        );
        assert_eq!(
            format_result(&root, "a.ts", "hover", &json!(null)),
            "（无 hover 信息）"
        );
    }

    /// diagnostics：级别 / 位置 / source+code 与空态。
    #[test]
    fn diagnostics_format_severity_and_location() {
        let (_d, root) = project();
        let value = json!({"items": [
            {"range": {"start": {"line": 2, "character": 2}, "end": {"line": 2, "character": 3}}, "severity": 1, "message": "Type 'string' is not assignable", "source": "ts", "code": 2322},
            {"range": {"start": {"line": 1, "character": 0}, "end": {"line": 1, "character": 1}}, "severity": 2, "message": "unused"}
        ]});
        let out = format_result(&root, "src/app.ts", "diagnostics", &value);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(
            lines[0],
            "src/app.ts:3:3 [Error] Type 'string' is not assignable (ts 2322)"
        );
        assert_eq!(lines[1], "src/app.ts:2:1 [Warning] unused");
        assert_eq!(
            format_result(&root, "src/app.ts", "diagnostics", &json!({"items": []})),
            "（无诊断）"
        );
    }

    /// workspace_symbol：名字 + 种类 + 位置（SymbolInformation 形态）。
    #[test]
    fn workspace_symbol_formats_name_kind_and_location() {
        let (_d, root) = project();
        let value = json!([
            {"name": "main", "kind": 12, "location": {"uri": uri(&root, "src/app.ts"), "range": {"start": {"line": 1, "character": 9}, "end": {"line": 1, "character": 13}}}},
            {"name": "Widget", "kind": 5, "location": {"uri": uri(&root, "src/app.ts"), "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}}}}
        ]);
        let out = format_result(&root, "src/app.ts", "workspace_symbol", &value);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "main (Function) src/app.ts:2:10");
        assert_eq!(lines[1], "Widget (Class) src/app.ts:1:1");
        assert_eq!(
            format_result(&root, "src/app.ts", "workspace_symbol", &json!([])),
            "（无匹配符号）"
        );
    }

    /// 未知 action 如实回报（不静默）。
    #[test]
    fn unknown_action_reported() {
        let (_d, root) = project();
        let out = format_result(&root, "a.ts", "bogus", &json!({}));
        assert!(out.contains("未知 lsp action"), "{out}");
    }
}
