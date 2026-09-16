//! 编排循环：从「任务已启动」到「进程该退了」之间的全部副作用。
//!
//! 文件名是 `loop.rs`，模块名只能写成 `r#loop`——`loop` 是关键字。用它是因为这个模块
//! 里确实只有一件事：那个循环。换个不撞关键字的名字（`orchestrate`、`main_loop`）都得
//! 多解释一句"它其实就是主循环"。
//!
//! # 这个模块是唯一有副作用的那个
//!
//! [`stop`] 是纯状态机，[`report`] 是纯数据。它们回答"该做什么"和"这次算不算成功"；
//! 这里回答"怎么做"，而"怎么做"是写死的一条序列：
//!
//! ```text
//! publish(Draining)        ← 公告先于任何取消
//! info!(shutdown_started)
//! root.cancel()            ← 级联到所有 child token
//! harvest(d1)              ← 只等 ShutdownClass::Graceful 的面自己收尾
//! abort_all(); reap(d2)    ← 独立的第二段预算；全部任务一律进这一段
//! storage.close(d3)        ← 存储最后关
//! ── block_on 返回 ──
//! runtimes.shutdown(d4)    ← 由 `run` 在 block_on 之后同步执行（在 runtime 里关它会 panic）
//! ```
//!
//! 序列里的每一步各自在 `core` 里有用例；这里验的是**顺序**与**边界**。
//!
//! [`stop`]: super::stop
//! [`report`]: super::report
//!
//! # 启动失败与正常关停走同一条序列
//!
//! [`Sequence::drain`] 一份，两条路径共用，差别只有两处：发的相位是 [`Phase::Aborting`]
//! 还是 [`Phase::Draining`]（启动失败时订阅者尚未全部就绪，不该按"优雅关停"处理），
//! 以及最后产出的是哪一种 [`RunReport`]。
//!
//! 不给启动失败另写一条短路径，是因为两条序列迟早会分叉——而它们分叉的那一天，被漏掉的
//! 那一步一定是"关存储"。顺带一提，走完整序列在启动失败时并不慢：`harvest` 在所有
//! `Graceful` 面退完的那一刻就返回，它的 deadline 是**上界**不是等待时长，而启动失败时
//! 面手上一个在途请求都没有。
//!
//! # 升级（第二次 Ctrl-C）作用于**下一段**，不截断正在跑的那一段
//!
//! [`TaskSupervisor::harvest`] 的返回值里装着这一段收回来的全部退出记录，而它**不是**
//! 取消安全的：把它丢在 `select!` 里换一个更短的 deadline，代价是那些记录连同
//! 「每个任务都有一条闭合记录」这条验收项一起丢掉。
//!
//! 所以第二次停止请求的落点是：[`StopPolicy`] 当场收紧计划，**下一段**（`reap`、
//! `storage_close`）现读那份新计划，于是收紧立刻生效；正在跑的那一段跑到它自己的截止
//! 时刻为止。最坏情况是多等一个 `harvest` 预算，而那是有上界的——`total_grace` 封顶。
//!
//! 第三次请求走 [`StopAction::ForceExit`]：那一条不等任何东西，正在跑的那一段当场丢弃。
//! 丢记录在这里是可以接受的，因为"立刻退"正是按第三次的人要的东西。
//!
//! # 第三段（关存储）不接受打断
//!
//! 它已经有自己的截止时刻，而中途放弃的代价是 WAL 的收尾没人做。这是整条关停路径上
//! 唯一一个「提前结束比晚一点结束更坏」的地方，所以它不进 `select!`。

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;

use futures_core::Stream;
use service_core::lifecycle::{LifecyclePublisher, Phase};
use service_core::shutdown::{Budgets, Plan, UnreapedTask};
use service_core::storage::{CloseOutcome, Storage};
use service_core::task::{ExitKind, TaskExit, TaskName, TaskSupervisor};
use service_storage::StorageOwner;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use super::report::RunReport;
use super::stop::{StopAction, StopCause, StopPolicy};
use crate::boot::assembly::Running;
use crate::boot::gate::{self, AbortCause, GateOutcome};
use crate::config::reload::{ReloadOutcome, ReloadSource};
use crate::signals::ProcessSignal;

/// 编排循环跑完之后留给 `run` 的东西。
///
/// 两个字段，因为 `block_on` 之后还剩一件事：关 runtime。它**不能**在这个模块里做——
/// 在一个 runtime 上下文里关这个 runtime 会 panic。把截止时刻带出去，
/// 是为了让第四段的边界仍然来自同一份 [`Plan`]，而不是 `run` 自己再算一遍。
#[derive(Debug)]
pub(crate) struct Finished {
    /// 这一次运行的全部结论。退出码只由它算。
    pub(crate) report: RunReport,
    /// 第四段（关 runtime）的截止时刻。
    pub(crate) runtime_deadline: Instant,
}

