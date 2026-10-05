//! 动作能力分级与执行判定（设计方案 §12.2 / §12.3，v1.89）。
//!
//! v1.89 移除审批门禁：A/B/C/D 只表达只读、沙箱、出网与不可逆风险边界。
//! 非只读动作直接执行；只读开关是唯一的分级级硬拒绝，工具黑名单由上层过滤。
//! 沙箱、快照、熔断器和审计不因免审批而省略。

use serde::{Deserialize, Serialize};

/// 动作能力分级（§12.2）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    /// A 只读
    A,
    /// B 沙箱写执行
    B,
    /// C 出网
    C,
    /// D 不可逆
    D,
    /// C+D 复合（出网 + 不可逆副作用，例如创建 PR）
    Composite,
}

impl Level {
    pub fn as_str(self) -> &'static str {
        match self {
            Level::A => "a",
            Level::B => "b",
            Level::C => "c",
            Level::D => "d",
            Level::Composite => "cd",
        }
    }
}

/// 权限引擎输入：动作的分级。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Action {
    pub level: Level,
}

/// 权限判定结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// 直接执行。
    Auto,
    /// 硬拒绝（当前仅只读会话）。
    Denied(&'static str),
}

/// 会话级执行上下文（v1.93：mode / trusted / snapshot_enabled / degraded
/// 均无读者，已删——档位随 v1.89 审批移除失效，快照与沙箱能力由执行器自身校验）。
#[derive(Debug, Clone, Default)]
pub struct Policy {
    /// 会话只读开关（§9.3；`set_readonly` 控制命令即时切换，v1.93）。
    pub readonly: bool,
}

impl Policy {
    pub fn decide(&self, action: Action) -> Decision {
        if action.level == Level::A {
            return Decision::Auto;
        }
        if self.readonly {
            Decision::Denied("readonly")
        } else {
            Decision::Auto
        }
    }
}

/// 团队策略（M3 §19 / §12.2）：由团队下发、**只收窄**的全局约束。
/// v1.89 移除审批后 `force_interactive` 无操作语义，已随 v1.92 删除
/// （旧 policy.toml 中的该键被 serde 静默忽略）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct TeamPolicy {
    /// 全局禁用工具（名单，跨会话）
    #[serde(default)]
    pub denied_tools: Vec<String>,
    /// 单任务成本上限（美元；低于配置者生效）
    #[serde(default)]
    pub max_cost_usd: Option<f64>,
}

impl TeamPolicy {
    /// 熔断上限取更严格者。
    pub fn clamp_cost(&self, configured: f64) -> f64 {
        match self.max_cost_usd {
            Some(team_cap) => configured.min(team_cap),
            None => configured,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_writes_execute_directly_unless_readonly() {
        let p = Policy::default();
        for level in [Level::B, Level::C, Level::D, Level::Composite] {
            assert_eq!(p.decide(Action { level }), Decision::Auto);
        }

        let readonly = Policy { readonly: true };
        for level in [Level::B, Level::C, Level::D, Level::Composite] {
            assert_eq!(
                readonly.decide(Action { level }),
                Decision::Denied("readonly")
            );
        }
    }

    #[test]
    fn team_policy_serialization_and_cost_clamp() {
        let tp: TeamPolicy = serde_json::from_str(r#"{"denied_tools": ["git_push"]}"#).unwrap();
        assert_eq!(tp.denied_tools, vec!["git_push".to_string()]);
        assert_eq!(tp.max_cost_usd, None);
        assert_eq!(tp.clamp_cost(5.0), 5.0);
        let capped = TeamPolicy {
            max_cost_usd: Some(2.0),
            ..TeamPolicy::default()
        };
        assert_eq!(capped.clamp_cost(5.0), 2.0);
    }
}
