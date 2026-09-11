//! 单元管理器：把对账清单落到真实任务上。
//!
//! 决策全在 [`plan_reconcile`]，这里只剩执行与记账：spawn、取消、收割、
//! 死亡计数与清零。两者分开后，本文件剩下的每一处 `await` 都是机械动作。
//!
//! # 收割超时即强杀（既定语义，有测试钉死）
//!
//! 收割超时只告警放任是个诱人的选项，但未收割的句柄一旦丢下，紧接着又为
//! 同一 key 拉起新化身，同一单元就有两个实例同时在跑——对轮询面是重复打同
//! 一个对端，对会话面是两个连接抢同一个身份。这里超时即强杀（abort）：宁可
//! 粗暴收场，不留双化身。口径与 core 的关停收敛一致（宽限期后 abort）。

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::task::{AbortHandle, JoinHandle};
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

use {{crate_prefix_snake}}_core::util::{TimestampMs, now_ms};

use crate::plan::{ReconcilePlan, RestartPolicy, RestartReason, UnitView, plan_reconcile};
use crate::unit::{UnitCtx, UnitExit, UnitFactory, UnitSpec, UnitStatus};

type KeyOf<F> = <<F as UnitFactory>::Spec as UnitSpec>::Key;

/// 一轮对账实际执行了什么。各列表按 key 升序（继承自计划的排序）。
///
/// 这是消费方获知**边沿事件**的唯一渠道：`gave_up` 里的 key 恰在本轮跨过
/// 放弃阈值，且只出现这一次；靠轮询快照对比拿不回这个时点。需要「放弃直到
/// 配置变更」这类边沿语义的面（例如把放弃状态写进对外的状态注册表）就消费
/// 这个字段，不要自己去比快照。
#[derive(Debug)]
pub struct ReconcileReport<K> {
    pub started: Vec<K>,
    pub stopped: Vec<K>,
    pub restarted: Vec<K>,
    /// 本轮刚达到放弃阈值的单元（最后一个化身仍会被拉起，此后不再重建）
    pub gave_up: Vec<K>,
    /// 因并发上限被推迟的单元（不烧死亡计数，等容量释放后的巡检再试）
    pub deferred: Vec<K>,
}

// 手写而不 derive：derive 会给 K 加 Default 约束
impl<K> Default for ReconcileReport<K> {
    fn default() -> Self {
        Self {
            started: Vec::new(),
            stopped: Vec::new(),
            restarted: Vec::new(),
            gave_up: Vec::new(),
            deferred: Vec::new(),
        }
    }
}

impl<K> ReconcileReport<K> {
    /// 本轮什么都没做（稳态）。
    pub fn is_noop(&self) -> bool {
        self.started.is_empty()
            && self.stopped.is_empty()
            && self.restarted.is_empty()
            && self.gave_up.is_empty()
            && self.deferred.is_empty()
    }
}

/// 在册单元的完整记录。
///
/// `cancel` 与 `handle` 刻意不外泄：`alive` 判定依赖「取消只由管理器发出、
/// 发出后立即把记录移出在册表」这一条件，任何外部持有者都能悄悄打破它。
struct RunningUnit<S> {
    spec: S,
    cancel: CancellationToken,
    handle: JoinHandle<UnitExit>,
    deaths: u32,
    next_restart_at_ms: TimestampMs,
    progress: Arc<AtomicU64>,
}

/// 一组同类单元的监管者。
///
/// 不含驱动循环——什么时候对账由 [`drive`] 决定，这里只提供「给我期望集，
/// 我把现实收敛过去」这一个动作。除 `snapshot` 外全部方法都要 `&mut`，
/// 天然单写者，无锁。
///
/// [`drive`]: crate::drive
pub struct UnitManager<F: UnitFactory> {
    factory: F,
    policy: RestartPolicy,
    max_units: Option<usize>,
    parent_cancel: CancellationToken,
    running: HashMap<KeyOf<F>, RunningUnit<F::Spec>>,
}

