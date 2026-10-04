//! 工具执行器（设计方案 §9.2 内置工具协议）：分级执行 + 写守卫 + 快照联动。

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tenon_core::tools::PatchOp;
use tenon_fs::{FileOps, FileService};
use tenon_sandbox::exec_command;
use tenon_sandbox::WriteGuard;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolOutput {
    pub ok: bool,
    pub content: String,
    /// B 级写操作产生的改动文件集（供熔断 / checkpoint / revert 记账）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changed_files: Vec<String>,
    /// 命令输出（run_tests / run_build）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// 人机共编三方合并冲突（§8.6）：写入被阻断，事件携带三栏内容。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dirty_conflict: Option<DirtConflictView>,
    /// 三方合并已自动应用。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dirty_merged: Option<bool>,
}

/// 三栏合并预览（§8.6：你的改动 / 代理改动 / 合并结果）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirtConflictView {
    pub path: String,
    pub base: String,
    pub ours: String,
    pub theirs: String,
}

impl ToolOutput {
    fn ok(content: impl Into<String>) -> Self {
        Self {
            ok: true,
            content: content.into(),
            changed_files: vec![],
            exit_code: None,
            dirty_conflict: None,
            dirty_merged: None,
        }
    }
    fn err(content: impl Into<String>) -> Self {
        Self {
            ok: false,
            content: content.into(),
            changed_files: vec![],
            exit_code: None,
            dirty_conflict: None,
            dirty_merged: None,
        }
    }
}

/// 工具执行上下文。
pub struct ToolContext {
    pub root: PathBuf,
    pub files: Arc<FileService>,
    pub ops: Arc<FileOps>,
    pub guard: WriteGuard,
    /// 单命令超时（附录 E `[agent.exec].command_timeout_s`）。
    pub command_timeout: Duration,
    /// 已批准的出网域名（C 级审批结果）。
    pub allowed_hosts: std::sync::Mutex<Vec<String>>,
    /// 只读开关（A 级可用、其余拒绝）。
    pub readonly: bool,
    /// 脏缓冲注册表（§8.6 人机共编；None = daemon 未接入）。
    pub dirty: Option<std::sync::Arc<tenon_fs::DirtyBufferRegistry>>,
    /// MCP 外部进程插件桥（§13.3；None = 未接入）。
    pub mcp: Option<std::sync::Arc<tenon_mcp::McpConnection>>,
    /// MCP 工具分级策略（默认 D；net:* → C）。
    pub mcp_policy: tenon_mcp::McpLevelPolicy,
    /// 团队策略工具黑名单（M3：跨会话只收窄；命中即拒绝）。
    pub team_denied_tools: Vec<String>,
    /// Laya 本地决策模型（§9.8 集成点 #2 命令风险辅助；None = 回退）。
    pub laya: Option<std::sync::Arc<tenon_laya::LayaRuntime>>,
}

impl ToolContext {
    pub fn new(root: impl Into<PathBuf>, command_timeout: Duration) -> Self {
        let root = root.into();
        Self {
            files: Arc::new(FileService::new(&root)),
            ops: Arc::new(FileOps::new(&root)),
            guard: WriteGuard::new(&root),
            root,
            command_timeout,
            allowed_hosts: std::sync::Mutex::new(Vec::new()),
            readonly: false,
            dirty: None,
            mcp: None,
            mcp_policy: tenon_mcp::McpLevelPolicy::default(),
            team_denied_tools: Vec::new(),
            laya: None,
        }
    }

    pub fn allow_host(&self, host: &str) {
        self.allowed_hosts
            .lock()
            .expect("hosts lock")
            .push(host.to_string());
    }
}

#[allow(clippy::result_large_err)]
fn project_rel(ctx: &ToolContext, path: &str) -> Result<String, ToolOutput> {
    ctx.guard
        .check_write_rel(path)
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .map_err(|e| ToolOutput::err(format!("路径越界: {e}")))
}

