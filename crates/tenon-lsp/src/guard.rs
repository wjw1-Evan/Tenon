//! 铁律七守卫（设计方案 §12.1 / ADR-9）：语言服务器 → 宿主的请求全部过审。

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// 宿主对服务器请求的裁决。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GuardDecision {
    /// 白名单放行（仅 executeCommand 且命令在 allowed_commands）。
    Allowed,
    /// 拒绝（默认）。
    Denied,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerRequestReport {
    pub method: String,
    pub decision: GuardDecision,
    pub detail: String,
}

#[derive(Debug, Clone)]
pub struct LspGuardConfig {
    /// 宿主命令白名单（铁律七；默认空 = 全拒，附录 E `[lsp].allowed_commands`）。
    pub allowed_commands: HashSet<String>,
    /// 项目根（workspace edits / showDocument 的路径边界）。
    pub project_root: PathBuf,
}

impl LspGuardConfig {
    pub fn new(project_root: impl Into<PathBuf>) -> Self {
        Self {
            allowed_commands: HashSet::new(),
            project_root: project_root.into(),
        }
    }
}

/// LSP 守卫：审查服务器下发请求。
pub struct LspGuard {
    config: LspGuardConfig,
    reports: std::sync::Mutex<Vec<ServerRequestReport>>,
}

impl LspGuard {
    pub fn new(config: LspGuardConfig) -> Self {
        Self {
            config,
            reports: std::sync::Mutex::new(Vec::new()),
        }
    }

    pub fn reports(&self) -> Vec<ServerRequestReport> {
        self.reports.lock().expect("guard lock").clone()
    }

    /// `workspace/executeCommand`：白名单外一律拒绝（ADR-9：宿主永不执行
    /// 服务器下发的任意命令——防「沙箱内进程借宿主之手逃逸」）。
    pub fn check_execute_command(&self, command: &str) -> GuardDecision {
        let d = if self.config.allowed_commands.contains(command) {
            GuardDecision::Allowed
        } else {
            GuardDecision::Denied
        };
        self.record("workspace/executeCommand", d, format!("command={command}"));
        d
    }

    /// `workspace/applyEdit`：所有编辑目标必须位于项目内（B 级写守卫边界）。
    /// 返回放行与否；放行后实际写盘仍由调用方走写守卫。
    pub fn check_workspace_edit(&self, edit: &serde_json::Value) -> GuardDecision {
        let mut paths = Vec::new();
        if let Some(changes) = edit.get("changes").and_then(|c| c.as_object()) {
            for key in changes.keys() {
                paths.push(key.clone());
            }
        }
        if let Some(items) = edit.get("documentChanges").and_then(|d| d.as_array()) {
            for item in items {
                if let Some(uri) = item
                    .get("textDocument")
                    .and_then(|t| t.get("uri"))
                    .and_then(|u| u.as_str())
                {
                    paths.push(uri.to_string());
                }
            }
        }
        let mut all_inside = true;
        for p in &paths {
            if !uri_within(&self.config.project_root, p) {
                all_inside = false;
            }
        }
        let d = if all_inside && !paths.is_empty() {
            GuardDecision::Allowed
        } else {
            GuardDecision::Denied
        };
        self.record("workspace/applyEdit", d, format!("files={paths:?}"));
        d
    }

    /// `window/showDocument`：仅项目内文件 URI；外部 URI / URL 一律拒绝。
    pub fn check_show_document(&self, uri: &str) -> GuardDecision {
        let d = if uri.starts_with("file://") && uri_within(&self.config.project_root, uri) {
            GuardDecision::Allowed
        } else {
            GuardDecision::Denied
        };
        self.record("window/showDocument", d, format!("uri={uri}"));
        d
    }

    /// `client/registerCapability`：默认拒绝（动态注册能力不自动放行，铁律七）。
    pub fn check_register_capability(&self, capability: &str) -> GuardDecision {
        let d = GuardDecision::Denied;
        self.record(
            "client/registerCapability",
            d,
            format!("capability={capability}"),
        );
        d
    }

    fn record(&self, method: &str, decision: GuardDecision, detail: String) {
        self.reports
            .lock()
            .expect("guard lock")
            .push(ServerRequestReport {
                method: method.to_string(),
                decision,
                detail,
            });
    }
}

