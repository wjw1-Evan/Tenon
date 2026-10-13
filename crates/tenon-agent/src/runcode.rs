//! run_code——PTC 程序化工具调用（§9.2 v2.63 DSH §3.3 对标；v2.67 内层写扩展）。
//!
//! 模型在一段 JS 脚本内经 `tool(name, ...)` 程序化批量调用工具（read_file /
//! glob / grep / list_dir 四只读 + apply_patch——v2.67 批量重构 codemod），
//! 一次回合完成「搜索→批量改」类工作——DSH 工具管线行「PTC `run_code`
//! 子调用重入同一管线」的落地：内层调用直接复用 executor::execute_tool
//! （同 `project_rel` 作用域守卫、同上下文），即「子调用重入管线」的执行层
//! 等价物。
//!
//! 变更记账（v2.67 不变式）：内层 apply_patch 的 changed_files 回传外层
//! ToolOutput → session 既有 B 级写前快照 + checkpoint / 撤销链路全覆盖；
//! 脚本顺序执行非原子（中途失败先前写入不回滚，回合撤销兜底）。
//!
//! 预算双封顶：宿主工具调用次数（IO 成本）+ boa RuntimeLimits（纯 JS
//! 循环 / 递归自旋）；同步执行、无事件循环；MCP / LSP / 出网不可达
//! （扩展须过策略总线，§9.2 如实留档）。

use std::cell::RefCell;
use std::sync::atomic::{AtomicUsize, Ordering};

use boa_engine::{Context, JsError, JsResult, JsString, JsValue, NativeFunction, Source};

use crate::executor::{execute_tool, truncate_output, ToolContext, ToolOutput};

/// 宿主工具调用次数上限（IO 成本封顶；超过即报错中断脚本）。
const MAX_TOOL_CALLS: usize = 500;
/// 默认 JS 循环迭代上限（纯 JS 自旋封顶；boa RuntimeLimits）。
const MAX_LOOP_ITERATIONS: u64 = 10_000_000;
/// JS 递归深度上限。
const MAX_RECURSION: usize = 128;
/// console 收集条数上限（防内存膨胀；超出静默丢弃——输出面另有 20k 截断）。
const MAX_LOG_LINES: usize = 200;

/// 内层工具面（§9.2 v2.63/v2.67：白名单即权限边界——脚本无法触达其他工具）。
const ALLOWED_TOOLS: [&str; 5] = ["read_file", "glob", "grep", "list_dir", "apply_patch"];

/// 评估期间的本线程宿主状态：ctx 指针仅在 eval 同步存续期内有效，
/// 调用计数、console 收集与内层变更清单（v2.67 变更记账）在 eval 结束后取回。
struct HostSlot {
    ctx: *const ToolContext,
    calls: AtomicUsize,
    logs: RefCell<Vec<String>>,
    /// 内层写工具的 changed_files 汇总（回传外层 → session checkpoint）。
    changed: RefCell<Vec<String>>,
}

thread_local! {
    static HOST: RefCell<Option<HostSlot>> = const { RefCell::new(None) };
}

/// 取宿主（eval 存续期内必在；native 函数只在 eval 调用栈上运行）。
fn with_host<T>(f: impl FnOnce(&mut HostSlot) -> T) -> T {
    HOST.with(|slot| {
        let mut slot = slot.borrow_mut();
        let st = slot
            .as_mut()
            .expect("run_code 宿主状态仅在 eval 存续期内被访问");
        f(st)
    })
}

fn js_error(msg: impl Into<String>) -> JsError {
    JsError::from_opaque(JsValue::from(JsString::from(msg.into())))
}

fn arg_str(args: &[JsValue], i: usize) -> Option<String> {
    args.get(i)
        .and_then(|v| v.as_string())
        .map(|s| s.to_std_string_escaped())
}

