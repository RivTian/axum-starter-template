//! 停止策略：一次、两次、三次分别意味着什么。
//!
//! 全部逻辑在一个**纯状态机**里，时钟是参数不是全局。这条选择买到的是：「连按三次
//! Ctrl-C」能在一个毫秒级的同步用例里验完，而不需要真的架起一个 runtime 等 30 秒。
//! 反过来说，如果这里读 `Instant::now()`，那么"第二次请求不会把截止时间往后推"这条
//! 性质就只能靠盯着日志看——而它恰好是关停里最容易写反的一条。
//!
//! # 三次的语义
//!
//! | 第几次 | 动作 | 理由 |
//! | --- | --- | --- |
//! | 1 | 按配置算出一份 [`Plan`]，开始排空 | 正常路径 |
//! | 2 | **收紧**总边界到 `now + escalate`，保留首因与原计划 | 人已经等得不耐烦了，但"加速"不等于"放弃"——在途请求还有机会答完 |
//! | 3 | 立即强退 | 逃生门。没有它的话，一个卡死在同步段里的任务能让 Ctrl-C 彻底失效，使用者只剩 `kill -9`，而那会跳过全部收尾 |
//!
//! 第二次**不重新计时**：`Plan::escalate` 的输入是原计划自己，输出与原计划取 `min`。
//! 于是重复按键只会让关停更快。这是「保留第一个 cause 与第一个 plan」的落地
//! 形态——不是"把新计划丢掉"，而是"新计划只能是旧计划的收紧"。

use std::time::Instant;

use service_core::shutdown::{Budgets, Plan};
use service_core::task::TaskExit;

use crate::signals::ProcessSignal;

/// 这一次关停是被什么触发的。
///
/// 只有 [`Self::Signal`] 算"预期内的停止"——[`RunReport::succeeded`] 六项合取里的第一项
/// 就是它。其余三个变体都表示"进程是被迫停的"，哪怕后续收尾全都干净。
///
/// [`RunReport::succeeded`]: crate::lifecycle::report::RunReport::succeeded
#[derive(Debug)]
pub enum StopCause {
    /// 收到了 SIGTERM / SIGINT。
    ///
    /// 不会是 `ProcessSignal::Reload`：构造点只有编排循环里那一处，且在
    /// [`ProcessSignal::is_stop`] 的守卫之后。
    Signal(ProcessSignal),
    /// 某个顶层任务先退出了（first-failure）。
    ///
    /// 带着完整的 [`TaskExit`] 而不是只带一个名字：退出**原因**决定了退出码，也决定了
    /// 运维该去看哪一类问题。
    TaskExited(TaskExit),
    /// 一个任务都没有了。
    ///
    /// 正常情况下不会发生——HTTP 面在 `cancel` 之前不会自己返回。会走到这里，说明
    /// 装配时一个面都没登记，或者所有面都提前结束了。留痕之后正常退出，但**不算成功**。
    SupervisorDrained,
    /// 信号流在收到任何停止请求之前就结束了。
    ///
    /// 生产里意味着信号驱动没了（runtime 正在消失）。用例里意味着注入的合成流放完了。
    /// 注意只有"排空开始**之前**"结束才算一个 cause：排空期间流结束只是"不会再有第二次
    /// 请求"，不是新的停止理由。
    SignalsEnded,
}

impl StopCause {
    /// 报告与日志里使用的稳定短名。
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Signal(_) => "signal",
            Self::TaskExited(_) => "task_exit",
            Self::SupervisorDrained => "supervisor_drained",
            Self::SignalsEnded => "signal_stream_ended",
        }
    }

    /// 补充细节：哪个信号、哪个面。渲染用。
    #[must_use]
    pub fn detail(&self) -> &'static str {
        match self {
            Self::Signal(signal) => signal.as_str(),
            Self::TaskExited(exit) => exit.name_str(),
            Self::SupervisorDrained | Self::SignalsEnded => "-",
        }
    }

    /// 这次关停是不是"外部按预期要求我们停"。
    ///
    /// 无 `_` 臂：新增一个触发源时，编译器会在这里拦住人，逼他回答"它算不算正常停止"。
    #[must_use]
    pub fn is_expected(&self) -> bool {
        match self {
            Self::Signal(_) => true,
            Self::TaskExited(_) | Self::SupervisorDrained | Self::SignalsEnded => false,
        }
    }
}

