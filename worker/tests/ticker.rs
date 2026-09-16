//! 周期任务面的契约用例。
//!
//! 只用公共 API：`TickerPlane::build` 加 `core` 的那几个句柄。这一层看不见 `tick` 的实现，
//! 也看不见循环内部的任何变量——能断言的只有**外部可观察**的东西：回执有没有发、future
//! 什么时候返回、日志里有几条 `ticker_tick`、每条带的是第几版配置。
//!
//! 全部用例跑在**暂停的时钟**上（`start_paused = true`）。默认间隔是 30 秒，靠真实等待的话
//! 一条用例就得跑半分钟，而且"等多久算够"会随机器负载漂。暂停之后时间只在
//! `tokio::time::advance` 里前进，断言因此是确定性的，不是概率性的。
//!
//! 末尾那行 `use tracing as _;` 是给 `unused_crate_dependencies` 交代的：集成测试目标会链接
//! 本 crate 的**全部**普通依赖，一个没用到就是一条警告，而本工作区 `-D warnings`。`as _`
//! 只满足链接检查，不引入任何可命名的东西——「这个文件自己不发事件、只看别人发了什么」
//! 这条断言原样成立。

use std::time::Duration;

use service_core::config::{
    Config, ConfigPublisher, ConfigReader, HeatDiff, ReloadDecision, evaluate_reload,
};
use service_core::lifecycle::{LifecyclePublisher, Phase};
use service_core::task::{AckReceiver, PlaneFuture, ShutdownClass, TaskName, ack_channel};
use service_testkit::LogCapture;
use service_worker::TickerPlane;
use tokio_util::sync::CancellationToken;
use tracing as _;

/// 用例里统一的 tick 间隔。取整秒是为了让 `advance` 的算术一眼能看懂。
const TICK: Duration = Duration::from_secs(10);

/// 一套刚好够跑起这个面的句柄。
///
/// future 本身**不在**这个结构里：它一开始就要被 `spawn` 走，留在结构里的话，任何一次
/// `&mut rig`（比如重新发布配置）都会撞上"部分移动之后又借用"。分开返回省掉一个 `Option`，
/// 也省掉与之配套的 `unwrap`。
struct Rig {
    ack: AckReceiver,
    cancel: CancellationToken,
    lifecycle: LifecyclePublisher,
    config: ConfigPublisher,
    reader: ConfigReader,
}

impl Rig {
    /// 起一套，间隔为 [`TICK`]。
    fn new() -> (PlaneFuture, Self) {
        let mut initial = Config::default();
        initial.worker.tick_interval = TICK;

        let (config, reader) = ConfigPublisher::new(initial);
        let (lifecycle, lifecycle_reader) = LifecyclePublisher::new();
        let (ack_tx, ack) = ack_channel(TaskName::Ticker);
        let cancel = CancellationToken::new();

        let plane = TickerPlane::build(
            reader.clone(),
            lifecycle_reader,
            ack_tx,
            cancel.child_token(),
        );

        (
            plane,
            Self {
                ack,
                cancel,
                lifecycle,
                config,
                reader,
            },
        )
    }

    /// 发布一份只改了间隔的新配置，返回这次重载有没有被接受。
    ///
    /// 走的是真的 `evaluate_reload`，不是绕过它直接塞值——`tick_interval` 是热的，这次重载
    /// 必须被接受；哪天有人把它改成冷段，调用点会立刻红，而不是让用例继续"通过"。
    ///
    /// 判定用返回值交回调用点，而不是在这里 `panic!`：`clippy.toml` 里那三条
    /// `allow-*-in-tests` 是**结构性**豁免——只认 `#[test]` 函数体和 `#[cfg(test)]` 模块。
    /// 集成测试的辅助函数两样都不占，于是 `-D warnings` 下一句 `panic!` 就是一次门禁失败。
    /// 把断言放回 `#[test]` 里本来也更对：失败位置指向那条用例，而不是指向这个夹具。
    fn republish_interval(&mut self, interval: Duration) -> Result<(), HeatDiff> {
        let mut next = self.reader.load().config().clone();
        next.worker.tick_interval = interval;

        match evaluate_reload(self.reader.load().config(), next) {
            ReloadDecision::Accept { config, .. } => {
                self.config.publish(config);
                Ok(())
            }
            ReloadDecision::RejectCold { diff } => Err(diff),
        }
    }
}

/// 给面一点机会往前跑，但时钟一步都不动。
///
/// `yield_now` 只把当前任务放回队尾；在暂停的时钟上，被 `sleep` 挡住的那个面不会因此多跑
/// 一拍。用它来表达"让已经就绪的部分跑完"，而不是用 `sleep` 去猜一个够用的时长。
async fn settle() {
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
}

