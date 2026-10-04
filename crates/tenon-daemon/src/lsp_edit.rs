//! LSP WorkspaceEdit 归一化与内容计算（§8.5 / §10.3）。

use std::collections::BTreeMap;
use std::path::Path;

use serde::Serialize;
use tenon_core::tools::unified_diff;
use tenon_lsp::guard::uri_to_path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LspTextEdit {
    pub start_line: usize,
    pub start_character: usize,
    pub end_line: usize,
    pub end_character: usize,
    pub new_text: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlannedTextFile {
    pub path: String,
    pub before: String,
    pub after: String,
    pub diff: String,
}

fn utf16_column_to_offset(line: &str, utf16_column: usize) -> usize {
    let mut seen_utf16 = 0usize;
    for (byte_offset, character) in line.char_indices() {
        if seen_utf16 >= utf16_column {
            return byte_offset;
        }
        seen_utf16 += character.len_utf16();
        if seen_utf16 > utf16_column {
            return byte_offset;
        }
    }
    line.len()
}

fn absolute_offset(source: &str, line: usize, utf16_column: usize) -> Result<usize, String> {
    let lines: Vec<&str> = source.split_inclusive('\n').collect();
    if line > lines.len() {
        return Err(format!("LSP edit line {line} 超出文件"));
    }
    if line == lines.len() {
        return Ok(source.len());
    }
    let line_start: usize = lines[..line].iter().map(|line| line.len()).sum();
    let content = lines[line].trim_end_matches(['\n', '\r']);
    Ok(line_start + utf16_column_to_offset(content, utf16_column))
}

fn relative_uri_path(root: &Path, uri: &str) -> Result<String, String> {
    let absolute = uri_to_path(uri).ok_or_else(|| format!("非法 LSP URI: {uri}"))?;
    let absolute = std::fs::canonicalize(&absolute).unwrap_or(absolute);
    let canonical_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let relative = absolute.strip_prefix(&canonical_root).map_err(|_| {
        format!(
            "workspace edit 越界：{} 不在 {} 内",
            absolute.display(),
            canonical_root.display()
        )
    })?;
    Ok(relative.to_string_lossy().replace('\\', "/"))
}

fn parse_edit(value: &serde_json::Value) -> Result<LspTextEdit, String> {
    let range = value.get("range").ok_or("LSP edit 缺少 range")?;
    let start = range.get("start").ok_or("LSP edit 缺少 range.start")?;
    let end = range.get("end").ok_or("LSP edit 缺少 range.end")?;
    let number = |parent: &serde_json::Value, key: &str| -> Result<usize, String> {
        parent
            .get(key)
            .and_then(serde_json::Value::as_u64)
            .map(|value| value as usize)
            .ok_or_else(|| format!("LSP edit 缺少 {key}"))
    };
    let new_text = value
        .get("newText")
        .and_then(serde_json::Value::as_str)
        .ok_or("LSP edit 缺少 newText")?;
    Ok(LspTextEdit {
        start_line: number(start, "line")?,
        start_character: number(start, "character")?,
        end_line: number(end, "line")?,
        end_character: number(end, "character")?,
        new_text: new_text.to_string(),
    })
}

fn add_document_change(
    root: &Path,
    value: &serde_json::Value,
    out: &mut BTreeMap<String, Vec<LspTextEdit>>,
) -> Result<(), String> {
    let object = value
        .as_object()
        .ok_or("documentChanges entry 必须是对象")?;
    let edits = object
        .get("edits")
        .and_then(serde_json::Value::as_array)
        .ok_or("documentChanges entry 缺少 edits")?;
    let object_value = serde_json::Value::Object(object.clone());
    let uri = object_value
        .pointer("/textDocument/uri")
        .and_then(serde_json::Value::as_str)
        .ok_or("documentChanges entry 缺少 textDocument.uri")?;
    let path = relative_uri_path(root, uri)?;
    let target = out.entry(path).or_default();
    for edit in edits {
        target.push(parse_edit(edit)?);
    }
    Ok(())
}

pub fn normalize_workspace_edit(
    root: &Path,
    edit: &serde_json::Value,
) -> Result<BTreeMap<String, Vec<LspTextEdit>>, String> {
    if !edit.is_object() {
        return Err("workspace edit 必须是对象".into());
    }
    let mut out: BTreeMap<String, Vec<LspTextEdit>> = BTreeMap::new();
    if let Some(changes) = edit.get("changes").and_then(serde_json::Value::as_object) {
        for (uri, edits) in changes {
            let path = relative_uri_path(root, uri)?;
            let target = out.entry(path).or_default();
            let edits = edits.as_array().ok_or("changes edits 必须是数组")?;
            for edit in edits {
                target.push(parse_edit(edit)?);
            }
        }
    }
    if let Some(document_changes) = edit
        .get("documentChanges")
        .and_then(serde_json::Value::as_array)
    {
        for change in document_changes {
            add_document_change(root, change, &mut out)?;
        }
    }
    if out.values().all(|edits| edits.is_empty()) {
        return Err("workspace edit 不包含文本修改".into());
    }
    Ok(out)
}

pub fn apply_text_edits(source: &str, edits: &[LspTextEdit]) -> Result<String, String> {
    let mut offsets = edits
        .iter()
        .map(|edit| {
            Ok((
                absolute_offset(source, edit.start_line, edit.start_character)?,
                absolute_offset(source, edit.end_line, edit.end_character)?,
                edit,
            ))
        })
        .collect::<Result<Vec<_>, String>>()?;
    offsets.sort_by(|left, right| right.0.cmp(&left.0).then(right.1.cmp(&left.1)));
    if let Some(window) = offsets.windows(2).next() {
        let (later_start, _, _) = window[0];
        let (_, earlier_end, _) = window[1];
        if later_start < earlier_end {
            return Err("LSP edits 相互重叠".into());
        }
    }
    let mut output = source.to_string();
    for (start, end, edit) in offsets {
        output.replace_range(start..end, &edit.new_text);
    }
    Ok(output)
}

pub fn plan_workspace_edit(
    root: &Path,
    edit: &serde_json::Value,
) -> Result<Vec<PlannedTextFile>, String> {
    let mut planned = Vec::new();
    for (path, edits) in normalize_workspace_edit(root, edit)? {
        let absolute = root.join(&path);
        let before = std::fs::read_to_string(&absolute).map_err(|e| format!("读取 {path}: {e}"))?;
        let after = apply_text_edits(&before, &edits)?;
        if before != after {
            planned.push(PlannedTextFile {
                path: path.clone(),
                diff: unified_diff(&before, &after, path.as_str()),
                before,
                after,
            });
        }
    }
    if planned.is_empty() {
        return Err("workspace edit 未产生内容变化".into());
    }
    Ok(planned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn utf16_ranges_use_unicode_scalar_columns() {
        let source = "const s = \"漢字\";\nconst n = 1;\n";
        let edit = LspTextEdit {
            start_line: 0,
            start_character: 11,
            end_line: 0,
            end_character: 13,
            new_text: "X".into(),
        };
        assert_eq!(
            apply_text_edits(source, &[edit]).unwrap(),
            "const s = \"X\";\nconst n = 1;\n"
        );
    }

    #[test]
    fn normalizes_changes_and_document_changes_without_writing() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo");
        let file_a = root.join("a.rs");
        let file_b = root.join("src/b.rs");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(&file_a, "old a\n").unwrap();
        std::fs::create_dir_all(file_b.parent().unwrap()).unwrap();
        std::fs::write(&file_b, "old b\n").unwrap();
        let uri_a = format!("file://{}", file_a.to_string_lossy());
        let uri_b = format!("file://{}", file_b.to_string_lossy());
        let edit = json!({
            "changes": { uri_a.clone(): [{ "range": {"start":{"line":0,"character":0},"end":{"line":0,"character":3}}, "newText": "new a" }]},
            "documentChanges": [{ "textDocument": {"uri": uri_b}, "edits": [{ "range": {"start":{"line":0,"character":0},"end":{"line":0,"character":3}}, "newText": "new b" }]}]
        });
        let normalized = normalize_workspace_edit(&root, &edit).unwrap();
        assert_eq!(normalized.len(), 2);
        let planned = plan_workspace_edit(&root, &edit).unwrap();
        assert_eq!(planned.len(), 2);
        assert!(planned.iter().all(|file| file.diff.contains("@@")));
        assert_eq!(std::fs::read_to_string(file_a).unwrap(), "old a\n");
        assert_eq!(std::fs::read_to_string(file_b).unwrap(), "old b\n");
    }

    #[test]
    fn rejects_overlap_out_of_root_and_empty_edits() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.rs"), "abcdef\n").unwrap();
        assert!(normalize_workspace_edit(&root, &json!({"changes": {}})).is_err());
        let outside = "file:///etc/hosts".to_string();
        assert!(normalize_workspace_edit(
            &root,
            &json!({"changes": {outside: [{"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":0}},"newText":"x"}]}})
        ).is_err());
        let source = "abcdef\n";
        let edits = [
            LspTextEdit {
                start_line: 0,
                start_character: 0,
                end_line: 0,
                end_character: 3,
                new_text: "x".into(),
            },
            LspTextEdit {
                start_line: 0,
                start_character: 2,
                end_line: 0,
                end_character: 4,
                new_text: "y".into(),
            },
        ];
        assert!(apply_text_edits(source, &edits).is_err());
    }
}
