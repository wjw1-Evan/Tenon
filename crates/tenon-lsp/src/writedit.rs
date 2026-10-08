//! 服务端发起编辑的解析与拼接（§8.5 写回通道 v1.200 库层）。
//!
//! `workspace/applyEdit`（changes / documentChanges 双形态）→ 守卫边界内的
//! 逐文件 UTF-16 位置编辑 → 内存拼接为新内容。**落盘由 daemon 注入的
//! [`EditApplier`] 执行**；缺席（纯库消费方）= `applied:false` 维持 v1.199
//! 前行为，不得谎报落盘。守卫不变式：URI 越界 / 非 file:// / 双形态歧义 /
//! 空编辑集 / 区间重叠 / 行号越界一律拒绝。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::guard::uri_to_path;

/// 守卫放行的编辑落盘执行器：返回成功落盘的文件数。
/// daemon 注入（写盘统一走宿主写链：写守卫 / 脏缓冲协调 / watcher）。
pub type EditApplier = Arc<dyn Fn(Vec<ServerFileEdit>) -> Result<usize, String> + Send + Sync>;

use std::sync::Arc;

/// 单文件编辑集：路径已过项目边界校验（绝对路径）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerFileEdit {
    pub path: PathBuf,
    pub edits: Vec<LspTextEdit>,
}

/// 单条文本编辑：LSP 位置为 UTF-16 码元（行 / 列均 0 基）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LspTextEdit {
    pub start_line: u32,
    pub start_col_utf16: u32,
    pub end_line: u32,
    pub end_col_utf16: u32,
    pub new_text: String,
}

/// 解析 WorkspaceEdit 双形态 → 守卫边界内的逐文件编辑集（编辑按起点升序归一）。
pub fn parse_workspace_edit(
    root: &Path,
    edit: &serde_json::Value,
) -> Result<Vec<ServerFileEdit>, String> {
    let has_changes = edit.get("changes").is_some_and(|c| c.is_object());
    let has_docs = edit.get("documentChanges").is_some_and(|d| d.is_array());
    if has_changes && has_docs {
        return Err("changes 与 documentChanges 不可同时出现".into());
    }
    let mut files: BTreeMap<PathBuf, Vec<LspTextEdit>> = BTreeMap::new();
    let mut push_edit = |uri: &str, e: &serde_json::Value| -> Result<(), String> {
        let path = uri_to_path(uri).ok_or_else(|| format!("非法 file URI: {uri}"))?;
        if !path.starts_with(root) {
            return Err(format!("越界路径: {}", path.display()));
        }
        let range = e.get("range").ok_or("编辑缺少 range")?;
        let (sl, sc) = parse_pos(range.get("start"))?;
        let (el, ec) = parse_pos(range.get("end"))?;
        let new_text = e
            .get("newText")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string();
        files.entry(path).or_default().push(LspTextEdit {
            start_line: sl,
            start_col_utf16: sc,
            end_line: el,
            end_col_utf16: ec,
            new_text,
        });
        Ok(())
    };
    if has_changes {
        for (uri, edits) in edit["changes"].as_object().expect("changes is_object") {
            let arr = edits.as_array().ok_or("changes 条目须为数组")?;
            for e in arr {
                push_edit(uri, e)?;
            }
        }
    }
    if has_docs {
        for item in edit["documentChanges"]
            .as_array()
            .expect("documentChanges is_array")
        {
            let uri = item
                .get("textDocument")
                .and_then(|t| t.get("uri"))
                .and_then(|u| u.as_str())
                .ok_or("documentChanges 条目缺少 textDocument.uri")?;
            let arr = item
                .get("edits")
                .and_then(|x| x.as_array())
                .ok_or("documentChanges.edits 须为数组")?;
            for e in arr {
                push_edit(uri, e)?;
            }
        }
    }
    if files.is_empty() {
        return Err("空编辑集".into());
    }
    let mut out = Vec::new();
    for (path, mut edits) in files {
        edits.sort_by_key(|e| (e.start_line, e.start_col_utf16));
        out.push(ServerFileEdit { path, edits });
    }
    Ok(out)
}

fn parse_pos(v: Option<&serde_json::Value>) -> Result<(u32, u32), String> {
    let v = v.ok_or("range 缺少起点 / 终点")?;
    let line = v
        .get("line")
        .and_then(|x| x.as_u64())
        .ok_or("range.line 须为非负整数")?;
    let character = v
        .get("character")
        .and_then(|x| x.as_u64())
        .ok_or("range.character 须为非负整数")?;
    Ok((
        u32::try_from(line).map_err(|_| "range.line 越界")?,
        u32::try_from(character).map_err(|_| "range.character 越界")?,
    ))
}

