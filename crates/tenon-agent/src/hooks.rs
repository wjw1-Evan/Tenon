//! 用户 hooks（§13.6 v1.180）：配置驱动的生命周期回调——工具 / 任务固定点
//! 同步调用外部命令，承接 lint-on-edit、自定义审计、外部通知等自动化工作流。
//!
//! 信任模型：hooks 是用户在自有配置文件里声明的自有命令——用户配置 = 用户
//! 信任，直接 exec 不经 shell 切分、不过沙箱（与 config.toml 的 provider
//! 命令同信任级）；每次调用发 `hook_run` 事件入 Trace（不含参数原文）。

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// 单条 hook 的触发点（§13.6）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookEvent {
    /// 工具执行前（含内联 A 级工具 subtasks / submit_plan / laya_decide）。
    PreTool,
    /// 工具执行后。
    PostTool,
    /// 任务开始前（模型首次调用前）。
    PreTurn,
    /// 任务终态前（Done / Paused / Error）。
    PostTurn,
}

impl HookEvent {
    pub fn as_str(self) -> &'static str {
        match self {
            HookEvent::PreTool => "pre_tool",
            HookEvent::PostTool => "post_tool",
            HookEvent::PreTurn => "pre_turn",
            HookEvent::PostTurn => "post_turn",
        }
    }
}

/// pre_tool 非 0 退出的处置：拒绝该工具调用（stderr 回给模型）或仅记录继续。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum HookOnFail {
    #[default]
    Continue,
    Block,
}

/// 单条 hook 配置（settings.json `[hooks]` 数组元素）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HookConfig {
    pub event: HookEvent,
    /// 外部命令（直接 exec 不经 shell 切分；argv[0] + 空格切分参数由用户以
    /// 包装脚本承载——配置面保持单一字符串，复杂参数走包装脚本）。
    pub command: String,
    /// 超时毫秒（校验层钳 100–30000）。
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default)]
    pub on_fail: HookOnFail,
    /// 可选工具 glob 过滤（`run_*` 形态；空 = 全部工具）。
    #[serde(default)]
    pub tools: Vec<String>,
}

fn default_timeout_ms() -> u64 {
    5000
}

/// hooks 执行上下文。
pub struct HookContext<'a> {
    pub session_id: &'a str,
    /// 会话工作根（命令 cwd）。
    pub root: &'a Path,
    /// 工具名（pre_tool / post_tool）；turn 级为空。
    pub tool: Option<&'a str>,
    /// 工具参数 JSON（pre_tool）；截 32k。
    pub tool_args: Option<&'a str>,
    /// 工具输出（post_tool）；截 2k。
    pub tool_output: Option<&'a str>,
}

