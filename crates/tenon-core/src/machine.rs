//! 自主决策状态机（设计方案 §9.1）。
//!
//! 转移表覆盖：正常路径（SENSING→DECIDING→[首改缓冲]→EXECUTING→VERIFYING→
//! SUMMARIZING→DONE / ANSWERING→DONE）、修复循环（VERIFYING ⇄ FIXING，轮次
//! 收敛 ≤max_rounds）、侧向出口（任意 →PAUSED / →ERROR，ERROR 重试 ≤2 或
//! 切模型上下文随迁，中止→ROLLED_BACK）。v1.89 无审批状态。

use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Idle,
    Sensing,
    Deciding,
    /// 首改缓冲（2s，Esc 可断；位于进入执行之前，§9.3）
    FirstEditBuffer,
    Executing,
    Verifying,
    Fixing,
    /// 无需改动的纯回答路径（§9.1 图：ANSWERING）
    Answering,
    /// 证据卡汇总
    Summarizing,
    Paused,
    Error,
    Done,
    RolledBack,
}

impl fmt::Display for State {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            State::Idle => "idle",
            State::Sensing => "sensing",
            State::Deciding => "deciding",
            State::FirstEditBuffer => "first_edit_buffer",
            State::Executing => "executing",
            State::Verifying => "verifying",
            State::Fixing => "fixing",
            State::Answering => "answering",
            State::Summarizing => "summarizing",
            State::Paused => "paused",
            State::Error => "error",
            State::Done => "done",
            State::RolledBack => "rolled_back",
        };
        f.write_str(s)
    }
}

