//! 内置工具目录与分级（设计方案 §9.2）。
//!
//! | 级 | 工具 |
//! |---|---|
//! | A | read_file / list_dir / grep / git_read / lsp_query / laya_decide（v1.124）/ skill_use（v1.130 §13.4）/ subtasks（v1.146） |
//! | B | apply_patch / run_tests / run_build / install_deps |
//! | C | http_fetch / web_search（v1.182） |
//! | D | git_commit / git_push / create_pr |
//! | 按声明 | plugin_*（外部进程插件映射分级） |

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tool {
    ReadFile,
    ListDir,
    Grep,
    GitRead,
    LspQuery,
    /// Laya 本地判定（§9.8 #4，v1.124）：本地 CPU 推理，只读零副作用。
    LayaDecide,
    /// 代理技能读取（§13.4，v1.130）：读 SKILL.md 全文进上下文，只读零副作用。
    SkillUse,
    /// 子任务清单（§9.2，v1.146）：会话内计划状态全量替换，零工作区副作用。
    Subtasks,
    /// 计划提交（§9.2，v1.179 Codex 形态计划模式）：复杂任务先计划后执行，
    /// 零工作区副作用；提交即暂停待用户批准。
    SubmitPlan,
    /// MCP 元工具（§13.3，v1.187）：跨服务器 resources / prompts 枚举与读取，
    /// 零工作区副作用（外部进程只读调用）。
    McpMeta,
    /// 并行子代理（§9.5，v1.190）：worktree 隔离的子任务分发——子代理在其
    /// 受管 worktree 内做 B 级写，本工具面按 B 级闸门。
    SpawnSubagents,
    /// 网络搜索（§9.2，v1.182）：DuckDuckGo Lite 免密钥后端，C 级出网并审计，
    /// SSRF 守卫与 http_fetch 同轨（tenon-agent webfetch）。
    WebSearch,
    ApplyPatch,
    RunTests,
    RunBuild,
    InstallDeps,
    HttpFetch,
    GitCommit,
    GitPush,
    CreatePr,
    /// 外部进程插件工具（`plugin_*`），分级随声明。
    Plugin,
}

impl Tool {
    pub fn name(self) -> &'static str {
        match self {
            Tool::ReadFile => "read_file",
            Tool::ListDir => "list_dir",
            Tool::Grep => "grep",
            Tool::GitRead => "git_read",
            Tool::LspQuery => "lsp_query",
            Tool::LayaDecide => "laya_decide",
            Tool::SkillUse => "skill_use",
            Tool::Subtasks => "subtasks",
            Tool::SubmitPlan => "submit_plan",
            Tool::McpMeta => "mcp_meta_*",
            Tool::SpawnSubagents => "spawn_subagents",
            Tool::WebSearch => "web_search",
            Tool::ApplyPatch => "apply_patch",
            Tool::RunTests => "run_tests",
            Tool::RunBuild => "run_build",
            Tool::InstallDeps => "install_deps",
            Tool::HttpFetch => "http_fetch",
            Tool::GitCommit => "git_commit",
            Tool::GitPush => "git_push",
            Tool::CreatePr => "create_pr",
            Tool::Plugin => "plugin_*",
        }
    }

    /// 静态分级（§9.2）。plugin 工具按插件声明映射，此处返回 None。
    pub fn level(self) -> Option<crate::policy::Level> {
        use crate::policy::Level;
        Some(match self {
            Tool::ReadFile
            | Tool::ListDir
            | Tool::Grep
            | Tool::GitRead
            | Tool::LspQuery
            | Tool::LayaDecide
            | Tool::SkillUse
            | Tool::Subtasks
            | Tool::SubmitPlan
            | Tool::McpMeta => Level::A,
            Tool::ApplyPatch
            | Tool::RunTests
            | Tool::RunBuild
            | Tool::InstallDeps
            | Tool::SpawnSubagents => Level::B,
            Tool::HttpFetch | Tool::WebSearch => Level::C,
            Tool::GitCommit | Tool::GitPush => Level::D,
            Tool::CreatePr => Level::Composite,
            Tool::Plugin => return None,
        })
    }

    pub fn from_name(name: &str) -> Option<Tool> {
        Some(match name {
            "read_file" => Tool::ReadFile,
            "list_dir" => Tool::ListDir,
            "grep" => Tool::Grep,
            "git_read" => Tool::GitRead,
            "lsp_query" => Tool::LspQuery,
            "laya_decide" => Tool::LayaDecide,
            "skill_use" => Tool::SkillUse,
            "subtasks" => Tool::Subtasks,
            "submit_plan" => Tool::SubmitPlan,
            "mcp_meta_resources_list"
            | "mcp_meta_resources_read"
            | "mcp_meta_prompts_list"
            | "mcp_meta_prompts_get" => Tool::McpMeta,
            "web_search" => Tool::WebSearch,
            "apply_patch" => Tool::ApplyPatch,
            "run_tests" => Tool::RunTests,
            "run_build" => Tool::RunBuild,
            "install_deps" => Tool::InstallDeps,
            "http_fetch" => Tool::HttpFetch,
            "git_commit" => Tool::GitCommit,
            "git_push" => Tool::GitPush,
            "create_pr" => Tool::CreatePr,
            n if n.starts_with("plugin_") => Tool::Plugin,
            _ => return None,
        })
    }
}

