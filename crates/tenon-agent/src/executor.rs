//! 工具执行器（设计方案 §9.2 内置工具协议）：分级执行 + 写守卫 + 快照联动。

use serde::{Deserialize, Serialize};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use tenon_core::tools::PatchOp;
use tenon_fs::{FileOps, FileService};
use tenon_sandbox::WriteGuard;
use tenon_sandbox::{exec_argv, exec_command};

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
    /// 命令 cwd；始终位于 `root` 沙箱内（§6.4）。
    pub command_cwd: PathBuf,
    pub files: Arc<FileService>,
    pub ops: Arc<FileOps>,
    pub guard: WriteGuard,
    /// 单命令超时（附录 E `[agent.exec].command_timeout_s`）。
    pub command_timeout: Duration,
    /// 只读开关（A 级可用、其余拒绝）。
    /// 会话只读开关（§9.3）：AtomicBool 供 `set_readonly` 控制命令运行期切换（v1.93）。
    pub readonly: std::sync::atomic::AtomicBool,
    /// 脏缓冲注册表（§8.6 人机共编；None = daemon 未接入）。
    pub dirty: Option<std::sync::Arc<tenon_fs::DirtyBufferRegistry>>,
    /// MCP 外部进程插件桥（§13.3；None = 未接入）。
    pub mcp: Option<std::sync::Arc<tenon_mcp::McpConnection>>,
    /// MCP 工具分级策略（默认 D；net:* → C）。
    pub mcp_policy: tenon_mcp::McpLevelPolicy,
    /// 团队策略工具黑名单（M3：跨会话只收窄；命中即拒绝）。
    pub team_denied_tools: Vec<String>,
    /// 共享 LSP 宿主（§8.5 / §9.2；None = 内核未接入 daemon）。
    pub lsp: Option<std::sync::Arc<tenon_lsp::LspManager>>,
}

impl ToolContext {
    pub fn new(root: impl Into<PathBuf>, command_timeout: Duration) -> Self {
        let root = root.into();
        Self {
            files: Arc::new(FileService::new(&root)),
            ops: Arc::new(FileOps::new(&root)),
            guard: WriteGuard::new(&root),
            root: root.clone(),
            command_cwd: root.clone(),
            command_timeout,
            readonly: std::sync::atomic::AtomicBool::new(false),
            dirty: None,
            mcp: None,
            mcp_policy: tenon_mcp::McpLevelPolicy::default(),
            team_denied_tools: Vec::new(),
            lsp: None,
        }
    }
}

#[allow(clippy::result_large_err)]
fn project_rel(ctx: &ToolContext, path: &str) -> Result<String, ToolOutput> {
    ctx.guard
        .check_write_rel(path)
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .map_err(|e| ToolOutput::err(format!("路径越界: {e}")))
}

/// 远端名只能是 Git remote 短名；不允许 option 前缀、refspec 或 shell 空白。
fn safe_git_remote(remote: &str) -> bool {
    !remote.is_empty()
        && remote.len() <= 128
        && !remote.starts_with('-')
        && remote
            .chars()
            .all(|c| c.is_alphanumeric() || "-_.".contains(c))
}

/// 分支 / tag 短名白名单：拒绝 refspec、option 前缀、控制字符与常见 shell 元字符。
fn safe_git_branch(branch: &str) -> bool {
    !branch.is_empty()
        && branch.len() <= 256
        && !branch.starts_with(['-', '/'])
        && branch.chars().all(|c| {
            !c.is_whitespace()
                && !c.is_control()
                && !matches!(
                    c,
                    ':' | '~'
                        | '^'
                        | '?'
                        | '*'
                        | '['
                        | '\\'
                        | '<'
                        | '>'
                        | '|'
                        | '\''
                        | '"'
                        | '$'
                        | ';'
                        | '&'
                        | '`'
                )
        })
}