/// 执行单个工具调用（已过权限审批；本函数只做执行与边界检查）。
/// Err 变体含三栏冲突预览（§8.6）——尺寸可接受（clippy result_large_err 白名单）。
#[allow(clippy::result_large_err)]
pub fn execute_tool(ctx: &ToolContext, tool: &str, args: &serde_json::Value) -> ToolOutput {
    // 团队策略工具黑名单（M3：只收窄；优先级最高，只读开关之前）
    if ctx.team_denied_tools.iter().any(|t| t == tool) {
        return ToolOutput::err(format!("团队策略禁用工具: {tool}（只收窄，§19/§12.2）"));
    }
    match tool {
        // ---------- A 级只读 ----------
        "read_file" => {
            let Some(path) = args.get("path").and_then(|p| p.as_str()) else {
                return ToolOutput::err("缺少 path 参数");
            };
            let rel = match project_rel(ctx, path) {
                Ok(r) => r,
                Err(e) => return e,
            };
            match ctx.files.read_file(&rel) {
                Ok(content) => ToolOutput::ok(content),
                Err(e) => ToolOutput::err(format!("读取失败: {e}")),
            }
        }
        "list_dir" => {
            let path = args.get("path").and_then(|p| p.as_str()).unwrap_or(".");
            let dir = ctx.root.join(path);
            match std::fs::read_dir(&dir) {
                Ok(entries) => {
                    let mut names: Vec<String> = entries
                        .filter_map(|e| e.ok())
                        .map(|e| {
                            let name = e.file_name().to_string_lossy().into_owned();
                            if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                                format!("{name}/")
                            } else {
                                name
                            }
                        })
                        .collect();
                    names.sort();
                    ToolOutput::ok(names.join("\n"))
                }
                Err(e) => ToolOutput::err(format!("列目录失败: {e}")),
            }
        }
        "grep" => {
            let Some(pattern) = args.get("pattern").and_then(|p| p.as_str()) else {
                return ToolOutput::err("缺少 pattern 参数");
            };
            let opts = tenon_fs::SearchOptions {
                max_hits: 200,
                ..Default::default()
            };
            match tenon_fs::search::search(&ctx.root, pattern, &opts) {
                Ok(hits) => {
                    if hits.is_empty() {
                        return ToolOutput::ok("（无匹配）");
                    }
                    let lines: Vec<String> = hits
                        .iter()
                        .map(|h| format!("{}:{}: {}", h.path, h.line, h.text.trim()))
                        .collect();
                    ToolOutput::ok(lines.join("\n"))
                }
                Err(e) => ToolOutput::err(format!("搜索失败: {e}")),
            }
        }
        "git_read" => {
            let sub = args.get("sub").and_then(|s| s.as_str()).unwrap_or("status");
            let kind = match sub {
                "log" => tenon_fs::git::GitReadKind::Log,
                "diff" => tenon_fs::git::GitReadKind::Diff,
                _ => tenon_fs::git::GitReadKind::Status,
            };
            match tenon_fs::git::git_read(&ctx.root, kind, &[]) {
                Ok(out) => ToolOutput::ok(tenon_core::redact::redact(&out)),
                Err(e) => ToolOutput::err(format!("git 失败: {e}")),
            }
        }
        "lsp_query" => {
            // LSP 查询由 daemon 侧的共享宿主承接；M0 内核侧返回占位（§8.5 共享宿主随 M1 全量）。
            ToolOutput::ok("（lsp_query：共享 LSP 宿主 M1 落地；当前无活动语言服务器）")
        }

        // ---------- B 级写执行 ----------
        "apply_patch" => {
            if ctx.readonly {
                return ToolOutput::err("只读会话禁用写操作");
            }
            let Ok(op) = serde_json::from_value::<PatchOp>(args.clone()) else {
                return ToolOutput::err("apply_patch 参数非法（需 file/range/content）");
            };
            let rel = match project_rel(ctx, &op.file) {
                Ok(r) => r,
                Err(e) => return e,
            };
            let before = ctx.files.read_file(&rel).unwrap_or_default();
            let mut lines: Vec<String> = before.lines().map(String::from).collect();
            match op.range {
                Some((start, end)) => {
                    if start < 1 || end < start || start > lines.len() + 1 {
                        return ToolOutput::err(format!(
                            "非法行区间 [{start},{end}]（文件 {len} 行）",
                            len = lines.len()
                        ));
                    }
                    let new_lines: Vec<String> = op.content.lines().map(String::from).collect();
                    let end_clamped = (end).min(lines.len());
                    lines.splice(start - 1..end_clamped, new_lines);
                }
                None => {
                    if !op.content.is_empty() {
                        for l in op.content.lines() {
                            lines.push(l.to_string());
                        }
                    }
                }
            }
            let after = if lines.is_empty() {
                String::new()
            } else {
                let mut s = lines.join("\n");
                if before.ends_with('\n') || before.is_empty() {
                    s.push('\n');
                }
                s
            };
            // ---- 人机共编（§8.6）：目标文件有用户脏缓冲 → 三方合并；
            // 合并冲突 → 阻断写入并出三栏预览（你的改动 / 代理改动 / base）----
            if let Some(registry) = &ctx.dirty {
                if let Some(buffer) = registry.get(&rel) {
                    let merged =
                        tenon_core::merge::merge_three_way(&buffer.base, &after, &buffer.dirty);
                    match merged {
                        Ok(merged_text) => {
                            let write_result = ctx.files.write_file(&rel, &merged_text);
                            if let Err(e) = write_result {
                                return ToolOutput::err(format!("写入失败: {e}"));
                            }
                            // 合并后的内容成为脏缓冲的新基线（用户编辑继续保留）
                            registry.update_base(&rel, &merged_text);
                            let diff = tenon_core::tools::unified_diff(&before, &merged_text, &rel);
                            return ToolOutput {
                                ok: true,
                                content: format!(
                                    "已写入 {rel}（人机共编：与未保存的用户改动三方合并）\n{diff}"
                                ),
                                changed_files: vec![rel],
                                exit_code: None,
                                dirty_conflict: None,
                                dirty_merged: Some(true),
                            };
                        }
                        Err(conflict) => {
                            return ToolOutput {
                                ok: false,
                                content: format!(
                                    "人机共编冲突：{rel} 存在未保存的用户改动，与代理改动冲突。                                     写入已阻断，等待用户在三栏预览中处置（§8.6）。"
                                ),
                                changed_files: vec![],
                                exit_code: None,
                                dirty_conflict: Some(DirtConflictView {
                                    path: rel,
                                    base: conflict.base,
                                    ours: conflict.ours,
                                    theirs: conflict.theirs,
                                }),
                                dirty_merged: None,
                            };
                        }
                    }
                }
            }

            match ctx.files.write_file(&rel, &after) {
                Ok(_) => {
                    let diff = tenon_core::tools::unified_diff(&before, &after, &rel);
                    ToolOutput {
                        ok: true,
                        content: format!("已写入 {rel}\n{diff}"),
                        changed_files: vec![rel],
                        exit_code: None,
                        dirty_conflict: None,
                        dirty_merged: None,
                    }
                }
                Err(e) => ToolOutput::err(format!("写入失败: {e}")),
            }
        }
        "run_tests" | "run_build" => {
            if ctx.readonly {
                return ToolOutput::err("只读会话禁用命令执行");
            }
            // §9.8 集成点 #2 命令风险辅助由 session 层编排（规则为主 + Laya
            // 补盲区，decider_call 入 Trace、提示注入输出），此处只执行
            let default_cmd = if tool == "run_tests" {
                detect_test_command(&ctx.root)
            } else {
                detect_build_command(&ctx.root)
            };
            let Some(cmd) = args
                .get("command")
                .and_then(|c| c.as_str())
                .map(String::from)
                .or(default_cmd)
            else {
                return ToolOutput::ok("（无可识别的测试/构建命令——走降级验证通道）");
            };
            // M0：断网态执行（Seatbelt profile 在 macOS 生成并应用；其他平台写守卫兜底）
            let spec = tenon_sandbox::SandboxSpec::Offline {
                project_root: ctx.root.clone(),
            };
            match exec_command(&cmd, &ctx.root, ctx.command_timeout, &spec) {
                Ok(out) => ToolOutput {
                    ok: out.success(),
                    content: format!(
                        "$ {cmd}\nexit={}\n{}\n{}",
                        out.exit_code.unwrap_or(-1),
                        out.stdout.trim(),
                        out.stderr.trim()
                    ),
                    changed_files: vec![],
                    exit_code: out.exit_code,
                    dirty_conflict: None,
                    dirty_merged: None,
                },
                Err(e) => ToolOutput::err(format!("执行失败: {e}")),
            }
        }
        "install_deps" => {
            if ctx.readonly {
                return ToolOutput::err("只读会话禁用依赖安装");
            }
            let Some(cmd) = args.get("command").and_then(|c| c.as_str()) else {
                return ToolOutput::err("缺少 command 参数");
            };
            // 镜像代理态（§12.3 B 级）：registry 域白名单过滤在代理进程（M2）；
            // 当前沙箱放行网络，代理进程落地前由审批+镜像配置约束
            let spec = tenon_sandbox::SandboxSpec::MirrorProxy {
                project_root: ctx.root.clone(),
            };
            match exec_command(cmd, &ctx.root, ctx.command_timeout, &spec) {
                Ok(out) => ToolOutput {
                    ok: out.success(),
                    content: format!(
                        "$ {cmd}\nexit={}\n{}",
                        out.exit_code.unwrap_or(-1),
                        out.stderr.trim()
                    ),
                    changed_files: vec![],
                    exit_code: out.exit_code,
                    dirty_conflict: None,
                    dirty_merged: None,
                },
                Err(e) => ToolOutput::err(format!("执行失败: {e}")),
            }
        }

        // ---------- C 级出网 ----------
        "http_fetch" => {
            if ctx.readonly {
                return ToolOutput::err("只读会话禁用网络访问");
            }
            let Some(url) = args.get("url").and_then(|u| u.as_str()) else {
                return ToolOutput::err("缺少 url 参数");
            };
            let host = url
                .strip_prefix("https://")
                .or_else(|| url.strip_prefix("http://"))
                .and_then(|rest| rest.split('/').next())
                .unwrap_or("");
            let allowed = ctx.allowed_hosts.lock().expect("hosts lock").clone();
            if !allowed.iter().any(|h| h == host) {
                return ToolOutput::err(format!("域名 {host} 未获审批（C 级恒审批，§12.2）"));
            }
            // 复用会话的 tokio 运行时（executor 在 async 上下文中被调用）
            let url = url.to_string();
            let fetched = match tokio::runtime::Handle::try_current() {
                Ok(handle) => handle.block_on(async {
                    match reqwest::Client::new()
                        .get(&url)
                        .timeout(Duration::from_secs(30))
                        .send()
                        .await
                    {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            let body = resp.text().await.unwrap_or_default();
                            Ok((status, body))
                        }
                        Err(e) => Err(e.to_string()),
                    }
                }),
                Err(e) => Err(format!("无 tokio 运行时: {e}")),
            };
            match fetched {
                Ok((status, body)) => {
                    let truncated: String = body.chars().take(20_000).collect();
                    ToolOutput::ok(format!(
                        "HTTP {status}\n{}",
                        tenon_core::redact::redact(&truncated)
                    ))
                }
                Err(e) => ToolOutput::err(format!("抓取失败: {e}")),
            }
        }

        // ---------- MCP 外部进程工具（§13.3：默认 C/D，永不自动执行） ----------
        name if name.starts_with("mcp:") => {
            if ctx.readonly {
                return ToolOutput::err("只读会话禁用 MCP 工具");
            }
            let Some(conn) = &ctx.mcp else {
                return ToolOutput::err("MCP 桥未接入");
            };
            let tool = name.trim_start_matches("mcp:");
            let conn = conn.clone();
            let tool_owned = tool.to_string();
            let args_owned = args.clone();
            let call = tokio::runtime::Handle::try_current().map(|h| {
                h.block_on(async move {
                    tokio::task::spawn_blocking(move || {
                        conn.call_tool(&tool_owned, args_owned)
                            .map_err(|e| e.to_string())
                    })
                    .await
                    .map_err(|e| format!("join: {e}"))
                    .and_then(|r| r)
                })
            });
            let result: std::result::Result<String, String> = match call {
                Ok(Ok(text)) => Ok(text),
                Ok(Err(e)) => Err(e),
                Err(e) => Err(e.to_string()),
            };
            match result {
                Ok(text) => {
                    let truncated: String = text.chars().take(20_000).collect();
                    ToolOutput::ok(format!(
                        "MCP {tool}\n{}",
                        tenon_core::redact::redact(&truncated)
                    ))
                }
                Err(e) => ToolOutput::err(format!("MCP {tool} 失败: {e}")),
            }
        }

        // ---------- D 级（执行前已审批） ----------
        "git_commit" => {
            if ctx.readonly {
                return ToolOutput::err("只读会话禁用 git 提交");
            }
            let msg = args
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("task commit");
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(&ctx.root)
                // 排除快照 / 内核托管目录（§9.5 / §10.3：不触碰用户提交内容）
                .args([
                    "add",
                    "-A",
                    "--",
                    ":(exclude).tenon-snapshots",
                    ":(exclude).tenon/**",
                ])
                .output()
                .and_then(|_| {
                    std::process::Command::new("git")
                        .arg("-C")
                        .arg(&ctx.root)
                        .args(["-c", "user.email=tenon@local", "-c", "user.name=tenon"])
                        .args(["commit", "-m", msg])
                        .output()
                });
            match out {
                Ok(o) if o.status.success() => {
                    let sha = String::from_utf8_lossy(&o.stdout).trim().to_string();
                    ToolOutput::ok(format!("已提交: {sha}"))
                }
                Ok(o) => ToolOutput::err(format!(
                    "提交失败: {}",
                    String::from_utf8_lossy(&o.stderr).trim()
                )),
                Err(e) => ToolOutput::err(format!("git 失败: {e}")),
            }
        }
        "git_push" | "create_pr" => {
            // 推送 / PR 需要 D 级审批 + 远端凭据；M0 返回明确未实现（不静默失败）
            ToolOutput::err(format!(
                "{tool} 需要远端凭据配置，当前版本未启用（审批已过，执行通道随 M2 落地）"
            ))
        }

        other => ToolOutput::err(format!("未知工具: {other}")),
    }
}

