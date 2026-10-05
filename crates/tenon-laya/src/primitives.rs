//! 三类判定原语与结果语义（§9.8）：
//! choice（选项分类）/ score（量表打分）/ bool（布尔判断）。

use serde::{Deserialize, Serialize};

/// 集成点（§9.8 表 #1-4，逐项可开关；v1.92 收敛 #1-3，v1.124 增 #4 agent 工具面）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Feature {
    /// #1 意图预判：DECIDING 前对用户消息分类
    Intent,
    /// #2 命令风险辅助：规则库外命令补盲区（不改变 A/B/C/D 分级）
    Risk,
    /// #3 路由启发：纯读任务建议轻模型
    Routing,
    /// #4 agent 可调用判定工具 laya_decide（v1.124）
    AgentTool,
}

impl Feature {
    pub fn as_str(self) -> &'static str {
        match self {
            Feature::Intent => "intent",
            Feature::Risk => "risk",
            Feature::Routing => "routing",
            Feature::AgentTool => "agent_tool",
        }
    }

    /// 从 config `[models.laya].features` 列表解析开关集
    /// （旧配置中的 prefilter / triage 项被静默忽略）。
    pub fn enabled_set(features: &[String]) -> std::collections::HashSet<Feature> {
        let mut set = std::collections::HashSet::new();
        for f in features {
            match f.as_str() {
                "intent" => {
                    set.insert(Feature::Intent);
                }
                "risk" => {
                    set.insert(Feature::Risk);
                }
                "routing" => {
                    set.insert(Feature::Routing);
                }
                "agent_tool" => {
                    set.insert(Feature::AgentTool);
                }
                _ => {}
            };
        }
        set
    }
}

/// 判定种类（decider_call 事件负载）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DecisionKind {
    Choice,
    Score,
    Bool,
}

/// 判定结果语义：不可用即回退现状（§9.8 边界）。
#[derive(Debug, Clone, PartialEq)]
pub enum LayaOutcome<T> {
    /// 集成点被关闭 → 调用方按现状执行。
    Disabled,
    /// 模型不可用（未下载 / 加载失败）→ 回退现状。
    Unavailable(&'static str),
    /// 推理超时（默认 200ms）→ 回退现状。
    TimedOut,
    /// 判定成功（confidence 未校准，仅排序参考）。
    Success {
        value: T,
        confidence: Option<f32>,
        duration_ms: u128,
    },
}

impl<T> LayaOutcome<T> {
    pub fn is_success(&self) -> bool {
        matches!(self, LayaOutcome::Success { .. })
    }

    /// 成功时取值。
    pub fn value(self) -> Option<T> {
        match self {
            LayaOutcome::Success { value, .. } => Some(value),
            _ => None,
        }
    }
}

/// #4 agent 可调用判定工具（v1.124）：laya_decide 的 kind 参数。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecideKind {
    /// choice：文本 → 意图标签
    Intent,
    /// score：命令 → 0..1 风险分
    Risk,
    /// bool：任务文本 → 是否建议轻模型
    Route,
}

impl DecideKind {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "intent" => DecideKind::Intent,
            "risk" => DecideKind::Risk,
            "route" => DecideKind::Route,
            _ => return None,
        })
    }
}

/// #4 判定值（v1.124）：序列化即工具输出 JSON（tag = kind）。
/// 置信 / 分值均未校准，仅作排序、预筛、提示参考（§9.8 边界）。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DecideValue {
    /// 意图标签 + softmax 原始概率（未校准）
    Intent { label: IntentLabel, confidence: f32 },
    /// 风险分 0..1（未校准）
    Risk { score: f32 },
    /// 是否建议轻模型
    Route { suggest_light: bool },
}

impl DecideValue {
    /// decider_call Trace 的 kind 字段。
    pub fn kind_str(&self) -> &'static str {
        match self {
            DecideValue::Intent { .. } => "choice",
            DecideValue::Risk { .. } => "score",
            DecideValue::Route { .. } => "bool",
        }
    }
}

/// 意图预判标签（§9.8 集成点 #1）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntentLabel {
    /// 纯问答
    PureQa,
    /// 需要改动
    NeedsChange,
    /// 只读分析
    ReadOnlyAnalysis,
    /// 需要出网
    NeedsNetwork,
}

impl IntentLabel {
    pub fn as_str(self) -> &'static str {
        match self {
            IntentLabel::PureQa => "pure_qa",
            IntentLabel::NeedsChange => "needs_change",
            IntentLabel::ReadOnlyAnalysis => "read_only_analysis",
            IntentLabel::NeedsNetwork => "needs_network",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "pure_qa" => IntentLabel::PureQa,
            "needs_change" => IntentLabel::NeedsChange,
            "read_only_analysis" => IntentLabel::ReadOnlyAnalysis,
            "needs_network" => IntentLabel::NeedsNetwork,
            _ => return None,
        })
    }

    pub fn all() -> [IntentLabel; 4] {
        [
            IntentLabel::PureQa,
            IntentLabel::NeedsChange,
            IntentLabel::ReadOnlyAnalysis,
            IntentLabel::NeedsNetwork,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feature_switches_from_config() {
        let set = Feature::enabled_set(&[
            "intent".into(),
            "risk".into(),
            "routing".into(),
            // 旧配置残留项被静默忽略
            "prefilter".into(),
            "triage".into(),
        ]);
        assert_eq!(set.len(), 3);
        let partial = Feature::enabled_set(&["intent".into()]);
        assert!(partial.contains(&Feature::Intent));
        assert!(!partial.contains(&Feature::Risk));
        assert!(Feature::enabled_set(&["bogus".into()]).is_empty());
    }

    #[test]
    fn labels_roundtrip() {
        for l in IntentLabel::all() {
            assert_eq!(IntentLabel::parse(l.as_str()), Some(l));
        }
    }
}
