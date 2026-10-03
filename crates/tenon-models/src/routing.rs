//! 模型路由（设计方案 §11）：v1 显式 + 轻量启发式（纯读任务建议轻模型）。
//! auto 路由为实验特性默认关（§5 非目标 8）。

/// 判断任务是否为纯读任务（启发式，仅生成「建议轻模型」提示，不自动改派）。
pub fn is_pure_read_task(text: &str) -> bool {
    let t = text.to_lowercase();
    let read_markers = [
        "解释",
        "分析",
        "总结",
        "什么是",
        "为什么",
        "怎么看",
        "如何理解",
        "审查",
        "review",
        "explain",
        "analyze",
        "summarize",
        "what is",
        "why",
        "how does",
        "read",
        "不改",
        "不要改",
        "只读",
        "read-only",
        "no changes",
        "don't change",
    ];
    let write_markers = [
        "修复",
        "修改",
        "重构",
        "实现",
        "添加",
        "删除",
        "创建",
        "升级",
        "迁移",
        "fix",
        "refactor",
        "implement",
        "add",
        "remove",
        "delete",
        "create",
        "update",
        "migrate",
    ];
    let reads = read_markers.iter().any(|m| t.contains(m));
    let writes = write_markers.iter().any(|m| t.contains(m));
    reads && !writes
}

/// 显式路由器：默认模型 + 可选轻模型（纯读任务建议项）。
#[derive(Debug, Clone)]
pub struct Router {
    default_model: String,
    /// 轻模型（配置了才有建议；§11 轻量启发式）。
    light_model: Option<String>,
    /// auto 自动路由（实验，默认关，§5-8）。
    pub auto: bool,
}

impl Router {
    pub fn new(default_model: impl Into<String>) -> Self {
        Self {
            default_model: default_model.into(),
            light_model: None,
            auto: false,
        }
    }

    pub fn with_light_model(mut self, light: impl Into<String>) -> Self {
        self.light_model = Some(light.into());
        self
    }

    pub fn default_model(&self) -> &str {
        &self.default_model
    }

    /// 为任务建议模型：纯读任务 → 轻模型建议（展示层一键采纳，不强制）。
    pub fn suggest(&self, task_text: &str) -> ModelSuggestion {
        if !self.auto && self.light_model.is_some() && is_pure_read_task(task_text) {
            ModelSuggestion {
                model: self.default_model.clone(),
                lighter_alternative: self.light_model.clone(),
                reason: if is_pure_read_task(task_text) {
                    SuggestionReason::PureReadTask
                } else {
                    SuggestionReason::Default
                },
            }
        } else {
            ModelSuggestion {
                model: self.default_model.clone(),
                lighter_alternative: None,
                reason: SuggestionReason::Default,
            }
        }
    }

    /// 会话内显式切换模型（降级 / 用户改派；上下文随迁由调用方保证，§11）。
    pub fn switch_to(&mut self, model: &str) {
        self.default_model = model.to_string();
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SuggestionReason {
    Default,
    PureReadTask,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelSuggestion {
    pub model: String,
    pub lighter_alternative: Option<String>,
    pub reason: SuggestionReason,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pure_read_detection_bilingual() {
        assert!(is_pure_read_task("解释一下认证流程，不要改任何文件"));
        assert!(is_pure_read_task("explain the auth flow"));
        assert!(is_pure_read_task("帮我分析这个模块的性能瓶颈"));
        assert!(!is_pure_read_task("修复登录 bug"));
        assert!(!is_pure_read_task("explain the auth flow and fix the bug"));
        assert!(!is_pure_read_task("add a new endpoint"));
    }

    #[test]
    fn suggestion_offers_lighter_for_read_tasks() {
        let r = Router::new("glm-4.6").with_light_model("glm-4.6-air");
        let s = r.suggest("总结这个 PR 的改动");
        assert_eq!(s.model, "glm-4.6");
        assert_eq!(s.lighter_alternative.as_deref(), Some("glm-4.6-air"));
        assert_eq!(s.reason, SuggestionReason::PureReadTask);

        let s2 = r.suggest("修复这个编译错误");
        assert_eq!(s2.lighter_alternative, None, "写任务不给轻模型建议");
    }

    #[test]
    fn no_light_model_means_no_suggestion() {
        let r = Router::new("m1");
        let s = r.suggest("解释代码");
        assert_eq!(s.lighter_alternative, None);
    }

    #[test]
    fn explicit_switch_changes_default() {
        let mut r = Router::new("a");
        r.switch_to("b");
        assert_eq!(r.default_model(), "b");
    }
}