/// 关停序列要动的四样东西 + 根令牌。
///
/// 打包成一个结构体而不是五个参数，是为了让 [`Self::drain`] 只有三个入参——五个以上的
/// 参数列表里，调换两个同类型参数的位置是编译不出错的。
struct Sequence {
    supervisor: TaskSupervisor,
    lifecycle: LifecyclePublisher,
    storage_owner: StorageOwner,
    /// 只用来数，不用来调。`Arc::strong_count` 回到 1 是「没有别的面还攥着这个池」
    /// 的唯一证据，也是 [`CloseOutcome::SkippedUnproven`] 要的那份证据。
    storage: Arc<dyn Storage>,
    cancel: CancellationToken,
}

/// 关停序列跑完之后的全部事实。
struct Drained {
    exits: Vec<TaskExit>,
    unreaped: Vec<UnreapedTask>,
    forced: bool,
    storage_close: CloseOutcome,
    /// 最终生效的计划（可能已被升级收紧过）。报告要它的 `invalid`。
    plan: Plan,
}

/// 信号流的一层薄包装：**流结束之后永远挂起**。
///
/// 没有这一层的话，那条 `select!` 分支在流结束后每一轮都会立刻就绪，把等待变成忙等——
/// 关停路径上那意味着一个吃满一颗核的进程。第一次的 `None` 仍然如实报出来（主循环要用它
/// 判 [`StopCause::SignalsEnded`]），只有之后的才挂起。
struct Signals<'a, S: ?Sized> {
    stream: Pin<&'a mut S>,
    ended: bool,
}

impl<S> Signals<'_, S>
where
    S: Stream<Item = ProcessSignal> + ?Sized,
{
    /// 等下一个信号。
    ///
    /// **取消安全。** `ended` 只在 await 返回之后才写；被 `select!` 丢弃时底层流原地不动。
    async fn next(&mut self) -> Option<ProcessSignal> {
        if self.ended {
            return std::future::pending().await;
        }
        let signal = std::future::poll_fn(|cx| self.stream.as_mut().poll_next(cx)).await;
        if signal.is_none() {
            self.ended = true;
        }
        signal
    }
}

/// 跑完这一次运行：提交门 → 主循环 → 固定的关停序列。
///
/// 从 [`Running`] 往后的一切都在这里面，于是 `run` 只有一次调用、只有一处关存储。
pub(crate) async fn orchestrate<S>(running: Running, reload: &ReloadSource, stops: S) -> Finished
where
    S: Stream<Item = ProcessSignal>,
{
    let Running {
        mut supervisor,
        mut acks,
        lifecycle,
        mut config,
        storage_owner,
        storage,
        cancel,
    } = running;

    // 关停预算是**冷段**（`heat.rs` 的 `ColdView` 里整段 `shutdown`），重载改不了它。
    // 于是在这里读一次就够了，不需要在关停路径上再回头查配置。
    let budgets: Budgets = config.current().config().shutdown;
    let mut policy = StopPolicy::new(budgets);

    let stops = std::pin::pin!(stops);
    let mut signals = Signals {
        stream: stops,
        ended: false,
    };

    // ── 提交门 ────────────────────────────────────────────────────────────
    if let GateOutcome::Aborted(cause) = gate::commit(&mut acks, &mut supervisor, &cancel).await {
        let sequence = Sequence {
            supervisor,
            lifecycle,
            storage_owner,
            storage,
            cancel,
        };
        return abort_boot(
            cause,
            acks.outstanding(),
            sequence,
            budgets,
            &mut policy,
            &mut signals,
        )
        .await;
    }

    lifecycle.publish(Phase::Running);
    info!(name: "startup_committed", "all planes acknowledged readiness; serving");

    // ── 主循环：等第一次停止请求 ──────────────────────────────────────────
    let cause = loop {
        tokio::select! {
            biased;

            // ① 有面退出了。排在信号前面不是因为它更常见，恰恰相反——它是**故障**，
            //    而信号是常态。两件事落在同一个 poll 窗口里时，`StopPolicy` 的首因只认
            //    第一个，于是这里的排序就是在选首因：报告里该留下的是那个更需要人看的。
            exit = supervisor.next_exit() => {
                break match exit {
                    Some(exit) => StopCause::TaskExited(exit),
                    // 一个在册任务都不剩了。`enabled = false` 且 HTTP 面也退了才会到这儿。
                    None => StopCause::SupervisorDrained,
                };
            }

            // ② 信号。
            signal = signals.next() => match signal {
                Some(signal) if signal.is_stop() => break StopCause::Signal(signal),
                // SIGHUP：重载，不停机。整条事务在 `reload` 里，这里只负责给它那个
                // 独占的 `&mut ConfigPublisher`——single-flight 就是这么来的。
                //
                // 四个分支逐个写出来，而不是 `let _ =`。`ReloadOutcome` 不是 `Result`，
                // 正是为了不让人在这儿顺手写 `?` 把一次手抖变成停机；而 `let _ =` 是同一个
                // 错误的另一面——它让"新增一种结局"这件事悄悄溜过编排层。现在它会在这个
                // 无 `_` 臂的 `match` 上撞墙，逼人回答"编排循环该拿它怎么办"。
                //
                // 这四个答案眼下都是"什么都不做"，**这不是巧合**：重载的四条日志是
                // `config::reload` 的对外契约，记在一处；而热字段是各面每 tick 现读的，
                // 没有谁需要被通知。会让这里长出代码的是"某个面需要被叫醒"——那一天
                // 编译器会把人带到这里。
                Some(_) => match reload.reload(&mut config) {
                    // 换上去了。热字段下一 tick 自动生效。
                    ReloadOutcome::Applied { .. } => {}
                    // 与生效中的逐字段相同。生成号没动，更没有人要通知。
                    ReloadOutcome::Unchanged => {}
                    // 含冷段变更，整批拒绝。待重启清单已经进了日志；这里**不能**代替
                    // 运维去重启——「什么时候重启」是部署决策。
                    ReloadOutcome::RejectedCold { .. } => {}
                    // 读不出来 / 不认。last-good 原封不动，服务继续跑。
                    ReloadOutcome::Failed(_) => {}
                },
                // 流自己结束了（信号驱动随 runtime 消失）。继续挂着等于永远等一个
                // 再也不会来的信号，所以这算一次停止请求。
                None => break StopCause::SignalsEnded,
            },
        }
    };

    // ── 关停 ──────────────────────────────────────────────────────────────
    let now = Instant::now();
    let plan = match policy.on_request(cause, now) {
        StopAction::Begin(plan) | StopAction::Escalate(plan) => plan,
        // 第一次请求走不到这里：`StopPolicy` 的计数从 0 开始，第一次必然是 `Begin`。
        // 真走到了就说明状态机被改坏了，此时最安全的动作是立刻退，而不是拿一个并不存在
        // 的计划继续往下跑。
        StopAction::ForceExit => {
            error!(
                name: "stop_policy_inconsistent",
                "the first stop request returned ForceExit; exiting without a drain"
            );
            return force_exit(policy.take_cause(), Vec::new(), now);
        }
    };

    let sequence = Sequence {
        supervisor,
        lifecycle,
        storage_owner,
        storage,
        cancel,
    };
    let Some(drained) = sequence
        .drain(Phase::Draining, plan, &mut policy, &mut signals)
        .await
    else {
        return force_exit(policy.take_cause(), Vec::new(), Instant::now());
    };

    Finished {
        report: RunReport::stopped(
            policy.take_cause(),
            drained.exits,
            drained.unreaped,
            drained.forced,
            drained.plan.invalid,
            drained.storage_close,
        ),
        runtime_deadline: drained.plan.runtime_shutdown,
    }
}