#[tokio::test(start_paused = true)]
async fn the_plane_acks_before_the_commit_and_does_nothing_until_running() {
    let capture = LogCapture::start().expect("日志捕获应当可用");
    let (plane, rig) = Rig::new();
    let handle = tokio::spawn(plane);

    settle().await;

    assert!(
        rig.ack.recv().await,
        "回执必须在提交之前就到——提交门等的就是它"
    );
    assert_eq!(
        rig.lifecycle.current(),
        Phase::Starting,
        "面发完回执不该顺手把阶段推到 Running：那是提交门的决定"
    );

    // 还没提交，时钟走过好几个间隔也不该有 tick。
    tokio::time::advance(TICK * 5).await;
    settle().await;
    assert_eq!(
        capture.count("ticker_tick"),
        0,
        "提交之前不处理业务；实际捕获到：{}",
        capture.summary()
    );

    rig.cancel.cancel();
    let outcome = handle.await.expect("面不该 panic");
    assert!(outcome.is_ok(), "取消是正常收尾，不是失败：{outcome:?}");
}

#[tokio::test(start_paused = true)]
async fn a_boot_that_never_publishes_running_still_lets_the_plane_return_cleanly() {
    // `abort_boot` 的路径：只取消，**不发** `Draining`，阶段停在 `Starting` 上。
    // 只等阶段的写法会在这里永远挂着，最后靠 abort 收掉——一次干净的启动失败会在退出报告里
    // 变成 `Cancelled`，看起来像超时。
    let (plane, rig) = Rig::new();
    let handle = tokio::spawn(plane);

    settle().await;
    rig.cancel.cancel();

    let outcome = tokio::time::timeout(Duration::from_secs(1), handle)
        .await
        .expect("面必须自己返回，而不是等着被 abort")
        .expect("面不该 panic");
    assert!(outcome.is_ok(), "启动失败路径上的收尾是 Ok：{outcome:?}");
    assert_eq!(
        rig.lifecycle.current(),
        Phase::Starting,
        "这条用例的前提就是阶段没有越过 Starting"
    );
}

#[tokio::test(start_paused = true)]
async fn shutdown_while_draining_is_also_a_clean_return() {
    // 正常关停路径：先 `publish(Draining)` 再 `cancel()`。等阶段的那一臂会先醒，
    // 拿到 `false`。两条路径都必须返回 `Ok`，否则关停报告里会凭空多出一条失败。
    let (plane, rig) = Rig::new();
    let handle = tokio::spawn(plane);

    settle().await;
    rig.lifecycle.publish(Phase::Draining);

    let outcome = tokio::time::timeout(Duration::from_secs(1), handle)
        .await
        .expect("公告 Draining 之后面应当自己返回")
        .expect("面不该 panic");
    assert!(outcome.is_ok(), "{outcome:?}");
}

#[tokio::test(start_paused = true)]
async fn it_ticks_once_per_interval_and_never_before_the_first_one_elapses() {
    let capture = LogCapture::start().expect("日志捕获应当可用");
    let (plane, rig) = Rig::new();
    let handle = tokio::spawn(plane);

    settle().await;
    rig.lifecycle.publish(Phase::Running);
    settle().await;

    // 先睡后 tick：提交的那一瞬间不该有一拍。理由是启动那一刻整个进程都还在落位，
    // 一个在 t=0 就开工的后台任务会和别的启动步骤抢同一批资源。
    assert_eq!(capture.count("ticker_tick"), 0, "提交瞬间不该 tick");

    for expected in 1..=3_u64 {
        tokio::time::advance(TICK).await;
        settle().await;
        assert_eq!(
            capture.count("ticker_tick"),
            usize::try_from(expected).expect("小计数"),
            "第 {expected} 个间隔之后应当正好 {expected} 拍；实际：{}",
            capture.summary()
        );
    }

    let ticks = capture.find("ticker_tick");
    assert_eq!(ticks[0].field("seq"), Some("1"), "序号从 1 起，逐拍加一");
    assert_eq!(ticks[2].field("seq"), Some("3"));
    assert_eq!(
        ticks[0].field("slept_ms"),
        Some("10000"),
        "`slept_ms` 记的是这一拍真正睡掉的间隔"
    );

    rig.cancel.cancel();
    handle.await.expect("面不该 panic").expect("收尾应当是 Ok");
}