/// 收到一次停止请求之后该做什么。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StopAction {
    /// 第一次：按这份计划开始排空。
    Begin(Plan),
    /// 第二次：边界已收紧到这份新计划。
    Escalate(Plan),
    /// 第三次及以后：不再等待，立刻退出。
    ForceExit,
}

/// 停止请求的状态机。
#[derive(Debug)]
pub(crate) struct StopPolicy {
    budgets: Budgets,
    /// 当前生效的计划。`None` 表示还没有人请求过停止。
    plan: Option<Plan>,
    /// 第一个触发原因。后续请求不会覆盖它。
    cause: Option<StopCause>,
    /// 已经收到过几次。饱和加法：第三次之后再按多少下都是 `ForceExit`。
    requests: u8,
}

impl StopPolicy {
    /// 用一份预算配置建一个还没开始关停的策略。
    pub(crate) const fn new(budgets: Budgets) -> Self {
        Self {
            budgets,
            plan: None,
            cause: None,
            requests: 0,
        }
    }

    /// 处理一次停止请求。
    ///
    /// `now` 是注入的时钟读数。同一次编排循环里必须来自同一个 `Instant::now()` 调用点，
    /// 否则"第二次请求晚于第一次"这个前提就只是巧合。
    pub(crate) fn on_request(&mut self, cause: StopCause, now: Instant) -> StopAction {
        self.requests = self.requests.saturating_add(1);

        // 首因只认第一个。第二次 Ctrl-C 不该把"HTTP 面 panic 了"改写成"用户按了 Ctrl-C"——
        // 那会让报告把一次真实的故障描述成一次正常关机。
        if self.cause.is_none() {
            self.cause = Some(cause);
        }

        match self.requests {
            1 => {
                let plan = Plan::new(now, &self.budgets);
                self.plan = Some(plan);
                StopAction::Begin(plan)
            }
            2 => {
                // `escalate` 的输入是原计划自己：新边界是 `min(原边界, now + escalate)`，
                // 于是这一步只可能让关停更快。
                let tightened = self.plan.map_or_else(
                    || Plan::new(now, &self.budgets),
                    |p| p.escalate(now, &self.budgets),
                );
                self.plan = Some(tightened);
                StopAction::Escalate(tightened)
            }
            _ => StopAction::ForceExit,
        }
    }

    /// 当前生效的计划。排空循环每一段都从这里取截止时刻，于是收紧能立刻生效。
    pub(crate) const fn plan(&self) -> Option<Plan> {
        self.plan
    }

    /// 已经请求过停止了吗。
    ///
    /// `#[cfg(test)]`：编排层从不问这个问题——它靠**自己在哪一段代码里**就知道答案
    /// （主循环里必然没停，`drive` 里必然在停）。给非 test 构建留着它，等于留一个"可以
    /// 跑去问状态机"的口子，而那正是把判断散开的第一步。用例需要它，因为用例是从外面看
    /// 这台状态机的。
    #[cfg(test)]
    pub(crate) const fn is_stopping(&self) -> bool {
        self.requests > 0
    }

