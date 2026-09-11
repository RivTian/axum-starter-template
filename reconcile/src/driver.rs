//! 对账驱动循环：事件唤醒 + 定时兜底 + 期望集读取，写一次、测一次。
//!
//! # 为什么循环收进框架，而不是留给各面手写
//!
//! 这段循环手写三遍会得到三份形态一致的代码（先干活后等待、读期望集带超时、
//! 读失败跳轮）。引入事件唤醒后，它成了最容易写错的部分：无关事件一旦落回
//! 外层循环就会触发一轮全量对账，而本面每次写库都发事件时，就形成「每轮
//! 写入 → 一次全量巡检」的自激热循环。把循环收进一处配齐测试，面从此只提供
//! 期望集来源与唤醒谓词，不再拥有循环本身。
//!
//! # 循环骨架
//!
//! - **先干活后等待**：启动即对账一轮；
//! - **读期望集带超时**（缺省 5s），超时或失败即跳过本轮，在册单元原样保留
//!   （坏一拍不动现状，下一拍重试）；
//! - **等待期事件只当闹钟**：只有 `wake` 谓词判真的事件才提前唤醒；其余事件
//!   既不唤醒也不重置兜底期限——期限按进入等待时算好的 deadline 计，不随事件
//!   重算，防自激也防饥饿；
//! - **唤醒即合流**：把队列里攒着的事件一次吸干，一阵突发只换一轮对账；
//! - **取消只在读期望集与等待两处生效**：对账执行不被中途打断——半执行的对账
//!   会让在册表与真实任务脱节（取消最多等一轮对账做完）。

use std::time::Duration;

use tokio::time::{Instant, sleep_until, timeout};
use tokio_util::sync::CancellationToken;

use {{crate_prefix_snake}}_core::events::{AppEvent, EventBus, EventStream};

use crate::manager::{ReconcileReport, UnitManager};
use crate::unit::{UnitFactory, UnitSpec};

/// 期望集来源：接入驱动循环的面唯一要实现的东西。
///
/// 实现者持有自己的依赖（存储句柄、配置读端……）并可以带内部状态
/// （方法都拿 `&mut self`，像「每小时才做一次的周期清扫」这种节流状态
/// 直接放字段里）。
pub trait UnitSource: Send {
    type Spec: UnitSpec;

    /// 读出当前期望集。`Err` → 本轮跳过、在册单元不动，下一拍重试。
    fn list_desired(
        &mut self,
    ) -> impl Future<Output = Result<Vec<Self::Spec>, anyhow::Error>> + Send;

    /// 兜底巡检间隔。每轮询问一次——热重载改掉的扫描周期从下一轮生效。
    fn tick_interval(&mut self) -> Duration;

    /// 每轮对账后的附加动作（如过期检查点清扫）。默认什么都不做。
    ///
    /// 读期望集失败的轮次也会被调用（传入空报告）：这里承担的往往是时钟类
    /// 职责，不因坏库停摆——口径是「失败也推进时钟，坏库每小时告警一次即可，
    /// 不必每拍刷屏」。
    fn after_round(
        &mut self,
        report: &ReconcileReport<<Self::Spec as UnitSpec>::Key>,
    ) -> impl Future<Output = ()> + Send {
        let _ = report;
        async {}
    }
}

/// 驱动参数。
#[derive(Debug, Clone)]
pub struct DriveOptions {
    /// 本面标识，只进日志。**唤醒条件由 `drive` 的 `wake` 谓词决定，不看这个字段**
    pub plane: &'static str,
    /// 期望集读取超时
    pub list_timeout: Duration,
}

impl DriveOptions {
    pub fn new(plane: &'static str) -> Self {
        Self {
            plane,
            list_timeout: Duration::from_secs(5),
        }
    }
}

/// 最常用的唤醒谓词：只认本面的 [`AppEvent::DesiredSetChanged`]。
///
/// 单独给一个构造器，是因为手写这个闭包时最容易写成「见事件就唤醒」，
/// 而那正是自激热循环的写法（见模块文档）。需要别的唤醒源（例如把
/// `ConfigReloaded` 也算进来）时再自己写谓词。
pub fn wake_on_plane(plane: &'static str) -> impl Fn(&AppEvent) -> bool {
    move |ev| matches!(ev, AppEvent::DesiredSetChanged { plane: changed } if *changed == plane)
}

