//! 关停语义：相位机（StopSignal / StopToken）、绝对预算（ShutdownBudget）与诚实报告（ShutdownReport）。
//!
//! 相位机只有一个通道，两级观察点：
//! - `Draining`：广播"要停了"——能自己收尾的任务在这时收尾（返回 Ok 也算 `Cancelled + Returned`）；
//! - `Cancelling`：协作取消——任务应在 `cancelled()` 上做最后的清理；
//! - `Forced`：第二次停止请求——只把每段等待压到 `forced_phase`，**不刷新**任何绝对截止。
//!
//! 预算的每一段都是绝对上限：`total` 是本层（L1），各段之和必须小于它（有测试钉住）。

use std::time::Duration;

use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::exit::ExitRecord;
use crate::id::TaskKey;

/// 停止相位。单调推进：`Running < Draining < Cancelling < Forced`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum StopPhase {
    Running,
    Draining,
    Cancelling,
    Forced,
}

/// 停止信号：装配层（信号处理）与测试用它推进相位。
#[derive(Debug, Clone)]
pub struct StopSignal {
    phase: watch::Sender<StopPhase>,
}

impl Default for StopSignal {
    fn default() -> Self {
        Self::new()
    }
}

impl StopSignal {
    pub fn new() -> Self {
        let (phase, _) = watch::channel(StopPhase::Running);
        Self { phase }
    }

    /// 第一次停止请求：广播 draining（任务应开始收尾）。
    pub fn request_drain(&self) {
        self.advance(StopPhase::Draining);
    }

    /// 协作取消：任务应在 `cancelled()` 上做清理。
    pub fn request_cancel(&self) {
        self.advance(StopPhase::Cancelling);
    }

    /// 第二次停止请求：只加速，不刷新截止。
    pub fn force(&self) {
        self.advance(StopPhase::Forced);
    }

    /// 当前相位。
    pub fn phase(&self) -> StopPhase {
        *self.phase.borrow()
    }

    pub fn subscribe(&self) -> watch::Receiver<StopPhase> {
        self.phase.subscribe()
    }

    fn advance(&self, next: StopPhase) {
        self.phase.send_if_modified(|current| {
            if next > *current {
                *current = next;
                true
            } else {
                false
            }
        });
    }
}

/// 任务面看到的停止令牌：两级观察点 + 本化身单独的取消信号（重启用）。
#[derive(Debug, Clone)]
pub struct StopToken {
    phase: watch::Receiver<StopPhase>,
    individual: CancellationToken,
}

impl StopToken {
    pub(crate) fn new(phase: watch::Receiver<StopPhase>, individual: CancellationToken) -> Self {
        Self { phase, individual }
    }

    /// 收到 draining 广播（或本化身被单独取消）时返回。
    pub async fn draining(&self) {
        let mut phase = self.phase.clone();
        let individual = self.individual.clone();
        tokio::select! {
            _ = phase.wait_for(|value| *value >= StopPhase::Draining) => {}
            _ = individual.cancelled() => {}
        }
    }

    /// 收到取消（或本化身被单独取消）时返回。
    pub async fn cancelled(&self) {
        let mut phase = self.phase.clone();
        let individual = self.individual.clone();
        tokio::select! {
            _ = phase.wait_for(|value| *value >= StopPhase::Cancelling) => {}
            _ = individual.cancelled() => {}
        }
    }

    /// 本化身是否已被单独取消（重启替换）。
    pub fn is_individual_cancelled(&self) -> bool {
        self.individual.is_cancelled()
    }
}

/// 分层绝对预算。内层之和必须小于 `total`（`inner_sum` 与测试一起钉住这条）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShutdownBudget {
    /// L1：整段关停的绝对上限：每个阶段取 `min(阶段预算, 剩余)`。
    pub total: Duration,
    /// L2a：给任务自己收尾的窗口（进入 Draining 之后、取消之前）。
    pub drain: Duration,
    /// L2b：协作取消之后等待任务结束的窗口（共享截止）。
    pub harvest: Duration,
    /// L2c：abort 之后观察结果的窗口。
    pub reap: Duration,
    /// L2d：资源关闭（逆注册序，共享截止）。
    pub resources: Duration,
    /// 二次信号之后每段剩余的等待上限（只加速）。
    pub forced_phase: Duration,
}