impl<F: UnitFactory> UnitManager<F> {
    /// `max_units`：`None` 不设上限，适合单元数由配置枚举、天然有界的面；
    /// `Some(n)` 上限 n，适合单元数随库表增长的面（值取该面配置的并发上限）。
    pub fn new(
        factory: F,
        policy: RestartPolicy,
        max_units: Option<usize>,
        parent_cancel: CancellationToken,
    ) -> Self {
        Self {
            factory,
            policy,
            max_units,
            parent_cancel,
            running: HashMap::new(),
        }
    }

    /// 在册单元数（含已死但仍在退避或已被放弃的）。
    pub fn len(&self) -> usize {
        self.running.len()
    }

    pub fn is_empty(&self) -> bool {
        self.running.is_empty()
    }

    /// 按 key 升序的只读快照。
    pub fn snapshot(&self) -> Vec<UnitStatus> {
        let mut out: Vec<UnitStatus> = self
            .running
            .iter()
            .map(|(key, unit)| UnitStatus {
                key: key.to_string(),
                alive: !unit.handle.is_finished(),
                deaths: unit.deaths,
                next_restart_at_ms: unit.next_restart_at_ms,
            })
            .collect();
        out.sort_by(|a, b| a.key.cmp(&b.key));
        out
    }

    /// 把在册单元收敛到 `desired`，返回本轮实际执行的动作。
    pub async fn reconcile(&mut self, desired: Vec<F::Spec>) -> ReconcileReport<KeyOf<F>> {
        let desired_map = dedup_desired(desired);
        self.clear_recovered();

        let views: HashMap<KeyOf<F>, UnitView<F::Spec>> = self
            .running
            .iter()
            .map(|(key, unit)| {
                (
                    key.clone(),
                    UnitView {
                        spec: unit.spec.clone(),
                        alive: !unit.handle.is_finished(),
                        deaths: unit.deaths,
                        next_restart_at_ms: unit.next_restart_at_ms,
                    },
                )
            })
            .collect();

        let plan = plan_reconcile::<F::Spec>(
            &desired_map,
            &views,
            now_ms(),
            self.policy.max_deaths,
            self.max_units,
        );
        if plan.is_noop() {
            return ReconcileReport::default();
        }

        // 先向所有待停单元发取消，再统一限时收割。逐个「取消并等待」会让总耗时
        // 变成各单元收尾时间之和，一个慢单元就能拖住整轮对账
        let mut prev_deaths: HashMap<KeyOf<F>, u32> = HashMap::new();
        let mut reaping: Vec<(KeyOf<F>, JoinHandle<UnitExit>)> = Vec::new();
        let mut aborts = Vec::new();
        let stopping = plan
            .stop
            .iter()
            .cloned()
            .chain(plan.restart.iter().map(|(key, _, _)| key.clone()));
        for key in stopping {
            if let Some(unit) = self.running.remove(&key) {
                unit.cancel.cancel();
                prev_deaths.insert(key.clone(), unit.deaths);
                aborts.push(unit.handle.abort_handle());
                reaping.push((key, unit.handle));
            }
        }

        let exits = self.reap_units(reaping, aborts).await;

        self.execute_spawns(plan, &prev_deaths, &exits)
    }

    /// 进度清零：死过几次、但重建后确实干成过事的单元，不该带着旧账继续
    /// 累加，否则迟早在一次无关抖动里被误判为「反复失败」而放弃。
    /// 判据是每化身一个全新计数器，读数大于 0 即有过进度。
    fn clear_recovered(&mut self) {
        for (key, unit) in &mut self.running {
            if unit.deaths > 0 && unit.progress.load(Ordering::Relaxed) > 0 {
                tracing::info!(%key, deaths = unit.deaths, "unit made progress; death counter cleared");
                unit.deaths = 0;
            }
        }
    }