impl Sequence {
    /// 固定的关停序列（见模块文档）。返回 `None` 表示中途收到了第三次停止请求。
    ///
    /// `phase` 决定公告的是 [`Phase::Draining`] 还是 [`Phase::Aborting`]；除此之外两条
    /// 路径逐行相同。
    async fn drain<S>(
        mut self,
        phase: Phase,
        plan: Plan,
        policy: &mut StopPolicy,
        signals: &mut Signals<'_, S>,
    ) -> Option<Drained>
    where
        S: Stream<Item = ProcessSignal> + ?Sized,
    {
        // ① 公告先于任何取消。订阅者要先知道"不再接新活"，才轮得到取消令牌去打断在途的。
        self.lifecycle.publish(phase);
        info!(
            name: "shutdown_started",
            phase = phase.as_str(),
            plan_invalid = plan.invalid,
            "shutdown sequence started"
        );

        // ② 级联取消。根令牌一响，所有 child token 同时响。
        self.cancel.cancel();

        let mut exits = Vec::new();
        let mut unreaped = Vec::new();
        let mut forced = false;

        // ③ 第一段：只等 Graceful 的面自己收尾。
        {
            let harvest = self.supervisor.harvest(plan.harvest);
            let mut harvest = std::pin::pin!(harvest);
            let outcome = drive(harvest.as_mut(), signals, policy).await?;
            exits.extend(outcome.exits);
            unreaped.extend(outcome.unreaped);
            forced |= outcome.forced;
        }

        // 升级过的话，从这里开始现读新计划——收紧于是立刻生效。
        let plan = policy.plan().unwrap_or(plan);

        // ④ 第二段：abort 剩下的，然后在独立预算内回收。
        if !self.supervisor.is_empty() {
            // 到这一步还有任务在册，下一行就是 `abort_all()`——这才是真的在"强制"。
            //
            // 这个判据能成立，靠的是 `harvest` 返回前把**已经自己结束**的任务都收掉了。
            // 没有那一步的话，Abortable 面拿不到 harvest 预算，于是它在这里
            // **必然**还在册上，`Forcing` 就成了每次关停都发的一条固定日志——一个恒真的
            // 相位不告诉任何人任何事。改动 `harvest` 的收尾之前先回来看这一段。
            self.lifecycle.publish(Phase::Forcing);
        }
        {
            let reap = self.supervisor.reap(plan.reap);
            let mut reap = std::pin::pin!(reap);
            let outcome = drive(reap.as_mut(), signals, policy).await?;
            exits.extend(outcome.exits);
            unreaped.extend(outcome.unreaped);
            forced |= outcome.forced;
        }

        record(&exits, &unreaped);

        // ⑤ 第三段：存储最后关，且不接受打断（理由见模块文档）。
        let plan = policy.plan().unwrap_or(plan);
        let storage_close =
            close_storage(self.storage_owner, self.storage, plan.storage_close).await;

        // 这条记录有两个活儿，缺一个都不够：
        //
        // - **顺序证据**。「存储最后关」这条不变量在代码里是靠**排序**成立的，类型不管它
        //   （见 `boot::assembly` 的模块文档）。唯一能把它钉住的，是这条记录晚于 `record`
        //   发出的全部 `task_exit`——编排用例正是这么断言的。没有这条记录，那条不变量就
        //   只剩一句注释。
        // - **结论证据**。`storage_close` 会进 `RunReport::stopped`，于是它能一路决定退出
        //   码。一个只影响退出码、不留日志的结论，排查时是这样的：进程退了 1，日志从头到尾
        //   干净——那是最难查的一类。
        //
        // 级别按结局分：`SkippedUnproven` 是"需要人来看"，不是失败，所以它和 `Failed` 不能
        // 共用 `error`——都报红等于两者都不必看。
        match &storage_close {
            CloseOutcome::Closed => info!(
                name: "storage_close_result",
                outcome = storage_close.as_str(),
                "the storage pool was closed"
            ),
            CloseOutcome::SkippedUnproven { reason } => warn!(
                name: "storage_close_result",
                outcome = storage_close.as_str(),
                reason,
                "the storage pool was left open: nothing proved it was unused"
            ),
            CloseOutcome::Failed(error) => error!(
                name: "storage_close_result",
                outcome = storage_close.as_str(),
                kind = error.kind_str(),
                %error,
                "the storage pool could not be closed"
            ),
        }

        self.lifecycle.publish(Phase::Stopped);

        Some(Drained {
            exits,
            unreaped,
            forced,
            storage_close,
            plan,
        })
    }
}

