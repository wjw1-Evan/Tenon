//! 动作能力分级与权限判定（设计方案 §12.2 / §12.7 / §12.3）。
//!
//! 规则要点：
//! - A 级只读恒自动；C / D 恒审批（七条铁律之一，Laya 与档位均不可放宽）；
//! - 档位（自动 / 交互）只影响 B 级；
//! - 未信任仓库（TOFU）默认交互档：B 级也需审批；
//! - 只读开关禁用全部写类动作（B / C / D）；
//! - checkpoint 不可用（`snapshot_enabled = false`）→「自动 = 必可回滚」不变式，
//!   自动档整体降为交互（§10.3）；
//! - Windows 降级档（无 WSL2）强制交互档，且 B 级必审（§12.3）。

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

/// 档位（§12.2）：只影响 B 级。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Interactive,
    Auto,
}

/// 权限引擎输入：动作的分级。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Action {
    pub level: Level,
}

/// 权限判定结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// 自动执行。
    Auto,
    /// 需要审批卡（once / session / deny）。
    NeedsApproval,
    /// 直接拒绝（只读开关 / 降级档禁止项）。
    Denied(&'static str),
}

/// 会话级权限上下文（§12.2 / §12.3 / §12.7）。
#[derive(Debug, Clone)]
pub struct Policy {
    pub mode: Mode,
    /// TOFU：仓库是否已信任（§12.7）。
    pub trusted: bool,
    /// 会话只读开关（§9.3）。
    pub readonly: bool,
    /// 快照能力可用（§10.3：不可用 → 强制交互档）。
    pub snapshot_enabled: bool,
    /// Windows 降级档（无 WSL2，§12.3）：仅 A 级 + 写守卫、强制交互。
    pub degraded: bool,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            mode: Mode::Interactive,
            trusted: false,
            readonly: false,
            snapshot_enabled: true,
            degraded: false,
        }
    }
}

impl Policy {
    pub fn decide(&self, action: Action) -> Decision {
        match action.level {
            Level::A => Decision::Auto,
            Level::B => self.decide_b(),
            // C / D 恒审批：任何档位、信任状态、Laya 判定均不可放宽。
            Level::C | Level::D | Level::Composite => {
                if self.readonly {
                    Decision::Denied("readonly")
                } else {
                    Decision::NeedsApproval
                }
            }
        }
    }

    fn decide_b(&self) -> Decision {
        if self.readonly {
            return Decision::Denied("readonly");
        }
        if self.degraded {
            // 降级档：B 级必审（无完整沙箱兜底）。
            return Decision::NeedsApproval;
        }
        let auto_allowed = self.mode == Mode::Auto && self.trusted && self.snapshot_enabled;
        if auto_allowed {
            Decision::Auto
        } else {
            Decision::NeedsApproval
        }
    }