/// 结构化编辑参数（§9.2：file + range + content）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PatchOp {
    pub file: String,
    /// 1-based 行区间（含端点）；None = 追加到文件尾。
    #[serde(default)]
    pub range: Option<(usize, usize)>,
    pub content: String,
}

/// 生成统一 diff（用于证据卡与 diff 面板）。
pub fn unified_diff(before: &str, after: &str, path: &str) -> String {
    let diff = similar::TextDiff::from_lines(before, after);
    // 头部交给 similar 生成（带真实路径）；再手工 push 一对 `--- a` 会双头
    let a = format!("a/{path}");
    let b = format!("b/{path}");
    diff.unified_diff()
        .context_radius(3)
        .header(a.as_str(), b.as_str())
        .to_string()
}

/// 统计 diff 的变更行数（新增 + 删除），供熔断器记账。
pub fn count_changed_lines(before: &str, after: &str) -> u64 {
    let diff = similar::TextDiff::from_lines(before, after);
    let mut n = 0u64;
    for change in diff.iter_all_changes() {
        match change.tag() {
            similar::ChangeTag::Insert | similar::ChangeTag::Delete => n += 1,
            similar::ChangeTag::Equal => {}
        }
    }
    n
}

#[cfg(test)]
mod diff_tests {
    use super::unified_diff;

    #[test]
    fn unified_diff_single_header_with_path() {
        let out = unified_diff("a\nb\n", "a\nB\n", "src/lib.rs");
        assert!(out.starts_with("--- a/src/lib.rs\n+++ b/src/lib.rs\n"));
        assert_eq!(
            out.matches("--- a/src/lib.rs").count(),
            1,
            "diff 头不得重复输出：{out}"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::Level;

    #[test]
    fn builtin_levels_match_design_table() {
        assert_eq!(Tool::ReadFile.level(), Some(Level::A));
        assert_eq!(Tool::Grep.level(), Some(Level::A));
        assert_eq!(Tool::GitRead.level(), Some(Level::A));
        assert_eq!(Tool::LspQuery.level(), Some(Level::A));
        assert_eq!(Tool::SkillUse.level(), Some(Level::A), "v1.130 §13.4");
        assert_eq!(
            Tool::Subtasks.level(),
            Some(Level::A),
            "v1.146 §9.2 零工作区副作用"
        );
        assert_eq!(Tool::ApplyPatch.level(), Some(Level::B));
        assert_eq!(Tool::RunTests.level(), Some(Level::B));
        assert_eq!(Tool::RunBuild.level(), Some(Level::B));
        assert_eq!(Tool::InstallDeps.level(), Some(Level::B));
        assert_eq!(Tool::HttpFetch.level(), Some(Level::C));
        assert_eq!(Tool::WebSearch.level(), Some(Level::C), "v1.182 §9.2 出网");
        assert_eq!(Tool::GitCommit.level(), Some(Level::D));
        assert_eq!(Tool::GitPush.level(), Some(Level::D));
        assert_eq!(Tool::CreatePr.level(), Some(Level::Composite));
        assert_eq!(Tool::Plugin.level(), None, "plugin 按声明映射");
    }

    #[test]
    fn names_roundtrip() {
        for t in [
            Tool::ReadFile,
            Tool::ListDir,
            Tool::Grep,
            Tool::GitRead,
            Tool::LspQuery,
            Tool::SkillUse,
            Tool::Subtasks,
            Tool::ApplyPatch,
            Tool::RunTests,
            Tool::RunBuild,
            Tool::InstallDeps,
            Tool::HttpFetch,
            Tool::WebSearch,
            Tool::GitCommit,
            Tool::GitPush,
            Tool::CreatePr,
        ] {
            assert_eq!(Tool::from_name(t.name()), Some(t));
        }
        assert_eq!(Tool::from_name("plugin_db.query"), Some(Tool::Plugin));
        assert_eq!(Tool::from_name("nope"), None);
    }

    #[test]
    fn unified_diff_and_line_counting() {
        let before = "a\nb\nc\n";
        let after = "a\nB\nc\nd\n";
        let d = unified_diff(before, after, "f.txt");
        assert!(d.contains("--- a/f.txt"));
        assert!(d.contains("+++ b/f.txt"));
        assert!(d.contains("-b"));
        assert!(d.contains("+B"));
        assert!(d.contains("+d"));
        // 1 删 + 2 增
        assert_eq!(count_changed_lines(before, after), 3);
    }

    #[test]
    fn patch_op_serializes() {
        let op: PatchOp =
            serde_json::from_str(r#"{"file":"src/main.rs","range":[3,5],"content":"new"}"#)
                .unwrap();
        assert_eq!(op.range, Some((3, 5)));
        let op2: PatchOp = serde_json::from_str(r#"{"file":"new.txt","content":"x"}"#).unwrap();
        assert_eq!(op2.range, None, "缺省追加到文件尾");
    }
}