impl ShutdownBudget {
    /// 默认预算：L2 合计 8.5s，L1 12s（余量 3.5s）。
    ///
    /// 不做成配置项：截止时间的来源只能有一个（配置刷新会引入"刷新截止时间"的歧义）。
    pub const DEFAULT: Self = Self {
        total: Duration::from_secs(12),
        drain: Duration::from_secs(2),
        harvest: Duration::from_secs(4),
        reap: Duration::from_millis(500),
        resources: Duration::from_secs(2),
        forced_phase: Duration::from_millis(250),
    };

    /// 各段之和（不含 `total` 与 `forced_phase`）。
    pub fn inner_sum(&self) -> Duration {
        self.drain + self.harvest + self.reap + self.resources
    }
}

impl Default for ShutdownBudget {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// 关停是谁触发的：人类信号，还是 fatal 任务自行退出。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopTrigger {
    Signal,
    TaskFailure { key: TaskKey },
}

/// 关停报告：如实区分"真正收尾"与"收不回来"。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShutdownReport {
    pub trigger: StopTrigger,
    /// 在预算内真正收尾的任务。
    pub stopped: Vec<TaskKey>,
    /// 预算用尽仍未结束（abort 之后也没来得及被观察到结束）的任务。
    pub still_running: Vec<TaskKey>,
    /// 被 supervisor abort 收尾的任务（不保证 graceful）。
    pub aborted: Vec<TaskKey>,
    /// 按逆注册序关闭成功的资源。
    pub resources_closed: Vec<String>,
    /// 关闭失败或超时的资源（`名字: 原因`）。
    pub resource_failures: Vec<String>,
    /// 二次信号生效过。
    pub forced: bool,
    /// 全部退出记录（时间序），便于测试与诊断。
    pub records: Vec<ExitRecord>,
    /// 关停序列耗时。
    pub elapsed: Duration,
}

impl ShutdownReport {
    pub fn new(trigger: StopTrigger) -> Self {
        Self {
            trigger,
            stopped: Vec::new(),
            still_running: Vec::new(),
            aborted: Vec::new(),
            resources_closed: Vec::new(),
            resource_failures: Vec::new(),
            forced: false,
            records: Vec::new(),
            elapsed: Duration::ZERO,
        }
    }

    /// 干净关停：没有收不回来的任务、没有资源失败、没有被强推。
    pub fn clean(&self) -> bool {
        self.still_running.is_empty() && self.resource_failures.is_empty() && !self.forced
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phases_only_advance() {
        let signal = StopSignal::new();
        assert_eq!(signal.phase(), StopPhase::Running);
        signal.request_drain();
        signal.force();
        assert_eq!(
            signal.phase(),
            StopPhase::Forced,
            "强制之后不能被 drain 拉回去"
        );
        signal.request_drain();
        assert_eq!(signal.phase(), StopPhase::Forced);
        signal.request_cancel();
        assert_eq!(signal.phase(), StopPhase::Forced);
    }

    #[test]
    fn inner_budget_leaves_margin_under_total() {
        let budget = ShutdownBudget::DEFAULT;
        assert!(
            budget.inner_sum() < budget.total,
            "内层之和 {:?} 必须小于外层 {:?}",
            budget.inner_sum(),
            budget.total
        );
        assert!(budget.forced_phase < budget.harvest);
    }

    #[test]
    fn token_draining_and_cancelled_observe_phases() {
        let signal = StopSignal::new();
        let token = StopToken::new(signal.subscribe(), CancellationToken::new());
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        runtime.block_on(async {
            let drained = token.clone();
            let cancelled = token.clone();
            let drained_task = tokio::spawn(async move { drained.draining().await });
            let cancelled_task = tokio::spawn(async move { cancelled.cancelled().await });
            signal.request_drain();
            drained_task.await.expect("draining 应立即返回");
            assert!(!cancelled_task.is_finished());
            signal.request_cancel();
            cancelled_task.await.expect("cancelled 应立即返回");
        });
    }

    #[test]
    fn individual_cancel_wakes_both_levels() {
        let signal = StopSignal::new();
        let individual = CancellationToken::new();
        let token = StopToken::new(signal.subscribe(), individual.clone());
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        runtime.block_on(async {
            individual.cancel();
            token.draining().await;
            token.cancelled().await;
            assert!(token.is_individual_cancelled());
        });
    }
}