/// file:// URI 是否位于项目根内。
pub fn uri_within(project_root: &Path, uri: &str) -> bool {
    let Some(path) = uri_to_path(uri) else {
        return false;
    };
    let root = std::fs::canonicalize(project_root).unwrap_or_else(|_| project_root.to_path_buf());
    // 文件可能尚不存在：先试文件本身，失败则归一化父目录再拼接
    let path = match std::fs::canonicalize(&path) {
        Ok(p) => p,
        Err(_) => {
            let name = path.file_name().map(|n| n.to_os_string());
            match name {
                Some(n) => match path.parent().map(std::fs::canonicalize) {
                    Some(Ok(parent)) => parent.join(n),
                    _ => path,
                },
                None => path,
            }
        }
    };
    path.starts_with(&root)
}

/// file:// URI → 本地路径（做 percent-decoding 与平台差异处理）。
pub fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    // file:///path/to → /path/to
    let raw = if let Some(idx) = rest.find('/') {
        &rest[idx..]
    } else {
        rest
    };
    // percent-decode 按字节累积再整体 UTF-8 解码：逐字节 `as char` 是 Latin-1
    // 语义，`%E4%B8%AD%E6%96%87` 会被拆成 6 个错误字符（非 ASCII 路径全解错，
    // 诊断缓存键与守卫判定连锁失真）
    let bytes = percent_decode(raw.as_bytes())?;
    let decoded = String::from_utf8(bytes).ok()?;
    // Windows: /C:/... → C:/...
    let p = if decoded.len() > 2 && decoded.starts_with('/') && decoded.as_bytes()[2] == b':' {
        &decoded[1..]
    } else {
        &decoded
    };
    Some(PathBuf::from(p))
}

fn percent_decode(input: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        if input[i] == b'%' && i + 2 < input.len() {
            let hex = std::str::from_utf8(&input[i + 1..i + 3]).ok()?;
            if let Ok(v) = u8::from_str_radix(hex, 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(input[i]);
        i += 1;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guard(allowed: &[&str]) -> (tempfile::TempDir, LspGuard) {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = LspGuardConfig::new(dir.path());
        for a in allowed {
            cfg.allowed_commands.insert(a.to_string());
        }
        (dir, LspGuard::new(cfg))
    }

    #[test]
    fn execute_command_denied_by_default() {
        let (_d, g) = guard(&[]);
        assert_eq!(
            g.check_execute_command("editor.action.format"),
            GuardDecision::Denied
        );
        assert_eq!(g.reports().len(), 1);
    }

    #[test]
    fn execute_command_whitelisted_only() {
        let (_d, g) = guard(&["generate"]);
        assert_eq!(g.check_execute_command("generate"), GuardDecision::Allowed);
        assert_eq!(g.check_execute_command("rm -rf /"), GuardDecision::Denied);
    }

    #[test]
    fn workspace_edit_inside_project_allowed() {
        let (d, g) = guard(&[]);
        let file_uri = format!("file://{}", d.path().join("a.rs").to_string_lossy());
        let edit = serde_json::json!({"changes": {file_uri: [{"range": {"start": {"line":0,"character":0}, "end": {"line":0,"character":0}}, "newText": "x"}]}});
        assert_eq!(g.check_workspace_edit(&edit), GuardDecision::Allowed);
    }

    #[test]
    fn workspace_edit_outside_project_denied() {
        let (_d, g) = guard(&[]);
        let edit = serde_json::json!({"changes": {"file:///etc/hosts": []}});
        assert_eq!(g.check_workspace_edit(&edit), GuardDecision::Denied);
    }

    #[test]
    fn show_document_rules() {
        let (d, g) = guard(&[]);
        let inside = format!("file://{}", d.path().join("a.rs").to_string_lossy());
        assert_eq!(g.check_show_document(&inside), GuardDecision::Allowed);
        assert_eq!(
            g.check_show_document("file:///etc/passwd"),
            GuardDecision::Denied
        );
        assert_eq!(
            g.check_show_document("https://evil.example.com/payload"),
            GuardDecision::Denied,
            "外部 URL 拒绝"
        );
    }

    #[test]
    fn register_capability_denied_by_default() {
        let (_d, g) = guard(&[]);
        assert_eq!(
            g.check_register_capability("workspace.didChangeWatchedFiles"),
            GuardDecision::Denied
        );
    }

    #[test]
    fn uri_to_path_percent_decoding() {
        let p = uri_to_path("file:///Users/x/my%20proj/a.rs").unwrap();
        assert_eq!(p, PathBuf::from("/Users/x/my proj/a.rs"));
    }
}