/// `tool(name, ...)`：程序化调用内层工具，返回工具输出文本（失败即抛错中断
/// 脚本）；apply_patch 以 JSON 对象传参（boa serde to_json 直传既有编辑全语义）。
fn tool_impl(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    with_host(|st| {
        let calls = st.calls.fetch_add(1, Ordering::SeqCst) + 1;
        if calls > MAX_TOOL_CALLS {
            return Err(js_error(format!(
                "工具调用次数超限（≤{MAX_TOOL_CALLS}）——请缩小检索范围"
            )));
        }
        let Some(name) = arg_str(args, 0) else {
            return Err(js_error("tool 第 1 参须为工具名字符串"));
        };
        if !ALLOWED_TOOLS.contains(&name.as_str()) {
            return Err(js_error(format!(
                "run_code 内层仅允许 {ALLOWED_TOOLS:?}，未知工具: {name}"
            )));
        }
        let json_args = if name == "apply_patch" {
            // v2.67：JSON 对象参数——search/replace、replace_all、range、ops 全支持
            //（boa serde_json 转换常驻可用：to_json 返回 Option——undefined 即缺参）
            let Some(obj) = args.get(1) else {
                return Err(js_error(
                    "tool(\"apply_patch\") 缺少第 2 参（编辑参数对象）",
                ));
            };
            let value = obj
                .to_json(ctx)
                .map_err(|e| js_error(format!("apply_patch 参数解析失败: {e}")))?
                .ok_or_else(|| {
                    js_error("tool(\"apply_patch\") 参数须为对象（如 {file, search, replace}）")
                })?;
            if !value.is_object() {
                return Err(js_error(
                    "tool(\"apply_patch\") 参数须为对象（如 {file, search, replace}）",
                ));
            }
            value
        } else {
            match name.as_str() {
                "list_dir" => match arg_str(args, 1) {
                    Some(p) => serde_json::json!({"path": p}),
                    None => serde_json::json!({}),
                },
                "read_file" => {
                    let Some(p) = arg_str(args, 1) else {
                        return Err(js_error("tool(\"read_file\") 缺少第 2 参（路径字符串）"));
                    };
                    serde_json::json!({"path": p})
                }
                "grep" => {
                    let Some(p) = arg_str(args, 1) else {
                        return Err(js_error("tool(\"grep\") 缺少第 2 参（正则字符串）"));
                    };
                    serde_json::json!({"pattern": p})
                }
                _ => {
                    let Some(p) = arg_str(args, 1) else {
                        return Err(js_error("tool(\"glob\") 缺少第 2 参（模式字符串）"));
                    };
                    match arg_str(args, 2) {
                        Some(dir) => serde_json::json!({"pattern": p, "path": dir}),
                        None => serde_json::json!({"pattern": p}),
                    }
                }
            }
        };
        // SAFETY：ctx 指针由 run_code_with_limits 在 eval 前写入、eval 同步
        // 返回后即取回清除；native 函数只在 eval 调用栈上执行，指针恒有效。
        let tool_ctx = unsafe { &*st.ctx };
        let out = execute_tool(tool_ctx, &name, &json_args);
        if out.ok {
            // v2.67 变更记账：内层写的 changed_files 汇总回传外层（session checkpoint）
            if !out.changed_files.is_empty() {
                st.changed.borrow_mut().extend(out.changed_files);
            }
            Ok(JsValue::from(JsString::from(out.content)))
        } else {
            Err(js_error(format!("tool(\"{name}\") 失败: {}", out.content)))
        }
    })
}

/// `log(...)`：console 收集（空格连接各参数），供脚本输出中间结果。
fn log_impl(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    with_host(|st| {
        if st.logs.borrow().len() >= MAX_LOG_LINES {
            return Ok(JsValue::undefined());
        }
        let mut parts = Vec::with_capacity(args.len());
        for v in args {
            parts.push(match v.as_string() {
                Some(s) => s.to_std_string_escaped(),
                None => v
                    .to_string(ctx)
                    .map(|s| s.to_std_string_escaped())
                    .unwrap_or_else(|_| "<unstringifiable>".into()),
            });
        }
        st.logs.borrow_mut().push(parts.join(" "));
        Ok(JsValue::undefined())
    })
}

/// 执行 run_code 脚本：console 收集 + 返回值字符串化，合计 20k 截断。
pub(crate) fn run_code_script(ctx: &ToolContext, code: &str) -> ToolOutput {
    run_code_with_limits(ctx, code, MAX_LOOP_ITERATIONS)
}