/// 按 UTF-16 位置把编辑集拼接入内存内容，返回新内容。
/// 编辑按起点**降序**应用（早前偏移不受后续拼接影响）；区间重叠 / 终点在
/// 起点前 / 行号越界拒绝；列越界钳到行尾（对 server 宽容）。
pub fn apply_file_edits(content: &str, edits: &[LspTextEdit]) -> Result<String, String> {
    if edits.is_empty() {
        return Err("空编辑".into());
    }
    let mut sorted: Vec<&LspTextEdit> = edits.iter().collect();
    sorted.sort_by_key(|e| std::cmp::Reverse((e.start_line, e.start_col_utf16)));
    // 行首字节偏移表（\n 为行分隔；\r 属于行内容，计入 UTF-16 列）
    let mut line_starts: Vec<usize> = vec![0];
    for (i, b) in content.bytes().enumerate() {
        if b == b'\n' {
            line_starts.push(i + 1);
        }
    }
    let line_count = line_starts.len();
    let conv = |line: u32, col: u32| -> Result<usize, String> {
        let line = line as usize;
        if line >= line_count {
            return Err(format!("行号越界: {line}（共 {line_count} 行）"));
        }
        let line_start = line_starts[line];
        let line_end = if line + 1 < line_count {
            line_starts[line + 1] - 1 // 去掉 \n
        } else {
            content.len()
        };
        let line_str = &content[line_start..line_end];
        Ok(line_start + byte_offset_in_line(line_str, col))
    };
    let mut out = content.to_string();
    let mut prev_start: Option<(u32, u32)> = None;
    for e in &sorted {
        if let Some(ps) = prev_start {
            if (e.end_line, e.end_col_utf16) > ps {
                return Err("编辑区间重叠".into());
            }
        }
        let s = conv(e.start_line, e.start_col_utf16)?;
        let en = conv(e.end_line, e.end_col_utf16)?;
        if en < s {
            return Err("编辑终点在起点之前".into());
        }
        out.replace_range(s..en, &e.new_text);
        prev_start = Some((e.start_line, e.start_col_utf16));
    }
    Ok(out)
}

