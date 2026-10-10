//! v2.0 安全档位（design-v2.md §4.1）：ExecMode × Approval 二维正交。
//!
//! 推翻 v1.89「唯一零审批档」：执行边界与确认策略各自独立取档——
//! - ExecMode 决定执行边界（只读 / 工作区写 / 完全放行——后者含命令
//!   沙箱降级，见 executor 的 full_access 接线）；
//! - Approval 决定哪些级别需要用户确认（never = v1 零审批，降为可选档）；
//! - 默认档 `workspace-write × on-irreversible`：日常零打扰（B/C 直执），
//!   唯 D 级 / C+D 复合（commit / push / PR / 装插件等不可逆动作）Hold
//!   等待确认，本会话可记忆放行。
//!
//! 判定是纯函数：只读开关（会话级 hard 边界）与团队黑名单仍在上游
//! 各自独立拦截，不经本模块；`judge` 只回答「放行还是等待确认」。

use serde::{Deserialize, Serialize};

use crate::policy::Level;

/// 执行边界档（横向维度）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ExecMode {
    /// 只读：B/C/D 全拒（接线为会话 readonly 开关，A 级照常）。
    ReadOnly,
    /// 工作区写（默认）：B 级沙箱写、C 级直执审计——v1 现状语义。
    #[default]
    WorkspaceWrite,
    /// 完全放行：同 workspace-write，另将 run_tests / run_build /
    /// install_deps 的沙箱降为无（用户显式信任，对齐 codex
    /// danger-full-access；Approval 维度独立，不因此跳过确认）。
    FullAccess,
}

impl ExecMode {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "read_only" | "readonly" => ExecMode::ReadOnly,
            "workspace_write" => ExecMode::WorkspaceWrite,
            "full_access" => ExecMode::FullAccess,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ExecMode::ReadOnly => "read_only",
            ExecMode::WorkspaceWrite => "workspace_write",
            ExecMode::FullAccess => "full_access",
        }
    }
}

/// 确认策略档（纵向维度）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalGear {
    /// 从不确认（v1.89 零审批，降为可选档）。
    Never,
    /// 仅不可逆动作确认（默认）：D 级 / C+D 复合 Hold。
    #[default]
    OnIrreversible,
    /// 全部非只读动作确认（新手 / 企业档）。
    Always,
}

impl ApprovalGear {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "never" => ApprovalGear::Never,
            "on_irreversible" => ApprovalGear::OnIrreversible,
            "always" => ApprovalGear::Always,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ApprovalGear::Never => "never",
            ApprovalGear::OnIrreversible => "on_irreversible",
            ApprovalGear::Always => "always",
        }
    }
}

/// 档位判定结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateVerdict {
    /// 直接执行（照旧审计路径）。
    Allow,
    /// 等待用户确认（confirm_request 事件 + 会话 AwaitingConfirm）。
    Hold,
}

/// 确认决议（`POST /session/:id/confirm`，design-v2.md §4.1）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfirmDecision {
    /// 允许本次。
    AllowOnce,
    /// 允许并本会话记忆（同名工具后续不再 Hold）。
    AllowSession,
    /// 拒绝（理由回传模型，回合继续）。
    Deny,
}

impl ConfirmDecision {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "allow_once" => ConfirmDecision::AllowOnce,
            "allow_session" => ConfirmDecision::AllowSession,
            "deny" => ConfirmDecision::Deny,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ConfirmDecision::AllowOnce => "allow_once",
            ConfirmDecision::AllowSession => "allow_session",
            ConfirmDecision::Deny => "deny",
        }
    }
}

/// 会话档位（AgentConfig 携带；Approval 可经控制命令运行中切换）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Gate {
    pub exec_mode: ExecMode,
    pub approval: ApprovalGear,
}

impl Default for Gate {
    fn default() -> Self {
        Gate {
            exec_mode: ExecMode::WorkspaceWrite,
            approval: ApprovalGear::OnIrreversible,
        }
    }
}

impl Gate {
    /// 判定某级别动作是否需要确认。`already_allowed` = 本会话已记忆放行的
    /// 工具（allow_session 决议）——同名工具后续调用不再 Hold。
    pub fn judge(&self, level: Level, already_allowed: bool) -> GateVerdict {
        if already_allowed {
            return GateVerdict::Allow;
        }
        match self.approval {
            ApprovalGear::Never => GateVerdict::Allow,
            ApprovalGear::OnIrreversible => match level {
                Level::D | Level::Composite => GateVerdict::Hold,
                _ => GateVerdict::Allow,
            },
            ApprovalGear::Always => match level {
                Level::A => GateVerdict::Allow,
                _ => GateVerdict::Hold,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_gear_is_workspace_write_on_irreversible() {
        let g = Gate::default();
        assert_eq!(g.exec_mode, ExecMode::WorkspaceWrite);
        assert_eq!(g.approval, ApprovalGear::OnIrreversible);
    }

    #[test]
    fn on_irreversible_holds_only_d_and_composite() {
        let g = Gate::default();
        assert_eq!(g.judge(Level::A, false), GateVerdict::Allow);
        assert_eq!(g.judge(Level::B, false), GateVerdict::Allow);
        assert_eq!(g.judge(Level::C, false), GateVerdict::Allow);
        assert_eq!(g.judge(Level::D, false), GateVerdict::Hold);
        assert_eq!(g.judge(Level::Composite, false), GateVerdict::Hold);
    }

    #[test]
    fn never_is_v1_zero_approval() {
        let g = Gate {
            approval: ApprovalGear::Never,
            ..Gate::default()
        };
        for level in [Level::A, Level::B, Level::C, Level::D, Level::Composite] {
            assert_eq!(g.judge(level, false), GateVerdict::Allow);
        }
    }

    #[test]
    fn always_holds_all_non_readonly() {
        let g = Gate {
            approval: ApprovalGear::Always,
            ..Gate::default()
        };
        assert_eq!(g.judge(Level::A, false), GateVerdict::Allow);
        for level in [Level::B, Level::C, Level::D, Level::Composite] {
            assert_eq!(g.judge(level, false), GateVerdict::Hold);
        }
    }

    #[test]
    fn session_allowed_memory_short_circuits_hold() {
        let g = Gate::default();
        assert_eq!(g.judge(Level::D, true), GateVerdict::Allow);
        let strict = Gate {
            approval: ApprovalGear::Always,
            ..Gate::default()
        };
        assert_eq!(strict.judge(Level::B, true), GateVerdict::Allow);
    }

    #[test]
    fn parses_and_round_trips() {
        for s in ["read_only", "workspace_write", "full_access"] {
            assert_eq!(ExecMode::parse(s).unwrap().as_str(), s);
        }
        assert_eq!(ExecMode::parse("readonly").unwrap(), ExecMode::ReadOnly);
        for s in ["never", "on_irreversible", "always"] {
            assert_eq!(ApprovalGear::parse(s).unwrap().as_str(), s);
        }
        assert!(ExecMode::parse("yolo").is_none());
        assert!(ApprovalGear::parse("sometimes").is_none());
    }
}
