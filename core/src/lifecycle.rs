//! 进程生命周期阶段的广播。
//!
//! 写端唯一是**类型级事实**：[`LifecyclePublisher`] 不是 `Clone`，全进程只有装配层持有它。
//! 读端 [`LifecycleReader`] 随便克隆。
//!
//! 用 `watch` 而不是广播 channel：新订阅者需要的是"现在处于哪个阶段"，不是"历史上发生过
//! 哪些转换"。`watch` 天然持有当前值，晚到的订阅者不会错过已经发生的转换。

use tokio::sync::watch;

/// 进程所处的阶段。
///
/// ```text
/// Starting ──ack 全到齐──► Running ──stop 请求──► Draining ──► [Forcing] ──► Stopped
///     │                                              │
///     └──任一 ack 失败 / 取消 / 首个任务退出──► Aborting ─┘
/// ```
///
/// 方括号表示 [`Forcing`](Self::Forcing) 是**可跳过的**：宽限段结束时所有面都已经自己停了，
/// 就直接进 [`Stopped`](Self::Stopped)。它不可跳过的话就不是一个阶段，只是一行固定日志。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Phase {
    /// 装配完成、任务已启动，但还没有全部就绪。此时面**不处理业务**。
    Starting,
    /// 全部面已就绪并提交。这是唯一可以对外提供服务的阶段。
    Running,
    /// 正在优雅关停：已公告，正在等面自己收尾。
    Draining,
    /// 宽限段结束时仍有面在跑，正在 abort 回收。一次干净的关停**不经过**这个阶段。
    Forcing,
    /// 启动失败路径。与 `Draining` 的区别：订阅者尚未全部就绪，不该按"优雅关停"处理。
    Aborting,
    /// 终态。
    Stopped,
}

impl Phase {
    /// 是否可以对外提供业务服务。
    ///
    /// 无 `_` 臂：新增阶段时必须显式回答它算不算"可服务"。
    #[must_use]
    pub fn serves_traffic(self) -> bool {
        match self {
            Self::Running => true,
            Self::Starting | Self::Draining | Self::Forcing | Self::Aborting | Self::Stopped => {
                false
            }
        }
    }

    /// 报告与日志里使用的稳定短名。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Draining => "draining",
            Self::Forcing => "forcing",
            Self::Aborting => "aborting",
            Self::Stopped => "stopped",
        }
    }
}

/// 阶段的写端。**不是 `Clone`**——全进程唯一。
#[derive(Debug)]
pub struct LifecyclePublisher {
    tx: watch::Sender<Phase>,
}

impl LifecyclePublisher {
    /// 创建一对读写端，初始阶段为 [`Phase::Starting`]。
    #[must_use]
    pub fn new() -> (Self, LifecycleReader) {
        let (tx, rx) = watch::channel(Phase::Starting);
        (Self { tx }, LifecycleReader { rx })
    }

    /// 发布一个新阶段。
    ///
    /// 用 `send_replace` 而不是 `send`：没有订阅者时 `send` 会返回 `Err`，而"当前阶段是什么"
    /// 与"有没有人在听"是两件无关的事。阶段必须无条件写进去，否则后来的订阅者会读到过时的值。
    pub fn publish(&self, phase: Phase) {
        let previous = self.tx.send_replace(phase);
        if previous != phase {
            // `name:` 与那句话是**两件东西**：名字是稳定标识符，给断言和日志检索用；
            // 那句话是给人读的。不写 `name:` 的话 `tracing` 自动生成的是
            // `event core/src/lifecycle.rs:83` 这种带行号的名字——行号会烂，靠它做断言
            // 等于把用例钉在源码布局上。凡是可能被断言的事件，这个模板一律显式起名。
            tracing::info!(
                name: "lifecycle_phase_changed",
                from = previous.as_str(),
                to = phase.as_str(),
                "lifecycle phase changed"
            );
        }
    }

    /// 当前阶段。
    #[must_use]
    pub fn current(&self) -> Phase {
        *self.tx.borrow()
    }

    /// 派生一个新的读端。
    #[must_use]
    pub fn reader(&self) -> LifecycleReader {
        LifecycleReader {
            rx: self.tx.subscribe(),
        }
    }
}

/// 阶段的读端。克隆代价低廉，可以随意分发给各个面。
#[derive(Clone, Debug)]
pub struct LifecycleReader {
    rx: watch::Receiver<Phase>,
}

impl LifecycleReader {
    /// 当前阶段。
    ///
    /// 借用在函数返回前就释放——**不要**把 `borrow()` 的结果跨 `await` 持有：
    /// `watch` 的 borrow 会阻塞写端。
    #[must_use]
    pub fn current(&self) -> Phase {
        *self.rx.borrow()
    }

    /// 等到进入 [`Phase::Running`]，或等到不可能再进入为止。
    ///
    /// 返回 `true` 表示确实进入了 `Running`；返回 `false` 表示进程已走向关停
    /// （写端消失或阶段已越过 `Running`）。面用它实现"绑定 ≠ 就绪"：
    /// 端口已经监听，但在收到 `Running` 之前不处理业务。
    pub async fn wait_for_running(&mut self) -> bool {
        loop {
            // 先看当前值，避免错过在订阅之前就已经发生的转换。
            match self.current() {
                Phase::Running => return true,
                Phase::Draining | Phase::Forcing | Phase::Aborting | Phase::Stopped => {
                    return false;
                }
                Phase::Starting => {}
            }
            if self.rx.changed().await.is_err() {
                // 写端已 drop：不会再有转换了。
                return false;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_running_serves_traffic() {
        let cases = [
            (Phase::Starting, false),
            (Phase::Running, true),
            (Phase::Draining, false),
            (Phase::Forcing, false),
            (Phase::Aborting, false),
            (Phase::Stopped, false),
        ];
        for (i, (phase, expected)) in cases.into_iter().enumerate() {
            assert_eq!(
                phase.serves_traffic(),
                expected,
                "TC{i} ({}) 的可服务判定不符",
                phase.as_str()
            );
        }
    }

    #[test]
    fn publish_without_subscribers_still_records_the_phase() {
        let (publisher, reader) = LifecyclePublisher::new();
        drop(reader);
        // 没有订阅者也必须写进去——否则后来的订阅者会读到过时的阶段。
        publisher.publish(Phase::Running);
        assert_eq!(publisher.current(), Phase::Running);
        assert_eq!(publisher.reader().current(), Phase::Running);
    }

    #[tokio::test]
    async fn wait_for_running_returns_false_when_shutdown_overtakes_it() {
        // 跨越 draining 的等待者必须得到"不会 running 了"，而不是永远挂着。
        let (publisher, mut reader) = LifecyclePublisher::new();
        publisher.publish(Phase::Draining);
        assert!(!reader.wait_for_running().await);
    }

    #[tokio::test]
    async fn wait_for_running_observes_a_later_transition() {
        let (publisher, mut reader) = LifecyclePublisher::new();
        let waiter = tokio::spawn(async move { reader.wait_for_running().await });
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        publisher.publish(Phase::Running);
        assert!(waiter.await.expect("等待任务不该 panic"));
    }
}
