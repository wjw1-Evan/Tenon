//! LSP 管理器（设计方案 §8.5 共享 LSP 多路复用 / §15 POST /lsp）：
//! 一项目 × 语言一个宿主实例，编辑器与代理共用；文件打开 / 变更自动同步；
//! 语义操作：completion / hover / definition / references / diagnostics / format。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

use crate::guard::LspGuardConfig;
use crate::host::{LspHost, LspHostConfig};
use crate::pack::{file_uri, language_id_for, pack_for_file};
use crate::transport::ProcessConnection;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(45);
/// 诊断推送等待窗口。
const DIAGNOSTICS_WAIT: Duration = Duration::from_secs(15);

#[derive(Debug, thiserror::Error)]
pub enum LspManagerError {
    #[error("语言包不可用：{0}")]
    PackUnavailable(String),
    #[error("语言服务器请求失败：{0}")]
    Request(String),
    #[error("文件不在项目内：{0}")]
    BadPath(String),
}

struct HostEntry {
    host: Arc<LspHost>,
    /// 已 didOpen 的 URI → (version, 内容缓存)
    open_docs: HashMap<String, (i64, String)>,
    /// 诊断缓存：URI → (收到时刻, publishDiagnostics params)
    diagnostics: Arc<std::sync::Mutex<HashMap<String, (Instant, serde_json::Value)>>>,
}

type HostEntryCell = Arc<std::sync::Mutex<HostEntry>>;

/// 共享 LSP 管理器：键 = (项目根, 语言)。
#[derive(Default)]
pub struct LspManager {
    hosts: Mutex<HashMap<(PathBuf, String), HostEntryCell>>,
}

impl std::fmt::Debug for LspManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LspManager").finish_non_exhaustive()
    }
}