/// 一边跑一段收割，一边继续收信号。
///
/// `stage` 是**已经 pin 住的**收割 future，每轮 `select!` 用 `as_mut()` 重借：没完成的
/// 那一轮它原地不动，下一轮接着从断点跑。这正是"升级不截断正在跑的那一段"的实现方式。
///
/// 返回 `None` = 第三次停止请求，当场放弃这一段。
async fn drive<F, S>(
    mut stage: Pin<&mut F>,
    signals: &mut Signals<'_, S>,
    policy: &mut StopPolicy,
) -> Option<F::Output>
where
    F: Future,
    S: Stream<Item = ProcessSignal> + ?Sized,
{
    loop {
        tokio::select! {
            biased;

            // ① 这一段自己跑完了。排最前：已经在收尾了，新来的请求不该抢在结论前面。
            outcome = stage.as_mut() => return Some(outcome),

            // ② 又来一个信号。
            signal = signals.next() => {
                // 流结束：关停期间无所谓，`Signals` 之后会永远挂起，这个循环于是退化成
                // "只等收割"。
                let Some(signal) = signal else { continue };

                if !signal.is_stop() {
                    // 排空期间不重载：关停预算是冷段、已经定下来了，此时换配置改不了任何
                    // 一段的边界，只会让日志更难读。
                    warn!(
                        name: "config_reload_ignored",
                        reason = "shutting down",
                        "SIGHUP ignored: the process is already shutting down"
                    );
                    continue;
                }

                match policy.on_request(StopCause::Signal(signal), Instant::now()) {
                    StopAction::Begin(plan) | StopAction::Escalate(plan) => {
                        info!(
                            name: "shutdown_escalated",
                            signal = signal.as_str(),
                            plan_invalid = plan.invalid,
                            "another stop request arrived; the remaining stages were tightened"
                        );
                    }
                    StopAction::ForceExit => {
                        warn!(
                            name: "shutdown_forced",
                            signal = signal.as_str(),
                            "third stop request; abandoning the shutdown sequence"
                        );
                        return None;
                    }
                }
            }
        }
    }
}

/// 第三段：关存储，前提是举得出「没有别的面还攥着它」的证据。
///
/// 证据就是 `Arc::strong_count == 1`——手上这一份是全进程最后一份。举不出来时**不关**，
/// 并如实报 [`CloseOutcome::SkippedUnproven`]：一个还被别人持有的池，`close()` 会等它
/// 归还，那是把一次关停拖成一次挂起。
async fn close_storage(
    owner: StorageOwner,
    storage: Arc<dyn Storage>,
    deadline: Instant,
) -> CloseOutcome {
    let holders = Arc::strong_count(&storage);
    if holders > 1 {
        // 只可能是被 abort 但没 join 上的任务还没析构。它是 `unreaped` 的同一件事在
        // 存储这一侧的投影，所以这里不另报一次故障，只报"没关"。
        return CloseOutcome::SkippedUnproven {
            reason: "a plane may still hold the pool",
        };
    }

    // 最后一份也放掉再关。留着它不会让 `close()` 失败（sqlx 的池内部自己是 Arc），
    // 但会让"关完之后谁还拿着它"这个问题多一个答案。
    drop(storage);
    owner.close(deadline).await
}

