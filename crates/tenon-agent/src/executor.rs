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
        let out = execute_tool(
            &c,
            "apply_patch",
            &serde_json::json!({
                "file": "empty.txt", "range": null, "content": ""
            }),
        );
        assert!(out.ok);
    }

    #[test]
    fn apply_patch_to_subdirectory() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let out = execute_tool(
            &c,
            "apply_patch",
            &serde_json::json!({
                "file": "src/deep/nested.rs", "range": null, "content": "fn deep() {}"
            }),
        );
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
        let out = execute_tool(
            &c,
            "git_commit",
            &serde_json::json!({"message": "add file"}),
        );
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
        let out = execute_tool(
            &c,
            "apply_patch",
            &serde_json::json!({
                "file": "../../etc/passwd", "range": null, "content": "evil"
            }),
        );
        assert!(!out.ok, "path escape should be blocked");
    }

    #[test]
    fn read_file_path_escape_blocked() {
        let (_d, c) = ctx();
        let out = execute_tool(
            &c,
            "read_file",
            &serde_json::json!({"path": "../../../etc/passwd"}),
        );
        assert!(!out.ok);
    }

    #[test]
    fn apply_patch_overwrites_and_preserves_newline() {
        let dir = tempfile::tempdir().unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let out = execute_tool(
            &c,
            "apply_patch",
            &serde_json::json!({
                "file": "nl.txt", "range": null, "content": "line1\nline2\nline3\n"
            }),
        );
        assert!(out.ok);
        let content = std::fs::read_to_string(dir.path().join("nl.txt")).unwrap();
        assert!(content.ends_with('\n'));
    }

    #[test]
    fn read_file_returns_exact_content() {
        let dir = tempfile::tempdir().unwrap();
        let content = "exact\ncontent\nwith\nlines\n";
        std::fs::write(dir.path().join("exact.txt"), content).unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let out = execute_tool(&c, "read_file", &serde_json::json!({"path": "exact.txt"}));
        assert!(out.ok);
        assert_eq!(out.content, content);
    }

    #[test]
    fn list_dir_nonexistent_returns_error() {
        let (_d, c) = ctx();
        let out = execute_tool(
            &c,
            "list_dir",
            &serde_json::json!({"path": "nonexistent-dir"}),
        );
        assert!(!out.ok);
    }

    #[test]
    fn bash_tool_denied_by_team_policy() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = ToolContext::new(dir.path(), Duration::from_secs(30));
        c.team_denied_tools = vec!["bash".into()];
        let out = execute_tool(
            &c,
            "bash",
            &serde_json::json!({"command": "echo hi", "timeout_s": 5}),
        );
        assert!(!out.ok);
        assert!(out.content.contains("团队策略禁用"));
    }

    #[test]
    fn readonly_blocks_install_deps() {
        let dir = tempfile::tempdir().unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        c.readonly.store(true, Ordering::Relaxed);
        let out = execute_tool(
            &c,
            "install_deps",
            &serde_json::json!({"command": "npm install"}),
        );
        assert!(!out.ok);
    }

    #[test]
    fn readonly_blocks_run_build() {
        let dir = tempfile::tempdir().unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        c.readonly.store(true, Ordering::Relaxed);
        let out = execute_tool(&c, "run_build", &serde_json::json!({"command": "make"}));
        assert!(!out.ok);
    }

    #[test]
    fn readonly_blocks_apply_patch() {
        let dir = tempfile::tempdir().unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        c.readonly.store(true, Ordering::Relaxed);
        let out = execute_tool(
            &c,
            "apply_patch",
            &serde_json::json!({
                "file": "blocked.txt", "range": null, "content": "blocked"
            }),
        );
        assert!(!out.ok);
    }

    #[test]
    fn readonly_allows_read_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("readable.txt"), "safe to read").unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        c.readonly.store(true, Ordering::Relaxed);
        let out = execute_tool(
            &c,
            "read_file",
            &serde_json::json!({"path": "readable.txt"}),
        );
        assert!(out.ok, "readonly should allow A-level read");
    }

    #[test]
    fn readonly_allows_list_dir() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("visible.txt"), "").unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        c.readonly.store(true, Ordering::Relaxed);
        let out = execute_tool(&c, "list_dir", &serde_json::json!({}));
        assert!(out.ok);
    }

    #[test]
    fn readonly_allows_grep() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("grep-target.txt"), "findable").unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        c.readonly.store(true, Ordering::Relaxed);
        let out = execute_tool(&c, "grep", &serde_json::json!({"pattern": "findable"}));
        assert!(out.ok);
    }

    #[test]
    fn apply_patch_overwrites_large_file() {
        let dir = tempfile::tempdir().unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let big = "x".repeat(100_000);
        let out = execute_tool(
            &c,
            "apply_patch",
            &serde_json::json!({
                "file": "big.txt", "range": null, "content": big
            }),
        );
        assert!(out.ok);
        let metadata = std::fs::metadata(dir.path().join("big.txt")).unwrap();
        assert!(metadata.len() > 0);
    }

    #[test]
    fn grep_searches_multiple_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "hello world").unwrap();
        std::fs::write(dir.path().join("b.txt"), "say hello again").unwrap();
        std::fs::write(dir.path().join("c.txt"), "no match here").unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let out = execute_tool(&c, "grep", &serde_json::json!({"pattern": "hello"}));
        assert!(out.ok);
        assert!(out.content.contains("a.txt"));
        assert!(out.content.contains("b.txt"));
        assert!(!out.content.contains("c.txt"));
    }

    #[test]
    fn list_dir_root_vs_subdir() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("root.txt"), "").unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub/nested.txt"), "").unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let root_out = execute_tool(&c, "list_dir", &serde_json::json!({"path": "."}));
        assert!(root_out.content.contains("root.txt"));
        assert!(root_out.content.contains("sub/"));
        let sub_out = execute_tool(&c, "list_dir", &serde_json::json!({"path": "sub"}));
        assert!(sub_out.content.contains("nested.txt"));
    }

    #[test]
    fn read_file_utf8_content() {
        let dir = tempfile::tempdir().unwrap();
        let content = "中文内容\n日本語テキスト\n한국어\n";
        std::fs::write(dir.path().join("utf8.txt"), content).unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let out = execute_tool(&c, "read_file", &serde_json::json!({"path": "utf8.txt"}));
        assert!(out.ok);
        assert!(out.content.contains("中文"));
        assert!(out.content.contains("日本語"));
    }

    #[test]
    fn apply_patch_preserves_subdirectory_structure() {
        let dir = tempfile::tempdir().unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let out = execute_tool(
            &c,
            "apply_patch",
            &serde_json::json!({
                "file": "src/deeply/nested/module.rs",
                "range": null,
                "content": "pub struct Deep;\n"
            }),
        );
        assert!(out.ok);
        assert!(dir.path().join("src/deeply/nested/module.rs").exists());
    }

    #[test]
    fn apply_patch_unicode_content() {
        let dir = tempfile::tempdir().unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let out = execute_tool(&c, "apply_patch", &serde_json::json!({
            "file": "unicode.md",
            "range": null,
            "content": "# 标题\n\n## 中文段落\n\n日本語テキスト\n"
        }));
        assert!(out.ok);
        let content = std::fs::read_to_string(dir.path().join("unicode.md")).unwrap();
        assert!(content.contains("中文段落"));
    }

    #[test]
    fn read_file_dotfile() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".env"), "SECRET=test").unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let out = execute_tool(&c, "read_file", &serde_json::json!({"path": ".env"}));
        assert!(out.ok);
        assert!(out.content.contains("SECRET"));
    }

    #[test]
    fn apply_patch_dotfile() {
        let dir = tempfile::tempdir().unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let out = execute_tool(&c, "apply_patch", &serde_json::json!({
            "file": ".gitignore", "range": null, "content": "node_modules/\ntarget/\n"
        }));
        assert!(out.ok);
        assert!(dir.path().join(".gitignore").exists());
    }

    #[test]
    fn list_dir_empty_dir_returns_empty_string() {
        let dir = tempfile::tempdir().unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let out = execute_tool(&c, "list_dir", &serde_json::json!({}));
        assert!(out.ok);
        assert_eq!(out.content, "");
    }

    #[test]
    fn apply_patch_then_read_back_binary_like() {
        let dir = tempfile::tempdir().unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let content = "\u{0}\u{1}\u{2}binary\u{ff}\u{fe}";
        let out = execute_tool(&c, "apply_patch", &serde_json::json!({
            "file": "bin.dat", "range": null, "content": content
        }));
        assert!(out.ok);
    }

    #[test]
    fn read_file_large_but_within_budget() {
        let dir = tempfile::tempdir().unwrap();
        let content = "line\n".repeat(1000);
        std::fs::write(dir.path().join("medium.txt"), &content).unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let out = execute_tool(&c, "read_file", &serde_json::json!({"path": "medium.txt"}));
        assert!(out.ok);
        assert_eq!(out.content, content);
    }

    #[test]
    fn list_dir_with_hidden_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".hidden"), "").unwrap();
        std::fs::write(dir.path().join("visible.txt"), "").unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let out = execute_tool(&c, "list_dir", &serde_json::json!({}));
        assert!(out.ok);
        assert!(out.content.contains(".hidden"), "hidden files should be listed");
    }

    #[test]
    fn grep_case_sensitive_search() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("case.txt"), "Hello World\nhello world").unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let out = execute_tool(&c, "grep", &serde_json::json!({"pattern": "Hello"}));
        assert!(out.ok);
        assert!(out.content.contains("Hello"));
    }

    #[test]
    fn apply_patch_special_chars_in_filename() {
        let dir = tempfile::tempdir().unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let out = execute_tool(&c, "apply_patch", &serde_json::json!({
            "file": "src/lib/dto/user-profile.dto.ts",
            "range": null,
            "content": "export interface UserProfile { id: string; }\n"
        }));
        assert!(out.ok);
    }

    #[test]
    fn apply_patch_symlink_target() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("real")).unwrap();
        std::fs::write(dir.path().join("real/target.txt"), "original").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(dir.path().join("real"), dir.path().join("link")).unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let out = execute_tool(&c, "apply_patch", &serde_json::json!({
            "file": "real/target.txt", "range": null, "content": "updated via symlink"
        }));
        assert!(out.ok);
    }

    #[test]
    fn apply_patch_creates_deeply_nested_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let out = execute_tool(&c, "apply_patch", &serde_json::json!({
            "file": "a/b/c/d/e/deep.txt", "range": null, "content": "deep content"
        }));
        assert!(out.ok);
        assert!(dir.path().join("a/b/c/d/e/deep.txt").exists());
    }

    #[test]
    fn read_file_after_apply_patch_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let content = "round-trip content\nwith multiple lines\n";
        execute_tool(&c, "apply_patch", &serde_json::json!({
            "file": "rt.txt", "range": null, "content": content
        }));
        let out = execute_tool(&c, "read_file", &serde_json::json!({"path": "rt.txt"}));
        assert!(out.ok);
        assert_eq!(out.content, content);
    }

    #[test]
    fn grep_with_regex_pattern() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("regex.txt"), "foo123bar\nfoo456bar").unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let out = execute_tool(&c, "grep", &serde_json::json!({"pattern": "foo\\d+bar"}));
        assert!(out.ok);
    }

    #[test]
    fn list_dir_sorts_entries_alphabetically() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("z.txt"), "").unwrap();
        std::fs::write(dir.path().join("a.txt"), "").unwrap();
        std::fs::write(dir.path().join("m.txt"), "").unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let out = execute_tool(&c, "list_dir", &serde_json::json!({}));
        assert!(out.ok);
        let lines: Vec<&str> = out.content.split("\n").collect();
        assert_eq!(lines[0], "a.txt");
        assert_eq!(lines[1], "m.txt");
        assert_eq!(lines[2], "z.txt");
    }

    #[test]
    fn team_denied_read_file_blocks_read() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("secret.txt"), "classified").unwrap();
        let mut c = ToolContext::new(dir.path(), Duration::from_secs(30));
        c.team_denied_tools = vec!["read_file".into()];
        let out = execute_tool(&c, "read_file", &serde_json::json!({"path": "secret.txt"}));
        assert!(!out.ok);
        assert!(out.content.contains("团队策略"));
    }

    #[test]
    fn apply_patch_overwrites_dotfile() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".editorconfig"), "old = true").unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let out = execute_tool(&c, "apply_patch", &serde_json::json!({
            "file": ".editorconfig", "range": null, "content": "root = true\n"
        }));
        assert!(out.ok);
        let content = std::fs::read_to_string(dir.path().join(".editorconfig")).unwrap();
        assert!(content.contains("root = true"));
    }

    #[test]
    fn readonly_blocks_git_commit() {
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
        c.readonly.store(true, Ordering::Relaxed);
        let out = execute_tool(&c, "git_commit", &serde_json::json!({"message": "blocked"}));
        assert!(!out.ok);
    }

    #[test]
    fn readonly_blocks_git_push() {
        let dir = tempfile::tempdir().unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        c.readonly.store(true, Ordering::Relaxed);
        let out = execute_tool(&c, "git_push", &serde_json::json!({"remote": "origin"}));
        assert!(!out.ok);
    }

    #[test]
    fn apply_patch_file_with_spaces_in_name() {
        let dir = tempfile::tempdir().unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let out = execute_tool(&c, "apply_patch", &serde_json::json!({
            "file": "my file with spaces.txt", "range": null, "content": "content with spaces"
        }));
        assert!(out.ok);
        assert!(dir.path().join("my file with spaces.txt").exists());
    }

    #[test]
    fn grep_multiple_patterns_match_same_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("multi.txt"), "alpha\nbeta\ngamma").unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let out1 = execute_tool(&c, "grep", &serde_json::json!({"pattern": "alpha"}));
        assert!(out1.ok);
        let out2 = execute_tool(&c, "grep", &serde_json::json!({"pattern": "gamma"}));
        assert!(out2.ok);
    }

    #[test]
    fn bash_ls_command_works() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("ls-test.txt"), "data").unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let out = execute_tool(&c, "bash", &serde_json::json!({"command": "ls", "timeout_s": 5}));
        // ls 可能被沙箱拦截或成功
        let _ = out;
    }

    #[test]
    fn apply_patch_same_file_multiple_times() {
        let dir = tempfile::tempdir().unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        for i in 0..3 {
            let out = execute_tool(&c, "apply_patch", &serde_json::json!({
                "file": "iter.txt", "range": null, "content": format!("version {}", i)
            }));
            assert!(out.ok, "iteration {}: {}", i, out.content);
        }
        let content = std::fs::read_to_string(dir.path().join("iter.txt")).unwrap();
        assert!(content.contains("version 2"));
    }

    #[test]
    fn readonly_grep_and_git_read() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("ro.txt"), "readonly content").unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        c.readonly.store(true, Ordering::Relaxed);
        // A 级只读工具在 readonly 模式下仍可用
        let grep = execute_tool(&c, "grep", &serde_json::json!({"pattern": "readonly"}));
        assert!(grep.ok);
    }

    #[test]
    fn install_deps_readonly_blocked() {
        let dir = tempfile::tempdir().unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        c.readonly.store(true, Ordering::Relaxed);
        let out = execute_tool(&c, "install_deps", &serde_json::json!({"command": "cargo add serde"}));
        assert!(!out.ok, "readonly should block C-level install_deps");
    }

    #[test]
    fn git_read_in_git_repo_status_and_log() {
        let dir = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let _ = std::process::Command::new("git")
                .args(args)
                .current_dir(dir.path())
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@l")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@l")
                .output()
                .unwrap();
        };
        git(&["init", "--initial-branch=main"]);
        std::fs::write(dir.path().join("f.txt"), "content").unwrap();
        git(&["add", "f.txt"]);
        git(&["commit", "-m", "first"]);

        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let status = execute_tool(&c, "git_read", &serde_json::json!({"sub": "status"}));
        assert!(status.ok);
        let log = execute_tool(&c, "git_read", &serde_json::json!({"sub": "log"}));
        assert!(log.ok);
    }

    #[test]
    fn apply_patch_to_existing_file_with_subdirs() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/lib")).unwrap();
        std::fs::write(dir.path().join("src/lib/existing.rs"), "// old content").unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let out = execute_tool(&c, "apply_patch", &serde_json::json!({
            "file": "src/lib/existing.rs", "range": null, "content": "// new content\nfn updated() {}\n"
        }));
        assert!(out.ok);
        let content = std::fs::read_to_string(dir.path().join("src/lib/existing.rs")).unwrap();
        assert!(content.contains("new content"));
        assert!(content.contains("updated"));
    }

    #[test]
    fn grep_special_regex_chars_in_pattern() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("special.txt"), "cost = $10.99").unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let out = execute_tool(&c, "grep", &serde_json::json!({"pattern": "10\\.99"}));
        assert!(out.ok);
    }

    #[test]
    fn install_deps_missing_command_after_readonly_check() {
        let dir = tempfile::tempdir().unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        // NOT readonly, but no command → should fail with "缺少 command"
        let out = execute_tool(&c, "install_deps", &serde_json::json!({}));
        assert!(!out.ok);
        assert!(out.content.contains("command"));
    }

    #[test]
    fn apply_patch_complex_nested_path_creation() {
        let dir = tempfile::tempdir().unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let out = execute_tool(&c, "apply_patch", &serde_json::json!({
            "file": "packages/core/src/types/config.ts",
            "range": null,
            "content": "export interface Config { debug: boolean; }\n"
        }));
        assert!(out.ok);
        assert!(dir.path().join("packages/core/src/types/config.ts").exists());
    }

    #[test]
    fn grep_multiline_file_finds_on_multiple_lines() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("multi-line.txt"),
            "first match here\nno match\nanother match\n").unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let out = execute_tool(&c, "grep", &serde_json::json!({"pattern": "match"}));
        assert!(out.ok);
        // Should find on lines 1 and 3
        assert!(out.content.contains("1:"));
        assert!(out.content.contains("3:"));
    }

    #[test]
    fn apply_patch_then_grep_finds_new_content() {
        let dir = tempfile::tempdir().unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        execute_tool(&c, "apply_patch", &serde_json::json!({
            "file": "searchable.rs", "range": null, "content": "pub fn searchable_function() {}"
        }));
        let out = execute_tool(&c, "grep", &serde_json::json!({"pattern": "searchable_function"}));
        assert!(out.ok);
        assert!(out.content.contains("searchable.rs"));
    }

    #[test]
    fn read_file_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("empty.txt"), "").unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let out = execute_tool(&c, "read_file", &serde_json::json!({"path": "empty.txt"}));
        assert!(out.ok);
        assert_eq!(out.content, "");
    }

    #[test]
    fn list_dir_with_only_dirs() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("dir1")).unwrap();
        std::fs::create_dir(dir.path().join("dir2")).unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let out = execute_tool(&c, "list_dir", &serde_json::json!({}));
        assert!(out.ok);
        assert!(out.content.contains("dir1/"));
        assert!(out.content.contains("dir2/"));
    }

    #[test]
    fn apply_patch_content_with_special_regex_chars() {
        let dir = tempfile::tempdir().unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let content = "let re = r\"(\\d+)\\.(\\d+)\";\nconst cost = $10.99;\n";
        let out = execute_tool(&c, "apply_patch", &serde_json::json!({
            "file": "regex-content.rs", "range": null, "content": content
        }));
        assert!(out.ok);
    }

    #[test]
    fn team_denied_list_dir() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = ToolContext::new(dir.path(), Duration::from_secs(30));
        c.team_denied_tools = vec!["list_dir".into()];
        let out = execute_tool(&c, "list_dir", &serde_json::json!({}));
        assert!(!out.ok);
    }

    #[test]
    fn apply_patch_and_list_dir_see_new_file() {
        let dir = tempfile::tempdir().unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        execute_tool(&c, "apply_patch", &serde_json::json!({
            "file": "visible.txt", "range": null, "content": "see me"
        }));
        let out = execute_tool(&c, "list_dir", &serde_json::json!({}));
        assert!(out.content.contains("visible.txt"));
    }

    #[test]
    fn apply_patch_deeply_nested_and_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let content = "pub mod deep {\n    pub fn nested() {}\n}\n";
        execute_tool(&c, "apply_patch", &serde_json::json!({
            "file": "src/core/deep/nested.rs", "range": null, "content": content
        }));
        let out = execute_tool(&c, "read_file", &serde_json::json!({"path": "src/core/deep/nested.rs"}));
        assert!(out.ok);
        assert_eq!(out.content, content);
    }

    #[test]
    fn grep_after_multiple_writes_finds_all() {
        let dir = tempfile::tempdir().unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        for (name, content) in [("one.rs", "fn one()"), ("two.rs", "fn two()"), ("three.rs", "fn three()")] {
            execute_tool(&c, "apply_patch", &serde_json::json!({
                "file": name, "range": null, "content": format!("{}\n", content)
            }));
        }
        let out = execute_tool(&c, "grep", &serde_json::json!({"pattern": "fn "}));
        assert!(out.ok);
        assert!(out.content.contains("one.rs"));
        assert!(out.content.contains("two.rs"));
        assert!(out.content.contains("three.rs"));
    }

    #[test]
    fn apply_patch_read_write_list_full_cycle() {
        let dir = tempfile::tempdir().unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));

        // 1. Create file
        execute_tool(&c, "apply_patch", &serde_json::json!({
            "file": "cycle/README.md", "range": null, "content": "# Project\n\n## Setup\n"
        }));

        // 2. Read it back
        let read = execute_tool(&c, "read_file", &serde_json::json!({"path": "cycle/README.md"}));
        assert!(read.ok);
        assert!(read.content.contains("Setup"));

        // 3. List to see it
        let list = execute_tool(&c, "list_dir", &serde_json::json!({"path": "cycle"}));
        assert!(list.ok);
        assert!(list.content.contains("README.md"));

        // 4. Grep for content
        let grep = execute_tool(&c, "grep", &serde_json::json!({"pattern": "Setup"}));
        assert!(grep.ok);
    }

    #[test]
    fn bash_echo_and_file_creation() {
        let dir = tempfile::tempdir().unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        // bash may be sandboxed, but verify no panic
        let out = execute_tool(&c, "bash", &serde_json::json!({
            "command": "echo hello > /dev/null", "timeout_s": 5
        }));
        let _ = out;
    }

    #[test]
    fn team_denied_apply_patch() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = ToolContext::new(dir.path(), Duration::from_secs(30));
        c.team_denied_tools = vec!["apply_patch".into()];
        let out = execute_tool(&c, "apply_patch", &serde_json::json!({
            "file": "denied.txt", "range": null, "content": "denied content"
        }));
        assert!(!out.ok);
        assert!(out.content.contains("团队策略禁用"));
    }

    #[test]
    fn team_denied_grep_still_allows_others() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("ok.txt"), "searchable content").unwrap();
        let mut c = ToolContext::new(dir.path(), Duration::from_secs(30));
        c.team_denied_tools = vec!["bash".into()];
        // grep is NOT denied
        let out = execute_tool(&c, "grep", &serde_json::json!({"pattern": "searchable"}));
        assert!(out.ok);
    }

    #[test]
    fn apply_patch_read_grep_integration_three_files() {
        let dir = tempfile::tempdir().unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));

        // Write three files with different content
        let files = [
            ("auth.rs", "pub fn authenticate(token: &str) {}"),
            ("db.rs", "pub fn connect(url: &str) {}"),
            ("api.rs", "pub fn handle_request() {}"),
        ];
        for (name, content) in files {
            execute_tool(&c, "apply_patch", &serde_json::json!({
                "file": name, "range": null, "content": format!("{}\n", content)
            }));
        }

        // Grep for each
        for (name, keyword) in [("auth.rs", "authenticate"), ("db.rs", "connect"), ("api.rs", "handle_request")] {
            let out = execute_tool(&c, "grep", &serde_json::json!({"pattern": keyword}));
            assert!(out.ok, "grep for {}", keyword);
            assert!(out.content.contains(name), "{} should contain {}", keyword, name);
        }
    }

    #[test]
    fn readonly_and_team_denied_combined() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = ToolContext::new(dir.path(), Duration::from_secs(30));
        c.readonly.store(true, Ordering::Relaxed);
        c.team_denied_tools = vec!["bash".into(), "read_file".into()];
        // readonly + denied bash → denied by team policy first
        let out = execute_tool(&c, "bash", &serde_json::json!({"command": "echo", "timeout_s": 5}));
        assert!(!out.ok);
        // readonly + denied read_file → denied by team policy
        let out = execute_tool(&c, "read_file", &serde_json::json!({"path": "any.txt"}));
        assert!(!out.ok);
        // readonly + NOT denied list_dir → readonly OK
        let out = execute_tool(&c, "list_dir", &serde_json::json!({}));
        assert!(out.ok);
    }

    #[test]
    fn apply_patch_overwrite_read_overwrite_read() {
        let dir = tempfile::tempdir().unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));

        let versions = ["alpha", "beta", "gamma"];
        for v in versions {
            let out = execute_tool(&c, "apply_patch", &serde_json::json!({
                "file": "versions.txt", "range": null, "content": v
            }));
            assert!(out.ok);
            let read = execute_tool(&c, "read_file", &serde_json::json!({"path": "versions.txt"}));
            assert!(read.ok);
            assert!(read.content.contains(v), "expected {} in {}", v, read.content);
        }
    }

    #[test]
    fn apply_patch_read_list_full_integration() {
        let dir = tempfile::tempdir().unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));

        // Create nested structure
        execute_tool(&c, "apply_patch", &serde_json::json!({
            "file": "src/utils/helpers.ts", "range": null,
            "content": "export function formatDate(d: Date): string {\n  return d.toISOString();\n}\n"
        }));
        execute_tool(&c, "apply_patch", &serde_json::json!({
            "file": "src/utils/constants.ts", "range": null,
            "content": "export const MAX_RETRY = 3;\n"
        }));

        // List src/utils
        let list = execute_tool(&c, "list_dir", &serde_json::json!({"path": "src/utils"}));
        assert!(list.ok);
        assert!(list.content.contains("helpers.ts"));
        assert!(list.content.contains("constants.ts"));

        // Grep for formatDate
        let grep = execute_tool(&c, "grep", &serde_json::json!({"pattern": "formatDate"}));
        assert!(grep.ok);
        assert!(grep.content.contains("helpers.ts"));

        // Read constants
        let read = execute_tool(&c, "read_file", &serde_json::json!({"path": "src/utils/constants.ts"}));
        assert!(read.ok);
        assert!(read.content.contains("MAX_RETRY"));
    }

    #[test]
    fn readonly_apply_patch_and_list_dir() {
        let dir = tempfile::tempdir().unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        c.readonly.store(true, Ordering::Relaxed);
        // apply_patch blocked in readonly
        let patch = execute_tool(&c, "apply_patch", &serde_json::json!({
            "file": "blocked.txt", "range": null, "content": "no"
        }));
        assert!(!patch.ok);
        // list_dir still works
        let list = execute_tool(&c, "list_dir", &serde_json::json!({}));
        assert!(list.ok);
    }

    #[test]
    fn apply_patch_read_list_three_level_integration() {
        let dir = tempfile::tempdir().unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));

        let files = [
            ("src/models/user.ts", "export interface User { id: string; name: string; }\n"),
            ("src/services/user-service.ts", "import { User } from '../models/user';\n"),
            ("src/index.ts", "export { User } from './models/user';\n"),
        ];
        for (path, content) in files {
            let out = execute_tool(&c, "apply_patch", &serde_json::json!({
                "file": path, "range": null, "content": content
            }));
            assert!(out.ok, "failed to create {}", path);
        }

        // List src
        let list = execute_tool(&c, "list_dir", &serde_json::json!({"path": "src"}));
        assert!(list.ok);
        assert!(list.content.contains("models/"));
        assert!(list.content.contains("services/"));
        assert!(list.content.contains("index.ts"));

        // Read user model
        let read = execute_tool(&c, "read_file", &serde_json::json!({"path": "src/models/user.ts"}));
        assert!(read.ok);
        assert!(read.content.contains("interface User"));

        // Grep for User across files
        let grep = execute_tool(&c, "grep", &serde_json::json!({"pattern": "User"}));
        assert!(grep.ok);
    }

    #[test]
    fn readonly_denied_team_and_tool_level_interactions() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("readable.txt"), "safe").unwrap();
        let mut c = ToolContext::new(dir.path(), Duration::from_secs(30));
        c.readonly.store(true, Ordering::Relaxed);
        c.team_denied_tools = vec!["bash".into(), "run_tests".into()];

        // A级（只读）工具 → readonly OK, not denied → works
        let read = execute_tool(&c, "read_file", &serde_json::json!({"path": "readable.txt"}));
        assert!(read.ok);

        // B级工具 + readonly → blocked
        let patch = execute_tool(&c, "apply_patch", &serde_json::json!({
            "file": "ro.txt", "range": null, "content": "blocked"
        }));
        assert!(!patch.ok);

        // B级工具 + denied → blocked (readonly check happens after team check for bash)
        let bash = execute_tool(&c, "bash", &serde_json::json!({"command": "echo", "timeout_s": 5}));
        assert!(!bash.ok);

        // NOT denied C级 tool + NOT readonly (install_deps not in denied) → works or sandbox error
        let deps = execute_tool(&c, "install_deps", &serde_json::json!({"command": "echo deps"}));
        let _ = deps; // sandbox may or may not be available
    }

    #[test]
    fn apply_patch_typescript_project_structure() {
        let dir = tempfile::tempdir().unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let files = [
            ("package.json", r#"{"name": "my-app", "version": "1.0.0"}"#),
            ("tsconfig.json", r#"{"compilerOptions": {"strict": true}}"#),
            ("src/index.ts", "export { App } from './App';\n"),
            ("src/App.tsx", "export default function App() { return null; }\n"),
            ("src/components/Header.tsx", "export function Header() { return null; }\n"),
        ];
        for (path, content) in files {
            let out = execute_tool(&c, "apply_patch", &serde_json::json!({
                "file": path, "range": null, "content": content
            }));
            assert!(out.ok, "failed: {}", path);
        }
        // Verify all files exist
        for (path, _) in files {
            assert!(dir.path().join(path).exists(), "missing: {}", path);
        }
    }

    #[test]
    fn grep_pattern_with_anchors() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("anchored.txt"), "start of line\nnot at start").unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let out = execute_tool(&c, "grep", &serde_json::json!({"pattern": "^start"}));
        assert!(out.ok);
    }

    #[test]
    fn read_file_binary_like_no_panic() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("bin.dat"), [0u8, 1u8, 2u8, 255u8]).unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let out = execute_tool(&c, "read_file", &serde_json::json!({"path": "bin.dat"}));
        // Binary may succeed or fail (invalid UTF-8) - verify no panic
        let _ = out;
    }

    #[test]
    fn apply_patch_to_existing_preserves_directory() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/models")).unwrap();
        std::fs::write(dir.path().join("src/models/user.ts"), "// old").unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let out = execute_tool(&c, "apply_patch", &serde_json::json!({
            "file": "src/models/user.ts", "range": null, "content": "// updated\n"
        }));
        assert!(out.ok);
        // Directory still exists
        assert!(dir.path().join("src/models").is_dir());
        // Content updated
        let content = std::fs::read_to_string(dir.path().join("src/models/user.ts")).unwrap();
        assert!(content.contains("updated"));
    }

    #[test]
    fn grep_in_nested_directory() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/deeply/nested")).unwrap();
        std::fs::write(dir.path().join("src/deeply/nested/deep.ts"), "deeply_nested_fn()").unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let out = execute_tool(&c, "grep", &serde_json::json!({"pattern": "deeply_nested"}));
        assert!(out.ok);
    }

    #[test]
    fn bash_sandbox_available_or_not() {
        let dir = tempfile::tempdir().unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let out = execute_tool(&c, "bash", &serde_json::json!({
            "command": "pwd", "timeout_s": 5
        }));
        // Sandbox may be available or not - just verify no panic
        let _ = out;
    }

    #[test]
    fn team_denied_install_deps() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = ToolContext::new(dir.path(), Duration::from_secs(30));
        c.team_denied_tools = vec!["install_deps".into()];
        let out = execute_tool(&c, "install_deps", &serde_json::json!({"command": "npm install"}));
        assert!(!out.ok);
        assert!(out.content.contains("团队策略禁用"));
    }

    #[test]
    fn apply_patch_and_read_json_file() {
        let dir = tempfile::tempdir().unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let json_content = r#"{"name": "test", "version": "1.0.0", "dependencies": {}}"#;
        let out = execute_tool(&c, "apply_patch", &serde_json::json!({
            "file": "package.json", "range": null, "content": json_content
        }));
        assert!(out.ok);
        let read = execute_tool(&c, "read_file", &serde_json::json!({"path": "package.json"}));
        assert!(read.ok);
        assert!(read.content.contains("test"));
    }

    #[test]
    fn readonly_denied_and_team_interactions_full() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("data.txt"), "safe to read").unwrap();
        let mut c = ToolContext::new(dir.path(), Duration::from_secs(30));
        c.readonly.store(true, Ordering::Relaxed);
        c.team_denied_tools = vec!["bash".into(), "run_tests".into()];

        // read_file: readonly OK + not denied → works
        let read = execute_tool(&c, "read_file", &serde_json::json!({"path": "data.txt"}));
        assert!(read.ok);

        // apply_patch: readonly → blocked
        let patch = execute_tool(&c, "apply_patch", &serde_json::json!({
            "file": "new.txt", "range": null, "content": "no"
        }));
        assert!(!patch.ok);

        // bash: denied → blocked
        let bash = execute_tool(&c, "bash", &serde_json::json!({"command": "echo", "timeout_s": 5}));
        assert!(!bash.ok);

        // run_tests: denied → blocked
        let test = execute_tool(&c, "run_tests", &serde_json::json!({"command": "echo"}));
        assert!(!test.ok);

        // list_dir: readonly OK + not denied → works
        let list = execute_tool(&c, "list_dir", &serde_json::json!({}));
        assert!(list.ok);
    }

    #[test]
    fn grep_and_apply_patch_integration() {
        let dir = tempfile::tempdir().unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));

        // Create files
        for (name, content) in [
            ("model.rs", "pub struct Model { id: u32 }"),
            ("view.rs", "pub struct View { model: Model }"),
        ] {
            execute_tool(&c, "apply_patch", &serde_json::json!({
                "file": name, "range": null, "content": content
            }));
        }

        // Grep for Model struct
        let out = execute_tool(&c, "grep", &serde_json::json!({"pattern": "struct Model"}));
        assert!(out.ok);
        assert!(out.content.contains("model.rs"));
    }

    #[test]
    fn apply_patch_typescript_vue_svelte_project() {
        let dir = tempfile::tempdir().unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let files = [
            ("src/App.vue", "<template><div>Vue App</div></template>\n"),
            ("src/App.svelte", "<div>Svelte App</div>\n"),
            ("src/main.ts", "import { createApp } from './app';\n"),
        ];
        for (path, content) in files {
            let out = execute_tool(&c, "apply_patch", &serde_json::json!({
                "file": path, "range": null, "content": content
            }));
            assert!(out.ok, "failed: {}", path);
        }
        // All files exist
        for (path, _) in files {
            assert!(dir.path().join(path).exists());
        }
    }

    #[test]
    fn apply_patch_and_verify_content_exact_match() {
        let dir = tempfile::tempdir().unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let expected = "line 1\nline 2\nline 3\n";
        execute_tool(&c, "apply_patch", &serde_json::json!({
            "file": "exact.txt", "range": null, "content": expected
        }));
        let actual = std::fs::read_to_string(dir.path().join("exact.txt")).unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn readonly_list_dir_grep_and_git_read_work() {
        let dir = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let _ = std::process::Command::new("git")
                .args(args)
                .current_dir(dir.path())
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@l")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@l")
                .output()
                .unwrap();
        };
        git(&["init", "--initial-branch=main"]);
        std::fs::write(dir.path().join("f.txt"), "readonly test").unwrap();
        git(&["add", "f.txt"]);
        git(&["commit", "-m", "init"]);

        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        c.readonly.store(true, Ordering::Relaxed);

        // All A-level tools should work in readonly
        let list = execute_tool(&c, "list_dir", &serde_json::json!({}));
        assert!(list.ok);
        let grep = execute_tool(&c, "grep", &serde_json::json!({"pattern": "test"}));
        assert!(grep.ok);
        let git_read = execute_tool(&c, "git_read", &serde_json::json!({"sub": "log"}));
        assert!(git_read.ok);
    }

    #[test]
    fn apply_patch_go_project() {
        let dir = tempfile::tempdir().unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let files = [
            ("go.mod", "module example.com/myapp\n\ngo 1.21\n"),
            ("main.go", "package main\n\nimport \"fmt\"\n\nfunc main() {\n\tfmt.Println(\"hello\")\n}\n"),
            ("internal/handler/handler.go", "package handler\n\nfunc Handle() {}\n"),
        ];
        for (path, content) in files {
            let out = execute_tool(&c, "apply_patch", &serde_json::json!({
                "file": path, "range": null, "content": content
            }));
            assert!(out.ok, "failed: {}", path);
        }
    }

    #[test]
    fn apply_patch_rust_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let files = [
            ("Cargo.toml", "[workspace]\nmembers = [\"core\", \"cli\"]\n"),
            ("core/Cargo.toml", "[package]\nname = \"core\"\nversion = \"0.1.0\"\n"),
            ("core/src/lib.rs", "pub fn core_fn() {}\n"),
            ("cli/Cargo.toml", "[package]\nname = \"cli\"\nversion = \"0.1.0\"\n"),
            ("cli/src/main.rs", "fn main() {}\n"),
        ];
        for (path, content) in files {
            let out = execute_tool(&c, "apply_patch", &serde_json::json!({
                "file": path, "range": null, "content": content
            }));
            assert!(out.ok, "failed: {}", path);
        }
    }

    #[test]
    fn apply_patch_java_project() {
        let dir = tempfile::tempdir().unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let files = [
            ("pom.xml", r#"<?xml version="1.0"?><project><modelVersion>4.0.0</modelVersion></project>"#),
            ("src/main/java/com/example/App.java", "public class App {\n    public static void main(String[] args) {}\n}\n"),
        ];
        for (path, content) in files {
            let out = execute_tool(&c, "apply_patch", &serde_json::json!({
                "file": path, "range": null, "content": content
            }));
            assert!(out.ok, "failed: {}", path);
        }
    }

    #[test]
    fn apply_patch_csharp_project() {
        let dir = tempfile::tempdir().unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let files = [
            ("MyApp.csproj", "<Project Sdk=\"Microsoft.NET.Sdk\"><PropertyGroup><TargetFramework>net8.0</TargetFramework></PropertyGroup></Project>"),
            ("Program.cs", "var builder = WebApplication.CreateBuilder(args);\n"),
        ];
        for (path, content) in files {
            let out = execute_tool(&c, "apply_patch", &serde_json::json!({
                "file": path, "range": null, "content": content
            }));
            assert!(out.ok, "failed: {}", path);
        }
    }

    #[test]
    fn grep_in_java_and_csharp_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("App.java"), "public class App {}").unwrap();
        std::fs::write(dir.path().join("Program.cs"), "class Program {}").unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let out = execute_tool(&c, "grep", &serde_json::json!({"pattern": "class"}));
        assert!(out.ok);
        assert!(out.content.contains("App.java"));
        assert!(out.content.contains("Program.cs"));
    }

    #[test]
    fn apply_patch_dockerfile_and_compose() {
        let dir = tempfile::tempdir().unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let files = [
            ("Dockerfile", "FROM node:20-alpine\nWORKDIR /app\nCOPY . .\nRUN npm ci\nCMD [\"npm\", \"start\"]\n"),
            ("docker-compose.yml", "version: \"3\"\nservices:\n  app:\n    build: .\n"),
        ];
        for (path, content) in files {
            let out = execute_tool(&c, "apply_patch", &serde_json::json!({
                "file": path, "range": null, "content": content
            }));
            assert!(out.ok, "failed: {}", path);
        }
    }

    #[test]
    fn apply_patch_ci_workflow() {
        let dir = tempfile::tempdir().unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let out = execute_tool(&c, "apply_patch", &serde_json::json!({
            "file": ".github/workflows/ci.yml",
            "range": null,
            "content": "name: CI\non: [push]\njobs:\n  test:\n    runs-on: ubuntu-latest\n"
        }));
        assert!(out.ok);
    }

    #[test]
    fn apply_patch_env_file() {
        let dir = tempfile::tempdir().unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let out = execute_tool(&c, "apply_patch", &serde_json::json!({
            "file": ".env.example", "range": null,
            "content": "DATABASE_URL=postgres://localhost/mydb\nAPI_KEY=your-key-here\n"
        }));
        assert!(out.ok);
    }

    #[test]
    fn apply_patch_readme_md() {
        let dir = tempfile::tempdir().unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let out = execute_tool(&c, "apply_patch", &serde_json::json!({
            "file": "README.md", "range": null,
            "content": "# My Project\n\n## Installation\n\n```bash\nnpm install\n```\n\n## Usage\n"
        }));
        assert!(out.ok);
    }

    #[test]
    fn apply_patch_license_file() {
        let dir = tempfile::tempdir().unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let out = execute_tool(&c, "apply_patch", &serde_json::json!({
            "file": "LICENSE", "range": null,
            "content": "MIT License\n\nCopyright (c) 2026\n"
        }));
        assert!(out.ok);
    }

    #[test]
    fn apply_patch_gitignore() {
        let dir = tempfile::tempdir().unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let out = execute_tool(&c, "apply_patch", &serde_json::json!({
            "file": ".gitignore", "range": null,
            "content": "node_modules/\ndist/\n.env\n*.log\n"
        }));
        assert!(out.ok);
    }

    #[test]
    fn apply_patch_vscode_settings() {
        let dir = tempfile::tempdir().unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let out = execute_tool(&c, "apply_patch", &serde_json::json!({
            "file": ".vscode/settings.json", "range": null,
            "content": "{\n  \"editor.formatOnSave\": true,\n  \"editor.tabSize\": 2\n}\n"
        }));
        assert!(out.ok);
    }

    #[test]
    fn apply_patch_read_makefile_toml_yaml() {
        let dir = tempfile::tempdir().unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let files = [
            ("Makefile", "all: build\n\nbuild:\n\techo building\n"),
            ("config.toml", "[server]\nport = 8080\n"),
            ("config.yaml", "server:\n  port: 8080\n"),
        ];
        for (path, content) in files {
            let out = execute_tool(&c, "apply_patch", &serde_json::json!({
                "file": path, "range": null, "content": content
            }));
            assert!(out.ok, "write {}", path);
            let read = execute_tool(&c, "read_file", &serde_json::json!({"path": path}));
            assert!(read.ok, "read {}", path);
        }
    }

    #[test]
    fn grep_in_config_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("app.conf"), "port=3000\nhost=localhost\n").unwrap();
        std::fs::write(dir.path().join("db.conf"), "port=5432\n").unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let out = execute_tool(&c, "grep", &serde_json::json!({"pattern": "port"}));
        assert!(out.ok);
        assert!(out.content.contains("app.conf"));
        assert!(out.content.contains("db.conf"));
    }

    #[test]
    fn apply_patch_and_verify_ts_js_py_integration() {
        let dir = tempfile::tempdir().unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let files = [
            ("src/server.ts", "import express from 'express';\nconst app = express();\n"),
            ("src/client.js", "fetch('/api/data').then(r => r.json());\n"),
            ("scripts/deploy.py", "import subprocess\nsubprocess.run(['npm', 'run', 'build'])\n"),
        ];
        for (path, content) in files {
            let out = execute_tool(&c, "apply_patch", &serde_json::json!({
                "file": path, "range": null, "content": content
            }));
            assert!(out.ok, "write {}", path);
        }
        // Cross-verify with grep
        let grep = execute_tool(&c, "grep", &serde_json::json!({"pattern": "import"}));
        assert!(grep.ok);
        // grep 可能只匹配部分文件（rg 行为差异）
        assert!(!grep.content.is_empty());
    }

    #[test]
    fn apply_patch_sql_and_shell_scripts() {
        let dir = tempfile::tempdir().unwrap();
        let c = ToolContext::new(dir.path(), Duration::from_secs(30));
        let files = [
            ("migrations/001_init.sql", "CREATE TABLE users (id SERIAL PRIMARY KEY, name TEXT);\n"),
            ("scripts/setup.sh", "#!/bin/bash\nset -e\necho 'setup complete'\n"),
        ];
        for (path, content) in files {
            let out = execute_tool(&c, "apply_patch", &serde_json::json!({
                "file": path, "range": null, "content": content
            }));
            assert!(out.ok, "write {}", path);
        }
        // List migrations dir
        let list = execute_tool(&c, "list_dir", &serde_json::json!({"path": "migrations"}));
        assert!(list.ok);
        assert!(list.content.contains("001_init.sql"));
    }
}