impl LspManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// 测试注入：预置宿主（假服务器）。
    pub async fn insert_host(&self, root: PathBuf, language: &str, host: Arc<LspHost>) {
        self.hosts.lock().await.insert(
            (root, language.to_string()),
            Arc::new(std::sync::Mutex::new(HostEntry {
                host,
                open_docs: HashMap::new(),
                diagnostics: Arc::new(std::sync::Mutex::new(HashMap::new())),
            })),
        );
    }

    /// 关闭某个项目的全部语言服务器（ProjectRuntime 关闭 / 空闲回收，§6.4）。
    pub async fn close_project(&self, project_root: &Path) -> usize {
        let mut hosts = self.hosts.lock().await;
        let keys: Vec<(PathBuf, String)> = hosts
            .keys()
            .filter(|(root, _)| root == project_root)
            .cloned()
            .collect();
        let removed: Vec<HostEntryCell> = keys
            .into_iter()
            .filter_map(|key| hosts.remove(&key))
            .collect();
        let count = removed.len();
        // 阻塞 shutdown 放到独立线程，避免持有 tokio 锁或卡住 async worker。
        for entry in removed {
            let host = entry.lock().expect("host entry lock").host.clone();
            tokio::task::spawn_blocking(move || host.shutdown());
        }
        count
    }

    async fn entry(
        &self,
        root: &Path,
        file: &str,
    ) -> Result<Arc<std::sync::Mutex<HostEntry>>, LspManagerError> {
        let Some(pack) = pack_for_file(root, file) else {
            return Err(LspManagerError::PackUnavailable(format!(
                "无 {file} 对应的内置语言包"
            )));
        };
        let key = (root.to_path_buf(), pack.language.to_string());
        let mut hosts = self.hosts.lock().await;
        if let Some(e) = hosts.get(&key) {
            return Ok(e.clone());
        }
        if !command_on_path(&pack.command) {
            return Err(LspManagerError::PackUnavailable(format!(
                "命令 {} 未安装（语言包 {}；安装向导随 M1 语言包体系）",
                pack.command, pack.language
            )));
        }
        // 启动语言服务器（§12.1 铁律七：宿主命令白名单默认空 = 全拒）
        let conn = ProcessConnection::spawn_str(&pack.command, &pack.args, root)
            .map_err(|e| LspManagerError::Request(format!("启动 {} 失败: {e}", pack.command)))?;
        let cfg = LspHostConfig {
            language: pack.language.to_string(),
            root_path: root.to_path_buf(),
            guard: LspGuardConfig::new(root),
            initialization_options: init_options_for(&pack, root),
        };
        let host = LspHost::connect(cfg, Box::new(conn.reader), Box::new(conn.writer));
        host.initialize(REQUEST_TIMEOUT)
            .map_err(|e| LspManagerError::Request(format!("initialize 失败: {e}")))?;

        // 诊断缓存线程：收集 publishDiagnostics 推送
        let diagnostics: Arc<std::sync::Mutex<HashMap<String, (Instant, serde_json::Value)>>> =
            Arc::new(std::sync::Mutex::new(HashMap::new()));
        let rx = host.subscribe();
        let diag_cache = diagnostics.clone();
        std::thread::spawn(move || {
            for note in rx {
                if note.method == "textDocument/publishDiagnostics" {
                    if let Some(uri) = note.params.get("uri").and_then(|u| u.as_str()) {
                        diag_cache
                            .lock()
                            .expect("diag lock")
                            .insert(uri.to_string(), (Instant::now(), note.params.clone()));
                    }
                }
            }
        });

        let entry = Arc::new(std::sync::Mutex::new(HostEntry {
            host,
            open_docs: HashMap::new(),
            diagnostics,
        }));
        hosts.insert(key, entry.clone());
        Ok(entry)
    }

    /// 语义请求入口（§15 POST /lsp）。
    ///
    /// `op ∈ { completion, hover, definition, references, diagnostics,
    /// codeaction, rename, workspace_symbol, signature_help, format }`；
    /// `line` / `character` 为 LSP 0-based 坐标（character 为 UTF-16 单位）；
    /// `extra`：rename → `new_name`，workspace_symbol → `query`。
    pub async fn request(
        &self,
        project_root: &Path,
        file: &str,
        op: &str,
        line: u32,
        character: u32,
        extra: Option<&str>,
    ) -> Result<serde_json::Value, LspManagerError> {
        // spawn_blocking 需 'static：转为 owned
        let extra_owned: Option<String> = extra.map(String::from);
        // 路径边界：目标文件必须位于项目内（§12.1 铁律七的宿主侧边界检查）
        let uri = file_uri(project_root, file);
        if !crate::guard::uri_within(project_root, &uri) {
            return Err(LspManagerError::BadPath(file.to_string()));
        }

        let entry = self.entry(project_root, file).await?;
        let host = entry.lock().expect("host entry lock").host.clone();

        // didOpen / didChange 同步：磁盘内容与缓存不一致 → 全量变更（version+1）。
        // 锁内只做决策，阻塞 IO 在锁外执行（std Mutex 不得跨 await）
        let disk = std::fs::read_to_string(project_root.join(file))
            .map_err(|err| LspManagerError::Request(format!("读取 {file} 失败: {err}")))?;
        enum Sync {
            Open(serde_json::Value),
            Change { ver: i64, params: serde_json::Value },
            None { ver: i64 },
        }
        let sync = {
            let mut e = entry.lock().expect("host entry lock");
            match e.open_docs.get(&uri) {
                None => {
                    let params = serde_json::json!({
                        "textDocument": {
                            "uri": uri,
                            "languageId": language_id_for(file),
                            "version": 1,
                            "text": disk,
                        }
                    });
                    e.open_docs.insert(uri.clone(), (1, disk));
                    Sync::Open(params)
                }
                Some((ver, cached)) if *cached != disk => {
                    let new_ver = ver + 1;
                    let params = serde_json::json!({
                        "textDocument": { "uri": uri, "version": new_ver },
                        "contentChanges": [{ "text": disk }],
                    });
                    e.open_docs.insert(uri.clone(), (new_ver, disk));
                    Sync::Change {
                        ver: new_ver,
                        params,
                    }
                }
                Some((ver, _)) => Sync::None { ver: *ver },
            }
        };
        let version = match sync {
            Sync::Open(params) => {
                let h = host.clone();
                run_blocking(move || h.notify("textDocument/didOpen", params)).await?;
                1
            }
            Sync::Change { ver, params } => {
                let h = host.clone();
                run_blocking(move || h.notify("textDocument/didChange", params)).await?;
                ver
            }
            Sync::None { ver } => ver,
        };
        let diag_cache = entry.lock().expect("host entry lock").diagnostics.clone();

        let text_doc = serde_json::json!({ "uri": uri, "version": version });
        let position = serde_json::json!({ "line": line, "character": character });

        // workspace/symbol：按查询名搜全工作区符号（不绑定单文件）
        if op == "workspace_symbol" {
            let query = extra_owned.clone().unwrap_or_default();
            let h = host.clone();
            return run_blocking(move || {
                h.request(
                    "workspace/symbol",
                    serde_json::json!({ "query": query }),
                    REQUEST_TIMEOUT,
                )
            })
            .await;
        }

        if op == "diagnostics" {
            // 拉取优先（LSP 3.17 textDocument/diagnostic）；不支持拉取的服务器
            // 退回等待 publishDiagnostics 推送（§15 diagnostics 语义）
            let h = host.clone();
            let td = text_doc.clone();
            let pull = run_blocking(move || {
                h.request(
                    "textDocument/diagnostic",
                    serde_json::json!({ "textDocument": td }),
                    Duration::from_secs(10),
                )
            })
            .await
            .ok();
            if let Some(report) = pull {
                let kind = report.get("kind").and_then(|k| k.as_str()).unwrap_or("");
                if kind == "full" {
                    let items = report.get("items").cloned().unwrap_or_default();
                    // 刚 didOpen 时，部分服务器先回 full 空，再推真实诊断；
                    // 给推送一个窗口，若得到非空则优先采用。
                    let pushed = wait_diagnostics(&diag_cache, &uri, DIAGNOSTICS_WAIT)?;
                    let pushed_items = pushed.as_array().cloned().unwrap_or_default();
                    if !pushed_items.is_empty() {
                        return Ok(serde_json::json!({ "items": pushed_items }));
                    }
                    return Ok(serde_json::json!({ "items": items }));
                }
                if kind == "unchanged" {
                    return Ok(serde_json::json!({ "items": [] }));
                }
            }
            let items = wait_diagnostics(&diag_cache, &uri, DIAGNOSTICS_WAIT)?;
            return Ok(serde_json::json!({ "items": items }));
        }

        let (method, params): (&str, serde_json::Value) = match op {
            "completion" => (
                "textDocument/completion",
                serde_json::json!({
                    "textDocument": text_doc,
                    "position": position,
                    "context": { "triggerKind": 1 },
                }),
            ),
            "hover" => (
                "textDocument/hover",
                serde_json::json!({ "textDocument": text_doc, "position": position }),
            ),
            "definition" => (
                "textDocument/definition",
                serde_json::json!({ "textDocument": text_doc, "position": position }),
            ),
            "references" => (
                "textDocument/references",
                serde_json::json!({
                    "textDocument": text_doc,
                    "position": position,
                    "context": { "includeDeclaration": true },
                }),
            ),
            "codeaction" => (
                "textDocument/codeAction",
                serde_json::json!({
                    "textDocument": text_doc,
                    "range": { "start": position, "end": position },
                    "context": { "diagnostics": [], "only": ["quickfix"] },
                }),
            ),
            "signature_help" => (
                "textDocument/signatureHelp",
                serde_json::json!({ "textDocument": text_doc, "position": position }),
            ),
            "rename" => {
                let Some(new_name) = extra_owned.clone() else {
                    return Err(LspManagerError::Request(
                        "rename 需 extra=new_name（§15：安全重命名走 LSP workspace edit，§8.1）"
                            .into(),
                    ));
                };
                let new_name = new_name.to_string();
                (
                    "textDocument/rename",
                    serde_json::json!({
                        "textDocument": text_doc,
                        "position": position,
                        "newName": new_name,
                    }),
                )
            }
            "format" => (
                "textDocument/formatting",
                serde_json::json!({
                    "textDocument": text_doc,
                    "options": { "tabSize": 4, "insertSpaces": true },
                }),
            ),
            other => return Err(LspManagerError::Request(format!("未知语义操作: {other}"))),
        };

        let h = host.clone();
        run_blocking(move || h.request(method, params, REQUEST_TIMEOUT)).await
    }
}