/// 检测测试命令（项目感知，§8.4 规则的最小子集）。
pub fn detect_test_command(root: &Path) -> Option<String> {
    if root.join("package.json").exists() {
        return Some("npm test --silent".into());
    }
    if root.join("Cargo.toml").exists() {
        return Some("cargo test --quiet".into());
    }
    if root.join("pyproject.toml").exists() || root.join("pytest.ini").exists() {
        return Some("python3 -m pytest -q".into());
    }
    if root.join("go.mod").exists() {
        return Some("go test ./...".into());
    }
    None
}

pub fn detect_build_command(root: &Path) -> Option<String> {
    if root.join("Cargo.toml").exists() {
        return Some("cargo build --quiet".into());
    }
    if root.join("package.json").exists() {
        return Some("npm run build --silent".into());
    }
    if root.join("go.mod").exists() {
        return Some("go build ./...".into());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> (tempfile::TempDir, ToolContext) {
        let dir = tempfile::tempdir().unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        (dir, c)
    }

    #[test]
    fn read_write_list_flow() {
        let (_d, c) = ctx();
        let out = execute_tool(
            &c,
            "apply_patch",
            &serde_json::json!({
                "file": "a.txt", "range": null, "content": "hello\n"
            }),
        );
        assert!(out.ok, "{}", out.content);
        assert_eq!(out.changed_files, vec!["a.txt".to_string()]);

        let out = execute_tool(&c, "read_file", &serde_json::json!({"path": "a.txt"}));
        assert_eq!(out.content, "hello\n");

        let out = execute_tool(&c, "list_dir", &serde_json::json!({"path": "."}));
        assert!(out.content.contains("a.txt"));
    }

    #[test]
    fn apply_patch_replaces_range() {
        let (_d, c) = ctx();
        execute_tool(
            &c,
            "apply_patch",
            &serde_json::json!({
                "file": "f.txt", "range": null, "content": "l1\nl2\nl3\n"
            }),
        );
        let out = execute_tool(
            &c,
            "apply_patch",
            &serde_json::json!({
                "file": "f.txt", "range": [2, 2], "content": "L2-EDITED"
            }),
        );
        assert!(out.ok, "{}", out.content);
        let content = c.files.read_file("f.txt").unwrap();
        assert_eq!(content, "l1\nL2-EDITED\nl3\n");
    }

    #[test]
    fn path_escape_rejected() {
        let (_d, c) = ctx();
        let out = execute_tool(
            &c,
            "apply_patch",
            &serde_json::json!({
                "file": "../evil.txt", "range": null, "content": "x"
            }),
        );
        assert!(!out.ok);
        assert!(out.content.contains("路径越界"));
    }

    #[test]
    fn readonly_blocks_writes_but_allows_reads() {
        let (d, mut c) = ctx();
        c.readonly = true;
        let out = execute_tool(
            &c,
            "apply_patch",
            &serde_json::json!({
                "file": "x.txt", "range": null, "content": "x"
            }),
        );
        assert!(!out.ok, "只读开关禁写");

        std::fs::write(d.path().join("r.txt"), "data").unwrap();
        let out = execute_tool(&c, "read_file", &serde_json::json!({"path": "r.txt"}));
        assert!(out.ok, "只读不禁 A 级");
    }

    #[test]
    fn grep_tool_reports_matches() {
        let (_d, c) = ctx();
        execute_tool(
            &c,
            "apply_patch",
            &serde_json::json!({
                "file": "code.rs", "range": null, "content": "fn find_me() {}\n"
            }),
        );
        let out = execute_tool(&c, "grep", &serde_json::json!({"pattern": "find_me"}));
        assert!(out.ok);
        assert!(out.content.contains("code.rs"));
    }

    #[test]
    fn http_fetch_requires_approved_host() {
        let (_d, c) = ctx();
        let out = execute_tool(
            &c,
            "http_fetch",
            &serde_json::json!({"url": "https://example.com/x"}),
        );
        assert!(!out.ok);
        assert!(out.content.contains("未获审批"));
    }

    #[test]
    fn run_tests_detects_command_by_manifest() {
        let (d, c) = ctx();
        std::fs::write(
            d.path().join("Cargo.toml"),
            "[package]\nname=\"x\"\nversion=\"0.1.0\"\n",
        )
        .unwrap();
        let out = execute_tool(&c, "run_tests", &serde_json::json!({}));
        assert!(out.content.contains("cargo test"));
        // 输出包含 exit 码（真实执行了 cargo test）
        assert!(out.content.contains("exit="));
    }

    #[test]
    fn no_manifest_falls_back_to_degraded_channel() {
        let (_d, c) = ctx();
        let out = execute_tool(&c, "run_tests", &serde_json::json!({}));
        assert!(out.content.contains("降级验证通道"));
    }

    #[test]
    fn git_commit_requires_git_repo() {
        let (_d, c) = ctx();
        let out = execute_tool(&c, "git_commit", &serde_json::json!({"message": "m"}));
        assert!(!out.ok, "非 git 仓库提交失败但不崩溃");
    }
}
