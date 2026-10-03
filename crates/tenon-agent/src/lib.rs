//! Agent 内核（设计方案 §9）：状态机驱动的自主决策循环。
//!
//! 流程（§9.1）：IDLE → SENSING → DECIDING →[首改缓冲]→ EXECUTING →
//! VERIFYING → SUMMARIZING → DONE；C/D 转 AWAITING_APPROVAL；侧向出口
//! PAUSED / ERROR / ROLLED_BACK。每次状态变化落 events 表并向订阅者广播。
//!
//! 安全联动：B 级写前自动快照（先快照后写入，§10.3）；写守卫收敛项目内；
//! 熔断器超限即暂停（§9.3）；只读开关禁写；C/D 恒审批。

pub mod evals;
pub mod executor;
pub mod recovery;
pub mod session;
pub mod subagents;

pub use evals::{
    ApprovalPolicy, Assertion, EvalBudget, EvalCaseResult, EvalRunner, EvalSuiteReport, EvalTask,
};
pub use executor::{execute_tool, ToolContext, ToolOutput};
pub use recovery::{recover_stale_sessions, RecoveryReport};
pub use session::{
    AgentConfig, AgentError, AgentSession, ControlCommand, EvidenceCard, ProjectWriteLock,
    TaskOutcome,
};
pub use subagents::{
    composite_commit_summary, plan_parallel, SubAgentError, SubTask, WorktreePool,
};
pub use tenon_core::machine::State;

#[derive(Debug, thiserror::Error)]
pub enum LoopError {
    #[error("模型调用失败: {0}")]
    Provider(String),
    #[error("工具执行失败: {0}")]
    Tool(String),
    #[error("任务已中止")]
    Aborted,
}