async fn run_blocking<T, F>(f: F) -> Result<T, LspManagerError>
where
    F: FnOnce() -> Result<T, crate::host::LspHostError> + Send + 'static,
    T: Send + 'static,
{
    // LspHost 是阻塞实现；丢进阻塞线程池避免卡住 async 执行器
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| LspManagerError::Request(format!("join 失败: {e}")))?
        .map_err(|e| LspManagerError::Request(e.to_string()))
}

/// 初始化选项：TypeScript 服务器在无工作区 typescript 时按序回退：
/// 工作区 → `~/.tenon/runtimes/typescript`（§8.4 语言包专用锁定版运行时）→ 全局旧布局。
fn init_options_for(pack: &crate::pack::LanguagePack, root: &Path) -> Option<serde_json::Value> {
    if pack.language != "typescript" {
        return None;
    }
    if root
        .join("node_modules/typescript/lib/tsserver.js")
        .exists()
    {
        return None; // 工作区自带，交给服务器自取
    }
    let candidates = [
        tenon_config::Config::data_dir().join("runtimes/typescript/node_modules/typescript/lib"),
        global_npm_root()
            .map(|r| PathBuf::from(r).join("typescript/lib"))
            .unwrap_or_default(),
    ];
    candidates
        .into_iter()
        .find(|dir| dir.join("tsserver.js").exists())
        .map(|dir| serde_json::json!({ "tsserver": { "path": dir } }))
}

