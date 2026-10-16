//! codex 提示词 A/B 行为对照探针（§9.6 v1.185 / v2.72；一次性验证，非门禁）。
//!
//! 同一批探针任务在两种系统提示下各跑一遍，差异即提示词的行为效应：
//! - full     = `build_system_prompt` 完整产品提示（含 codex 集成纪律节）
//! - baseline = 同一组装但剥掉 codex 集成引入的节（任务执行与验证 / 专项任务
//!   规范 / 对话纪律 / 最终回答 / 子任务清单 / 计划模式）
//!
//! 真实端点调用（读仓库根 config.local.toml 的 GLM 配置），有成本，
//! 默认 `#[ignore]`，手动运行：
//! `cargo test -p tenon-agent --test codex_prompt_probe -- --ignored --nocapture`
//!
//! 探针设计（每条对应一节纪律的可观测行为）：
//! - P1 对话纪律（本仓库事实先读码）：问 average 函数隐患——看是否先调工具
//!   读代码再回答，还是凭函数名空谈；
//! - P2 专项任务规范（review 心态）：要求 review calc.rs——看发现是否带
//!   文件:行号、按严重度组织、总结置后、提及测试盲区；
//! - P3 计划质量纪律（简单任务不走计划）：单行版本号修改——看是否误提交
//!   submit_plan、是否直达 apply_patch、改动是否落盘。

use std::path::{Path, PathBuf};

use tenon_config::Config;
use tenon_core::context::{ProjectRules, SessionMemory};
use tenon_core::prompt;
use tenon_models::{
    build_provider, ChatMessage, ChatRequest, EnvKeyStore, ModelProvider, ToolSpec,
};

// ---------- 夹具 ----------

const CALC_RS: &str = r#"#[derive(Clone)]
pub struct User {
    pub name: String,
    pub email: String,
}

/// 计算一组数值的平均值。
pub fn average(nums: &[f64]) -> f64 {
    let total: f64 = nums.iter().sum();
    total / nums.len() as f64
}

/// 按名字查找用户；找不到时返回第一个用户。
pub fn find_user(users: &[User], name: &str) -> User {
    for u in users {
        if u.name == name {
            return u.clone();
        }
    }
    users[0].clone()
}

/// 计算 0..n 的累加和。
pub fn accumulate(n: u32) -> u32 {
    let mut acc: u32 = 0;
    for i in 0..n {
        acc += i;
    }
    acc
}
"#;

const README_MD: &str = "# demo-math\n\n版本：0.1.0\n\n一个演示用的数学工具库。\n";

fn write_fixture(root: &Path) {
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/calc.rs"), CALC_RS).unwrap();
    std::fs::write(root.join("README.md"), README_MD).unwrap();
}

// ---------- 工具面（与产品 tool_specs 同名同参的子集） ----------

fn probe_tools() -> Vec<ToolSpec> {
    vec![
        (
            "read_file",
            "读取文本文件；大文件用 offset（1-based 起始行）/ limit（行数）分段读取——输出带行号前缀",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "offset": {"type": "integer", "minimum": 1, "description": "1-based 起始行"},
                    "limit": {"type": "integer", "minimum": 1, "description": "读取行数"}
                },
                "required": ["path"]
            }),
        ),
        (
            "list_dir",
            "列出目录",
            serde_json::json!({"type": "object", "properties": {"path": {"type": "string"}}}),
        ),
        (
            "grep",
            "搜索代码",
            serde_json::json!({
                "type": "object",
                "properties": {"pattern": {"type": "string"}},
                "required": ["pattern"]
            }),
        ),
        (
            "apply_patch",
            "编辑文件：① 定向替换 file + search + replace（search 须唯一）② file + range(1-based 行区间含端点，缺省追加) + content",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "file": {"type": "string"},
                    "range": {"type": "array", "items": {"type": "integer"}, "minItems": 2, "maxItems": 2},
                    "content": {"type": "string"},
                    "search": {"type": "string"},
                    "replace": {"type": "string"},
                    "replace_all": {"type": "boolean"}
                },
                "required": ["file"]
            }),
        ),
        (
            "submit_plan",
            "提交执行计划并暂停等待用户批准（复杂任务先计划后执行）：items 为 1-12 项的一句话计划；单步任务、纯问答与简单改动不用",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "items": {"type": "array", "items": {"type": "string"}, "minItems": 1, "maxItems": 12}
                },
                "required": ["items"]
            }),
        ),
    ]
    .into_iter()
    .map(|(name, description, parameters)| ToolSpec {
        name: name.into(),
        description: description.into(),
        parameters,
    })
    .collect()
}