/// 给每个任务留一条闭合记录。
///
/// 这是验收项「Ctrl-C 之后的关停日志里，每个任务都有一条闭合记录」的落点。级别按退出
/// 形态分：正常返回是 `info`，其余都是 `warn` 以上——一个被 abort 掉的面没有机会跑完
/// 自己的收尾代码，那件事该被看见。
fn record(exits: &[TaskExit], unreaped: &[UnreapedTask]) {
    for exit in exits {
        if exit.kind.is_failure() {
            error!(
                name: "task_exit",
                task = exit.name_str(),
                runtime = %exit.runtime_str(),
                kind = exit.kind.as_str(),
                detail = %Detail(&exit.kind),
                "a plane exited with a failure"
            );
        } else if matches!(exit.kind, ExitKind::Returned) {
            info!(
                name: "task_exit",
                task = exit.name_str(),
                runtime = %exit.runtime_str(),
                kind = exit.kind.as_str(),
                "a plane exited"
            );
        } else {
            warn!(
                name: "task_exit",
                task = exit.name_str(),
                runtime = %exit.runtime_str(),
                kind = exit.kind.as_str(),
                "a plane was cancelled before it could finish"
            );
        }
    }

    for task in unreaped {
        warn!(
            name: "task_unreaped",
            task = ?task.name,
            runtime = ?task.runtime,
            class = task.class.as_str(),
            stalled_at = task.stalled_at.as_str(),
            "a task did not come back before its deadline"
        );
    }
}

/// 第三次停止请求之后的出口：什么都不等，如实报"没关干净"。
fn force_exit(cause: Option<StopCause>, exits: Vec<TaskExit>, now: Instant) -> Finished {
    Finished {
        report: RunReport::stopped(
            cause,
            exits,
            Vec::new(),
            true,
            false,
            CloseOutcome::SkippedUnproven {
                reason: "forced exit before the pool could be closed",
            },
        ),
        // 第四段也不等：`runtimes.shutdown` 收到一个已经过去的截止时刻就是"立刻放弃"。
        runtime_deadline: now,
    }
}

/// 提交门没通过。取消 → 走完同一条序列 → 报启动失败。
///
/// **不发** [`Phase::Draining`]：订阅者还没全部就绪，把这一次按"优雅关停"公告
/// 会让它们以为自己曾经服务过。
async fn abort_boot<S>(
    cause: AbortCause,
    outstanding: Vec<TaskName>,
    sequence: Sequence,
    budgets: Budgets,
    policy: &mut StopPolicy,
    signals: &mut Signals<'_, S>,
) -> Finished
where
    S: Stream<Item = ProcessSignal> + ?Sized,
{
    let kind = cause.as_str();
    error!(
        name: "startup_aborted",
        reason = kind,
        outstanding = ?outstanding,
        "startup aborted before the commit gate"
    );

    // 启动失败这一路没有"停止请求"，`StopPolicy` 手上是空的，所以计划在这里现算。
    // 预算仍然是同一份——一次失败的启动不该比一次正常关停有更宽或更窄的余地。
    let plan = Plan::new(Instant::now(), &budgets);
    let Some(drained) = sequence.drain(Phase::Aborting, plan, policy, signals).await else {
        return force_exit(None, Vec::new(), Instant::now());
    };

    // `AckDropped` 说不出原因——回执端被 drop 时，`JoinSet` 的记账可能还没轮到。原因是
    // 在刚才那次收割里补上的：第一条失败退出就是它。也就是说，原因由 abort_boot 的
    // 那次收割补上。
    let recovered = drained.exits.iter().find(|exit| exit.kind.is_failure());
    let error = BootAborted {
        reason: kind,
        detail: detail_of(&cause, recovered),
    };

    Finished {
        report: RunReport::startup_failed(kind, &error),
        runtime_deadline: drained.plan.runtime_shutdown,
    }
}

/// 启动失败时给 [`RunReport::startup_failed`] 的那张错误面孔。
///
/// 存在的唯一理由是它收的是 `&dyn Error`：一份报告要能沿着 `source()` 链把原因打印全，
/// 而 [`AbortCause`] 是个枚举，不是错误。
#[derive(Debug, thiserror::Error)]
#[error("startup aborted before the commit gate: {reason} ({detail})")]
struct BootAborted {
    reason: &'static str,
    detail: Box<str>,
}