fn run_code_with_limits(ctx: &ToolContext, code: &str, loop_limit: u64) -> ToolOutput {
    if code.trim().is_empty() {
        return ToolOutput::err("code 不可为空（JS 脚本）");
    }
    let mut engine = Context::default();
    let limits = engine.runtime_limits_mut();
    limits.set_loop_iteration_limit(loop_limit);
    limits.set_recursion_limit(MAX_RECURSION);
    if let Err(e) = engine.register_global_callable(
        JsString::from("tool"),
        2,
        NativeFunction::from_fn_ptr(tool_impl),
    ) {
        return ToolOutput::err(format!("run_code 宿主初始化失败: {e}"));
    }
    if let Err(e) = engine.register_global_callable(
        JsString::from("log"),
        0,
        NativeFunction::from_fn_ptr(log_impl),
    ) {
        return ToolOutput::err(format!("run_code 宿主初始化失败: {e}"));
    }

    HOST.with(|slot| {
        *slot.borrow_mut() = Some(HostSlot {
            ctx: ctx as *const ToolContext,
            calls: AtomicUsize::new(0),
            logs: RefCell::new(Vec::new()),
            changed: RefCell::new(Vec::new()),
        });
    });

    let eval_result: Result<JsValue, _> = engine.eval(Source::from_bytes(code));

    let (calls, logs, changed) = HOST.with(|slot| {
        let st = slot.borrow_mut().take();
        match st {
            Some(st) => (
                st.calls.load(Ordering::SeqCst),
                st.logs.into_inner(),
                st.changed.into_inner(),
            ),
            None => (0, Vec::new(), Vec::new()),
        }
    });

    let value = match eval_result {
        Ok(v) => v,
        Err(e) => {
            let msg = e.to_string();
            let hint = if msg.contains("iteration loops")
                || msg.contains("loop iteration limit")
                || msg.contains("recursion")
                || msg.contains("RuntimeLimit")
            {
                "（脚本超出运行预算：循环 / 递归上限）"
            } else {
                ""
            };
            // 失败前的 console 同样回流（模型可据已输出内容自纠）
            let mut out = String::new();
            if !logs.is_empty() {
                out.push_str("console:\n");
                out.push_str(&logs.join("\n"));
                out.push('\n');
            }
            out.push_str(&format!("脚本执行失败{hint}: {msg}"));
            return ToolOutput::err(out);
        }
    };

    let mut out = String::new();
    if !logs.is_empty() {
        out.push_str("console:\n");
        out.push_str(&logs.join("\n"));
        out.push('\n');
    }
    let ret = match value.to_string(&mut engine) {
        Ok(s) => s.to_std_string_escaped(),
        Err(e) => format!("<无法字符串化: {e}>"),
    };
    out.push_str(&format!("返回值: {ret}（tool 调用 {calls} 次）"));
    // v2.67 变更记账：内层写的 changed_files 回传外层 → session checkpoint / 撤销
    ToolOutput {
        ok: true,
        content: truncate_output(&out),
        changed_files: changed,
        exit_code: None,
        dirty_conflict: None,
        dirty_merged: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> (tempfile::TempDir, ToolContext) {
        let dir = tempfile::tempdir().unwrap();
        let c = ToolContext::new(dir.path(), std::time::Duration::from_secs(30));
        (dir, c)
    }

    #[test]
    fn batch_read_with_log() {
        let (dir, c) = ctx();
        std::fs::write(dir.path().join("a.txt"), "AAA\n").unwrap();
        std::fs::write(dir.path().join("b.txt"), "BBB\n").unwrap();
        let out = run_code_script(
            &c,
            "log(tool(\"read_file\", \"a.txt\").trim()); tool(\"read_file\", \"b.txt\")",
        );
        assert!(out.ok, "{}", out.content);
        assert!(out.content.contains("console:\nAAA"), "{}", out.content);
        assert!(out.content.contains("返回值: BBB"), "{}", out.content);
        assert!(out.content.contains("tool 调用 2 次"), "{}", out.content);
    }

    /// PTC 主场景：glob 定位 + 循环逐文件读汇总——一次回合完成。
    #[test]
    fn glob_then_loop_read() {
        let (dir, c) = ctx();
        for (name, body) in [("x.txt", "X1"), ("y.txt", "Y1"), ("z.log", "Z1")] {
            std::fs::write(dir.path().join(name), format!("{body}\n")).unwrap();
        }
        let script = r#"
            var files = tool("glob", "*.txt").split("\n").filter(Boolean);
            log("files=" + files);
            var summary = "";
            for (const f of files) {
                summary += f + "=" + tool("read_file", f).trim() + ";";
            }
            summary;
        "#;
        let out = run_code_script(&c, script);
        assert!(out.ok, "{}", out.content);
        assert!(
            out.content.contains("x.txt=X1;") && out.content.contains("y.txt=Y1;"),
            "{}",
            out.content
        );
        assert!(
            !out.content.contains("z.log"),
            "glob 模式过滤生效: {}",
            out.content
        );
    }

    #[test]
    fn grep_inside_script() {
        let (dir, c) = ctx();
        std::fs::write(dir.path().join("f.txt"), "needle here\nother\n").unwrap();
        let out = run_code_script(
            &c,
            "var hits = tool(\"grep\", \"needle\"); log(hits); hits.length > 0",
        );
        assert!(out.ok, "{}", out.content);
        assert!(out.content.contains("needle here"), "{}", out.content);
        assert!(out.content.contains("返回值: true"), "{}", out.content);
    }

    /// 内层调用与直接调用同一作用域守卫：越界路径被拒。
    #[test]
    fn scope_escape_rejected() {
        let (_d, c) = ctx();
        let out = run_code_script(&c, "tool(\"read_file\", \"../../etc/passwd\")");
        assert!(!out.ok);
        assert!(out.content.contains("路径越界"), "{}", out.content);
    }

    #[test]
    fn disallowed_and_malformed_calls() {
        let (_d, c) = ctx();
        // 白名单之外的工具不可达（run_command 不在内层面）
        let out = run_code_script(&c, "tool(\"run_command\", \"echo x\")");
        assert!(!out.ok);
        assert!(out.content.contains("内层仅允许"), "{}", out.content);
        // 未知工具
        let out = run_code_script(&c, "tool(\"nope\", \"x\")");
        assert!(!out.ok);
        assert!(out.content.contains("未知工具"), "{}", out.content);
        // 缺参
        let out = run_code_script(&c, "tool()");
        assert!(!out.ok);
        assert!(out.content.contains("工具名字符串"), "{}", out.content);
        let out = run_code_script(&c, "tool(\"read_file\")");
        assert!(!out.ok);
        assert!(out.content.contains("缺少第 2 参"), "{}", out.content);
        // apply_patch 参数须为对象
        let out = run_code_script(&c, "tool(\"apply_patch\", \"not-an-object\")");
        assert!(!out.ok);
        assert!(out.content.contains("参数须为对象"), "{}", out.content);
        // 空脚本
        let out = run_code_script(&c, "   ");
        assert!(!out.ok);
        assert!(out.content.contains("code 不可为空"), "{}", out.content);
    }

    #[test]
    fn tool_call_cap_interrupts() {
        let (_d, c) = ctx();
        let out = run_code_script(
            &c,
            "for (var i = 0; i < 1000; i++) { tool(\"list_dir\", \".\") }",
        );
        assert!(!out.ok);
        assert!(out.content.contains("工具调用次数超限"), "{}", out.content);
    }

    #[test]
    fn infinite_loop_bounded_by_runtime_limits() {
        let (_d, c) = ctx();
        // 测试用低循环预算（正式面 10_000_000）——死循环受 RuntimeLimits 约束
        let out = run_code_with_limits(&c, "var x = 0; while (true) { x++; } x", 100_000);
        assert!(!out.ok);
        assert!(out.content.contains("运行预算"), "{}", out.content);
    }

    #[test]
    fn syntax_error_reports_cleanly() {
        let (_d, c) = ctx();
        let out = run_code_script(&c, "var x = {");
        assert!(!out.ok);
        assert!(out.content.contains("脚本执行失败"), "{}", out.content);
    }

    #[test]
    fn output_truncated_at_budget() {
        let (_d, c) = ctx();
        let out = run_code_script(&c, "log(\"z\".repeat(40000)); \"done\"");
        assert!(out.ok, "{}", out.content);
        assert!(
            out.content.chars().count() <= 21000,
            "20k 截断生效（尾注在内）: {}",
            out.content.chars().count()
        );
        assert!(out.content.contains("丢弃"), "{}", out.content);
    }

    /// v2.67：B 级写面——只读会话拒绝（executor handler 检查；经 execute_tool 全链）。
    #[test]
    fn readonly_session_rejected() {
        let (dir, c) = ctx();
        std::fs::write(dir.path().join("a.txt"), "hi\n").unwrap();
        use std::sync::atomic::Ordering;
        c.readonly.store(true, Ordering::SeqCst);
        let out = execute_tool(
            &c,
            "run_code",
            &serde_json::json!({"code": "tool(\"read_file\", \"a.txt\")"}),
        );
        assert!(!out.ok);
        assert!(out.content.contains("只读会话"), "{}", out.content);
    }

    /// v2.67 主场景：批量重构 codemod——glob 循环 + apply_patch 一次回合
    /// 多文件改写，变更记账回传外层 changed_files（checkpoint 链路输入）。
    /// ⚠️search/replace 是行粒度替换（命中区间整行替换）——测试数据用整行文案。
    #[test]
    fn batch_codemod_via_apply_patch() {
        let (dir, c) = ctx();
        std::fs::write(dir.path().join("a.txt"), "foo\nkeep\n").unwrap();
        std::fs::write(dir.path().join("b.txt"), "foo\n").unwrap();
        let script = r#"
            var files = tool("glob", "*.txt").split("\n").filter(Boolean);
            for (const f of files) {
                tool("apply_patch", {file: f, search: "foo", replace: "bar"});
            }
            "edited " + files.length;
        "#;
        let out = run_code_script(&c, script);
        assert!(out.ok, "{}", out.content);
        assert!(out.content.contains("返回值: edited 2"), "{}", out.content);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "bar\nkeep\n"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("b.txt")).unwrap(),
            "bar\n"
        );
        // 变更记账：外层 changed_files 全列（session checkpoint 链路输入）
        assert_eq!(
            out.changed_files,
            vec!["a.txt".to_string(), "b.txt".to_string()]
        );
    }

    /// v2.67：内层编辑失败中断脚本（search 未命中）；先前迭代写入不回滚
    ///（顺序执行非原子，回合撤销兜底——如实语义）。
    #[test]
    fn inner_edit_failure_interrupts() {
        let (dir, c) = ctx();
        std::fs::write(dir.path().join("a.txt"), "hit\n").unwrap();
        std::fs::write(dir.path().join("b.txt"), "other\n").unwrap();
        let script = r#"
            tool("apply_patch", {file: "a.txt", search: "hit", replace: "HIT"});
            tool("apply_patch", {file: "b.txt", search: "不存在", replace: "x"});
            "done";
        "#;
        let out = run_code_script(&c, script);
        assert!(!out.ok);
        // JsError 对 opaque 字符串的 Display 带引号转义——断言内层报错原文即可
        assert!(out.content.contains("未在文件中命中"), "{}", out.content);
        // 先前迭代已落盘（非原子），且失败路径 changed_files 丢失（脚本未完成）
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "HIT\n"
        );
        let _ = dir;
    }

    /// v2.67：replace_all 内层可用（多命中一次改写）。
    #[test]
    fn inner_replace_all_supported() {
        let (dir, c) = ctx();
        std::fs::write(dir.path().join("f.txt"), "MARK\nx\nMARK\n").unwrap();
        let out = run_code_script(
            &c,
            "tool(\"apply_patch\", {file: \"f.txt\", search: \"MARK\", replace: \"DONE\", replace_all: true}); tool(\"read_file\", \"f.txt\")",
        );
        assert!(out.ok, "{}", out.content);
        assert!(out.content.contains("DONE"), "{}", out.content);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("f.txt")).unwrap(),
            "DONE\nx\nDONE\n"
        );
    }
}