fn walk_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name.starts_with('.') || name == "target" {
            continue;
        }
        if p.is_dir() {
            walk_files(&p, out);
        } else {
            out.push(p);
        }
    }
}

/// 极简工具执行器：够探针多轮跑通即可，不做沙箱。
fn execute_tool(root: &Path, name: &str, args: &serde_json::Value) -> String {
    let s = |k: &str| args.get(k).and_then(|v| v.as_str()).unwrap_or("");
    match name {
        "read_file" => {
            let path = root.join(s("path"));
            let text = match std::fs::read_to_string(&path) {
                Ok(t) => t,
                Err(e) => return format!("读取失败: {e}"),
            };
            let offset = args.get("offset").and_then(|v| v.as_u64()).unwrap_or(1) as usize;
            let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(2000) as usize;
            text.lines()
                .skip(offset.saturating_sub(1))
                .take(limit)
                .enumerate()
                .map(|(i, line)| format!("{:>4}: {}", offset + i, line))
                .collect::<Vec<_>>()
                .join("\n")
        }
        "list_dir" => {
            let dir = if s("path").is_empty() {
                root.to_path_buf()
            } else {
                root.join(s("path"))
            };
            let mut files = Vec::new();
            walk_files(&dir, &mut files);
            files
                .iter()
                .filter_map(|p| p.strip_prefix(root).ok())
                .map(|p| p.to_string_lossy().to_string())
                .collect::<Vec<_>>()
                .join("\n")
        }
        "grep" => {
            let mut files = Vec::new();
            walk_files(root, &mut files);
            let mut hits = Vec::new();
            for f in files {
                if let Ok(text) = std::fs::read_to_string(&f) {
                    for (i, line) in text.lines().enumerate() {
                        if line.contains(s("pattern")) {
                            let rel = f.strip_prefix(root).unwrap_or(&f).to_string_lossy();
                            hits.push(format!("{}:{}: {}", rel, i + 1, line.trim()));
                        }
                    }
                }
            }
            if hits.is_empty() {
                format!("无匹配: {}", s("pattern"))
            } else {
                hits.join("\n")
            }
        }
        "apply_patch" => {
            let file = s("file");
            let path = root.join(file);
            let text = match std::fs::read_to_string(&path) {
                Ok(t) => t,
                Err(e) => return format!("编辑失败（文件不存在）: {e}"),
            };
            let search = s("search");
            let replace = s("replace");
            if !search.is_empty() {
                if !text.contains(search) {
                    return format!("编辑失败：search 在 {file} 中零命中");
                }
                let next = if args
                    .get("replace_all")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
                {
                    text.replace(search, replace)
                } else {
                    text.replacen(search, replace, 1)
                };
                std::fs::write(&path, &next).unwrap();
                return format!("已编辑 {file}（search/replace 定向替换）");
            }
            if let Some(range) = args.get("range").and_then(|v| v.as_array()) {
                let start = range.first().and_then(|v| v.as_u64()).unwrap_or(1) as usize;
                let end = range.get(1).and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                let content = s("content");
                let mut lines: Vec<String> = text.lines().map(String::from).collect();
                if start > lines.len() {
                    lines.extend(content.lines().map(String::from));
                } else {
                    let end = end.min(lines.len());
                    lines.splice(start - 1..end, content.lines().map(String::from));
                }
                std::fs::write(&path, lines.join("\n") + "\n").unwrap();
                return format!("已编辑 {file}（range 行区间替换）");
            }
            // ops 多文件原子编辑（Codex apply_patch 形态）：逐项套用，任一失败即停。
            if let Some(ops) = args.get("ops").and_then(|v| v.as_array()) {
                for (i, op) in ops.iter().enumerate() {
                    let ofile = op.get("file").and_then(|v| v.as_str()).unwrap_or("");
                    let opath = root.join(ofile);
                    let Ok(otext) = std::fs::read_to_string(&opath) else {
                        return format!("ops 编辑失败：第 {} 项文件 {ofile} 不存在", i + 1);
                    };
                    let osearch = op.get("search").and_then(|v| v.as_str()).unwrap_or("");
                    let oreplace = op.get("replace").and_then(|v| v.as_str()).unwrap_or("");
                    if !osearch.is_empty() {
                        if !otext.contains(osearch) {
                            return format!(
                                "ops 编辑失败：第 {} 项 search 在 {ofile} 中零命中",
                                i + 1
                            );
                        }
                        std::fs::write(&opath, otext.replacen(osearch, oreplace, 1)).unwrap();
                    } else if let Some(range) = op.get("range").and_then(|v| v.as_array()) {
                        let start = range.first().and_then(|v| v.as_u64()).unwrap_or(1) as usize;
                        let end = range.get(1).and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                        let ocontent = op.get("content").and_then(|v| v.as_str()).unwrap_or("");
                        let mut lines: Vec<String> = otext.lines().map(String::from).collect();
                        if start > lines.len() {
                            lines.extend(ocontent.lines().map(String::from));
                        } else {
                            let end = end.min(lines.len());
                            lines.splice(start - 1..end, ocontent.lines().map(String::from));
                        }
                        std::fs::write(&opath, lines.join("\n") + "\n").unwrap();
                    } else {
                        return format!(
                            "ops 编辑失败：第 {} 项缺 search/replace 或 range/content",
                            i + 1
                        );
                    }
                }
                return format!("已按 ops 原子编辑 {} 个文件", ops.len());
            }
            "编辑失败：需要 search/replace、range/content 或 ops 形态".into()
        }
        "submit_plan" => "计划已提交，等待用户批准".into(),
        other => format!("未知工具: {other}"),
    }
}

