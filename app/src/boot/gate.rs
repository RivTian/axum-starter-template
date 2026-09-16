//! 提交门：这一次启动到底算不算成功。
//!
//! 一句话——**prepared ≠ committed**——在这里变成代码。面被 spawn 出去只说明它们
//! 开始跑了，不说明它们准备好了。HTTP 面还没把 listener 注册到自己的 runtime 上、worker 面
//! 还没算出第一个 tick 的时刻，这两件事都可能失败，而失败时进程已经在跑了。
//!
//! 所以「起来了」这个结论有一个显式的谓词：**每个面都回执了**。谓词成立之前，
//! [`Phase::Running`] 不发布，`/readyz` 保持 `503`——编排器因此不会在这个窗口里给它流量。
//!
//! [`Phase::Running`]: service_core::lifecycle::Phase::Running
//!
//! # 为什么不用 `_ = ready(()) => break` 这样的兜底分支
//!
//! 那种写法长这样：`select!` 最后挂一个立刻就绪的分支，靠 `biased` 的顺序保证它排在
//! 最后，于是"别的分支都没就绪"就等价于"可以提交了"。
//!
//! 这等于把提交条件寄存在**分支顺序**里。有人把那一臂往上挪一行、或者在它前面插一个新的
//! 分支，提交就会发生在回执之前——而这不会有任何编译错误、任何测试失败，症状是偶发的
//! "服务刚上线就 502"。这里的提交条件是 [`AckSet::all_received`] 这个能被直接断言的谓词，
//! 于是 `commit_never_precedes_a_pending_ack` 能真的守住它。
//!
//! # 三条失败路径，两种探测
//!
//! | 情况 | 怎么发现的 |
//! | --- | --- |
//! | 外部在启动途中要求停止 | `cancel` 那一臂 |
//! | 某个面还没回执就退出了 | `supervisor.next_exit()` 那一臂，**带着退出原因** |
//! | 一个面都不剩了 | 同上，`None` |
//! | 回执端被 drop（面没了但退出记录还没到） | `acks` 那一臂返回 `false` |
//!
//! 后两行是同一件事的两条独立探测（`core::task::ack` 的模块文档里也写了这条冗余）：
//! 回执端被 move 进了面的 future，面一结束它就没了。
//!
//! 两条探测**同一轮就绪**时 `biased` 让带原因的那条先被看见。但它们不一定同一轮就绪：
//! `JoinSet` 要等被 poll 到才记账，于是"回执端没了"可能先到一步。这时提交门**照样立刻
//! 中止**，不为了等一个原因而多挂一轮——原因不会因此丢失，`abort_boot` 的短宽限收割会把
//! 那条 `TaskExit` 收上来。相反，"再等一轮"要赌的是"面一定会退出"，而一个 `drop(ack)` 之后
//! 继续跑的面（没有面这么写，但没有任何东西拦着）会让提交门永远挂着——症状是进程起来了、
//! 永远不 ready，比少一句原因难查得多。

use std::future::{Future, poll_fn};
use std::pin::Pin;
use std::task::{Context, Poll};

use service_core::task::{AckReceiver, TaskExit, TaskName, TaskSupervisor};
use tokio_util::sync::CancellationToken;

/// 还没收齐的回执。
///
/// 内部是一组被 box 起来的 future 而不是 `FuturesUnordered`：[`AckReceiver::recv`] 按值
/// 消费接收端，没法在原地反复 poll 一个 `&mut AckReceiver`，而为一个提交门引一个
/// `futures-util` 不值当。
#[derive(Debug)]
pub(crate) struct AckSet {
    pending: Vec<Pending>,
}

/// 一个面的待收回执。
struct Pending {
    plane: TaskName,
    future: Pin<Box<dyn Future<Output = bool> + Send>>,
}

/// 手写 `Debug`：`dyn Future` 派生不出来。只打面名，够定位了。
impl std::fmt::Debug for Pending {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pending")
            .field("plane", &self.plane)
            .finish()
    }
}