/// 状态机输入事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// 用户下达任务（IDLE 起）。
    Start,
    /// 感知阶段完成（A 级只读探索结束）。
    SensingDone,
    /// DECIDING：需要改动（输出意图卡，进入首改缓冲）。
    NeedChange,
    /// DECIDING：无需改动（进入回答）。
    NoChangeNeeded,
    /// 首改缓冲计时结束（默认 2s，§9.3）。
    BufferElapsed,
    /// 首改缓冲期间 Esc 打断。
    BufferInterrupt,
    /// 执行完毕，进入验证。
    ExecutionDone,
    /// 验证失败且满足收敛条件（§9.4）。
    VerificationFailed,
    /// 验证通过。
    VerificationPassed,
    /// 一轮修复完成，回验证。
    FixRoundDone,
    /// 证据卡汇总完成。
    SummaryDone,
    /// 全局暂停：Esc / 托盘 / 熔断器触发（任意状态）。
    Pause,
    /// 从暂停恢复 → 回断点状态。
    Resume,
    /// 中止 / 时间轴回滚 → ROLLED_BACK（最近 checkpoint）。
    Abort,
    /// 模型 / 供应商 / 网络失败（任意工作状态）。
    ModelError,
    /// ERROR 后重试同一模型（≤2）。
    Retry,
    /// ERROR 后切换模型（上下文随迁，§11；计入重试预算）。
    FallbackModel,
    /// 任务完成后开始新任务（DONE → IDLE）。
    Reset,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Limits {
    /// 模型失败重试上限（§9.1：重试 ≤2；含切模型）。
    pub model_retries: u32,
    /// 修复循环轮次上限（§9.4：≤3；无测试仓库降级为 1）。
    pub fix_rounds: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            model_retries: 2,
            fix_rounds: 3,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransitionError {
    /// 当前状态不允许该事件。
    Invalid { state: State, event: Event },
    /// 重试预算耗尽（应转 PAUSED / ROLLED_BACK）。
    BudgetExhausted(&'static str),
}

#[derive(Debug, Clone)]
pub struct StateMachine {
    state: State,
    limits: Limits,
    /// PAUSED / ERROR 的断点状态。
    resume_state: Option<State>,
    model_retries_used: u32,
    fix_rounds_used: u32,
}

impl StateMachine {
    pub fn new() -> Self {
        Self::with_limits(Limits::default())
    }

    pub fn with_limits(limits: Limits) -> Self {
        Self {
            state: State::Idle,
            limits,
            resume_state: None,
            model_retries_used: 0,
            fix_rounds_used: 0,
        }
    }

    pub fn state(&self) -> State {
        self.state
    }

    pub fn fix_rounds_used(&self) -> u32 {
        self.fix_rounds_used
    }

    pub fn resume_state(&self) -> Option<State> {
        self.resume_state
    }

    /// 运行态直接对齐状态（转移合法性由事件日志承载；保留全部计数）。
    pub fn force_state(&mut self, state: State) {
        self.state = state;
    }

    /// 是否处于可暂停的工作状态（侧向出口）。
    fn is_working(state: State) -> bool {
        matches!(
            state,
            State::Sensing
                | State::Deciding
                | State::FirstEditBuffer
                | State::Executing
                | State::Verifying
                | State::Fixing
                | State::Answering
                | State::Summarizing
        )
    }

    pub fn transition(&mut self, event: Event) -> Result<State, TransitionError> {
        let from = self.state;
        let next = match (&from, &event) {
            // ---------- 主路径 ----------
            (State::Idle, Event::Start) => State::Sensing,
            (State::Sensing, Event::SensingDone) => State::Deciding,
            (State::Deciding, Event::NeedChange) => State::FirstEditBuffer,
            (State::Deciding, Event::NoChangeNeeded) => State::Answering,
            (State::FirstEditBuffer, Event::BufferElapsed) => State::Executing,
            (State::FirstEditBuffer, Event::BufferInterrupt) => {
                self.resume_state = Some(State::FirstEditBuffer);
                State::Paused
            }
            (State::Answering, Event::SummaryDone) => State::Done,
            (State::Executing, Event::ExecutionDone) => State::Verifying,

            // ---------- 验证与修复循环 ----------
            (State::Verifying, Event::VerificationPassed) => State::Summarizing,
            (State::Verifying, Event::VerificationFailed) => {
                if self.fix_rounds_used >= self.limits.fix_rounds {
                    // 不满足收敛即停（§9.4）
                    self.resume_state = Some(State::Verifying);
                    State::Paused
                } else {
                    self.fix_rounds_used += 1;
                    State::Fixing
                }
            }
            (State::Fixing, Event::FixRoundDone) => State::Verifying,

            // ---------- 汇总 ----------
            (State::Summarizing, Event::SummaryDone) => State::Done,

            // ---------- 侧向出口：暂停 ----------
            (s, Event::Pause) if Self::is_working(*s) => {
                self.resume_state = Some(*s);
                State::Paused
            }
            (State::Paused, Event::Resume) => self.resume_state.take().unwrap_or(State::Deciding),
            (State::Paused, Event::Abort) => State::RolledBack,

            // ---------- 侧向出口：模型失败 ----------
            (s, Event::ModelError) if Self::is_working(*s) => {
                self.resume_state = Some(*s);
                State::Error
            }
            (State::Error, Event::Retry) | (State::Error, Event::FallbackModel) => {
                if self.model_retries_used >= self.limits.model_retries {
                    return Err(TransitionError::BudgetExhausted("model_retries"));
                }
                self.model_retries_used += 1;
                self.resume_state.take().unwrap_or(State::Deciding)
            }
            (State::Error, Event::Abort) => State::RolledBack,

            // ---------- 终态 ----------
            (State::Done, Event::Reset) | (State::RolledBack, Event::Reset) => {
                // 任何终态复位都清零计数：否则「回滚→新任务」继承上一任务的
                // fix_rounds / model_retries 预算，首个验证失败即 Paused
                self.model_retries_used = 0;
                self.fix_rounds_used = 0;
                self.resume_state = None;
                State::Idle
            }
            (State::Done, Event::Abort) => State::RolledBack,

            _ => return Err(TransitionError::Invalid { state: from, event }),
        };
        self.state = next;
        Ok(next)
    }
}

impl Default for StateMachine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drive(m: &mut StateMachine, events: &[Event]) -> State {
        let mut last = m.state();
        for e in events {
            last = m.transition(e.clone()).expect("valid transition");
        }
        last
    }

    #[test]
    fn happy_path_answer_only() {
        let mut m = StateMachine::new();
        let s = drive(
            &mut m,
            &[
                Event::Start,
                Event::SensingDone,
                Event::NoChangeNeeded,
                Event::SummaryDone,
            ],
        );
        assert_eq!(s, State::Done);
    }

    #[test]
    fn happy_path_with_change() {
        let mut m = StateMachine::new();
        let s = drive(
            &mut m,
            &[
                Event::Start,
                Event::SensingDone,   // sensing 完成 → deciding
                Event::NeedChange,    // → first_edit_buffer
                Event::BufferElapsed, // → executing
                Event::ExecutionDone, // → verifying
                Event::VerificationPassed,
                Event::SummaryDone,
            ],
        );
        assert_eq!(s, State::Done);
    }

    #[test]
    fn first_edit_buffer_esc_pauses_and_resumes() {
        let mut m = StateMachine::new();
        drive(
            &mut m,
            &[Event::Start, Event::SensingDone, Event::NeedChange],
        );
        assert_eq!(m.state(), State::FirstEditBuffer);
        assert_eq!(m.transition(Event::BufferInterrupt).unwrap(), State::Paused);
        assert_eq!(m.transition(Event::Resume).unwrap(), State::FirstEditBuffer);
    }

    #[test]
    fn executing_has_no_approval_detour() {
        let mut m = StateMachine::new();
        drive(
            &mut m,
            &[
                Event::Start,
                Event::SensingDone,
                Event::NeedChange,
                Event::BufferElapsed,
            ],
        );
        // v1.89：C/D 只记录风险级别，不产生审批状态或等待事件。
        assert_eq!(m.state(), State::Executing);
        assert_eq!(
            m.transition(Event::ExecutionDone).unwrap(),
            State::Verifying
        );
    }

    #[test]
    fn fix_loop_respects_round_limit() {
        let mut m = StateMachine::with_limits(Limits {
            fix_rounds: 3,
            ..Limits::default()
        });
        drive(
            &mut m,
            &[
                Event::Start,
                Event::SensingDone,
                Event::NeedChange,
                Event::BufferElapsed,
                Event::ExecutionDone,
            ],
        );
        assert_eq!(m.state(), State::Verifying);
        for round in 1..=3 {
            assert_eq!(
                m.transition(Event::VerificationFailed).unwrap(),
                State::Fixing,
                "round {round}"
            );
            assert_eq!(m.transition(Event::FixRoundDone).unwrap(), State::Verifying);
        }
        assert_eq!(m.fix_rounds_used(), 3);
        // 第 4 次失败 → 不满足收敛即停
        assert_eq!(
            m.transition(Event::VerificationFailed).unwrap(),
            State::Paused
        );
    }

    #[test]
    fn low_verification_mode_fixes_at_most_once() {
        // §9.4 无测试仓库降级通道：修复循环降为 ≤1 轮
        let mut m = StateMachine::with_limits(Limits {
            fix_rounds: 1,
            ..Limits::default()
        });
        drive(
            &mut m,
            &[
                Event::Start,
                Event::SensingDone,
                Event::NeedChange,
                Event::BufferElapsed,
                Event::ExecutionDone,
            ],
        );
        assert_eq!(
            m.transition(Event::VerificationFailed).unwrap(),
            State::Fixing
        );
        assert_eq!(m.transition(Event::FixRoundDone).unwrap(), State::Verifying);
        assert_eq!(
            m.transition(Event::VerificationFailed).unwrap(),
            State::Paused
        );
    }

    #[test]
    fn pause_from_any_working_state_and_resume_to_breakpoint() {
        let mut m = StateMachine::new();
        drive(
            &mut m,
            &[
                Event::Start,
                Event::SensingDone,
                Event::NeedChange,
                Event::BufferElapsed,
            ],
        );
        assert_eq!(
            m.transition(Event::Pause).unwrap(),
            State::Paused,
            "熔断器 / Esc / 托盘"
        );
        assert_eq!(m.transition(Event::Resume).unwrap(), State::Executing);
    }

    #[test]
    fn abort_rolls_back_to_last_checkpoint_semantics() {
        let mut m = StateMachine::new();
        drive(
            &mut m,
            &[
                Event::Start,
                Event::SensingDone,
                Event::NeedChange,
                Event::BufferElapsed,
                Event::Pause,
            ],
        );
        assert_eq!(m.transition(Event::Abort).unwrap(), State::RolledBack);
        assert_eq!(m.transition(Event::Reset).unwrap(), State::Idle);
    }

    #[test]
    fn rolled_back_reset_clears_budget_counters() {
        // 「回滚 → 新任务」若继承上一任务的 fix_rounds/model_retries，
        // 新任务首个验证失败即 Paused、首个模型错误即预算耗尽
        let mut m = StateMachine::with_limits(Limits {
            fix_rounds: 1,
            ..Limits::default()
        });
        drive(
            &mut m,
            &[
                Event::Start,
                Event::SensingDone,
                Event::NeedChange,
                Event::BufferElapsed,
                Event::ExecutionDone,
                Event::VerificationFailed,
                Event::FixRoundDone,
                Event::VerificationFailed, // 预算用尽 → Paused
                Event::Abort,              // → RolledBack
                Event::Reset,
            ],
        );
        assert_eq!(m.state(), State::Idle);
        assert_eq!(m.fix_rounds_used(), 0, "Reset 后修复预算应清零");
        // 新任务首个验证失败应可正常进入 Fixing，而非立刻 Paused
        drive(
            &mut m,
            &[
                Event::Start,
                Event::SensingDone,
                Event::NeedChange,
                Event::BufferElapsed,
                Event::ExecutionDone,
            ],
        );
        assert_eq!(
            m.transition(Event::VerificationFailed).unwrap(),
            State::Fixing
        );
    }

    #[test]
    fn model_error_retry_then_fallback_then_exhaustion() {
        let mut m = StateMachine::new();
        drive(&mut m, &[Event::Start, Event::SensingDone]);
        // DECIDING 中模型失败
        assert_eq!(m.transition(Event::ModelError).unwrap(), State::Error);
        // 重试 ≤2：上下文随迁回断点
        assert_eq!(m.transition(Event::Retry).unwrap(), State::Deciding);
        assert_eq!(m.transition(Event::ModelError).unwrap(), State::Error);
        // 切模型（§11 上下文随迁）计入同一预算
        assert_eq!(m.transition(Event::FallbackModel).unwrap(), State::Deciding);
        assert_eq!(m.transition(Event::ModelError).unwrap(), State::Error);
        // 第 3 次重试超出 ≤2 → 报预算耗尽（上层转中止/回滚）
        assert_eq!(
            m.transition(Event::Retry),
            Err(TransitionError::BudgetExhausted("model_retries"))
        );
        assert_eq!(m.transition(Event::Abort).unwrap(), State::RolledBack);
    }

    #[test]
    fn invalid_transitions_rejected() {
        let mut m = StateMachine::new();
        assert!(
            m.transition(Event::SensingDone).is_err(),
            "IDLE 不能跳过 Start"
        );
        assert!(m.transition(Event::Reset).is_err(), "IDLE 无需 reset");
        m.transition(Event::Start).unwrap();
        assert!(m.transition(Event::VerificationPassed).is_err());
    }

    #[test]
    fn done_can_rollback_or_reset() {
        let mut m = StateMachine::new();
        drive(
            &mut m,
            &[
                Event::Start,
                Event::SensingDone,
                Event::NoChangeNeeded,
                Event::SummaryDone,
            ],
        );
        assert_eq!(
            m.transition(Event::Abort).unwrap(),
            State::RolledBack,
            "DONE 回滚走时间轴"
        );
        let mut m2 = StateMachine::new();
        drive(
            &mut m2,
            &[
                Event::Start,
                Event::SensingDone,
                Event::NoChangeNeeded,
                Event::SummaryDone,
            ],
        );
        assert_eq!(m2.transition(Event::Reset).unwrap(), State::Idle, "新任务");
        // 重置后计数清零
        assert_eq!(m2.fix_rounds_used(), 0);
    }
}