/// 行内 UTF-16 列 → 字节偏移；落在代理对中间的列解析到**包含它的字符起点**
/// （首尾钳制对称），列越界钳到行尾。
fn byte_offset_in_line(line: &str, col_utf16: u32) -> usize {
    let mut covered: u32 = 0;
    for (idx, ch) in line.char_indices() {
        let next = covered + ch.len_utf16() as u32;
        if col_utf16 < next {
            return idx;
        }
        covered = next;
    }
    line.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;

    fn root() -> PathBuf {
        PathBuf::from("/proj")
    }

    fn uri_changes() -> serde_json::Value {
        json!({
            "changes": {
                "file:///proj/a.rs": [
                    {"range": {"start": {"line": 1, "character": 0}, "end": {"line": 1, "character": 5}}, "newText": "BBB"}
                ],
                "file:///proj/src/b.ts": [
                    {"range": {"start": {"line": 0, "character": 2}, "end": {"line": 0, "character": 4}}, "newText": "X"}
                ]
            }
        })
    }

    #[test]
    fn parse_changes_form_groups_by_file() {
        let edits = parse_workspace_edit(&root(), &uri_changes()).unwrap();
        assert_eq!(edits.len(), 2);
        let a = edits.iter().find(|e| e.path.ends_with("a.rs")).unwrap();
        assert_eq!(a.edits.len(), 1);
        assert_eq!(a.edits[0].new_text, "BBB");
        assert_eq!(a.edits[0].start_line, 1);
    }

    #[test]
    fn parse_document_changes_form() {
        let v = json!({
            "documentChanges": [
                {"textDocument": {"uri": "file:///proj/a.rs", "version": 3},
                 "edits": [{"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 0}}, "newText": "插入"}]}
            ]
        });
        let edits = parse_workspace_edit(&root(), &v).unwrap();
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].edits[0].new_text, "插入");
    }

    #[test]
    fn parse_rejects_ambiguous_out_of_root_empty_and_non_file() {
        // 双形态歧义
        let both = json!({
            "changes": {"file:///proj/a.rs": []},
            "documentChanges": []
        });
        assert!(parse_workspace_edit(&root(), &both).is_err());
        // 越界
        let outside = json!({"changes": {"file:///etc/passwd": [
            {"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 0}}, "newText": "x"}
        ]}});
        assert!(parse_workspace_edit(&root(), &outside).is_err());
        // 非 file://
        let http = json!({"changes": {"https://evil/x": [
            {"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 0}}, "newText": "x"}
        ]}});
        assert!(parse_workspace_edit(&root(), &http).is_err());
        // 空编辑集
        assert!(parse_workspace_edit(&root(), &json!({"changes": {}})).is_err());
    }

    #[test]
    fn apply_simple_replace_and_multiline() {
        let content = "alpha\nbeta\ngamma\n";
        let edits = [LspTextEdit {
            start_line: 1,
            start_col_utf16: 0,
            end_line: 1,
            end_col_utf16: 4,
            new_text: "BETA!".into(),
        }];
        assert_eq!(
            apply_file_edits(content, &edits).unwrap(),
            "alpha\nBETA!\ngamma\n"
        );
        // 跨行替换
        let edits = [LspTextEdit {
            start_line: 0,
            start_col_utf16: 2,
            end_line: 2,
            end_col_utf16: 2,
            new_text: "MID".into(),
        }];
        assert_eq!(apply_file_edits(content, &edits).unwrap(), "alMIDmma\n");
    }

    #[test]
    fn apply_utf16_columns_with_cjk_and_emoji() {
        // 行：`中文 🦀 x` → UTF-16 列：中(1) 文(1) 空格(1) 🦀(2) 空格(1) x(1)
        let content = "中文 🦀 x\n";
        // 替换 x（col 6）：0..6 累计 6 个 UTF-16 码元后到 x
        let edits = [LspTextEdit {
            start_line: 0,
            start_col_utf16: 6,
            end_line: 0,
            end_col_utf16: 7,
            new_text: "Y".into(),
        }];
        assert_eq!(apply_file_edits(content, &edits).unwrap(), "中文 🦀 Y\n");
        // 列 4 落在 🦀 代理对中间 → 解析到包含它的字符起点（首尾钳制对称）：
        // [4,6) = 🦀 + 空格整体替换
        let edits = [LspTextEdit {
            start_line: 0,
            start_col_utf16: 4,
            end_line: 0,
            end_col_utf16: 6,
            new_text: "[蟹]".into(),
        }];
        assert_eq!(apply_file_edits(content, &edits).unwrap(), "中文 [蟹]x\n");
    }

    #[test]
    fn apply_multiple_edits_unsorted_and_descending_order() {
        let content = "one\ntwo\nthree\n";
        // 乱序传入：先应用靠后的编辑不破坏靠前偏移
        let edits = [
            LspTextEdit {
                start_line: 0,
                start_col_utf16: 0,
                end_line: 0,
                end_col_utf16: 3,
                new_text: "ONE".into(),
            },
            LspTextEdit {
                start_line: 2,
                start_col_utf16: 0,
                end_line: 2,
                end_col_utf16: 5,
                new_text: "THREE".into(),
            },
        ];
        assert_eq!(
            apply_file_edits(content, &edits).unwrap(),
            "ONE\ntwo\nTHREE\n"
        );
    }

    #[test]
    fn apply_rejects_overlap_out_of_bounds_and_backward() {
        let content = "ab\ncd\n";
        // 重叠：[0,0-0,2] 与 [0,1-0,2]
        let overlap = [
            LspTextEdit {
                start_line: 0,
                start_col_utf16: 0,
                end_line: 0,
                end_col_utf16: 2,
                new_text: "X".into(),
            },
            LspTextEdit {
                start_line: 0,
                start_col_utf16: 1,
                end_line: 0,
                end_col_utf16: 2,
                new_text: "Y".into(),
            },
        ];
        assert!(apply_file_edits(content, &overlap).is_err());
        // 行号越界
        let oob = [LspTextEdit {
            start_line: 9,
            start_col_utf16: 0,
            end_line: 9,
            end_col_utf16: 1,
            new_text: "X".into(),
        }];
        assert!(apply_file_edits(content, &oob).is_err());
        // 终点在起点前
        let backward = [LspTextEdit {
            start_line: 1,
            start_col_utf16: 2,
            end_line: 1,
            end_col_utf16: 0,
            new_text: "X".into(),
        }];
        assert!(apply_file_edits(content, &backward).is_err());
        // 空编辑
        assert!(apply_file_edits(content, &[]).is_err());
    }

    #[test]
    fn apply_col_clamps_to_line_end_and_crlf_content() {
        // 列越界钳到行尾
        let content = "ab\n";
        let edits = [LspTextEdit {
            start_line: 0,
            start_col_utf16: 99,
            end_line: 0,
            end_col_utf16: 99,
            new_text: "!".into(),
        }];
        assert_eq!(apply_file_edits(content, &edits).unwrap(), "ab!\n");
        // CRLF：\r 属于行内容（col 2 = \r 位置）
        let crlf = "ab\r\ncd\r\n";
        let edits = [LspTextEdit {
            start_line: 0,
            start_col_utf16: 2,
            end_line: 0,
            end_col_utf16: 3,
            new_text: "".into(),
        }];
        assert_eq!(apply_file_edits(crlf, &edits).unwrap(), "ab\ncd\r\n");
    }
}
