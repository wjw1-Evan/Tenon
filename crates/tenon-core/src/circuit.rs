//! 任务熔断器（设计方案 §9.3）：断路器而非 Plan。
//!
//! 超过任一预算即触发熔断 → 暂停待确认：
//! - 文件数（默认 15）/ 行数（默认 1500）
//! - token（默认 500k）或 $ 成本（默认 $5）先到为准；本地模型仅计 token。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TripReason {
    TooManyFiles,
    TooManyLines,
    OutOfTokens,
    OutOfBudget,
}

impl TripReason {
    pub fn label(self) -> &'static str {
        match self {
            TripReason::TooManyFiles => "max_files",
            TripReason::TooManyLines => "max_lines",
            TripReason::OutOfTokens => "max_tokens",
            TripReason::OutOfBudget => "max_cost_usd",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct CircuitLimits {
    pub max_files: u32,
    pub max_lines: u64,
    pub max_tokens: u64,
    pub max_cost_usd: f64,
}

impl Default for CircuitLimits {
    fn default() -> Self {
        Self {
            max_files: 15,
            max_lines: 1500,
            max_tokens: 500_000,
            max_cost_usd: 5.0,
        }
    }
}

impl From<&tenon_config::CircuitConfig> for CircuitLimits {
    fn from(c: &tenon_config::CircuitConfig) -> Self {
        Self {
            max_files: c.max_files,
            max_lines: c.max_lines,
            max_tokens: c.max_tokens,
            max_cost_usd: c.max_cost_usd,
        }
    }
}

/// 任务级熔断器。预算为「先到为准」；本地模型成本记 0、仅 token 生效。
#[derive(Debug, Clone)]
pub struct CircuitBreaker {
    limits: CircuitLimits,
    touched_files: u32,
    changed_lines: u64,
    tokens_used: u64,
    cost_used: f64,
}

/// 一次 B 级补丁的规模记录。
#[derive(Debug, Clone, Copy)]
pub struct PatchFootprint {
    /// 本次补丁涉及的新增文件数（相对任务已触及文件的去重由调用方保证：
    /// 传入 `new_files` 为任务累计去重后的文件总数）。
    pub total_files_touched: u32,
    /// 本次补丁新增+删除行数。
    pub lines_changed: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CircuitStatus {
    Open,
    Tripped(TripReason),
}

impl CircuitBreaker {
    pub fn new(limits: CircuitLimits) -> Self {
        Self {
            limits,
            touched_files: 0,
            changed_lines: 0,
            tokens_used: 0,
            cost_used: 0.0,
        }
    }

    pub fn limits(&self) -> CircuitLimits {
        self.limits
    }

    pub fn touched_files(&self) -> u32 {
        self.touched_files
    }

    pub fn changed_lines(&self) -> u64 {
        self.changed_lines
    }

    pub fn tokens_used(&self) -> u64 {
        self.tokens_used
    }

    pub fn cost_used(&self) -> f64 {
        self.cost_used
    }

    /// 记录一次补丁规模；与 [`Self::record_usage`] 同规（v1.166）：超限同样
    /// 记账——调用方在记账前已把补丁写入工作树，拒绝记账只会让熔断计数
    /// 与实际改动脱节：恢复后行预算从旧值重计，反复暂停/恢复可无限超刷。
    pub fn record_patch(&mut self, fp: PatchFootprint) -> CircuitStatus {
        self.touched_files = fp.total_files_touched;
        self.changed_lines += fp.lines_changed;
        if self.touched_files > self.limits.max_files {
            return CircuitStatus::Tripped(TripReason::TooManyFiles);
        }
        if self.changed_lines > self.limits.max_lines {
            return CircuitStatus::Tripped(TripReason::TooManyLines);
        }
        CircuitStatus::Open
    }

    /// 记录一次模型回合的消耗；token 与成本先到为准。
    /// 用量如实累计后再判定（v1.93）：超限同样记账——status() 轮询与
    /// 返回值两种消费路径看到一致的累计值，首次超限不产生盲区。
    pub fn record_usage(&mut self, tokens: u64, cost_usd: f64) -> CircuitStatus {
        self.tokens_used += tokens;
        self.cost_used += cost_usd;
        if self.tokens_used > self.limits.max_tokens {
            return CircuitStatus::Tripped(TripReason::OutOfTokens);
        }
        if self.cost_used > self.limits.max_cost_usd {
            return CircuitStatus::Tripped(TripReason::OutOfBudget);
        }
        CircuitStatus::Open
    }

    /// 当前状态（执行每步前轮询）。
    pub fn status(&self) -> CircuitStatus {
        if self.touched_files > self.limits.max_files {
            return CircuitStatus::Tripped(TripReason::TooManyFiles);
        }
        if self.changed_lines > self.limits.max_lines {
            return CircuitStatus::Tripped(TripReason::TooManyLines);
        }
        if self.tokens_used > self.limits.max_tokens {
            return CircuitStatus::Tripped(TripReason::OutOfTokens);
        }
        if self.cost_used > self.limits.max_cost_usd {
            return CircuitStatus::Tripped(TripReason::OutOfBudget);
        }
        CircuitStatus::Open
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn breaker() -> CircuitBreaker {
        CircuitBreaker::new(CircuitLimits {
            max_files: 3,
            max_lines: 100,
            max_tokens: 1000,
            max_cost_usd: 1.0,
        })
    }

    #[test]
    fn file_budget_trips() {
        let mut b = breaker();
        assert_eq!(
            b.record_patch(PatchFootprint {
                total_files_touched: 3,
                lines_changed: 10
            }),
            CircuitStatus::Open
        );
        assert_eq!(
            b.record_patch(PatchFootprint {
                total_files_touched: 4,
                lines_changed: 0
            }),
            CircuitStatus::Tripped(TripReason::TooManyFiles)
        );
    }

    #[test]
    fn line_budget_trips_accumulatively() {
        let mut b = breaker();
        assert_eq!(
            b.record_patch(PatchFootprint {
                total_files_touched: 1,
                lines_changed: 60
            }),
            CircuitStatus::Open
        );
        assert_eq!(
            b.record_patch(PatchFootprint {
                total_files_touched: 1,
                lines_changed: 41
            }),
            CircuitStatus::Tripped(TripReason::TooManyLines)
        );
        // 超限同样记账（v1.166，与 record_usage 同规）：调用方落盘在前，
        // status() 与恢复后的预算必须看到一致累计值
        assert_eq!(b.changed_lines(), 101);
        assert_eq!(b.status(), CircuitStatus::Tripped(TripReason::TooManyLines));
    }

    #[test]
    fn token_and_cost_whichever_first() {
        let mut b = breaker();
        assert_eq!(b.record_usage(600, 0.4), CircuitStatus::Open);
        // token 先到
        assert_eq!(
            b.record_usage(500, 0.0),
            CircuitStatus::Tripped(TripReason::OutOfTokens)
        );

        let mut b = breaker();
        assert_eq!(b.record_usage(100, 0.6), CircuitStatus::Open);
        // 成本先到（本地模型 cost=0 不会触发）
        assert_eq!(
            b.record_usage(100, 0.5),
            CircuitStatus::Tripped(TripReason::OutOfBudget)
        );

        let mut b = breaker();
        for _ in 0..10 {
            assert_eq!(
                b.record_usage(100, 0.0),
                CircuitStatus::Open,
                "本地模型仅计 token"
            );
        }
    }

    #[test]
    fn status_poll_reflects_state() {
        let mut b = breaker();
        assert_eq!(b.status(), CircuitStatus::Open);
        b.record_usage(999, 0.0);
        assert_eq!(b.status(), CircuitStatus::Open);
        // v1.93：超限如实落账并返回 Tripped——status() 轮询同样可见，
        // 首次超限不产生盲区（工具步检查点经 status() 消费）。
        assert_eq!(
            b.record_usage(2, 0.0),
            CircuitStatus::Tripped(TripReason::OutOfTokens)
        );
        assert_eq!(b.status(), CircuitStatus::Tripped(TripReason::OutOfTokens));
        assert_eq!(b.tokens_used(), 1001);
    }
}