impl AckSet {
    /// 收下装配层留下的那一组接收端。
    pub(crate) fn new(receivers: Vec<AckReceiver>) -> Self {
        Self {
            pending: receivers
                .into_iter()
                .map(|rx| Pending {
                    plane: rx.plane(),
                    future: Box::pin(rx.recv()),
                })
                .collect(),
        }
    }

    /// 提交条件。**这就是那个谓词**，提交门只认它。
    pub(crate) fn all_received(&self) -> bool {
        self.pending.is_empty()
    }

    /// 还差谁。失败时进日志用。
    pub(crate) fn outstanding(&self) -> Vec<TaskName> {
        self.pending.iter().map(|p| p.plane).collect()
    }

    /// 等下一份回执。
    ///
    /// **取消安全。** 返回的 future 只借 `&mut self`；它在 `select!` 里被丢弃时，
    /// `self.pending` 里那些真正的接收 future 原地不动。这条性质是
    /// `the_gate_keeps_its_progress_across_a_dropped_select` 验的东西，也是这个方法能
    /// 出现在 `select!` 里的全部前提。
    async fn next(&mut self) -> (TaskName, bool) {
        poll_fn(|cx| self.poll_next(cx)).await
    }

    /// 轮询一遍所有待收回执，谁先好取谁。
    ///
    /// 全部 `pending` 都是 `Pending` 时返回 `Poll::Pending`——包括空集合的情况。空集合
    /// 永远挂起是对的：提交门在 `all_received()` 为真时根本不会进 `select!`。
    fn poll_next(&mut self, cx: &mut Context<'_>) -> Poll<(TaskName, bool)> {
        for index in 0..self.pending.len() {
            if let Poll::Ready(received) = self.pending[index].future.as_mut().poll(cx) {
                let done = self.pending.swap_remove(index);
                return Poll::Ready((done.plane, received));
            }
        }
        Poll::Pending
    }
}

/// 提交门的结论。
#[derive(Debug)]
pub(crate) enum GateOutcome {
    /// 每个面都回执了。调用方接着发 [`Phase::Running`]。
    ///
    /// [`Phase::Running`]: service_core::lifecycle::Phase::Running
    Committed,
    /// 没能提交。`abort_boot` 接手：取消 → 短宽限收割 → 关存储 → 返回原始错误，
    /// 并且**不发** `Draining`（订阅者还没就绪）。
    Aborted(AbortCause),
}

/// 启动失败的四种成因。
#[derive(Debug)]
pub(crate) enum AbortCause {
    /// 启动途中收到了停止请求。
    Cancelled,
    /// 某个面在回执之前就退出了。**带着完整的退出记录**——原因决定了日志里写什么。
    EarlyExit(TaskExit),
    /// 一个在册任务都不剩了。
    SupervisorDrained,
    /// 回执端被 drop 了：面没了，但它的退出记录还没被 `next_exit` 取到。
    ///
    /// 与 [`Self::EarlyExit`] 是同一件事的两条探测，这一条没有原因——`JoinSet` 的记账
    /// 比 drop 晚一轮时就会走到这里，是时序，不是另一类故障。原因由 `abort_boot` 的
    /// 收割补上，见模块文档。
    AckDropped { plane: TaskName },
}

impl AbortCause {
    /// 日志与报告里用的稳定短名。
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::Cancelled => "cancelled",
            Self::EarlyExit(_) => "plane_exited_early",
            Self::SupervisorDrained => "no_planes_left",
            Self::AckDropped { .. } => "ack_dropped",
        }
    }
}