    /// 取走首因，交给报告。
    pub(crate) fn take_cause(&mut self) -> Option<StopCause> {
        self.cause.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn budgets() -> Budgets {
        Budgets {
            total_grace: Duration::from_secs(30),
            harvest: Duration::from_secs(10),
            reap: Duration::from_secs(5),
            storage_close: Duration::from_secs(5),
            runtime_shutdown: Duration::from_secs(5),
            escalate: Duration::from_secs(2),
        }
    }

    #[test]
    fn the_first_request_begins_and_records_its_cause() {
        let mut policy = StopPolicy::new(budgets());
        let now = Instant::now();

        let action = policy.on_request(StopCause::Signal(ProcessSignal::Terminate), now);

        let StopAction::Begin(plan) = action else {
            panic!("第一次请求必须是 Begin，实际是 {action:?}");
        };
        assert!(policy.is_stopping());
        assert_eq!(policy.plan(), Some(plan));
        assert_eq!(policy.take_cause().map(|c| c.as_str()), Some("signal"));
    }

    #[test]
    fn the_second_request_tightens_and_never_extends() {
        let mut policy = StopPolicy::new(budgets());
        let now = Instant::now();

        let StopAction::Begin(first) =
            policy.on_request(StopCause::Signal(ProcessSignal::Interrupt), now)
        else {
            panic!("第一次应当是 Begin");
        };

        // 1 秒后又按了一次：escalate = 2s ⇒ 新边界 ≈ now + 3s，远早于原来的 now + 30s。
        let second = policy.on_request(
            StopCause::Signal(ProcessSignal::Interrupt),
            now + Duration::from_secs(1),
        );
        let StopAction::Escalate(tightened) = second else {
            panic!("第二次请求必须是 Escalate，实际是 {second:?}");
        };

        assert!(tightened.total < first.total, "第二次必须收紧总边界");
        assert!(tightened.harvest <= first.harvest);
        assert!(tightened.runtime_shutdown <= first.runtime_shutdown);
        assert_eq!(
            policy.plan(),
            Some(tightened),
            "收紧后的计划必须立刻生效，否则排空循环还在用旧的截止时刻"
        );
    }

    #[test]
    fn a_very_late_second_request_still_never_extends() {
        // 关停已经跑了 29 秒，这时才来第二次请求。`now + escalate` 会落在原边界之后，
        // 如果实现是"直接取 now + escalate"，这里就会把边界往后推 1 秒。
        let mut policy = StopPolicy::new(budgets());
        let now = Instant::now();

        let StopAction::Begin(first) =
            policy.on_request(StopCause::Signal(ProcessSignal::Terminate), now)
        else {
            panic!("第一次应当是 Begin");
        };
        let StopAction::Escalate(late) = policy.on_request(
            StopCause::Signal(ProcessSignal::Terminate),
            now + Duration::from_secs(29),
        ) else {
            panic!("第二次应当是 Escalate");
        };

        assert!(late.total <= first.total, "迟到的第二次请求也不得放宽边界");
    }

    #[test]
    fn the_second_request_keeps_the_first_cause() {
        // 一个面 panic 导致关停，运维还没来得及看日志就按了 Ctrl-C。
        // 报告里必须写"任务退出"，而不是"用户要求停止"。
        let mut policy = StopPolicy::new(budgets());
        let now = Instant::now();

        policy.on_request(
            StopCause::TaskExited(TaskExit {
                name: Some(service_core::task::TaskName::Http),
                runtime: None,
                kind: service_core::task::ExitKind::Cancelled,
            }),
            now,
        );
        policy.on_request(
            StopCause::Signal(ProcessSignal::Interrupt),
            now + Duration::from_millis(1),
        );

        let cause = policy.take_cause().expect("首因必须还在");
        assert_eq!(cause.as_str(), "task_exit", "首因被第二次请求改写了");
        assert_eq!(cause.detail(), "http");
        assert!(!cause.is_expected(), "任务先退不算预期内的停止");
    }

    #[test]
    fn the_third_request_forces_an_exit_and_stays_forced() {
        let mut policy = StopPolicy::new(budgets());
        let now = Instant::now();

        policy.on_request(StopCause::Signal(ProcessSignal::Interrupt), now);
        policy.on_request(
            StopCause::Signal(ProcessSignal::Interrupt),
            now + Duration::from_millis(1),
        );

        for nth in 3..8 {
            let action = policy.on_request(
                StopCause::Signal(ProcessSignal::Interrupt),
                now + Duration::from_millis(nth),
            );
            assert_eq!(
                action,
                StopAction::ForceExit,
                "第 {nth} 次请求之后必须保持强退"
            );
        }
    }

    #[test]
    fn only_a_signal_counts_as_an_expected_stop() {
        // 无 `_` 臂的那个 match 的用例侧对照：四个变体各自的答案都点了名。
        assert!(StopCause::Signal(ProcessSignal::Terminate).is_expected());
        assert!(StopCause::Signal(ProcessSignal::Interrupt).is_expected());
        assert!(!StopCause::SupervisorDrained.is_expected());
        assert!(!StopCause::SignalsEnded.is_expected());
    }
}
