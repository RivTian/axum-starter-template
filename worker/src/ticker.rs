//! 周期任务面。
//!
//! 这一层是**样板**：它演示一个周期任务面该长什么样，业务内容留给使用者填。要填的地方只有
//! 一个——[`tick`]。其余部分（取消优先、每 tick 现读、先回执后等提交）是纪律，不是示例。
//!
//! # 三条要么全对要么全错的形状
//!
//! 1. **返回 future，不自己 spawn。** `build` 给的是一个 [`PlaneFuture`]，跑在哪个 runtime 上
//!    由 `app` 决定。本 crate 的 `tokio` 依赖里连 `rt` 都没开——能 spawn 就会有人
//!    spawn，少给一个 feature 比写一条纪律可靠。
//! 2. **`select!` 一律 `biased` 且取消在第一臂。** 不写 `biased` 时 tokio 随机挑分支，于是
//!    "收到取消之后还多跑了一次 tick"变成一个按概率出现的现象——这类缺陷在门禁里是查不出来的。
//! 3. **热字段每 tick 现读，不缓存。** 详见下面「间隔什么时候生效」。
//!
//! # `enabled` 不在这一层读
//!
//! `worker.enabled` 是**半热**的：它决定"这个面在不在"，而不是"这个面这一轮做不做事"。
//! 判断因此发生在装配期——`app` 的 `assemble()` 决定要不要登记这个面，关掉时它根本不会被
//! 构造出来。放在这里读的话会得到一个登记了、有 `TaskSpec`、会进关停报告、却什么都不做的
//! 空面，而那正是最难排查的一种"看起来在跑"。
//!
//! # 间隔什么时候生效
//!
//! 每一轮**先读间隔、再睡**。所以改配置之后，正在睡的那一觉不会被叫醒，新间隔从下一轮开始
//! 生效——延迟上界是**一个旧间隔**。
//!
//! 这条上界是故意换来的。要做到"改了立刻生效"就得持有一个可重建的 timer，而风险正落在
//! 那上面：若在回执之后用新配置重建 timer，重建失败就发生在提交之后，于是一个本该拦在
//! 启动期的错误变成了运行期的半死状态。现读只有 `load()` 一个动作，没有可失败的重建步骤，
//! 那一整类问题在结构上不存在。
//!
//! # 这一层信任配置已经钳过
//!
//! `tick_interval` 由三段式管线的 `clamp()` 保证不小于 `MIN_DURATION`（`core` 的
//! `out_of_range_values_are_clamped_and_recorded` 盯着这件事），而启动与重载走的是**同一条**
//! 管线，所以每一份发布出来的快照都钳过。
//!
//! 这里**不**再做一次防御性钳位。理由不是省事：再钳一次就等于"什么算合法间隔"在两个地方
//! 各有一份答案，而两份答案迟早会不一致——到那天，钳位记录说的是一个值、实际睡的是另一个值，
//! 排障的人会先怀疑日志。

use std::time::Duration;

use service_core::config::{ConfigReader, ConfigSnapshot};
use service_core::lifecycle::LifecycleReader;
use service_core::task::{AckSender, PlaneError, PlaneFuture, ShutdownClass, TaskName, TaskSpec};
use tokio_util::sync::CancellationToken;

/// 周期任务面。
///
/// 一个纯命名空间：它没有状态，也不需要被构造出来。面的状态全在 [`build`](Self::build)
/// 返回的那个 future 里，那正是它该待的地方——装配层拿不到它，也就无从在运行期改它。
#[derive(Debug)]
pub struct TickerPlane;

impl TickerPlane {
    /// 本面的登记规格。
    ///
    /// 由**这一层**给出，不是由装配层现编。`TaskSpec` 的文档说得很直接：「"这个面需不需要
    /// 优雅收尾"只有写这个面的人知道」——那就让写这个面的人把答案写在这里，而不是指望
    /// `app` 在登记的时候替它猜一个。
    ///
    /// [`ShutdownClass::Abortable`]：tick 之间没有任何 in-flight 状态，收到取消之后唯一会做
    /// 的事就是立刻返回。给它分配 harvest 预算是纯浪费，而那点预算本可以留给 HTTP 面把
    /// 已经在处理的请求答完。
    ///
    /// 使用者往 [`tick`] 里填了带缓冲的写入之后，**这一行要跟着改成 `Graceful`**。
    pub const SPEC: TaskSpec = TaskSpec::new(TaskName::Ticker, ShutdownClass::Abortable);

    /// 造出这个面的 future。
    ///
    /// 只构造，不 spawn，也不做任何可能失败的事——所以它不返回 `Result`。真正开始跑是
    /// `app` 把它交给 [`TaskSupervisor`](service_core::task::TaskSupervisor) 之后的事。
    #[must_use]
    pub fn build(
        config: ConfigReader,
        lifecycle: LifecycleReader,
        ack: AckSender,
        cancel: CancellationToken,
    ) -> PlaneFuture {
        Box::pin(run(config, lifecycle, ack, cancel))
    }
}