/// 把中止原因（外加收割补回来的那条退出记录）渲染成一句人能读的话。
fn detail_of(cause: &AbortCause, recovered: Option<&TaskExit>) -> Box<str> {
    let base = match cause {
        AbortCause::Cancelled => "a stop request arrived during startup".to_owned(),
        AbortCause::EarlyExit(exit) => {
            format!("{} exited with {}", exit.name_str(), exit.kind.as_str())
        }
        AbortCause::SupervisorDrained => "no planes left".to_owned(),
        AbortCause::AckDropped { plane } => {
            format!("{plane} dropped its readiness channel")
        }
    };

    match recovered {
        Some(exit) => {
            format!("{base}; {} {}", exit.name_str(), Detail(&exit.kind)).into_boxed_str()
        }
        None => base.into_boxed_str(),
    }
}

/// [`ExitKind`] 的一行渲染。
///
/// 单独一个 `Display` 包装而不是一个 `fn -> String`：它出现在 `tracing` 的 `%` 位置上，
/// 那里要的是 `Display`，而且只有真的要记这条日志时才会被格式化。
struct Detail<'a>(&'a ExitKind);

impl std::fmt::Display for Detail<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            ExitKind::Returned => f.write_str("returned"),
            ExitKind::Failed(err) => write!(f, "{err}"),
            ExitKind::Panicked(summary) => write!(f, "panicked: {summary}"),
            ExitKind::Cancelled => f.write_str("cancelled"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::net::{Ipv4Addr, SocketAddr};
    use std::process::ExitCode;
    use std::task::{Context, Poll};
    use std::time::Duration;

    use service_core::build_info::BuildInfo;
    use service_core::config::Config;
    use service_core::task::TaskName;
    use service_testkit::{LogCapture, TempInstallRoot};
    use tokio::runtime::Handle;

    use crate::ProcessEnv;
    use crate::boot::assembly::assemble;
    use crate::config::bootstrap::PathSource;
    use crate::lifecycle::report::exit_code;
    use crate::rt::Executors;

    /// 一条把事先写好的信号依次吐出来、然后结束的流。
    ///
    /// 手写而不是拉 `tokio-stream`：`app` 的依赖清单里只有 `futures-core`（"只要 trait，
    /// 不要组合子"），为了一个八行的测试夹具破例不值得。
    struct Canned(std::vec::IntoIter<ProcessSignal>);

    impl Canned {
        fn new(signals: Vec<ProcessSignal>) -> Self {
            Self(signals.into_iter())
        }
    }

    impl Stream for Canned {
        type Item = ProcessSignal;

        fn poll_next(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            Poll::Ready(self.0.next())
        }
    }

    /// 一个永远不完成的"收割段"，用来把 [`drive`] 逼进信号分支。
    async fn never() -> u8 {
        std::future::pending().await
    }

    /// 一个立刻完成的"收割段"。
    async fn done() -> u8 {
        7
    }

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

    #[tokio::test]
    async fn a_finished_stage_wins_over_a_pending_signal() {
        // `biased;` 的第一条分支是收割段。已经有结论了就不该再去看信号——否则一次恰好
        // 同时到达的 Ctrl-C 会把一段已经跑完的收割算成"被打断"。
        let mut stream = std::pin::pin!(Canned::new(vec![ProcessSignal::Interrupt]));
        let mut signals = Signals {
            stream: stream.as_mut(),
            ended: false,
        };
        let mut policy = StopPolicy::new(budgets());

        let stage = std::pin::pin!(done());
        let outcome = drive(stage, &mut signals, &mut policy).await;

        assert_eq!(outcome, Some(7));
        assert!(!policy.is_stopping(), "这一轮不该有任何停止请求被记下");
    }

    #[tokio::test]
    async fn a_second_stop_request_tightens_the_plan_without_abandoning_the_stage() {
        // 第二次 Ctrl-C：计划收紧，但正在跑的那一段不丢。用例里那一段永远不完成，
        // 于是"没丢"表现为 `drive` 仍然挂着——超时即证明。
        let mut stream = std::pin::pin!(Canned::new(vec![ProcessSignal::Interrupt]));
        let mut signals = Signals {
            stream: stream.as_mut(),
            ended: false,
        };
        let mut policy = StopPolicy::new(budgets());

        // 先记一次，让接下来那次成为"第二次"。
        let first = policy.on_request(StopCause::Signal(ProcessSignal::Terminate), Instant::now());
        let StopAction::Begin(before) = first else {
            panic!("第一次请求必须是 Begin");
        };

        let stage = std::pin::pin!(never());
        let waited = tokio::time::timeout(
            Duration::from_millis(50),
            drive(stage, &mut signals, &mut policy),
        )
        .await;

        assert!(waited.is_err(), "第二次请求不该让正在跑的那一段被放弃");
        let after = policy.plan().expect("已经在关停了，计划必然在");
        assert!(
            after.reap <= before.reap,
            "升级只能让后续几段更早，不能更晚"
        );
        assert!(after.total <= before.total, "硬边界只能收紧");
    }

    #[tokio::test]
    async fn a_third_stop_request_abandons_the_stage() {
        let mut stream = std::pin::pin!(Canned::new(vec![
            ProcessSignal::Interrupt,
            ProcessSignal::Interrupt,
        ]));
        let mut signals = Signals {
            stream: stream.as_mut(),
            ended: false,
        };
        let mut policy = StopPolicy::new(budgets());
        policy.on_request(StopCause::Signal(ProcessSignal::Terminate), Instant::now());

        let stage = std::pin::pin!(never());
        let outcome = tokio::time::timeout(
            Duration::from_millis(50),
            drive(stage, &mut signals, &mut policy),
        )
        .await
        .expect("第三次请求必须让 drive 立刻返回，而不是继续等那一段");

        assert_eq!(outcome, None, "第三次请求该放弃这一段");
    }

    #[tokio::test]
    async fn sighup_is_ignored_while_draining() {
        // 排空期间的 SIGHUP 既不重载也不停机——它不该被 `StopPolicy` 记成一次停止请求。
        let mut stream = std::pin::pin!(Canned::new(vec![ProcessSignal::Reload]));
        let mut signals = Signals {
            stream: stream.as_mut(),
            ended: false,
        };
        let mut policy = StopPolicy::new(budgets());

        let stage = std::pin::pin!(never());
        let waited = tokio::time::timeout(
            Duration::from_millis(50),
            drive(stage, &mut signals, &mut policy),
        )
        .await;

        assert!(waited.is_err(), "SIGHUP 不该结束这一段");
        assert!(!policy.is_stopping(), "SIGHUP 被当成停止请求了");
    }

    #[tokio::test]
    async fn an_ended_signal_stream_reports_once_and_then_hangs() {
        // 第一次 `None` 要如实报出来（主循环靠它判 `SignalsEnded`），之后必须永远挂起。
        // 少了后半条，关停路径上那条分支会每一轮立刻就绪，把等待变成忙等。
        let mut stream = std::pin::pin!(Canned::new(Vec::new()));
        let mut signals = Signals {
            stream: stream.as_mut(),
            ended: false,
        };

        assert_eq!(signals.next().await, None, "流结束要报一次");

        let again = tokio::time::timeout(Duration::from_millis(50), signals.next()).await;
        assert!(again.is_err(), "报过一次之后还在就绪——这就是那个忙等");
    }

    #[test]
    fn an_ack_dropped_abort_carries_the_reason_recovered_from_the_harvest() {
        // `AckDropped` 自己说不出原因（`JoinSet` 的记账比 drop 晚一轮）。收割补回来的
        // 那条失败退出必须出现在报告里，否则运维只看得到"某个面没回执"。
        let cause = AbortCause::AckDropped {
            plane: TaskName::Http,
        };
        let recovered = TaskExit {
            name: Some(TaskName::Http),
            runtime: None,
            kind: ExitKind::Failed(service_core::task::PlaneError::failed(
                TaskName::Http,
                "address already in use",
            )),
        };

        let rendered = detail_of(&cause, Some(&recovered));
        assert!(
            rendered.contains("dropped its readiness channel"),
            "该保留中止原因：{rendered}"
        );
        assert!(
            rendered.contains("address already in use"),
            "该补上收割回来的真正原因：{rendered}"
        );
    }

    #[test]
    fn an_abort_without_a_recovered_exit_still_says_something_useful() {
        let cause = AbortCause::Cancelled;
        let rendered = detail_of(&cause, None);
        assert_eq!(&*rendered, "a stop request arrived during startup");
    }

    #[test]
    fn a_forced_exit_never_claims_a_clean_close() {
        let now = Instant::now();
        let finished = force_exit(
            Some(StopCause::Signal(ProcessSignal::Interrupt)),
            Vec::new(),
            now,
        );
        assert!(!finished.report.succeeded(), "强制退出不可能是一次干净收尾");
        assert_eq!(
            finished.runtime_deadline, now,
            "第四段也不许再等——截止时刻就是现在"
        );
    }

    // ── 整条序列 ────────────────────────────────────────────────────────────
    //
    // 上面那几条验的是 `drive` 与 `force_exit` 这些零件。下面这条验的是把它们接起来之后
    // 的**顺序**——而顺序是这个模块唯一真正负责的东西（见模块文档）。
    //
    // 它为什么在 lib 的单元测试里，而不在 `app/tests/`：`run` 会装一个全局
    // tracing subscriber，[`LogCapture`] 也要装一个，一个进程里只有一个槽位。于是
    // **一个测试二进制里，`run` 与日志断言只能有一个**。凡是要读日志的编排层用例都落在
    // 这里（`orchestrate` 是 `pub(crate)`，从外部用例够不着），`app/tests/` 那几个则只
    // 断言 `RunReport` 与退出码，每个二进制恰好调一次 `run`。

    /// 一份能真的装起来的配置：端口交给内核挑，数据库落在临时目录里。
    fn config_in(root: &TempInstallRoot) -> Config {
        let mut cfg = Config::default();
        cfg.http.bind_addr = SocketAddr::from((Ipv4Addr::LOCALHOST, 0));
        cfg.storage.path = root.root().join("data").join("test.sqlite3");
        cfg
    }

    /// 一个不碰任何进程全局态的 [`ProcessEnv`]。
    ///
    /// 字段全是公开的，正是为了让用例能这样按字面量拼一个出来——`capture()` 读的是真实
    /// 进程，而真实进程的环境变量是测试之间共享的可变状态。
    ///
    /// 这里只有 `vars` 会被用到：[`ReloadSource`] 拿它做变量替换，而本用例走的是 SIGTERM，
    /// 一次重载都不会发生。
    fn env_for_tests() -> ProcessEnv {
        ProcessEnv {
            exe_path: std::path::PathBuf::from("/nonexistent/demo"),
            args: Vec::new(),
            vars: std::collections::BTreeMap::new(),
            stderr_is_terminal: false,
            bin_name: "test",
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_clean_shutdown_announces_before_it_cancels_and_closes_storage_last() {
        // 一次正常起停里能在进程内验的那几条，也是模块文档那张序列图唯一的自动化证据。
        //
        // 「存储最后关」在代码里是靠**排序**成立的，类型不管它——`Sequence::drain` 里把
        // `close_storage` 写在 `record` 后面，仅此而已。换句话说，任何人把那两行调个个儿
        // 都编译得过。能钉住它的只有一条：`storage_close_result` 这条记录晚于 `record`
        // 发出的全部 `task_exit`。
        //
        // 这条用例必须跑在多线程 runtime 上：各面是被 spawn 出去的真任务，
        // current-thread 上它们只有在本用例让出执行权时才动，那时验的就不是并发下的顺序。
        let root = TempInstallRoot::new().expect("临时安装根");
        let running = assemble(
            config_in(&root),
            BuildInfo::current("test"),
            &Executors::for_tests(Handle::current()),
        )
        .await
        .expect("这份配置该能装起来")
        .launch();

        let reload = ReloadSource::new(
            root.root().join("config").join("service.toml"),
            PathSource::Default,
            &env_for_tests(),
            root.root().to_path_buf(),
        );

        // 捕获从这里才开始：装配阶段的日志（绑端口、开库）不是本用例的断言对象，
        // 让它们进来只会把失败时的 `summary()` 撑长。
        let capture = LogCapture::start().expect("装捕获用的 subscriber");
        let finished = orchestrate(
            running,
            &reload,
            Canned::new(vec![ProcessSignal::Terminate]),
        )
        .await;

        // ① 公告先于任何取消。「取消」在日志里没有自己的记录（`cancel()` 是一个同步调用，
        //    给它加一条记录只是把同一件事说两遍），它**可观察**的后果是各面开始退出。
        //    于是判据是：`shutdown_started` 早于第一条 `task_exit`。
        let announced = capture
            .position("shutdown_started")
            .expect("必须公告过停机");
        let first_exit = capture.position("task_exit").expect("必须有任务退出");
        assert!(
            announced < first_exit,
            "有面在公告之前就退了——取消跑到了公告前面\n{}",
            capture.summary()
        );

        // ② 每个在册的面都有一条闭合记录。默认配置是 http + ticker。
        assert_eq!(
            capture.count("task_exit"),
            2,
            "两个面都该留下闭合记录\n{}",
            capture.summary()
        );

        // ③ 存储最后关。
        let last_exit = capture
            .last_position("task_exit")
            .expect("上面刚断言过有退出记录");
        let closed = capture
            .position("storage_close_result")
            .expect("关存储必须留下结论——它能一路决定退出码");
        assert!(
            closed > last_exit,
            "存储在还有面没退完的时候就关了\n{}",
            capture.summary()
        );
        assert_eq!(
            capture.find("storage_close_result")[0].field("outcome"),
            Some("closed"),
            "干净关停时没有别的面再攥着池子，该是 `closed`\n{}",
            capture.summary()
        );

        // ④ 一次干净的关停**不经过** `Forcing`：宽限段结束时册上已经没有任务了。
        //    这一条守的是「`forced` 取自结果而不是取自『调用过 abort_all()』」那次修正
        //   ——按后者算的话，每一次干净关停都会在这里报非零。
        assert_eq!(
            capture.count("task_unreaped"),
            0,
            "干净关停不该有掐不掉的任务\n{}",
            capture.summary()
        );

        // ⑤ 结论本身。日志是给人看的，退出码是给编排系统看的，两边必须说同一件事。
        assert!(
            finished.report.succeeded(),
            "全部面正常退出 + 存储正常关闭 = 一次成功的运行：{:?}",
            finished.report
        );
        assert!(finished.report.unreaped().is_empty());
        assert!(!finished.report.forced());
        assert_eq!(exit_code(&finished.report), ExitCode::SUCCESS);
    }
}