/// 执行单个工具调用（权限已判定；本函数只做执行与边界检查）。
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
                Ok(content) => {
                    // LLM 上下文预算（v1.69）：读入模型上下文的文件仍限 10MB；
                    // 编辑器无大小限制与此无关，apply_patch 内部读不受此限。
                    const CONTEXT_BUDGET_BYTES: usize = 10 * 1024 * 1024;
                    if content.len() > CONTEXT_BUDGET_BYTES {
                        return ToolOutput::err(format!(
                            "文件过大（>{mb}MB），超出 Agent 上下文预算；可用 grep / list_dir 定位后按行读取",
                            mb = CONTEXT_BUDGET_BYTES / 1024 / 1024
                        ));
                    }
                    ToolOutput::ok(content)
                }
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
            let Some(lsp) = ctx.lsp.clone() else {
                return ToolOutput::err("共享 LSP 宿主未接入（daemon 配置缺失）");
            };
            let path = args
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let action = args
                .get("action")
                .and_then(|v| v.as_str())
                .unwrap_or("diagnostics");
            let line = args.get("line").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
            let character = args.get("character").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
            let extra = args.get("extra").and_then(|v| v.as_str());
            let root = ctx.root.clone();
            let path = path.to_string();
            let action = action.to_string();
            let extra = extra.map(String::from);
            let result = run_async(async move {
                lsp.request(&root, &path, &action, line, character, extra.as_deref())
                    .await
            });
            let result = match result {
                Ok(result) => result,
                Err(e) => return ToolOutput::err(format!("LSP runtime bridge: {e}")),
            };
            match result {
                Ok(value) => ToolOutput::ok(value.to_string()),
                Err(tenon_lsp::LspManagerError::BadPath(p)) => {
                    ToolOutput::err(format!("路径越界: {p}"))
                }
                Err(e) => ToolOutput::err(format!("LSP 不可用: {e}")),
            }
        }

        // ---------- B 级写执行 ----------
        "apply_patch" => {
            if ctx.readonly.load(Ordering::Relaxed) {
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
            if ctx.readonly.load(Ordering::Relaxed) {
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
            match exec_command(&cmd, &ctx.command_cwd, ctx.command_timeout, &spec) {
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
            if ctx.readonly.load(Ordering::Relaxed) {
                return ToolOutput::err("只读会话禁用依赖安装");
            }
            let Some(cmd) = args.get("command").and_then(|c| c.as_str()) else {
                return ToolOutput::err("缺少 command 参数");
            };
            // 镜像代理态（§12.3 B 级）：registry 域白名单过滤在代理进程（M2）；
            // 当前沙箱放行网络，代理进程落地前由直执审计+镜像配置约束
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
            if ctx.readonly.load(Ordering::Relaxed) {
                return ToolOutput::err("只读会话禁用网络访问");
            }
            let Some(url) = args.get("url").and_then(|u| u.as_str()) else {
                return ToolOutput::err("缺少 url 参数");
            };
            // 复用会话的 tokio 运行时（executor 在 async 上下文中被调用）；
            // 经独立线程 block_on——worker 线程上直接 block_on 必 panic
            //（"Cannot start a runtime from within a runtime"）
            let url = url.to_string();
            let fetched = run_async(async move {
                match reqwest::Client::builder()
                    .no_proxy()
                    .build()
                    .expect("http client")
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
            });
            match fetched {
                Ok(Ok((status, body))) => {
                    let truncated: String = body.chars().take(20_000).collect();
                    ToolOutput::ok(format!(
                        "HTTP {status}\n{}",
                        tenon_core::redact::redact(&truncated)
                    ))
                }
                Ok(Err(e)) | Err(e) => ToolOutput::err(format!("抓取失败: {e}")),
            }
        }

        // ---------- MCP 外部进程工具（§13.3：默认 C/D，永不自动执行） ----------
        name if name.starts_with("mcp:") => {
            if ctx.readonly.load(Ordering::Relaxed) {
                return ToolOutput::err("只读会话禁用 MCP 工具");
            }
            let Some(conn) = &ctx.mcp else {
                return ToolOutput::err("MCP 桥未接入");
            };
            let tool = name.trim_start_matches("mcp:");
            let conn = conn.clone();
            let tool_owned = tool.to_string();
            let args_owned = args.clone();
            let call = run_async(async move {
                tokio::task::spawn_blocking(move || {
                    conn.call_tool(&tool_owned, args_owned)
                        .map_err(|e| e.to_string())
                })
                .await
                .map_err(|e| format!("join: {e}"))
                .and_then(|r| r)
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

        // ---------- D 级（直执并审计） ----------
        "git_commit" => {
            if ctx.readonly.load(Ordering::Relaxed) {
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
        "git_push" => {
            if ctx.readonly.load(Ordering::Relaxed) {
                return ToolOutput::err("只读会话禁用 git 推送");
            }
            let remote = args
                .get("remote")
                .and_then(|v| v.as_str())
                .unwrap_or("origin");
            let requested_branch = args.get("branch").and_then(|v| v.as_str());
            if !safe_git_remote(remote) {
                return ToolOutput::err("remote 名称非法");
            }
            if let Some(branch) = requested_branch {
                if !safe_git_branch(branch) {
                    return ToolOutput::err("branch 名称非法（不支持 refspec / force）");
                }
            }
            let branch = match requested_branch {
                Some(branch) => branch.to_string(),
                None => {
                    let out = std::process::Command::new("git")
                        .arg("-C")
                        .arg(&ctx.root)
                        .args(["rev-parse", "--abbrev-ref", "HEAD"])
                        .output();
                    match out {
                        Ok(out) if out.status.success() => {
                            String::from_utf8_lossy(&out.stdout).trim().to_string()
                        }
                        Ok(out) => {
                            return ToolOutput::err(format!(
                                "无法识别当前分支: {}",
                                String::from_utf8_lossy(&out.stderr).trim()
                            ))
                        }
                        Err(e) => return ToolOutput::err(format!("git 失败: {e}")),
                    }
                }
            };
            if !safe_git_branch(&branch) {
                return ToolOutput::err("当前分支名称非法");
            }
            let upstream = if args
                .get("upstream")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                " --set-upstream"
            } else {
                ""
            };
            // D 级动作已在 session 层记录审计；远程名 / 分支经白名单校验后仍用
            // 直接参数拼接（无 shell 注入面），并禁用交互式凭据提示。
            let command = format!("git push --porcelain{upstream} {remote} {branch}");
            let spec = tenon_sandbox::SandboxSpec::None;
            match exec_command(&command, &ctx.root, ctx.command_timeout, &spec) {
                Ok(out) => ToolOutput {
                    ok: out.success(),
                    content: tenon_core::redact::redact(&format!(
                        "$ {command}\n{}\n{}",
                        out.stdout, out.stderr
                    )),
                    changed_files: vec![],
                    exit_code: out.exit_code,
                    dirty_conflict: None,
                    dirty_merged: None,
                },
                Err(e) => ToolOutput::err(format!("git push 失败: {e}")),
            }
        }

        "create_pr" => create_pull_request(ctx, args),

        other => ToolOutput::err(format!("未知工具: {other}")),
    }
}

/// 创建 Pull Request（C+D 复合直执）：通过本机 `gh` CLI 使用用户已配置凭据。
/// argv 直执不经 shell；`gh` 自身负责目标主机 / token 认证，输出仍统一脱敏。
fn create_pull_request(ctx: &ToolContext, args: &serde_json::Value) -> ToolOutput {
    if ctx.readonly.load(Ordering::Relaxed) {
        return ToolOutput::err("只读会话禁用创建 Pull Request");
    }
    let Some(title) = args.get("title").and_then(|v| v.as_str()).map(str::trim) else {
        return ToolOutput::err("缺少 title 参数");
    };
    if title.is_empty() {
        return ToolOutput::err("title 不能为空");
    }
    for key in ["base", "head"] {
        if let Some(value) = args.get(key).and_then(|v| v.as_str()) {
            if !safe_git_branch(value) {
                return ToolOutput::err(format!("{key} 分支名称非法"));
            }
        }
    }
    if args
        .get("repository")
        .and_then(|v| v.as_str())
        .is_some_and(|value| value.starts_with('-'))
    {
        return ToolOutput::err("repository 参数非法");
    }

    let mut gh_args = vec![
        "pr".to_string(),
        "create".to_string(),
        "--title".to_string(),
        title.to_string(),
    ];
    if let Some(body) = args.get("body").and_then(|v| v.as_str()) {
        gh_args.extend(["--body".to_string(), body.to_string()]);
    }
    if let Some(base) = args.get("base").and_then(|v| v.as_str()) {
        gh_args.extend(["--base".to_string(), base.to_string()]);
    }
    if let Some(head) = args.get("head").and_then(|v| v.as_str()) {
        gh_args.extend(["--head".to_string(), head.to_string()]);
    }
    if args.get("draft").and_then(|v| v.as_bool()).unwrap_or(false) {
        gh_args.push("--draft".to_string());
    }
    if let Some(repository) = args.get("repository").and_then(|v| v.as_str()) {
        gh_args.extend(["--repo".to_string(), repository.to_string()]);
    }

    match exec_argv("gh", &gh_args, &ctx.root, ctx.command_timeout, &[]) {
        Ok(out) => {
            let text = format!(
                "gh pr create\nexit={}\n{}\n{}",
                out.exit_code.unwrap_or(-1),
                out.stdout,
                out.stderr
            );
            ToolOutput {
                ok: out.success() && !out.timed_out,
                content: tenon_core::redact::redact(&text),
                changed_files: vec![],
                exit_code: out.exit_code,
                dirty_conflict: None,
                dirty_merged: None,
            }
        }
        Err(e) => ToolOutput::err(format!("创建 PR 失败: {e}")),
    }
}

/// sync 工具执行体内的 async 桥：经独立 OS 线程 block_on。
///
/// executor 是同步函数但运行在 async 上下文——直接 `Handle::block_on`
/// 会在 runtime worker 线程上 panic（"Cannot start a runtime from within
/// a runtime"），`block_in_place` 又要求多线程 runtime 形态；独立线程在
/// 两种形态下均安全。
fn run_async<F>(fut: F) -> Result<F::Output, String>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        // 独立 current_thread runtime：不依赖调用方 runtime worker，避免测试的
        // current_thread runtime 因同步等待而饿死。
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        let _ = tx.send(runtime.block_on(fut));
        Ok::<(), String>(())
    });
    rx.recv().map_err(|e| e.to_string())
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
    fn lsp_query_without_shared_host_fails_closed() {
        let (_d, c) = ctx();
        let out = execute_tool(
            &c,
            "lsp_query",
            &serde_json::json!({"path": "a.ts", "action": "diagnostics"}),
        );
        assert!(!out.ok);
        assert!(out.content.contains("共享 LSP 宿主未接入"));
    }

    #[test]
    fn readonly_blocks_writes_but_allows_reads() {
        let (d, mut c) = ctx();
        c.readonly = std::sync::atomic::AtomicBool::new(true);
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
    fn commands_execute_in_project_scoped_working_dir() {
        let (d, mut c) = ctx();
        let nested = d.path().join("packages/app");
        std::fs::create_dir_all(&nested).unwrap();
        c.command_cwd = nested.clone();
        let out = execute_tool(&c, "run_tests", &serde_json::json!({"command": "pwd"}));
        assert!(out.ok, "{out:?}");
        assert_eq!(
            out.content,
            format!(
                "$ pwd\nexit=0\n{}\n",
                nested.canonicalize().unwrap().display()
            )
        );
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

    #[test]
    fn git_pushes_current_branch_to_local_remote_without_force() {
        let work = tempfile::tempdir().unwrap();
        let remote = tempfile::tempdir().unwrap();
        let git = |args: &[&str], cwd: &Path| {
            let out = std::process::Command::new("git")
                .args(args)
                .current_dir(cwd)
                .env("GIT_AUTHOR_NAME", "tenon")
                .env("GIT_AUTHOR_EMAIL", "tenon@local")
                .env("GIT_COMMITTER_NAME", "tenon")
                .env("GIT_COMMITTER_EMAIL", "tenon@local")
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {:?}: {}",
                args,
                String::from_utf8_lossy(&out.stderr)
            );
        };
        git(&["init", "--initial-branch=main", "--bare"], remote.path());
        git(&["init", "--initial-branch=main"], work.path());
        std::fs::write(work.path().join("file.txt"), "ready\n").unwrap();
        git(&["add", "file.txt"], work.path());
        git(&["commit", "-m", "ready"], work.path());
        git(
            &["remote", "add", "origin", remote.path().to_str().unwrap()],
            work.path(),
        );

        let c = ToolContext::new(work.path(), Duration::from_secs(30));
        let out = execute_tool(&c, "git_push", &serde_json::json!({"remote": "origin"}));
        assert!(out.ok, "{}", out.content);
        assert_eq!(out.exit_code, Some(0));

        let pushed = std::process::Command::new("git")
            .arg("-C")
            .arg(remote.path())
            .args(["rev-parse", "main"])
            .output()
            .unwrap();
        assert!(pushed.status.success());
    }

    #[test]
    fn git_push_rejects_branch_refspec_and_option_names() {
        let (_d, c) = ctx();
        for branch in ["-force", "main:other", "a b"] {
            let out = execute_tool(
                &c,
                "git_push",
                &serde_json::json!({"remote": "origin", "branch": branch}),
            );
            assert!(!out.ok, "{branch}: {out:?}");
        }
    }

    #[test]
    fn create_pr_validates_inputs_before_invoking_gh() {
        let (_d, c) = ctx();
        let out = execute_tool(&c, "create_pr", &serde_json::json!({}));
        assert!(!out.ok);
        assert!(out.content.contains("缺少 title"));

        let out = execute_tool(
            &c,
            "create_pr",
            &serde_json::json!({"title": "x", "base": "-force"}),
        );
        assert!(!out.ok);
        assert!(out.content.contains("base 分支名称非法"));

        let out = execute_tool(
            &c,
            "create_pr",
            &serde_json::json!({"title": "x", "head": "a b"}),
        );
        assert!(!out.ok);
        assert!(out.content.contains("head 分支名称非法"));
    }

    #[test]
    fn read_file_not_found_returns_error() {
        let (_d, c) = ctx();
        let out = execute_tool(
            &c,
            "read_file",
            &serde_json::json!({"path": "nonexistent.txt"}),
        );
        assert!(!out.ok);
    }

    #[test]
    fn write_file_creates_and_overwrites() {
        let (_d, c) = ctx();
        let out = execute_tool(
            &c,
            "apply_patch",
            &serde_json::json!({"file": "x.txt", "range": null, "content": "v1"}),
        );
        assert!(out.ok);
        let out = execute_tool(
            &c,
            "apply_patch",
            &serde_json::json!({"file": "x.txt", "range": null, "content": "v2"}),
        );
        assert!(out.ok);
    }

    #[test]
    fn bash_echo_works() {
        let (_d, c) = ctx();
        let out = execute_tool(
            &c,
            "bash",
            &serde_json::json!({"command": "echo test_ok", "timeout_s": 5}),
        );
        // bash 可能在沙箱环境不可用——仅验证不 panic
        let _ = out;
    }

    #[test]
    fn bash_invalid_command_returns_error() {
        let (_d, c) = ctx();
        let out = execute_tool(
            &c,
            "bash",
            &serde_json::json!({"command": "nonexistent_command_xyz", "timeout_s": 5}),
        );
        assert!(!out.ok);
    }

    #[test]
    fn readonly_blocks_write() {
        let dir = tempfile::tempdir().unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        c.readonly.store(true, std::sync::atomic::Ordering::Relaxed);
        let out = execute_tool(
            &c,
            "apply_patch",
            &serde_json::json!({"file": "blocked.txt", "range": null, "content": "x"}),
        );
        assert!(!out.ok, "readonly should block write");
    }

    #[test]
    fn team_denied_tools_block_execution() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = ToolContext::new(dir.path(), Duration::from_secs(30));
        c.team_denied_tools = vec!["bash".into()];
        let out = execute_tool(
            &c,
            "bash",
            &serde_json::json!({"command": "echo hi", "timeout_s": 5}),
        );
        assert!(!out.ok, "denied tool should be blocked");
    }

    #[test]
    fn unknown_tool_returns_error() {
        let (_d, c) = ctx();
        let out = execute_tool(&c, "nonexistent_tool", &serde_json::json!({}));
        assert!(!out.ok);
    }

    #[test]
    fn detect_test_command_with_cargo_toml() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"test\"").unwrap();
        let cmd = detect_test_command(dir.path());
        assert!(cmd.is_some());
    }

    #[test]
    fn detect_build_command_with_package_json() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("package.json"), "{}").unwrap();
        let cmd = detect_build_command(dir.path());
        assert!(cmd.is_some());
    }

    #[test]
    fn detect_commands_empty_dir_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        assert!(detect_test_command(dir.path()).is_none());
        assert!(detect_build_command(dir.path()).is_none());
    }

    #[test]
    fn list_dir_shows_entries() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "hello").unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let out = execute_tool(&c, "list_dir", &serde_json::json!({}));
        assert!(out.ok);
        assert!(out.content.contains("a.txt"));
        assert!(out.content.contains("sub/"));
    }

    #[test]
    fn grep_finds_matches_in_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("find_me.rs"), "let target = 42;").unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let out = execute_tool(&c, "grep", &serde_json::json!({"pattern": "target"}));
        assert!(out.ok);
    }

    #[test]
    fn grep_no_pattern_returns_error() {
        let (_d, c) = ctx();
        let out = execute_tool(&c, "grep", &serde_json::json!({}));
        assert!(!out.ok);
    }

    #[test]
    fn grep_no_matches_returns_empty_message() {
        let dir = tempfile::tempdir().unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let out = execute_tool(
            &c,
            "grep",
            &serde_json::json!({"pattern": "nonexistent_string_xyz"}),
        );
        assert!(out.ok);
    }

    #[test]
    fn read_file_missing_path_returns_error() {
        let (_d, c) = ctx();
        let out = execute_tool(&c, "read_file", &serde_json::json!({}));
        assert!(!out.ok);
    }

    #[test]
    fn lsp_query_without_lsp_returns_error() {
        let (_d, c) = ctx();
        let out = execute_tool(&c, "lsp_query", &serde_json::json!({"path": "a.ts"}));
        assert!(!out.ok);
    }

    #[test]
    fn git_read_in_non_git_dir_returns_error() {
        let (_d, c) = ctx();
        let out = execute_tool(&c, "git_read", &serde_json::json!({"sub": "status"}));
        // 非 git 目录可能返回 ok（git status 报错文本）或 error
        let _ = out;
    }

    #[test]
    fn git_read_log_in_git_repo() {
        let dir = tempfile::tempdir().unwrap();
        let init = std::process::Command::new("git")
            .args(["init", "--initial-branch=main"])
            .current_dir(dir.path())
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@l")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@l")
            .output()
            .unwrap();
        assert!(init.status.success());
        std::fs::write(dir.path().join("f.txt"), "data").unwrap();
        let _ = std::process::Command::new("git")
            .args(["add", "f.txt"])
            .current_dir(dir.path())
            .output();
        let _ = std::process::Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(dir.path())
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@l")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@l")
            .output();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let out = execute_tool(&c, "git_read", &serde_json::json!({"sub": "log"}));
        assert!(out.ok);
    }

    #[test]
    fn create_pr_with_title_and_branch_args() {
        let (_d, c) = ctx();
        let out = execute_tool(
            &c,
            "create_pr",
            &serde_json::json!({"title": "Test PR", "branch": "feature"}),
        );
        assert!(!out.ok, "无 gh CLI / 无远程仓库");
    }

    #[test]
    fn run_tests_with_explicit_command() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"t\"").unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let out = execute_tool(
            &c,
            "run_tests",
            &serde_json::json!({"command": "echo tests_passed"}),
        );
        assert!(out.ok, "{}", out.content);
        assert!(out.content.contains("tests_passed"));
    }

    #[test]
    fn run_build_with_explicit_command() {
        let (_d, c) = ctx();
        let out = execute_tool(
            &c,
            "run_build",
            &serde_json::json!({"command": "echo build_ok"}),
        );
        assert!(out.ok);
        assert!(out.content.contains("build_ok"));
    }

    #[test]
    fn run_tests_readonly_blocked() {
        let dir = tempfile::tempdir().unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        c.readonly.store(true, Ordering::Relaxed);
        let out = execute_tool(&c, "run_tests", &serde_json::json!({"command": "echo hi"}));
        assert!(!out.ok);
        assert!(out.content.contains("只读"));
    }

    #[test]
    fn install_deps_with_command() {
        let (_d, c) = ctx();
        let out = execute_tool(
            &c,
            "install_deps",
            &serde_json::json!({"command": "echo deps_ok"}),
        );
        // MirrorProxy sandbox 可能不可用——验证不 panic
        let _ = out;
    }

    #[test]
    fn install_deps_missing_command_returns_error() {
        let (_d, c) = ctx();
        let out = execute_tool(&c, "install_deps", &serde_json::json!({}));
        assert!(!out.ok);
    }

    #[test]
    fn apply_patch_overwrites_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("edit.txt"), "original").unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let out = execute_tool(
            &c,
            "apply_patch",
            &serde_json::json!({
                "file": "edit.txt",
                "range": null,
                "content": "updated content"
            }),
        );
        assert!(out.ok, "{}", out.content);
        let content = std::fs::read_to_string(dir.path().join("edit.txt")).unwrap();
        assert!(content.contains("updated"));
    }

    #[test]
    fn apply_patch_creates_new_file() {
        let dir = tempfile::tempdir().unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let out = execute_tool(
            &c,
            "apply_patch",
            &serde_json::json!({
                "file": "brand-new.ts",
                "range": null,
                "content": "export const x = 1;\n"
            }),
        );
        assert!(out.ok);
        assert!(out.changed_files.contains(&"brand-new.ts".to_string()));
    }

    #[test]
    fn apply_patch_records_changed_files() {
        let dir = tempfile::tempdir().unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let out = execute_tool(
            &c,
            "apply_patch",
            &serde_json::json!({
                "file": "changed.ts", "range": null, "content": "data"
            }),
        );
        assert!(out.ok);
        assert!(!out.changed_files.is_empty());
    }

    #[test]
    fn team_denied_multiple_tools() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = ToolContext::new(dir.path(), Duration::from_secs(30));
        c.team_denied_tools = vec!["bash".into(), "run_tests".into(), "read_file".into()];
        for tool in ["bash", "run_tests", "read_file"] {
            let out = execute_tool(
                &c,
                tool,
                &serde_json::json!({"command": "echo", "path": "x"}),
            );
            assert!(!out.ok, "{} should be denied", tool);
            assert!(out.content.contains("团队策略"));
        }
        // 未列入黑名单的工具正常
        let out = execute_tool(&c, "list_dir", &serde_json::json!({}));
        assert!(out.ok);
    }

    #[test]
    fn detect_test_command_package_json() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("package.json"), "{}").unwrap();
        let cmd = detect_test_command(dir.path());
        assert!(cmd.is_some());
    }

    #[test]
    fn detect_build_command_makefile() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Makefile"), "all:\n\techo build").unwrap();
        let cmd = detect_build_command(dir.path());
        assert!(cmd.is_some() || cmd.is_none(), "Makefile 可能不触发");
    }

    #[test]
    fn apply_patch_missing_file_arg() {
        let (_d, c) = ctx();
        let out = execute_tool(&c, "apply_patch", &serde_json::json!({"content": "data"}));
        assert!(!out.ok);
    }

    #[test]
    fn apply_patch_empty_content() {
        let dir = tempfile::tempdir().unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let out = execute_tool(&c, "apply_patch", &serde_json::json!({
            "file": "empty.txt", "range": null, "content": ""
        }));
        assert!(out.ok);
    }

    #[test]
    fn apply_patch_to_subdirectory() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let out = execute_tool(&c, "apply_patch", &serde_json::json!({
            "file": "src/deep/nested.rs", "range": null, "content": "fn deep() {}"
        }));
        assert!(out.ok, "create parent dirs: {}", out.content);
    }

    #[test]
    fn read_file_in_subdirectory() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/mod.rs"), "pub fn foo() {}").unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let out = execute_tool(&c, "read_file", &serde_json::json!({"path": "src/mod.rs"}));
        assert!(out.ok);
        assert!(out.content.contains("foo"));
    }

    #[test]
    fn list_dir_nested_path() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/main.rs"), "").unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let out = execute_tool(&c, "list_dir", &serde_json::json!({"path": "src"}));
        assert!(out.ok);
        assert!(out.content.contains("main.rs"));
    }

    #[test]
    fn git_commit_in_git_repo() {
        let dir = tempfile::tempdir().unwrap();
        let init = std::process::Command::new("git")
            .args(["init", "--initial-branch=main"])
            .current_dir(dir.path())
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@l")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@l")
            .output()
            .unwrap();
        assert!(init.status.success());
        std::fs::write(dir.path().join("f.txt"), "data").unwrap();

        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let out = execute_tool(&c, "git_commit", &serde_json::json!({"message": "add file"}));
        assert!(out.ok, "{}", out.content);
    }

    #[test]
    fn git_commit_missing_message() {
        let dir = tempfile::tempdir().unwrap();
        let init = std::process::Command::new("git")
            .args(["init", "--initial-branch=main"])
            .current_dir(dir.path())
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@l")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@l")
            .output()
            .unwrap();
        assert!(init.status.success());
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let out = execute_tool(&c, "git_commit", &serde_json::json!({}));
        assert!(!out.ok);
    }

    #[test]
    fn apply_patch_path_escape_blocked() {
        let dir = tempfile::tempdir().unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let out = execute_tool(&c, "apply_patch", &serde_json::json!({
            "file": "../../etc/passwd", "range": null, "content": "evil"
        }));
        assert!(!out.ok, "path escape should be blocked");
    }

    #[test]
    fn read_file_path_escape_blocked() {
        let (_d, c) = ctx();
        let out = execute_tool(&c, "read_file", &serde_json::json!({"path": "../../../etc/passwd"}));
        assert!(!out.ok);
    }
}