    /// 限时收割已取消的单元，带回观察到的退出值。
    ///
    /// 超时强杀：见模块文档。被 abort 的任务在下一个 await 点被掐掉，其退出值
    /// 不可得——重建路径会把死因记为「未观察到」。
    async fn reap_units(
        &self,
        reaping: Vec<(KeyOf<F>, JoinHandle<UnitExit>)>,
        aborts: Vec<AbortHandle>,
    ) -> HashMap<KeyOf<F>, UnitExit> {
        let mut exits: HashMap<KeyOf<F>, UnitExit> = HashMap::new();
        let reap = async {
            for (key, handle) in reaping {
                match handle.await {
                    Ok(exit) => {
                        exits.insert(key, exit);
                    }
                    Err(e) if e.is_cancelled() => {}
                    Err(e) => tracing::warn!(%key, error = %e, "unit task panicked while stopping"),
                }
            }
        };
        if timeout(self.policy.reap_timeout, reap).await.is_err() {
            tracing::warn!(
                "timed out reaping stopped units; aborting them to avoid duplicate incarnations"
            );
            for abort in aborts {
                abort.abort();
            }
        }
        exits
    }

    fn execute_spawns(
        &mut self,
        plan: ReconcilePlan<F::Spec>,
        prev_deaths: &HashMap<KeyOf<F>, u32>,
        exits: &HashMap<KeyOf<F>, UnitExit>,
    ) -> ReconcileReport<KeyOf<F>> {
        let mut report = ReconcileReport {
            stopped: plan.stop,
            ..ReconcileReport::default()
        };

        for (key, spec, reason) in plan.restart {
            let deaths = self.restart_deaths(&key, reason, prev_deaths, exits, &mut report);
            let unit = self.spawn_unit(spec, deaths);
            report.restarted.push(key.clone());
            self.running.insert(key, unit);
        }

        for (key, spec) in plan.start {
            let unit = self.spawn_unit(spec, 0);
            report.started.push(key.clone());
            self.running.insert(key, unit);
        }

        if !plan.over_cap.is_empty() {
            let keys: Vec<String> = plan.over_cap.iter().map(ToString::to_string).collect();
            tracing::warn!(
                deferred = keys.len(),
                keys = ?keys,
                "unit cap reached; deferring these to a later round"
            );
            report.deferred = plan.over_cap;
        }

        report
    }

    /// 算出重建化身应继承的死亡计数，并把跨过放弃阈值的边沿记入报告。
    /// 全部早退、零嵌套——这段是重建路径上最容易长杂枝的地方。
    fn restart_deaths(
        &self,
        key: &KeyOf<F>,
        reason: RestartReason,
        prev_deaths: &HashMap<KeyOf<F>, u32>,
        exits: &HashMap<KeyOf<F>, UnitExit>,
        report: &mut ReconcileReport<KeyOf<F>>,
    ) -> u32 {
        // 配置变更是运维动作，不是单元的过错
        if reason == RestartReason::SpecChanged {
            return 0;
        }

        // 干净退出说明单元按约定收了场（活儿做完了），计成死亡会让这类
        // 单元被慢慢退避到停摆
        if matches!(exits.get(key), Some(UnitExit::Normal)) {
            tracing::debug!(%key, "unit exited cleanly; restarting without penalty");
            return 0;
        }

        let n = prev_deaths.get(key).copied().unwrap_or(0) + 1;
        // 死因只有这一处能拿到：句柄已收割，新化身起来后就再也追溯不到
        // 上一条。不打出来，反复死的单元在日志里只会是一串「又死了」
        let cause = match exits.get(key) {
            Some(UnitExit::Failed(e)) => e.to_string(),
            _ => "exit not observed (task panicked or was not reaped in time)".to_owned(),
        };
        tracing::warn!(%key, deaths = n, cause = %cause, "unit died; restarting after backoff");

        if n >= self.policy.max_deaths {
            // 只在跨过阈值的这一轮打 error 并记入报告：下一轮起计划分支
            // 直接放弃，不会再走到这里
            tracing::error!(
                %key,
                deaths = n,
                "unit reached the death limit; giving up until its spec changes"
            );
            report.gave_up.push(key.clone());
        }
        n
    }