/// 单条 hook 的执行结果（入 `hook_run` 事件）。
#[derive(Debug, Clone, Serialize)]
pub struct HookRunResult {
    pub event: &'static str,
    pub command: String,
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    /// pass = 放行/成功；block = pre_tool 阻断；error = 超时 / 启动失败。
    pub action: &'static str,
    /// block 时的拒绝理由（stderr 截 500 字符）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// 一轮 hook 汇总：pre_tool 是否被阻断 + 各条结果。
pub struct HookOutcome {
    pub blocked: bool,
    /// block 时的拒绝理由（首条阻断 hook 的 stderr）。
    pub block_reason: Option<String>,
    pub results: Vec<HookRunResult>,
}

/// glob 工具过滤（`run_*` 前缀通配；空 patterns = 全部匹配）。
pub fn tool_matches(patterns: &[String], tool: &str) -> bool {
    if patterns.is_empty() {
        return true;
    }
    patterns.iter().any(|p| {
        let Some(prefix) = p.strip_suffix('*') else {
            return p == tool;
        };
        tool.starts_with(prefix)
    })
}

fn truncate(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// 同步执行一批 hook（阻塞调用点——hook 预算经 timeout_ms 钳制）。
/// 命令解析：按空白切分（无引号语义，复杂命令走包装脚本）。
pub async fn run_hooks(
    configs: &[HookConfig],
    event: HookEvent,
    ctx: &HookContext<'_>,
) -> HookOutcome {
    let mut outcome = HookOutcome {
        blocked: false,
        block_reason: None,
        results: Vec::new(),
    };
    for cfg in configs {
        if cfg.event != event {
            continue;
        }
        if let Some(tool) = ctx.tool {
            if !tool_matches(&cfg.tools, tool) {
                continue;
            }
        }
        let started = Instant::now();
        // tokio::process（异步等待不阻塞 worker）+ kill_on_drop（超时即杀，
        // 不留孤儿进程）；命令按空白切分（复杂命令走包装脚本，§13.6）
        let mut command =
            tokio::process::Command::new(cfg.command.split_whitespace().next().unwrap_or(""));
        command
            .args(cfg.command.split_whitespace().skip(1))
            .current_dir(ctx.root)
            .kill_on_drop(true)
            .env("TENON_HOOK_EVENT", event.as_str())
            .env("TENON_SESSION", ctx.session_id)
            .env("TENON_TOOL", ctx.tool.unwrap_or(""));
        if let Some(args) = ctx.tool_args {
            command.env("TENON_ARGS", truncate(args, 32_000));
        }
        if let Some(output) = ctx.tool_output {
            command.env("TENON_OUTPUT", truncate(output, 2_000));
        }
        let run =
            tokio::time::timeout(Duration::from_millis(cfg.timeout_ms), command.output()).await;
        let duration_ms = started.elapsed().as_millis() as u64;
        match run {
            Ok(Ok(out)) => {
                let code = out.status.code();
                let passed = out.status.success();
                let stderr = String::from_utf8_lossy(&out.stderr).to_string();
                let blocked =
                    !passed && event == HookEvent::PreTool && cfg.on_fail == HookOnFail::Block;
                if blocked && outcome.block_reason.is_none() {
                    outcome.block_reason = Some(truncate(stderr.trim(), 500));
                }
                outcome.blocked |= blocked;
                outcome.results.push(HookRunResult {
                    event: event.as_str(),
                    command: cfg.command.clone(),
                    exit_code: code,
                    duration_ms,
                    action: if passed {
                        "pass"
                    } else if blocked {
                        "block"
                    } else {
                        "pass"
                    },
                    reason: if blocked {
                        outcome.block_reason.clone()
                    } else {
                        None
                    },
                });
            }
            Ok(Err(_)) => {
                outcome.results.push(HookRunResult {
                    event: event.as_str(),
                    command: cfg.command.clone(),
                    exit_code: None,
                    duration_ms,
                    action: "error",
                    reason: Some("hook 启动失败".into()),
                });
            }
            Err(_) => {
                // 超时：kill_on_drop 保证 Child 被 drop 时终止
                outcome.results.push(HookRunResult {
                    event: event.as_str(),
                    command: cfg.command.clone(),
                    exit_code: None,
                    duration_ms,
                    action: "error",
                    reason: Some("hook 超时".into()),
                });
            }
        }
    }
    outcome
}

/// 配置校验（settings PUT 原子提交）：≤16 条、命令 1–512 字符、超时钳制、
/// 枚举合法、glob ≤16 条。非法返回错误文案。
pub fn validate_hook_configs(value: &serde_json::Value) -> Result<Vec<HookConfig>, String> {
    let arr = value.as_array().ok_or("hooks 须为数组")?;
    if arr.len() > 16 {
        return Err("hooks 最多 16 条".into());
    }
    let mut out = Vec::with_capacity(arr.len());
    for (i, item) in arr.iter().enumerate() {
        let cfg: HookConfig =
            serde_json::from_value(item.clone()).map_err(|e| format!("hooks[{i}]: {e}"))?;
        let cmd = cfg.command.trim();
        if cmd.is_empty() || cmd.len() > 512 {
            return Err(format!("hooks[{i}].command 须为 1-512 字符"));
        }
        if !(100..=30_000).contains(&cfg.timeout_ms) {
            return Err(format!("hooks[{i}].timeout_ms 取值 100-30000"));
        }
        if cfg.tools.len() > 16 {
            return Err(format!("hooks[{i}].tools 最多 16 条"));
        }
        out.push(HookConfig {
            command: cmd.to_string(),
            ..cfg
        });
    }
    Ok(out)
}

/// Claude Code hooks.json 导入（v2.25，design-v2.md §8④ / design.md §13.6）：
/// Claude Code settings.json 的 `hooks` 对象 → Tenon HookConfig 扁平数组。
/// 返回 (转换后待校验的 JSON 数组, 跳过报告)——不支持的事件 / 非 command 类型
/// 跳过并报告（不静默丢配置）；调用方再经 `validate_hook_configs` 整体校验。
pub fn import_claude_hooks(
    value: &serde_json::Value,
) -> Result<(Vec<HookConfig>, Vec<String>), String> {
    let obj = value
        .as_object()
        .ok_or("hooks.json 须为对象（事件名 → matcher 组数组）")?;
    // 事件名映射：UserPromptSubmit→pre_turn / Stop→post_turn 为语义近似（如实标注）
    let event_map: &[(&str, HookEvent)] = &[
        ("PreToolUse", HookEvent::PreTool),
        ("PostToolUse", HookEvent::PostTool),
        ("UserPromptSubmit", HookEvent::PreTurn),
        ("Stop", HookEvent::PostTurn),
    ];
    let mut out: Vec<HookConfig> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    for (cc_name, tenon_event) in event_map {
        let Some(groups) = obj.get(*cc_name).and_then(|g| g.as_array()) else {
            continue;
        };
        for (gi, group) in groups.iter().enumerate() {
            // matcher：工具名 / "*" / 正则——Tenon tools 为 glob 形态，字面工具名
            // 与 "*" 可直接映射；正则形态（含元字符）保守跳过并报告
            let matcher = group.get("matcher").and_then(|m| m.as_str()).unwrap_or("*");
            let tools: Vec<String> = if matcher == "*" || matcher.is_empty() {
                Vec::new()
            } else if matcher
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_')
            {
                vec![matcher.to_string()]
            } else {
                skipped.push(format!(
                    "{cc_name}[{gi}].matcher=\"{matcher}\"：正则形态暂不支持（tools 过滤为 glob），请改包装脚本内自行过滤"
                ));
                continue;
            };
            let Some(hooks_arr) = group.get("hooks").and_then(|h| h.as_array()) else {
                skipped.push(format!("{cc_name}[{gi}]：无 hooks 数组"));
                continue;
            };
            for (hi, h) in hooks_arr.iter().enumerate() {
                let htype = h.get("type").and_then(|t| t.as_str()).unwrap_or("command");
                if htype != "command" {
                    skipped.push(format!(
                        "{cc_name}[{gi}].hooks[{hi}].type={htype}：仅支持 command"
                    ));
                    continue;
                }
                let Some(command) = h.get("command").and_then(|c| c.as_str()) else {
                    skipped.push(format!("{cc_name}[{gi}].hooks[{hi}]：缺 command"));
                    continue;
                };
                // timeout 秒 → ms（钳 100–30000；缺省 Tenon 默认 5000）
                let timeout_ms = h
                    .get("timeout")
                    .and_then(|t| t.as_u64())
                    .map(|secs| (secs.saturating_mul(1000)).clamp(100, 30_000))
                    .unwrap_or(5000);
                out.push(HookConfig {
                    event: *tenon_event,
                    command: command.to_string(),
                    timeout_ms,
                    on_fail: HookOnFail::default(),
                    tools: tools.clone(),
                });
            }
        }
    }
    // Claude Code 独有事件（无 Tenon 对应）：显式报告
    for unsupported in [
        "Notification",
        "SessionStart",
        "SessionEnd",
        "PreCompact",
        "SubagentStop",
    ] {
        if obj.contains_key(unsupported) {
            skipped.push(format!(
                "{unsupported}：Claude Code 独有事件，Tenon 无对应（不导入）"
            ));
        }
    }
    // 复用既有校验（超 16 条 / 命令非法等——在副本上校验再返回）
    let json_arr = serde_json::to_value(&out).map_err(|e| e.to_string())?;
    let validated = validate_hook_configs(&json_arr)?;
    Ok((validated, skipped))
}

/// 工作目录解析兜底（测试 / 无根场景）。
pub fn default_root() -> PathBuf {
    PathBuf::from(".")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx<'a>(tool: Option<&'a str>, args: Option<&'a str>) -> HookContext<'a> {
        HookContext {
            session_id: "s-test",
            root: Path::new("."),
            tool,
            tool_args: args,
            tool_output: None,
        }
    }

    #[test]
    fn tool_glob_matching() {
        assert!(tool_matches(&[], "anything"));
        assert!(tool_matches(&["run_*".into()], "run_tests"));
        assert!(!tool_matches(&["run_*".into()], "apply_patch"));
        assert!(tool_matches(&["apply_patch".into()], "apply_patch"));
        assert!(tool_matches(&["run_*".into(), "grep".into()], "grep"));
    }

    #[tokio::test]
    async fn pre_tool_pass_and_env_injection() {
        let configs = vec![HookConfig {
            event: HookEvent::PreTool,
            command: "/usr/bin/env".into(),
            timeout_ms: 5000,
            on_fail: HookOnFail::Block,
            tools: vec![],
        }];
        let outcome = run_hooks(
            &configs,
            HookEvent::PreTool,
            &ctx(Some("grep"), Some(r#"{"pattern":"x"}"#)),
        )
        .await;
        assert!(!outcome.blocked);
        assert_eq!(outcome.results.len(), 1);
        assert_eq!(outcome.results[0].action, "pass");
        assert_eq!(outcome.results[0].exit_code, Some(0));
    }

    #[tokio::test]
    async fn pre_tool_block_on_nonzero_exit() {
        // 注：命令按空白朴素切分（§13.6，复杂命令走包装脚本），测试用 /usr/bin/false
        // 保证退出码非 0 且无引号依赖。
        let configs = vec![HookConfig {
            event: HookEvent::PreTool,
            command: "/usr/bin/false".into(),
            timeout_ms: 5000,
            on_fail: HookOnFail::Block,
            tools: vec!["apply_patch".into()],
        }];
        let outcome = run_hooks(
            &configs,
            HookEvent::PreTool,
            &ctx(Some("apply_patch"), None),
        )
        .await;
        assert!(outcome.blocked);
        assert_eq!(outcome.results[0].action, "block");
        assert_eq!(outcome.results[0].exit_code, Some(1));

        // continue 语义：非 0 不阻断
        let configs = vec![HookConfig {
            event: HookEvent::PreTool,
            command: "/usr/bin/false".into(),
            timeout_ms: 5000,
            on_fail: HookOnFail::Continue,
            tools: vec![],
        }];
        let outcome = run_hooks(
            &configs,
            HookEvent::PreTool,
            &ctx(Some("apply_patch"), None),
        )
        .await;
        assert!(!outcome.blocked);
    }

    #[tokio::test]
    async fn timeout_kills_and_marks_error() {
        let configs = vec![HookConfig {
            event: HookEvent::PostTurn,
            command: "/bin/sleep 5".into(),
            timeout_ms: 100,
            on_fail: HookOnFail::Continue,
            tools: vec![],
        }];
        let outcome = run_hooks(&configs, HookEvent::PostTurn, &ctx(None, None)).await;
        assert_eq!(outcome.results[0].action, "error");
        assert!(outcome.results[0]
            .reason
            .as_deref()
            .unwrap()
            .contains("超时"));
    }

    #[tokio::test]
    async fn tool_filter_skips_non_matching() {
        let configs = vec![HookConfig {
            event: HookEvent::PreTool,
            command: "/usr/bin/env".into(),
            timeout_ms: 5000,
            on_fail: HookOnFail::Block,
            tools: vec!["run_*".into()],
        }];
        let outcome = run_hooks(
            &configs,
            HookEvent::PreTool,
            &ctx(Some("apply_patch"), None),
        )
        .await;
        assert!(outcome.results.is_empty(), "不匹配的 hook 不执行");
    }

    #[test]
    fn validate_rejects_bad_configs() {
        assert!(validate_hook_configs(&serde_json::json!([{
            "event": "pre_tool", "command": "x", "timeout_ms": 50
        }]))
        .is_err());
        assert!(validate_hook_configs(&serde_json::json!([{
            "event": "nope", "command": "x"
        }]))
        .is_err());
        assert!(validate_hook_configs(&serde_json::json!([]))
            .unwrap()
            .is_empty());
    }
}

#[cfg(test)]
mod claude_import_tests {
    use super::*;

    /// v2.25：Claude Code hooks.json 导入矩阵。
    #[test]
    fn claude_import_maps_events_matcher_timeout() {
        let cc = serde_json::json!({
            "hooks": {
                "PreToolUse": [
                    {"matcher": "Bash", "hooks": [
                        {"type": "command", "command": "guard.sh", "timeout": 30}
                    ]}
                ],
                "PostToolUse": [
                    {"hooks": [{"type": "command", "command": "log.sh"}]}
                ],
                "UserPromptSubmit": [
                    {"hooks": [{"type": "command", "command": "prompt.sh", "timeout": 1}]}
                ],
                "Stop": [
                    {"hooks": [{"type": "command", "command": "stop.sh"}]}
                ],
                "Notification": [
                    {"hooks": [{"type": "command", "command": "notify.sh"}]}
                ]
            }
        });
        let hooks_obj = cc.get("hooks").unwrap();
        let (configs, skipped) = import_claude_hooks(hooks_obj).unwrap();
        // 四事件映射（timeout 30s→30000ms、1s→1000ms、缺省 5000）
        assert_eq!(configs.len(), 4);
        assert_eq!(configs[0].event, HookEvent::PreTool);
        assert_eq!(configs[0].tools, vec!["Bash".to_string()]);
        assert_eq!(configs[0].timeout_ms, 30_000);
        assert_eq!(configs[1].event, HookEvent::PostTool);
        assert!(configs[1].tools.is_empty(), "无 matcher = 全部工具");
        assert_eq!(configs[2].event, HookEvent::PreTurn);
        assert_eq!(configs[2].timeout_ms, 1000);
        assert_eq!(configs[3].event, HookEvent::PostTurn);
        // Notification 独有事件被报告
        assert!(
            skipped.iter().any(|s| s.contains("Notification")),
            "{skipped:?}"
        );
    }

    #[test]
    fn claude_import_reports_unsupported_shapes() {
        // 正则 matcher / 非 command type / 缺 command / 无 hooks 数组
        let cc = serde_json::json!({
            "PreToolUse": [
                {"matcher": "Bash|Edit", "hooks": [{"type": "command", "command": "a.sh"}]},
                {"matcher": "Edit", "hooks": [{"type": "prompt", "command": "b.sh"}]},
                {"matcher": "Edit", "hooks": [{"type": "command"}]},
                {"matcher": "Edit"}
            ],
            "SessionStart": [{"hooks": [{"type": "command", "command": "c.sh"}]}]
        });
        let (configs, skipped) = import_claude_hooks(&cc).unwrap();
        assert!(configs.is_empty(), "全部不可导入: {configs:?}");
        assert!(
            skipped.iter().any(|s| s.contains("正则形态")),
            "{skipped:?}"
        );
        assert!(
            skipped.iter().any(|s| s.contains("type=prompt")),
            "{skipped:?}"
        );
        assert!(
            skipped.iter().any(|s| s.contains("缺 command")),
            "{skipped:?}"
        );
        assert!(
            skipped.iter().any(|s| s.contains("无 hooks 数组")),
            "{skipped:?}"
        );
        assert!(
            skipped.iter().any(|s| s.contains("SessionStart")),
            "{skipped:?}"
        );
    }

    #[test]
    fn claude_import_over_limit_rejected_wholesale() {
        // 17 条合法命令 → 超上限整体拒绝（复用 validate_hook_configs）
        let mut groups = Vec::new();
        for i in 0..17 {
            groups.push(serde_json::json!({
                "hooks": [{"type": "command", "command": format!("h{i}.sh")}]
            }));
        }
        let cc = serde_json::json!({"Stop": groups});
        let err = import_claude_hooks(&cc);
        assert!(err.is_err(), "超 16 条应整体拒绝");
    }
}