// ---------- 探针执行 ----------

struct RunResult {
    rounds: usize,
    tool_trace: Vec<String>,
    plan_submitted: bool,
    answer: String,
    input_tokens: u64,
    output_tokens: u64,
}

/// 剥掉输出契约 JSON 壳：纯回答场景模型按 OUTPUT_CONTRACT 返回
/// {"intent","answer","needs_change":false}，取 answer 字段。
fn extract_answer(content: &str) -> String {
    let trimmed = content
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
        if let Some(a) = v.get("answer").and_then(|a| a.as_str()) {
            return a.to_string();
        }
    }
    content.to_string()
}

async fn run_probe(
    provider: &dyn ModelProvider,
    system_prompt: &str,
    task: &str,
    root: &Path,
) -> RunResult {
    let mut messages = vec![ChatMessage::system(system_prompt), ChatMessage::user(task)];
    let tools = probe_tools();
    let mut tool_trace = Vec::new();
    let mut plan_submitted = false;
    let mut input_tokens = 0u64;
    let mut output_tokens = 0u64;

    for round in 1..=5 {
        let mut req = ChatRequest::new(provider.default_model(), messages.clone());
        req.tools = tools.clone();
        req.temperature = 0.2;
        let resp = match provider.chat(&req).await {
            Ok(r) => r,
            Err(e) => {
                return RunResult {
                    rounds: round,
                    tool_trace,
                    plan_submitted,
                    answer: format!("(模型调用失败: {e})"),
                    input_tokens,
                    output_tokens,
                }
            }
        };
        input_tokens += resp.usage.input_tokens;
        output_tokens += resp.usage.output_tokens;
        if resp.tool_calls.is_empty() {
            return RunResult {
                rounds: round,
                tool_trace,
                plan_submitted,
                answer: extract_answer(&resp.content),
                input_tokens,
                output_tokens,
            };
        }
        let mut assistant = ChatMessage::assistant(resp.content.clone());
        assistant.tool_calls = resp.tool_calls.clone();
        messages.push(assistant);
        for tc in &resp.tool_calls {
            if tc.name == "submit_plan" {
                plan_submitted = true;
            }
            let arg = tc
                .arguments
                .get("path")
                .or_else(|| tc.arguments.get("pattern"))
                .or_else(|| tc.arguments.get("file"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            tool_trace.push(format!("r{round}:{}({arg})", tc.name));
            let output = execute_tool(root, &tc.name, &tc.arguments);
            messages.push(ChatMessage::tool_result(tc.id.clone(), output));
        }
    }
    RunResult {
        rounds: 5,
        tool_trace,
        plan_submitted,
        answer: "(5 轮内未收尾)".into(),
        input_tokens,
        output_tokens,
    }
}

// ---------- 两种系统提示 ----------

fn full_prompt() -> String {
    prompt::build_system_prompt(
        &ProjectRules::default(),
        &SessionMemory::default(),
        &[],
        &[],
    )
}

/// 与产品同骨架、但剥掉 codex 集成引入的全部纪律节。
fn baseline_prompt() -> String {
    let mut p = String::new();
    p.push_str(prompt::IDENTITY);
    p.push_str("\n\n## 安全铁律\n");
    p.push_str(prompt::IRON_RULES);
    p.push_str("\n\n## 会话记忆（L2）\n（新任务，尚无记忆）\n");
    p.push_str("\n## 项目规则（L3，仅收窄）\n");
    p.push('\n');
    p.push_str(&prompt::tool_catalog());
    p.push_str("\n## 输出契约\n");
    p.push_str(prompt::OUTPUT_CONTRACT);
    p
}

// ---------- 探针主流程 ----------

#[tokio::test]
#[ignore = "真实端点 A/B 对照（GLM 调用有成本）；手动运行：cargo test -p tenon-agent --test codex_prompt_probe -- --ignored --nocapture"]
async fn codex_prompt_ab_probe() {
    let cfg_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../config.local.toml");
    let Some(pcfg) = Config::parse_toml_file(&cfg_path)
        .ok()
        .and_then(|c| c.models.providers.get("glm").cloned())
    else {
        eprintln!("config.local.toml 无 glm provider，跳过探针");
        return;
    };
    let provider = build_provider("glm", &pcfg, &EnvKeyStore).expect("构建 provider");
    let tmp = tempfile::tempdir().unwrap();
    let _ = &tmp; // 各探针自建临时目录，此目录仅占位保生命周期

    struct Probe {
        id: &'static str,
        task: &'static str,
        check_readme: bool,
    }
    let probes = [
        Probe {
            id: "P1 对话纪律（先读码再答）",
            task: "src/calc.rs 里的 average 函数有没有什么隐患？",
            check_readme: false,
        },
        Probe {
            id: "P2 专项任务规范（review 心态）",
            task: "帮我 review 一下 src/calc.rs，把问题都指出来。",
            check_readme: false,
        },
        Probe {
            id: "P3 计划质量纪律（简单改动不走计划）",
            task: "把 README.md 里的版本号从 0.1.0 改成 0.2.0。",
            check_readme: true,
        },
    ];

    // PROBE_FILTER=P2 之类的子串可只跑部分探针（省真实端点成本）。
    let filter = std::env::var("PROBE_FILTER").unwrap_or_default();
    let mut report = String::new();
    for probe in &probes {
        if !filter.is_empty() && !probe.id.contains(&filter) {
            continue;
        }
        for (variant, sys) in [("full", full_prompt()), ("baseline", baseline_prompt())] {
            let dir = tempfile::tempdir().unwrap();
            write_fixture(dir.path());
            let r = run_probe(&*provider, &sys, probe.task, dir.path()).await;
            let answer_chars = r.answer.chars().count();
            let cites = r.answer.matches("calc.rs:").count();
            let readme_ok = if probe.check_readme {
                std::fs::read_to_string(dir.path().join("README.md"))
                    .map(|t| t.contains("0.2.0"))
                    .unwrap_or(false)
            } else {
                false
            };
            report.push_str(&format!(
                "===== {} [{variant}] =====\n轮数: {}  工具: {:?}\nplan_submitted: {}  readme改为0.2.0: {}\ntokens(in/out): {}/{}  回答字数: {answer_chars}  行号引用数: {cites}\n---- 回答 ----\n{}\n\n",
                probe.id,
                r.rounds,
                r.tool_trace,
                r.plan_submitted,
                if probe.check_readme { format!("{readme_ok}") } else { "-".into() },
                r.input_tokens,
                r.output_tokens,
                r.answer,
            ));
        }
    }
    let _ = tmp;
    println!("{report}");
}