/// 面的本体。
async fn run(
    config: ConfigReader,
    mut lifecycle: LifecycleReader,
    ack: AckSender,
    cancel: CancellationToken,
) -> Result<(), PlaneError> {
    // 回执。这个面**没有什么要准备**——没有端口要 bind，没有连接要建——所以回执是立刻发的。
    //
    // 那为什么还要发？因为提交门的判据是"登记过的面全部回执"这个谓词，谓词的定义域是登记集
    // 而不是"有准备工作的那些面"。让某些面免于回执，就等于把"哪些面算数"变成一份要人维护的
    // 名单，而名单是会漏的。一条立刻发出的回执花不了什么，却让那个谓词保持简单。
    ack.send();

    // 提交之前不做业务。
    //
    // 这里要 `select!` 而不是直接 `await`，是因为启动**失败**路径不发 `Draining`：
    // `abort_boot` 只做"取消 → 短宽限收割 → 关存储"，阶段停在 `Starting` 上。只等阶段的话，
    // 这个面会一直挂着，最后靠 abort 收掉——退出记录就从 `Returned` 变成了 `Cancelled`，
    // 一次干净的启动失败在报告里看起来像一次超时。
    let committed = tokio::select! {
        biased;
        () = cancel.cancelled() => false,
        running = lifecycle.wait_for_running() => running,
    };
    if !committed {
        return Ok(());
    }

    let mut seq: u64 = 0;
    loop {
        // 现读。`load()` 返回的是 `Arc` 的克隆，`watch` 的 guard 在这一行里就还回去了——
        // 跨 `await` 持有它会挡住配置发布端。
        let snapshot = config.load();
        let interval = snapshot.config().worker.tick_interval;

        tokio::select! {
            biased;
            () = cancel.cancelled() => return Ok(()),
            () = tokio::time::sleep(interval) => {}
        }

        // 饱和而不是回绕，理由与 `Generation::next` 同：回绕会让序号倒退，而序号在日志里
        // 的唯一用处就是"看出漏了几拍"。一次溢出要在每秒一 tick 的节奏下跑五千亿年。
        seq = seq.saturating_add(1);

        // 失败就把面结束掉。first-failure 会因此拉起整个进程的关停——这是**故意**
        // 选的响亮默认值。另一种写法（记一条日志然后继续下一轮）会让这个面的 `Result` 变成
        // 一句空话：一个永远不返回 `Err` 的面，和一个已经坏掉但还在转的面，在退出分类里
        // 长得一模一样，而那套四分类存在的全部意义就是把这两者分开。
        //
        // 使用者填进来的活儿如果会**偶发**失败（网络抖动、锁竞争），那属于 `tick` 内部该
        // 自己咽下去的事；能走到这里的，应当是"这个面已经干不了它该干的活了"。
        tick(seq, interval, &snapshot).await?;
    }
}

/// 一次 tick 做的事。**业务内容写在这里。**
///
/// 参数是刻意给全的：`slept` 是这一轮真正睡掉的间隔（不是"当前配置里的间隔"——那两个值在
/// 配置刚改过的那一轮不相等），`snapshot` 是读出这个间隔的那一份配置快照，连生成号一起。
///
/// # Errors
///
/// 返回 `Err` 表示这个面已经无法继续工作，进程会因此进入关停。偶发失败请在这个函数内部
/// 处理掉，不要上抛。
/// # 为什么签名是 `async` 而当前的实现一次 `await` 都没有
///
/// 因为填进来的活儿几乎一定要 `await`。让使用者在写第一行业务之前先把 `fn` 改成 `async fn`、
/// 再回到调用点补一个 `.await`，是一个没有必要的绊脚石。
async fn tick(seq: u64, slept: Duration, snapshot: &ConfigSnapshot) -> Result<(), PlaneError> {
    // 这一层**必须**自己打日志，与 `storage` 那条"一条事件都不发"的纪律正好相反：存储的失败
    // 能通过 `StorageError` 上抛到 `app` 的唯一调用点去记，tick 没有这样的上抛点——不在这里
    // 记就在任何地方都记不到。
    //
    // 级别选 `info` 而不是 `debug`：这是模板里唯一一个"改了配置立刻能观察到效果"的行为，
    // 它就是观察热重载有没有真的生效最省事的入口。一个默认看不见的样板举不了任何证。
    // 使用者把业务填进来之后，把它降成 `debug` 是完全合理的。
    //
    // `generation` 是这条事件里最有用的字段：它说的是"这一拍用的是第几版配置"，于是
    // "reload 报了成功，但面还在用旧值"这件事从日志上直接看得出来，不需要靠间隔去推断。
    tracing::info!(
        name: "ticker_tick",
        seq,
        slept_ms = u64::try_from(slept.as_millis()).unwrap_or(u64::MAX),
        generation = snapshot.generation().get(),
        "periodic worker tick"
    );

    Ok(())
}
