//! Tenon 内核纯逻辑（设计方案第 9 / 10 / 12 / 13 章）：
//! 动作分级与权限（policy）、密钥脱敏（redact）、任务熔断（circuit）、
//! 自主决策状态机（machine）、内置工具目录（tools）、上下文工程（context）、
//! 提示组装（prompt）、代理技能（skills，§13.4 v1.130）、
//! install_deps 命令白名单（install_policy，design-v2.md §4.5 v2.0）。

pub mod circuit;
pub mod context;
pub mod execpolicy;
pub mod gates;
pub mod install_policy;
pub mod machine;
pub mod merge;
pub mod policy;
pub mod prompt;
pub mod redact;
pub mod risk;
pub mod skills;
pub mod tools;

pub use circuit::{CircuitBreaker, CircuitLimits, CircuitStatus, PatchFootprint, TripReason};
pub use context::{
    estimate_tokens, ContextSlice, IndexEntry, ProjectRules, SessionMemory, TokenBudget, WorkingSet,
};
pub use gates::{ApprovalGear, ExecMode, Gate, GateVerdict};
pub use machine::{Event, Limits as StateLimits, State, StateMachine, TransitionError};
pub use merge::{merge_three_way, MergeConflict};
pub use policy::{Action, Decision, Level, Policy};
pub use redact::{contains_secret, redact, redact_tracked, scan, SecretKind};
pub use tools::{count_changed_lines, unified_diff, PatchOp, Tool};