/// `npm root -g`（缓存一次）。
fn global_npm_root() -> Option<String> {
    static ROOT: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    ROOT.get_or_init(|| {
        std::process::Command::new("npm")
            .args(["root", "-g"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty())
    })
    .clone()
}

fn wait_diagnostics(
    cache: &std::sync::Mutex<HashMap<String, (Instant, serde_json::Value)>>,
    uri: &str,
    window: Duration,
) -> Result<serde_json::Value, LspManagerError> {
    let deadline = Instant::now() + window;
    // 竞态防御：部分服务器（tsserver）先推一份空诊断、分析完成后再推真实
    // 结果；若首个空推送立即返回，会把真实类型错误吞成「干净」（验证通道
    // 误判）。空推送时给一个短宽限窗口等非空更新，干净文件最多多等 3s。
    const EMPTY_PUSH_GRACE: Duration = Duration::from_secs(3);
    let mut grace_deadline: Option<Instant> = None;
    loop {
        {
            let cache = cache.lock().expect("diag lock");
            if let Some((at, params)) = cache.get(uri) {
                if at.elapsed() < Duration::from_secs(30) {
                    let items = params.get("diagnostics").cloned().unwrap_or_default();
                    let is_empty = items.as_array().map(|a| a.is_empty()).unwrap_or(false);
                    if !is_empty {
                        return Ok(items);
                    }
                    let ed = Instant::now() + EMPTY_PUSH_GRACE;
                    grace_deadline = Some(match grace_deadline {
                        Some(d) => d.min(ed),
                        None => ed,
                    });
                }
            }
        }
        let effective_deadline = grace_deadline.map_or(deadline, |d| d.min(deadline));
        if Instant::now() >= effective_deadline {
            // 宽限内无非空更新：按最后已知（空）诊断返回（服务器可能不支持
            // 该文档的诊断，或文件确实干净）
            return Ok(serde_json::Value::Array(vec![]));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// PATH 上是否存在命令（避免引入 which 依赖）。
pub fn command_on_path(command: &str) -> bool {
    if command.contains('/') {
        return Path::new(command).exists();
    }
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|dir| dir.join(command))
        .any(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_on_path_finds_installed_servers() {
        assert!(command_on_path("node"), "node 应在 PATH");
        assert!(!command_on_path("definitely-not-a-real-lsp-server"));
    }

    #[tokio::test]
    async fn close_project_without_hosts_returns_zero() {
        let manager = LspManager::new();
        let root = std::env::temp_dir().join("tenon-test-lsp-empty");
        let closed = manager.close_project(&root).await;
        assert_eq!(closed, 0);
    }

    #[test]
    fn command_on_path_various() {
        assert!(command_on_path("node"));
        assert!(command_on_path("cargo"));
        assert!(!command_on_path(""));
        assert!(!command_on_path("fake_cmd_12345"));
    }
}