    fn spawn_unit(&self, spec: F::Spec, deaths: u32) -> RunningUnit<F::Spec> {
        let cancel = self.parent_cancel.child_token();
        // 每次 spawn 换一个新计数器：沿用旧的会让上一化身的进度替这一化身背书
        let progress = Arc::new(AtomicU64::new(0));
        let ctx = UnitCtx::new(cancel.clone(), progress.clone());
        let handle = tokio::spawn(self.factory.spawn_unit(spec.clone(), ctx));

        RunningUnit {
            spec,
            cancel,
            handle,
            deaths,
            // 全新单元与重建单元统一记 now + backoff(deaths) 的窗口：一条代码
            // 路径；且窗口判定是严格小于，新单元（deaths = 0）的窗口终点恰逢
            // 下一轮巡检，效果与零窗口一致。
            // 饱和运算：荒谬的退避配置最多把窗口推到 i64 上限，不回绕成
            // 负值——负窗口等于永远放行，退避就废了
            next_restart_at_ms: now_ms().saturating_add(
                i64::try_from(self.policy.restart_backoff(deaths).as_millis()).unwrap_or(i64::MAX),
            ),
            progress,
        }
    }

    /// 取消并等待全部单元退出，消费自身。
    ///
    /// 消费 `self` 是刻意的：关停之后再 `reconcile` 一次会把刚停掉的单元重新
    /// 拉起来，让类型系统直接堵掉这条路。宽限期用完即 abort（与 core 的关停
    /// 收敛同口径：先给宽限期，超时强杀，绝不留脱管任务）。
    pub async fn shutdown(mut self, grace: Duration) {
        let units: Vec<(KeyOf<F>, RunningUnit<F::Spec>)> = self.running.drain().collect();
        if units.is_empty() {
            return;
        }

        let mut aborts = Vec::with_capacity(units.len());
        let mut handles = Vec::with_capacity(units.len());
        for (key, unit) in units {
            unit.cancel.cancel();
            aborts.push(unit.handle.abort_handle());
            handles.push((key, unit.handle));
        }

        let graceful = async {
            for (key, handle) in handles {
                if let Err(e) = handle.await
                    && !e.is_cancelled()
                {
                    tracing::warn!(%key, error = %e, "unit task panicked during shutdown");
                }
            }
        };
        if timeout(grace, graceful).await.is_err() {
            tracing::warn!("units did not finish within the grace period; aborting them");
            for abort in aborts {
                abort.abort();
            }
        }
    }
}