#[tokio::test(start_paused = true)]
async fn a_reloaded_interval_takes_effect_after_the_round_that_already_read_the_old_one() {
    // 这条用例钉的是「延迟上界是**一个旧间隔**」的具体形状，不是一句含糊的"下一轮生效"。
    //
    // 循环是先读后睡，所以在任一时刻，已经睡下的那一轮**早就把旧值取走了**。重载不叫醒它，
    // 于是这一轮仍按旧间隔走完——真正用上新值的是它之后的那一轮。换句话说，最坏情况下新值
    // 迟到一个旧间隔，而这正是先读后睡换来的那件事：`load()` 之外没有任何可失败的重建步骤。
    let capture = LogCapture::start().expect("日志捕获应当可用");
    let (plane, mut rig) = Rig::new();
    let handle = tokio::spawn(plane);

    settle().await;
    rig.lifecycle.publish(Phase::Running);
    settle().await;

    // 第 1 拍：初始间隔。这一拍落下之后，第 2 轮立刻读走旧值并睡下。
    tokio::time::advance(TICK).await;
    settle().await;
    assert_eq!(capture.count("ticker_tick"), 1);

    // 改成十分之一。此刻第 2 轮已经在睡，它拿的是旧值。
    let faster = TICK / 10;
    assert_eq!(
        rig.republish_interval(faster),
        Ok(()),
        "worker.tick_interval 应当是热字段，却被判成冷段变更"
    );

    // 按**新**间隔推进一格：什么都不该发生——正在睡的那一轮还差得远。
    // 没有这一段断言的话，"改了立刻生效"和"迟到一个旧间隔"在用例里长得一样。
    tokio::time::advance(faster).await;
    settle().await;
    assert_eq!(
        capture.count("ticker_tick"),
        1,
        "重载不叫醒已经睡下的那一轮；实际：{}",
        capture.summary()
    );

    // 把旧间隔剩下的部分走完：第 2 拍按**旧**间隔落下，生成号仍是旧的。
    tokio::time::advance(TICK - faster).await;
    settle().await;
    assert_eq!(
        capture.count("ticker_tick"),
        2,
        "旧间隔走完时第 2 拍必须落下；实际：{}",
        capture.summary()
    );

    // 第 3 拍：这一轮才是"重载之后开始的那一轮"，新间隔在这里生效。
    tokio::time::advance(faster).await;
    settle().await;
    assert_eq!(
        capture.count("ticker_tick"),
        3,
        "新间隔应当在下一轮生效；实际：{}",
        capture.summary()
    );

    let ticks = capture.find("ticker_tick");
    assert_eq!(
        (ticks[0].field("generation"), ticks[0].field("slept_ms")),
        (Some("0"), Some("10000")),
        "第 1 拍读的是启动那一版配置"
    );
    assert_eq!(
        (ticks[1].field("generation"), ticks[1].field("slept_ms")),
        (Some("0"), Some("10000")),
        "第 2 拍在重载**之前**就把旧值取走了，它报的必须还是旧生成号——\
         这里要是变成 1，说明有人给循环加了「睡到一半重读」的花样"
    );
    assert_eq!(
        (ticks[2].field("generation"), ticks[2].field("slept_ms")),
        (Some("1"), Some("1000")),
        "第 3 拍必须同时报出新生成号和新间隔——\
         「reload 说成功了、面还在用旧值」这件事就是靠这两个字段看出来的"
    );

    rig.cancel.cancel();
    handle.await.expect("面不该 panic").expect("收尾应当是 Ok");
}

#[tokio::test(start_paused = true)]
async fn cancellation_wins_over_a_tick_that_is_due_at_the_same_instant() {
    // `biased` 的落点。不写 `biased` 时 tokio 随机挑分支，于是"取消之后还多跑了一拍"会变成
    // 一个按概率出现的现象——那种缺陷在门禁里是查不出来的。
    let capture = LogCapture::start().expect("日志捕获应当可用");
    let (plane, rig) = Rig::new();
    let handle = tokio::spawn(plane);

    settle().await;
    rig.lifecycle.publish(Phase::Running);
    settle().await;

    // 把时钟推到 sleep 的截止点，但**不给面运行的机会**，紧接着就取消。
    // 两个分支同时就绪，`biased` 要求取消赢。
    tokio::time::advance(TICK).await;
    rig.cancel.cancel();
    settle().await;

    let outcome = tokio::time::timeout(Duration::from_secs(1), handle)
        .await
        .expect("取消之后面应当立刻返回")
        .expect("面不该 panic");
    assert!(outcome.is_ok(), "{outcome:?}");
    assert_eq!(
        capture.count("ticker_tick"),
        0,
        "取消与到点同时就绪时不该再跑业务；实际：{}",
        capture.summary()
    );
}

#[test]
fn the_spec_says_abortable_and_the_plane_owns_that_answer() {
    // 规格由这一层给出而不是由装配层现编：「"这个面需不需要优雅收尾"只有写这个面的人知道」。
    // 哪天 `tick` 里填进了带缓冲的写入，改这一行的人就被迫重新想一遍关停预算。
    assert_eq!(TickerPlane::SPEC.name, TaskName::Ticker);
    assert_eq!(TickerPlane::SPEC.class, ShutdownClass::Abortable);
    assert!(
        !TickerPlane::SPEC.class.gets_harvest_budget(),
        "tick 之间没有 in-flight 状态，给它 harvest 预算是从 HTTP 面嘴里抢"
    );
}