/// 驱动一个 [`UnitManager`] 直到取消或事件总线销毁。
///
/// `wake` 判真的事件提前唤醒对账；判假的事件既不唤醒也不重置兜底期限。
/// 常规用法传 [`wake_on_plane`]。
///
/// 返回后管理器仍由调用方持有——关停序列由调用方走
/// [`UnitManager::shutdown`]，驱动循环不代劳（它不知道宽限期该多长）。
pub async fn drive<F, S, W>(
    manager: &mut UnitManager<F>,
    source: &mut S,
    options: &DriveOptions,
    events: &EventBus,
    cancel: &CancellationToken,
    mut wake: W,
) where
    F: UnitFactory,
    S: UnitSource<Spec = F::Spec>,
    W: FnMut(&AppEvent) -> bool,
{
    let mut stream = events.subscribe();

    loop {
        // ── 干活相：读期望集 → 对账 → 轮后钩子 ──
        // 取消只与「读期望集」竞速：对账一旦开始就做完，半执行的对账比慢一拍更糟
        let listed = tokio::select! {
            biased;
            _ = cancel.cancelled() => break,
            r = timeout(options.list_timeout, source.list_desired()) => r,
        };
        match listed {
            Ok(Ok(desired)) => {
                let report = manager.reconcile(desired).await;
                source.after_round(&report).await;
            }
            Ok(Err(error)) => {
                tracing::warn!(
                    plane = options.plane,
                    error = %error,
                    "desired-set listing failed; keeping units as they are this round"
                );
                source.after_round(&ReconcileReport::default()).await;
            }
            Err(_) => {
                tracing::warn!(
                    plane = options.plane,
                    timeout = ?options.list_timeout,
                    "desired-set listing timed out; keeping units as they are this round"
                );
                source.after_round(&ReconcileReport::default()).await;
            }
        }

        // ── 等待相：deadline 只算一次，事件只能把等待缩短，不能拉长 ──
        let deadline = Instant::now() + source.tick_interval();
        match wait_for_wake(&mut stream, deadline, &mut wake, cancel).await {
            WaitOutcome::Exit => return,
            WaitOutcome::Tick | WaitOutcome::Wake => {}
        }
    }
}

/// 等待相的裁决。
#[derive(Debug)]
enum WaitOutcome {
    /// 兜底 tick 到点
    Tick,
    /// 被谓词判真的事件唤醒（队列已吸干）
    Wake,
    /// 该退出了：取消令牌触发，或事件总线销毁（进程已在关停途中）
    Exit,
}

