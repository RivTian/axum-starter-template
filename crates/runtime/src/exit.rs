//! 退出分类：五类退出与记录形状。
//!
//! 两个字段分工明确，不要合并：
//! - [`ExitCause`] 回答"这次退出是谁的意图"：任务自己结束（`Completed`/`Failed`/`Panicked`），
//!   还是 supervisor 要求它停（`Cancelled`/`Restarted`）；
//! - [`TaskOutcome`] 回答"future 的结局是什么"：返回 Ok/Err、panic、还是被 abort（没来得及返回值）。
//!
//! 于是"关停期优雅返回 Ok 的任务"记录为 `Cancelled + Returned`——既没有假装它是被强杀的，
//! 也没有把"关停期间的退出"混进正常的 `Completed` 里。

use std::time::Duration;

use crate::id::{RuntimeId, TaskKey};

/// supervisor 观察到的任务退出意图。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExitCause {
    /// 自行正常返回，且无人要求它停。
    Completed,
    /// 自行返回错误，且无人要求它停。
    Failed,
    /// panic（`panic = "unwind"` 下由 `JoinError::is_panic` 观察）。
    Panicked,
    /// supervisor 因关停要求它停（含 abort 收尾）。
    Cancelled,
    /// supervisor 因重启替换要求它停。
    Restarted,
}

impl ExitCause {
    /// 稳定的短名：日志字段与测试断言都用它。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Panicked => "panicked",
            Self::Cancelled => "cancelled",
            Self::Restarted => "restarted",
        }
    }
}

/// future 的实际结局。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskOutcome {
    /// 返回 `Ok(())`。
    Returned,
    /// 返回 `Err`：附带一句话（不含敏感取值）。
    Failed { message: String },
    /// panic：附带 panic 载荷的最简描述。
    Panicked { message: String },
    /// 来不及返回值就被 abort（或者任务被取消后没再跑）。
    Aborted,
}

impl TaskOutcome {
    /// 稳定的短名：日志字段与测试断言都用它。
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Returned => "returned",
            Self::Failed { .. } => "failed",
            Self::Panicked { .. } => "panicked",
            Self::Aborted => "aborted",
        }
    }
}

/// 一条退出记录：任务身份 + 原因 + 结局 + 存活时长。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExitRecord {
    pub key: TaskKey,
    /// 任务被 spawn 到哪个 runtime（I2 要求每条退出记录都带上它）。
    pub runtime: RuntimeId,
    /// 同一个 key 的第几个化身，从 1 开始。
    pub incarnation: u64,
    pub cause: ExitCause,
    pub outcome: TaskOutcome,
    /// 这个化身活了多久。
    pub uptime: Duration,
    /// 这个 key 到这次退出为止已经重启过几次。
    pub restarts: u32,
}

impl ExitRecord {
    /// 供日志用的一行摘要（不含可能敏感的载荷）。
    pub fn summary(&self) -> String {
        format!(
            "{}@{} incarnation={} cause={} outcome={}",
            self.key,
            self.runtime,
            self.incarnation,
            self.cause.as_str(),
            self.outcome.as_str()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cause_and_outcome_names_are_stable() {
        assert_eq!(ExitCause::Completed.as_str(), "completed");
        assert_eq!(ExitCause::Failed.as_str(), "failed");
        assert_eq!(ExitCause::Panicked.as_str(), "panicked");
        assert_eq!(ExitCause::Cancelled.as_str(), "cancelled");
        assert_eq!(ExitCause::Restarted.as_str(), "restarted");
        assert_eq!(TaskOutcome::Returned.as_str(), "returned");
        assert_eq!(TaskOutcome::Aborted.as_str(), "aborted");
    }

    #[test]
    fn summary_carries_task_name_and_runtime() {
        let record = ExitRecord {
            key: TaskKey::new("flush"),
            runtime: RuntimeId::MAIN,
            incarnation: 2,
            cause: ExitCause::Cancelled,
            outcome: TaskOutcome::Returned,
            uptime: Duration::from_millis(5),
            restarts: 1,
        };
        let summary = record.summary();
        assert!(summary.contains("flush"), "{summary}");
        assert!(summary.contains("main"), "{summary}");
        assert!(summary.contains("incarnation=2"), "{summary}");
        assert!(summary.contains("cause=cancelled"), "{summary}");
        assert!(summary.contains("outcome=returned"), "{summary}");
    }
}