    /// 「自动档 = 必可回滚」不变式（§10.3）：快照库不可用时自动档不可用。
    pub fn effective_mode(&self) -> Mode {
        if self.mode == Mode::Auto && (!self.snapshot_enabled || !self.trusted) {
            Mode::Interactive
        } else {
            self.mode
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(mode: Mode, trusted: bool) -> Policy {
        Policy {
            mode,
            trusted,
            ..Policy::default()
        }
    }

    #[test]
    fn a_level_is_always_auto() {
        for p in [policy(Mode::Interactive, false), policy(Mode::Auto, true)] {
            assert_eq!(p.decide(Action { level: Level::A }), Decision::Auto);
        }
        // 只读开关不禁只读动作
        let ro = Policy {
            readonly: true,
            ..Policy::default()
        };
        assert_eq!(ro.decide(Action { level: Level::A }), Decision::Auto);
    }

    #[test]
    fn c_and_d_always_require_approval() {
        // 任何组合下 C/D 都不能自动
        for mode in [Mode::Interactive, Mode::Auto] {
            for trusted in [false, true] {
                for level in [Level::C, Level::D] {
                    let p = policy(mode, trusted);
                    assert_eq!(
                        p.decide(Action { level }),
                        Decision::NeedsApproval,
                        "C/D 恒审批：mode={mode:?} trusted={trusted} level={level:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn b_level_depends_on_mode_trust_and_snapshots() {
        // 未信任 + 自动档 → 仍需审批（TOFU 默认交互）
        assert_eq!(
            policy(Mode::Auto, false).decide(Action { level: Level::B }),
            Decision::NeedsApproval
        );
        // 未信任 + 交互档 → 审批
        assert_eq!(
            policy(Mode::Interactive, false).decide(Action { level: Level::B }),
            Decision::NeedsApproval
        );
        // 信任 + 自动档 → 自动
        assert_eq!(
            policy(Mode::Auto, true).decide(Action { level: Level::B }),
            Decision::Auto
        );
        // 信任 + 交互档 → 审批（档位只影响 B 级）
        assert_eq!(
            policy(Mode::Interactive, true).decide(Action { level: Level::B }),
            Decision::NeedsApproval
        );
    }

    #[test]
    fn readonly_switch_denies_all_writes() {
        let p = Policy {
            readonly: true,
            ..policy(Mode::Auto, true)
        };
        assert_eq!(
            p.decide(Action { level: Level::B }),
            Decision::Denied("readonly")
        );
        assert_eq!(
            p.decide(Action { level: Level::C }),
            Decision::Denied("readonly")
        );
        assert_eq!(
            p.decide(Action { level: Level::D }),
            Decision::Denied("readonly")
        );
        assert_eq!(
            p.decide(Action {
                level: Level::Composite
            }),
            Decision::Denied("readonly")
        );
    }

    #[test]
    fn snapshot_unavailable_downgrades_auto_mode() {
        let p = Policy {
            snapshot_enabled: false,
            ..policy(Mode::Auto, true)
        };
        // 「自动 = 必可回滚」不变式：无快照 → B 不得自动
        assert_eq!(
            p.decide(Action { level: Level::B }),
            Decision::NeedsApproval
        );
        assert_eq!(p.effective_mode(), Mode::Interactive);
    }

    #[test]
    fn degraded_windows_requires_approval_for_b() {
        let p = Policy {
            degraded: true,
            ..policy(Mode::Auto, true)
        };
        assert_eq!(
            p.decide(Action { level: Level::B }),
            Decision::NeedsApproval
        );
        // A 级不受影响
        assert_eq!(p.decide(Action { level: Level::A }), Decision::Auto);
    }
}

/// 团队策略（M3 §19 / §12.2）：由团队下发、**只收窄**的全局约束。
/// 与会话 Policy 相交后生效：交互档上限 / 工具黑名单 / 成本上限。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct TeamPolicy {
    /// 档位上限：true = 禁用自动档（强制交互）
    #[serde(default)]
    pub force_interactive: bool,
    /// 全局禁用工具（名单，跨会话）
    #[serde(default)]
    pub denied_tools: Vec<String>,
    /// 单任务成本上限（美元；低于配置者生效）
    #[serde(default)]
    pub max_cost_usd: Option<f64>,
}

impl TeamPolicy {
    /// 应用到会话 Policy（只收窄：force_interactive → 交互档；工具黑名单由
    /// 上层在工具目录过滤；成本上限由熔断器取 min）。
    pub fn apply_to(&self, policy: &mut Policy) {
        if self.force_interactive {
            policy.mode = Mode::Interactive;
        }
    }

    /// 熔断上限取更严格者。
    pub fn clamp_cost(&self, configured: f64) -> f64 {
        match self.max_cost_usd {
            Some(team_cap) => configured.min(team_cap),
            None => configured,
        }
    }
}

#[cfg(test)]
mod team_policy_tests {
    use super::*;

    fn policy(mode: Mode, trusted: bool) -> Policy {
        Policy {
            mode,
            trusted,
            ..Policy::default()
        }
    }

    #[test]
    fn team_policy_forces_interactive_and_clamps_cost() {
        let tp = TeamPolicy {
            force_interactive: true,
            denied_tools: vec!["git_push".into()],
            max_cost_usd: Some(2.0),
        };
        let mut p = policy(Mode::Auto, true);
        tp.apply_to(&mut p);
        // 自动档被团队策略禁用 → B 级需审批
        assert_eq!(
            p.decide(Action { level: Level::B }),
            Decision::NeedsApproval
        );
        assert_eq!(tp.clamp_cost(5.0), 2.0, "取更严上限");
        assert_eq!(tp.clamp_cost(1.0), 1.0, "配置更严则用配置");
    }

    #[test]
    fn team_policy_serialization() {
        let tp: TeamPolicy =
            serde_json::from_str(r#"{"force_interactive": true, "denied_tools": ["git_push"]}"#)
                .unwrap();
        assert!(tp.force_interactive);
        assert_eq!(tp.denied_tools, vec!["git_push".to_string()]);
        assert_eq!(tp.max_cost_usd, None);
    }
}