/// 等到下一个该对账的时刻，或裁决退出。
///
/// 单独成函有两个理由：嵌套 select 拍平成平铺的裁决枚举；「总线销毁 →
/// 退出」这条分支需要一个能让全部发送端消失的测试装置——驱动循环层面
/// 做不到（调用方必然还握着总线），只有在这一层能直测。
async fn wait_for_wake<W>(
    stream: &mut EventStream,
    deadline: Instant,
    wake: &mut W,
    cancel: &CancellationToken,
) -> WaitOutcome
where
    W: FnMut(&AppEvent) -> bool,
{
    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => return WaitOutcome::Exit,
            _ = sleep_until(deadline) => return WaitOutcome::Tick,
            ev = stream.recv() => match ev {
                None => return WaitOutcome::Exit,
                Some(ev) if wake(&ev) => {
                    // 合流：吸干已排队的事件，让突发只换一轮对账。
                    // 顺带丢弃的无关事件本就不该唤醒这条循环
                    while stream.try_recv().is_some() {}
                    return WaitOutcome::Wake;
                }
                // 谓词判假（别面变更、热重载通知等）：继续等。不重置
                // deadline——事件自激在这一行被堵死
                Some(_) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::plan::RestartPolicy;
    use crate::unit::{UnitCtx, UnitExit};

    /// 被驱动的那个面
    const PLANE: &str = "units";
    /// 另一个面：它的事件不该唤醒本面
    const OTHER_PLANE: &str = "other";

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct TestSpec {
        key: String,
        version: u32,
    }

    impl UnitSpec for TestSpec {
        type Key = String;

        fn key(&self) -> String {
            self.key.clone()
        }

        fn same_as(&self, other: &Self) -> bool {
            self == other
        }
    }

    fn spec(key: &str, version: u32) -> TestSpec {
        TestSpec {
            key: key.to_owned(),
            version,
        }
    }

    /// version == 0 的规格立刻异常退出，其余一直跑到被取消
    #[derive(Default)]
    struct TestFactory {
        spawns: Arc<Mutex<HashMap<String, u32>>>,
    }

    impl UnitFactory for TestFactory {
        type Spec = TestSpec;

        fn spawn_unit(
            &self,
            spec: TestSpec,
            ctx: UnitCtx,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = UnitExit> + Send>> {
            *self.spawns.lock().unwrap().entry(spec.key()).or_insert(0) += 1;
            Box::pin(async move {
                if spec.version == 0 {
                    UnitExit::Failed(anyhow::anyhow!("scripted failure"))
                } else {
                    ctx.cancel.cancelled().await;
                    UnitExit::Normal
                }
            })
        }
    }

    struct TestSource {
        specs: Arc<Mutex<Vec<TestSpec>>>,
        lists: Arc<AtomicU32>,
        after_rounds: Arc<AtomicU32>,
        gave_up_seen: Arc<Mutex<Vec<String>>>,
        fail_first: Arc<AtomicBool>,
        hang_first: Arc<AtomicBool>,
        tick: Duration,
    }

    impl TestSource {
        fn new(tick: Duration) -> Self {
            Self {
                specs: Arc::new(Mutex::new(Vec::new())),
                lists: Arc::new(AtomicU32::new(0)),
                after_rounds: Arc::new(AtomicU32::new(0)),
                gave_up_seen: Arc::new(Mutex::new(Vec::new())),
                fail_first: Arc::new(AtomicBool::new(false)),
                hang_first: Arc::new(AtomicBool::new(false)),
                tick,
            }
        }
    }

    impl UnitSource for TestSource {
        type Spec = TestSpec;

        async fn list_desired(&mut self) -> Result<Vec<TestSpec>, anyhow::Error> {
            self.lists.fetch_add(1, Ordering::SeqCst);
            if self.hang_first.swap(false, Ordering::SeqCst) {
                std::future::pending::<()>().await;
            }
            if self.fail_first.swap(false, Ordering::SeqCst) {
                anyhow::bail!("scripted list failure");
            }
            Ok(self.specs.lock().unwrap().clone())
        }

        fn tick_interval(&mut self) -> Duration {
            self.tick
        }

        async fn after_round(&mut self, report: &ReconcileReport<String>) {
            self.after_rounds.fetch_add(1, Ordering::SeqCst);
            self.gave_up_seen
                .lock()
                .unwrap()
                .extend(report.gave_up.iter().cloned());
        }
    }

    struct Harness {
        lists: Arc<AtomicU32>,
        after_rounds: Arc<AtomicU32>,
        gave_up_seen: Arc<Mutex<Vec<String>>>,
        spawns: Arc<Mutex<HashMap<String, u32>>>,
        events: EventBus,
        cancel: CancellationToken,
        handle: tokio::task::JoinHandle<UnitManager<TestFactory>>,
    }

    fn launch(source: TestSource, max_deaths: u32) -> Harness {
        launch_with(source, max_deaths, wake_on_plane(PLANE))
    }

    fn launch_with(
        source: TestSource,
        max_deaths: u32,
        wake: impl FnMut(&AppEvent) -> bool + Send + 'static,
    ) -> Harness {
        let factory = TestFactory::default();
        let spawns = factory.spawns.clone();
        // 退避基数取零：单元的 next_restart_at 用的是墙钟（now_ms），而本模块
        // 测试跑在 tokio 模拟时钟上——几十个虚拟 tick 在真实世界只过去几百微秒，
        // 任何非零的真实退避窗口都会把重建挡在所有虚拟轮次之外。退避的时序
        // 语义由 plan 层的纯函数测试负责，这里只验证循环本身
        let policy = RestartPolicy {
            max_deaths,
            backoff_base: Duration::ZERO,
            backoff_cap: Duration::from_millis(8),
            reap_timeout: Duration::from_secs(1),
        };
        let mut manager = UnitManager::new(factory, policy, None, CancellationToken::new());
        let events = EventBus::new(64);
        let cancel = CancellationToken::new();

        let lists = source.lists.clone();
        let after_rounds = source.after_rounds.clone();
        let gave_up_seen = source.gave_up_seen.clone();
        let events_for_task = events.clone();
        let cancel_for_task = cancel.clone();
        let handle = tokio::spawn(async move {
            let mut source = source;
            let options = DriveOptions::new(PLANE);
            drive(
                &mut manager,
                &mut source,
                &options,
                &events_for_task,
                &cancel_for_task,
                wake,
            )
            .await;
            manager
        });

        Harness {
            lists,
            after_rounds,
            gave_up_seen,
            spawns,
            events,
            cancel,
            handle,
        }
    }

    /// 纯让出调度权地等待计数到达期望值：不碰时钟，够不到就 panic
    async fn spin_until(counter: &AtomicU32, at_least: u32) {
        for _ in 0..500 {
            if counter.load(Ordering::SeqCst) >= at_least {
                return;
            }
            tokio::task::yield_now().await;
        }
        panic!(
            "counter stuck at {} (< {at_least})",
            counter.load(Ordering::SeqCst)
        );
    }

    /// 启动即对账一轮；之后按 tick 兜底推进
    #[tokio::test(start_paused = true)]
    async fn first_round_is_immediate_and_ticks_follow() {
        let h = launch(TestSource::new(Duration::from_secs(60)), 5);

        spin_until(&h.lists, 1).await;
        assert_eq!(h.lists.load(Ordering::SeqCst), 1, "启动只对账一轮");

        tokio::time::sleep(Duration::from_secs(61)).await;
        assert_eq!(
            h.lists.load(Ordering::SeqCst),
            2,
            "60s 兜底 tick 应触发第二轮"
        );

        h.cancel.cancel();
        let manager = h.handle.await.unwrap();
        assert!(manager.is_empty());
    }

    /// 谓词判真的事件提前唤醒：时钟一格未走，第二轮已经发生
    #[tokio::test(start_paused = true)]
    async fn a_relevant_event_wakes_the_loop_without_the_tick() {
        let h = launch(TestSource::new(Duration::from_secs(3600)), 5);
        spin_until(&h.lists, 1).await;

        let before = Instant::now();
        h.events
            .publish(AppEvent::DesiredSetChanged { plane: PLANE });
        spin_until(&h.lists, 2).await;
        assert_eq!(Instant::now(), before, "事件唤醒不应消耗任何模拟时钟");

        h.cancel.cancel();
        h.handle.await.unwrap();
    }

    /// 谓词判假的事件不唤醒、也不重置兜底期限
    #[tokio::test(start_paused = true)]
    async fn irrelevant_events_neither_wake_nor_starve_the_loop() {
        let h = launch(TestSource::new(Duration::from_secs(60)), 5);
        spin_until(&h.lists, 1).await;

        for _ in 0..5 {
            h.events
                .publish(AppEvent::DesiredSetChanged { plane: OTHER_PLANE });
            h.events.publish(AppEvent::ConfigReloaded { generation: 1 });
        }
        for _ in 0..200 {
            tokio::task::yield_now().await;
        }
        assert_eq!(
            h.lists.load(Ordering::SeqCst),
            1,
            "别面事件与热重载通知不得触发本面对账"
        );

        // deadline 不被无关事件重置：从进入等待算起 60s 兜底照常触发
        tokio::time::sleep(Duration::from_secs(61)).await;
        assert_eq!(h.lists.load(Ordering::SeqCst), 2);

        h.cancel.cancel();
        h.handle.await.unwrap();
    }

    /// 唤醒源由谓词说了算，不是由事件变体说了算：这里只认 `ConfigReloaded`，
    /// 于是本面自己的 `DesiredSetChanged` 反而唤不醒循环
    #[tokio::test(start_paused = true)]
    async fn a_custom_predicate_decides_what_wakes_the_loop() {
        let h = launch_with(TestSource::new(Duration::from_secs(3600)), 5, |ev| {
            matches!(ev, AppEvent::ConfigReloaded { .. })
        });
        spin_until(&h.lists, 1).await;

        h.events
            .publish(AppEvent::DesiredSetChanged { plane: PLANE });
        for _ in 0..200 {
            tokio::task::yield_now().await;
        }
        assert_eq!(
            h.lists.load(Ordering::SeqCst),
            1,
            "谓词判假的事件不唤醒，哪怕它是本面的期望集变更"
        );

        let before = Instant::now();
        h.events.publish(AppEvent::ConfigReloaded { generation: 7 });
        spin_until(&h.lists, 2).await;
        assert_eq!(Instant::now(), before, "谓词判真的事件即刻唤醒");

        h.cancel.cancel();
        h.handle.await.unwrap();
    }

    /// 事件突发合流：十连发只换一轮对账
    #[tokio::test(start_paused = true)]
    async fn a_burst_of_events_coalesces_into_one_round() {
        let h = launch(TestSource::new(Duration::from_secs(3600)), 5);
        spin_until(&h.lists, 1).await;

        for _ in 0..10 {
            h.events
                .publish(AppEvent::DesiredSetChanged { plane: PLANE });
        }
        spin_until(&h.lists, 2).await;
        for _ in 0..200 {
            tokio::task::yield_now().await;
        }
        assert_eq!(h.lists.load(Ordering::SeqCst), 2, "十个事件应合流成一轮");

        h.cancel.cancel();
        h.handle.await.unwrap();
    }

    /// 读期望集失败：本轮跳过、在册不动、after_round 照常推进（失败也推进时钟）
    #[tokio::test(start_paused = true)]
    async fn a_failed_listing_skips_the_round_but_keeps_the_clock() {
        let source = TestSource::new(Duration::from_millis(10));
        source.fail_first.store(true, Ordering::SeqCst);
        source.specs.lock().unwrap().push(spec("a", 1));
        let h = launch(source, 5);

        spin_until(&h.lists, 1).await;
        assert_eq!(
            h.after_rounds.load(Ordering::SeqCst),
            1,
            "失败轮也要走 after_round"
        );

        tokio::time::sleep(Duration::from_millis(15)).await;
        spin_until(&h.lists, 2).await;

        h.cancel.cancel();
        let manager = h.handle.await.unwrap();
        assert_eq!(manager.len(), 1, "第二轮成功后单元应已建立");
        assert_eq!(h.spawns.lock().unwrap()["a"], 1);
    }

    /// 读期望集超时：跳过本轮，下一拍重试成功
    #[tokio::test(start_paused = true)]
    async fn a_hanging_listing_times_out_and_the_next_round_recovers() {
        let source = TestSource::new(Duration::from_millis(10));
        source.hang_first.store(true, Ordering::SeqCst);
        source.specs.lock().unwrap().push(spec("a", 1));
        let h = launch(source, 5);

        // 5s 读取超时 + 10ms tick 之后进入第二轮
        tokio::time::sleep(Duration::from_secs(6)).await;
        spin_until(&h.lists, 2).await;

        h.cancel.cancel();
        let manager = h.handle.await.unwrap();
        assert_eq!(manager.len(), 1);
    }

    /// 放弃边沿穿过报告抵达 after_round，且只出现一次
    #[tokio::test(start_paused = true)]
    async fn the_gave_up_edge_reaches_after_round_exactly_once() {
        let source = TestSource::new(Duration::from_millis(10));
        source.specs.lock().unwrap().push(spec("a", 0)); // version 0 → 立刻失败
        let h = launch(source, 1);

        tokio::time::sleep(Duration::from_millis(200)).await;
        let seen = h.gave_up_seen.lock().unwrap().clone();
        assert_eq!(
            seen,
            vec!["a".to_owned()],
            "放弃边沿应恰好报告一次 (lists={}, after_rounds={}, spawns={:?})",
            h.lists.load(Ordering::SeqCst),
            h.after_rounds.load(Ordering::SeqCst),
            h.spawns.lock().unwrap().get("a")
        );

        h.cancel.cancel();
        h.handle.await.unwrap();
    }

    /// 总线销毁（全部发送端消失）让等待相立即裁决退出
    #[tokio::test]
    async fn bus_destruction_resolves_the_wait_phase_to_exit() {
        let bus = EventBus::new(4);
        let mut stream = bus.subscribe();
        drop(bus);

        let outcome = wait_for_wake(
            &mut stream,
            Instant::now() + Duration::from_secs(3600),
            &mut wake_on_plane(PLANE),
            &CancellationToken::new(),
        )
        .await;
        assert!(matches!(outcome, WaitOutcome::Exit), "got {outcome:?}");
    }

    /// 取消令牌让驱动在等待期立即退出
    #[tokio::test(start_paused = true)]
    async fn cancellation_exits_the_wait_phase_promptly() {
        let h = launch(TestSource::new(Duration::from_secs(3600)), 5);
        spin_until(&h.lists, 1).await;

        let before = Instant::now();
        h.cancel.cancel();
        h.handle.await.unwrap();
        assert_eq!(Instant::now(), before, "退出不应等待兜底 tick");
    }
}