/// 期望集去重：重复 key 意味着上游期望集算错了。后一条覆盖前一条只是为了
/// 让本轮能继续，真正的修复在期望集那一侧。
fn dedup_desired<S: UnitSpec>(desired: Vec<S>) -> HashMap<S::Key, S> {
    let mut map = HashMap::with_capacity(desired.len());
    for spec in desired {
        let key = spec.key();
        if map.insert(key.clone(), spec).is_some() {
            tracing::error!(%key, "duplicate unit key in desired set; later entry wins");
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    /// 单元的行为脚本：测试用它决定 spawn 出来的 future 怎么结束
    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Script {
        /// 一直跑到被取消，中途上报一次进度
        RunUntilCancel,
        /// 立刻异常终止
        FailImmediately,
        /// 立刻干净退出
        FinishImmediately,
        /// 无视取消令牌睡死过去（只有 abort 能收掉它）
        IgnoreCancel,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct TestSpec {
        key: String,
        version: u32,
        script: Script,
    }

    impl UnitSpec for TestSpec {
        type Key = String;

        fn key(&self) -> String {
            self.key.clone()
        }

        fn same_as(&self, other: &Self) -> bool {
            self.version == other.version && self.script == other.script
        }
    }

    fn spec(key: &str, version: u32, script: Script) -> TestSpec {
        TestSpec {
            key: key.to_owned(),
            version,
            script,
        }
    }

    #[derive(Default)]
    struct TestFactory {
        /// key → (spawn 次数, 最后一次 spawn 的版本)
        spawns: Arc<Mutex<HashMap<String, (u32, u32)>>>,
        /// IgnoreCancel 脚本握着它的克隆，靠 strong_count 观察 future 是否真被杀
        keepalive: Arc<()>,
    }

    impl TestFactory {
        fn spawn_count(&self, key: &str) -> u32 {
            self.spawns.lock().unwrap().get(key).map_or(0, |v| v.0)
        }

        fn last_version(&self, key: &str) -> u32 {
            self.spawns.lock().unwrap().get(key).map_or(0, |v| v.1)
        }
    }

    impl UnitFactory for TestFactory {
        type Spec = TestSpec;

        fn spawn_unit(
            &self,
            spec: TestSpec,
            ctx: UnitCtx,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = UnitExit> + Send>> {
            {
                let mut spawns = self.spawns.lock().unwrap();
                let entry = spawns.entry(spec.key()).or_insert((0, 0));
                entry.0 += 1;
                entry.1 = spec.version;
            }
            // 探针只交给卡死脚本：其余化身不持有，strong_count 才能精确指认
            // 「卡死的那个 future 是否真被销毁」
            let keep = (spec.script == Script::IgnoreCancel).then(|| self.keepalive.clone());
            Box::pin(async move {
                match spec.script {
                    Script::RunUntilCancel => {
                        ctx.mark_progress();
                        ctx.cancel.cancelled().await;
                        UnitExit::Normal
                    }
                    Script::FailImmediately => UnitExit::Failed(anyhow::anyhow!("boom")),
                    Script::FinishImmediately => UnitExit::Normal,
                    Script::IgnoreCancel => {
                        let _keep = keep;
                        loop {
                            tokio::time::sleep(Duration::from_secs(3600)).await;
                        }
                    }
                }
            })
        }
    }

    fn manager(
        max_deaths: u32,
        base_ms: u64,
        max_units: Option<usize>,
    ) -> UnitManager<TestFactory> {
        let policy = RestartPolicy {
            max_deaths,
            backoff_base: Duration::from_millis(base_ms),
            backoff_cap: Duration::from_millis(base_ms * 8),
            reap_timeout: Duration::from_secs(1),
        };
        UnitManager::new(
            TestFactory::default(),
            policy,
            max_units,
            CancellationToken::new(),
        )
    }

    /// 让已结束的任务真正被运行时标记完成，`is_finished` 才反映现实
    async fn settle() {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    #[tokio::test]
    async fn starts_and_stops_units_to_match_the_desired_set() {
        let mut m = manager(3, 10, None);

        let report = m
            .reconcile(vec![
                spec("a", 1, Script::RunUntilCancel),
                spec("b", 1, Script::RunUntilCancel),
            ])
            .await;
        assert_eq!(report.started, vec!["a".to_owned(), "b".to_owned()]);
        assert!(report.stopped.is_empty());
        assert_eq!(m.len(), 2);

        let report = m
            .reconcile(vec![spec("a", 1, Script::RunUntilCancel)])
            .await;
        assert_eq!(report.stopped, vec!["b".to_owned()]);
        assert_eq!(m.len(), 1);
        assert_eq!(m.snapshot()[0].key, "a");

        let report = m.reconcile(vec![]).await;
        assert_eq!(report.stopped, vec!["a".to_owned()]);
        assert!(m.is_empty());

        // 稳态：空对空，什么都不做
        assert!(m.reconcile(vec![]).await.is_noop());
    }

    /// 规格变更关旧建新，不留旧化身
    #[tokio::test]
    async fn spec_change_rebuilds_the_unit() {
        let mut m = manager(3, 10, None);

        m.reconcile(vec![spec("a", 1, Script::RunUntilCancel)])
            .await;
        let report = m
            .reconcile(vec![spec("a", 2, Script::RunUntilCancel)])
            .await;

        assert_eq!(report.restarted, vec!["a".to_owned()]);
        assert!(report.gave_up.is_empty());
        assert_eq!(m.len(), 1);
        assert_eq!(m.factory.spawn_count("a"), 2);
        assert_eq!(m.factory.last_version("a"), 2);
    }

    /// 期望集里同 key 出现两次：后者胜，且只 spawn 一次
    #[tokio::test]
    async fn duplicate_keys_in_the_desired_set_last_one_wins() {
        let mut m = manager(3, 10, None);

        m.reconcile(vec![
            spec("a", 1, Script::RunUntilCancel),
            spec("a", 2, Script::RunUntilCancel),
        ])
        .await;

        assert_eq!(m.len(), 1);
        assert_eq!(m.factory.spawn_count("a"), 1);
        assert_eq!(m.factory.last_version("a"), 2);
    }

    /// 连续死亡累加计数；跨过阈值的那一轮进 gave_up 报告（恰一次），此后不再重建
    #[tokio::test]
    async fn repeated_deaths_lead_to_giving_up_with_an_edge_report() {
        let mut m = manager(2, 1, None);
        let failing = vec![spec("a", 1, Script::FailImmediately)];
        let mut gave_up_rounds = 0u32;

        for _ in 0..6 {
            let report = m.reconcile(failing.clone()).await;
            if !report.gave_up.is_empty() {
                assert_eq!(report.gave_up, vec!["a".to_owned()]);
                gave_up_rounds += 1;
            }
            settle().await;
        }

        assert_eq!(gave_up_rounds, 1, "放弃是边沿事件，只报告一次");
        // 首建 + 死亡重建两次（deaths=1、deaths=2==max），此后放弃
        assert_eq!(m.factory.spawn_count("a"), 3, "放弃之后不应再有新化身");
        assert_eq!(m.snapshot()[0].deaths, 2);
    }

    /// 干净退出不计死亡：这类单元一直被重建，不会被退避到停摆
    #[tokio::test]
    async fn clean_exit_restarts_without_counting_a_death() {
        let mut m = manager(2, 1, None);
        let finishing = vec![spec("a", 1, Script::FinishImmediately)];

        for _ in 0..4 {
            m.reconcile(finishing.clone()).await;
            settle().await;
        }

        assert!(m.factory.spawn_count("a") >= 3);
        assert_eq!(m.snapshot()[0].deaths, 0);
    }

    /// 上报过进度的单元，死亡计数在下一轮对账时被清零
    #[tokio::test]
    async fn progress_clears_the_death_counter() {
        let mut m = manager(5, 1, None);

        m.reconcile(vec![spec("a", 1, Script::FailImmediately)])
            .await;
        settle().await;
        m.reconcile(vec![spec("a", 1, Script::FailImmediately)])
            .await;
        settle().await;
        assert_eq!(m.snapshot()[0].deaths, 1);

        // 换成会上报进度的脚本并让它跑起来，下一轮对账应清零
        m.reconcile(vec![spec("a", 2, Script::RunUntilCancel)])
            .await;
        settle().await;
        m.reconcile(vec![spec("a", 2, Script::RunUntilCancel)])
            .await;

        assert_eq!(m.snapshot()[0].deaths, 0);
    }

    /// 上限挡住超额新建：被挡下的不进在册表，但进 deferred 报告
    #[tokio::test]
    async fn cap_limits_the_number_of_spawned_units() {
        let mut m = manager(3, 10, Some(2));

        let report = m
            .reconcile(vec![
                spec("a", 1, Script::RunUntilCancel),
                spec("b", 1, Script::RunUntilCancel),
                spec("c", 1, Script::RunUntilCancel),
            ])
            .await;

        assert_eq!(report.started, vec!["a".to_owned(), "b".to_owned()]);
        assert_eq!(report.deferred, vec!["c".to_owned()]);
        assert_eq!(m.len(), 2);
    }

    /// 无视取消的单元在收割超时后被强杀：future 真被销毁，且不与新化身并存
    #[tokio::test]
    async fn stuck_unit_is_aborted_after_the_reap_timeout() {
        let factory = TestFactory::default();
        let spawns = factory.spawns.clone();
        let probe = factory.keepalive.clone();
        let policy = RestartPolicy {
            max_deaths: 3,
            backoff_base: Duration::from_millis(10),
            backoff_cap: Duration::from_millis(80),
            reap_timeout: Duration::from_millis(100),
        };
        let mut m = UnitManager::new(factory, policy, None, CancellationToken::new());

        m.reconcile(vec![spec("a", 1, Script::IgnoreCancel)]).await;
        settle().await;
        // 3 = 测试探针 + 工厂自身 + 卡死化身
        assert_eq!(Arc::strong_count(&probe), 3, "卡死化身应握着探针");

        // 规格变更 → 关旧建新；旧化身无视取消，只能等 100ms 超时后被 abort
        m.reconcile(vec![spec("a", 2, Script::RunUntilCancel)])
            .await;

        let mut killed = false;
        for _ in 0..100 {
            // 回落到 2（探针 + 工厂）即旧化身的 future 已被销毁
            if Arc::strong_count(&probe) == 2 {
                killed = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(killed, "收割超时后卡死化身必须被 abort，不得与新化身并存");
        assert_eq!(spawns.lock().unwrap()["a"].0, 2);
        assert!(m.snapshot()[0].alive, "新化身应在跑");
    }

    /// 荒谬退避配置下窗口饱和到 i64 上限，不回绕成负值
    #[tokio::test]
    async fn absurd_backoff_saturates_instead_of_wrapping() {
        let policy = RestartPolicy {
            max_deaths: 3,
            backoff_base: Duration::MAX,
            backoff_cap: Duration::MAX,
            reap_timeout: Duration::from_secs(1),
        };
        let mut m = UnitManager::new(
            TestFactory::default(),
            policy,
            None,
            CancellationToken::new(),
        );

        m.reconcile(vec![spec("a", 1, Script::RunUntilCancel)])
            .await;
        assert_eq!(m.snapshot()[0].next_restart_at_ms, i64::MAX);
    }

    /// 关停取消全部单元并等它们退出；宽限期内正常收敛
    #[tokio::test]
    async fn shutdown_cancels_every_unit() {
        let mut m = manager(3, 10, None);
        m.reconcile(vec![
            spec("a", 1, Script::RunUntilCancel),
            spec("b", 1, Script::RunUntilCancel),
        ])
        .await;

        m.shutdown(Duration::from_secs(1)).await;
    }

    /// 关停时卡死的单元被宽限期后的 abort 收掉
    #[tokio::test]
    async fn shutdown_aborts_units_that_outlive_the_grace_period() {
        let factory = TestFactory::default();
        let probe = factory.keepalive.clone();
        let mut m = UnitManager::new(
            factory,
            RestartPolicy::default(),
            None,
            CancellationToken::new(),
        );

        m.reconcile(vec![spec("a", 1, Script::IgnoreCancel)]).await;
        settle().await;

        m.shutdown(Duration::from_millis(100)).await;

        let mut killed = false;
        for _ in 0..100 {
            if Arc::strong_count(&probe) == 1 {
                killed = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(killed, "宽限期后卡死单元必须被 abort");
    }
}