/// 跑提交门。
///
/// 循环形态是 `while !all_received { select! }`：**先判谓词、再等事件**。反过来写
/// （先 `select!` 再判）会在回执已经收齐时多挂一次，而那一次挂起没有任何东西能唤醒它。
pub(crate) async fn commit(
    acks: &mut AckSet,
    supervisor: &mut TaskSupervisor,
    cancel: &CancellationToken,
) -> GateOutcome {
    while !acks.all_received() {
        let aborted = tokio::select! {
            biased;

            // ① 外部要求停止。排在最前：启动途中按 Ctrl-C 的人不该等到所有面都准备好。
            () = cancel.cancelled() => AbortCause::Cancelled,

            // ② 有面提前退出了。排在回执前面，因为这一臂说得出原因。
            exit = supervisor.next_exit() => match exit {
                Some(exit) => AbortCause::EarlyExit(exit),
                None => AbortCause::SupervisorDrained,
            },

            // ③ 收到一份回执。这是**唯一**让循环继续的分支。
            (plane, received) = acks.next() => {
                if received {
                    continue;
                }
                AbortCause::AckDropped { plane }
            }
        };
        return GateOutcome::Aborted(aborted);
    }
    GateOutcome::Committed
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    use service_core::task::{AckSender, ShutdownClass, TaskSpec, ack_channel};
    use tokio::runtime::Handle;

    /// 这条用例里提交门必须**等**，所以要一个真的会超时的短预算。
    const SHORT: Duration = Duration::from_millis(50);

    /// 一个永远不退出的面。提交门的三条失败臂全都不该因为它而触发。
    fn never_exits(supervisor: &mut TaskSupervisor, name: TaskName) {
        supervisor
            .spawn_on(
                TaskSpec::new(name, ShutdownClass::Abortable),
                &Handle::current(),
                Box::pin(async {
                    std::future::pending::<()>().await;
                    Ok(())
                }),
            )
            .expect("同一个面名只登记一次");
    }

    /// 建 n 份回执，返回 (发送端, 已装好的 AckSet)。
    fn channels(planes: &[TaskName]) -> (Vec<AckSender>, AckSet) {
        let mut senders = Vec::new();
        let mut receivers = Vec::new();
        for plane in planes {
            let (tx, rx) = ack_channel(*plane);
            senders.push(tx);
            receivers.push(rx);
        }
        (senders, AckSet::new(receivers))
    }

    #[tokio::test]
    async fn commit_never_precedes_a_pending_ack() {
        // 两个面，只有一个回执。提交门必须一直等——这条用例守的是「提交条件是谓词，
        // 不是分支顺序」：把 `select!` 里的分支随便换个顺序，这里立刻红。
        let mut supervisor = TaskSupervisor::new();
        never_exits(&mut supervisor, TaskName::Http);
        never_exits(&mut supervisor, TaskName::Ticker);

        let (mut senders, mut acks) = channels(&[TaskName::Http, TaskName::Ticker]);
        let cancel = CancellationToken::new();

        senders.remove(0).send();

        let waited = tokio::time::timeout(SHORT, commit(&mut acks, &mut supervisor, &cancel)).await;
        assert!(
            waited.is_err(),
            "只收到一份回执就提交了——提交条件没有真的是 all_received"
        );
        assert_eq!(
            acks.outstanding(),
            vec![TaskName::Ticker],
            "等的那一个必须报得出名字，否则失败日志说不清是谁没准备好"
        );

        // 上面那次 `timeout` 把 `commit` 的 future 丢掉了。已经收到的那份回执不能因此丢失,
        // 否则 `select!` 每转一圈就会漏掉一次就绪——这正是取消安全要保证的事。
        senders.remove(0).send();
        let outcome = tokio::time::timeout(SHORT, commit(&mut acks, &mut supervisor, &cancel))
            .await
            .expect("回执收齐之后提交门必须立刻返回");
        assert!(
            matches!(outcome, GateOutcome::Committed),
            "回执收齐了却没提交：{outcome:?}"
        );
    }

    #[tokio::test]
    async fn a_plane_that_dies_before_acking_aborts_the_boot_with_its_reason() {
        // HTTP 面 bind 成功、`from_std` 失败——面返回 `Err` 而且没回执。
        //
        // 这条用例把两条探测**放到同一轮**：先让失败的面真的跑完（`yield_now`），
        // 于是进 `select!` 时 `next_exit` 与 `acks` 都已就绪。此时结论必须是带原因的那条，
        // 这就是 `biased` 里那两臂的顺序在守的东西——把它们对调，这里立刻红。
        let mut supervisor = TaskSupervisor::new();
        never_exits(&mut supervisor, TaskName::Ticker);

        let (mut senders, mut acks) = channels(&[TaskName::Http, TaskName::Ticker]);
        // HTTP 的发送端丢掉：那个面不会回执了。ticker 的留着，免得它也变成失败源。
        drop(senders.remove(0));

        supervisor
            .spawn_on(
                TaskSpec::new(TaskName::Http, ShutdownClass::Graceful),
                &Handle::current(),
                Box::pin(async {
                    Err(service_core::task::PlaneError::failed(
                        TaskName::Http,
                        "the listener could not be registered with this runtime",
                    ))
                }),
            )
            .expect("同一个面名只登记一次");

        // 让那个面跑完并被 `JoinSet` 记上账。没有这一步，"回执端没了"会先到一轮，
        // 结论变成没有原因的 `AckDropped`——那也是对的，但不是这条用例要验的东西。
        tokio::task::yield_now().await;

        let cancel = CancellationToken::new();
        let outcome = tokio::time::timeout(SHORT, commit(&mut acks, &mut supervisor, &cancel))
            .await
            .expect("面已经退出了，提交门不该继续等");

        let GateOutcome::Aborted(cause) = outcome else {
            panic!("面退出了却提交成功：{outcome:?}");
        };
        assert_eq!(
            cause.as_str(),
            "plane_exited_early",
            "两条探测同一轮就绪时，带原因的那条必须赢，实际拿到 {cause:?}"
        );
    }

    #[tokio::test]
    async fn a_dropped_ack_aborts_instead_of_waiting_for_a_reason() {
        // 反过来的那一轮：回执端已经没了，退出记录还没到。提交门必须**立刻**中止。
        //
        // 多等一轮换一句原因，赌的是"面一定会退出"；而原因根本没丢——`abort_boot` 的
        // 收割会把它收上来。这条用例守的就是"不为了一句原因去赌一次可能的永久挂起"。
        let mut supervisor = TaskSupervisor::new();
        never_exits(&mut supervisor, TaskName::Http);

        let (senders, mut acks) = channels(&[TaskName::Http]);
        drop(senders);

        let cancel = CancellationToken::new();
        let outcome = tokio::time::timeout(SHORT, commit(&mut acks, &mut supervisor, &cancel))
            .await
            .expect("回执端没了就该出结论，不能挂着等一条退出记录");

        let GateOutcome::Aborted(AbortCause::AckDropped { plane }) = outcome else {
            panic!("回执端被 drop 了却没中止：{outcome:?}");
        };
        assert_eq!(plane, TaskName::Http, "中止理由必须说得出是哪个面");
    }

    #[tokio::test]
    async fn a_stop_request_during_boot_aborts_before_anything_else() {
        // 启动途中按 Ctrl-C。这一臂排最前，于是即使别的分支同时就绪，结论也是"被取消"——
        // 那是唯一一个**不算故障**的启动失败，日志级别与退出码都不一样。
        let mut supervisor = TaskSupervisor::new();
        never_exits(&mut supervisor, TaskName::Http);

        let (_senders, mut acks) = channels(&[TaskName::Http]);
        let cancel = CancellationToken::new();
        cancel.cancel();

        let outcome = tokio::time::timeout(SHORT, commit(&mut acks, &mut supervisor, &cancel))
            .await
            .expect("已经取消了，提交门不该继续等");

        let GateOutcome::Aborted(cause) = outcome else {
            panic!("取消之后还提交成功了：{outcome:?}");
        };
        assert_eq!(cause.as_str(), "cancelled");
    }

    #[tokio::test]
    async fn an_empty_supervisor_aborts_instead_of_hanging() {
        // 一个面都没登记。`next_exit()` 立刻返回 `None`——如果那一臂把 `None` 当成
        // "没事发生"而继续等，提交门会永远挂着，症状是进程起来了但永远不 ready。
        let mut supervisor = TaskSupervisor::new();
        let (_senders, mut acks) = channels(&[TaskName::Http]);
        let cancel = CancellationToken::new();

        let outcome = tokio::time::timeout(SHORT, commit(&mut acks, &mut supervisor, &cancel))
            .await
            .expect("空 supervisor 必须立刻给出结论，不能挂着");

        let GateOutcome::Aborted(cause) = outcome else {
            panic!("一个面都没有却提交成功了：{outcome:?}");
        };
        assert_eq!(cause.as_str(), "no_planes_left");
    }
}
