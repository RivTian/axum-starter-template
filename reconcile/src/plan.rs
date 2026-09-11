//! 对账决策：期望集 + 现状视图 → 动作清单。纯函数层。
//!
//! # 为什么把它从管理器里挖出来
//!
//! 对账的全部判断（谁该停、谁该重建、谁在退避窗口里、谁已被放弃、上限余额
//! 怎么分）都收在纯函数里，管理器只执行动作清单。纯函数拿不到 `JoinHandle`，
//! 不可能顺手 spawn/cancel，于是这些判断能用同步的输入输出表测完——不起
//! 运行时、不等真实时钟。「等一等再断言」正是任务类测试不稳定的主要来源。

use std::collections::HashMap;
use std::time::Duration;

use {{crate_prefix_snake}}_core::util::TimestampMs;

use crate::unit::UnitSpec;

/// 重建策略：死几次放弃、退避多长、收割等多久。
///
/// 各面落地时**显式传自己的值**，不要依赖 [`Default`]：`reap_timeout` 按该面
/// 单元收尾实际需要的时间给（收尾要落库的面给得比纯内存的面长）；
/// `backoff_base` 一律传各自的巡检周期——退避比巡检还短没有意义，重建了也要
/// 等下一轮巡检才看得到结果。
#[derive(Debug, Clone)]
pub struct RestartPolicy {
    /// 连续死亡达到此数即放弃，直到规格变更才解除
    pub max_deaths: u32,
    /// 退避基数（应为该面的巡检周期，让退避与对账节奏同量级）
    pub backoff_base: Duration,
    /// 退避上限
    pub backoff_cap: Duration,
    /// 一轮对账里待停单元的批量收割等待上限；超时强杀（见管理器文档）
    pub reap_timeout: Duration,
}

impl Default for RestartPolicy {
    fn default() -> Self {
        Self {
            max_deaths: 5,
            backoff_base: Duration::from_secs(5),
            backoff_cap: Duration::from_secs(300),
            reap_timeout: Duration::from_secs(10),
        }
    }
}

impl RestartPolicy {
    /// 死了 `deaths` 次之后应退避多久：`base × 2^deaths`，封顶 `backoff_cap`。
    ///
    /// `deaths.min(6)` 不只是「限制增长」——移位量到 64 会直接 panic，而
    /// 放弃阈值被调大时 `deaths` 能涨到那个量级。截到 6 时退避已是基数的
    /// 64 倍，再往上也会被 `backoff_cap` 吃掉。
    pub fn restart_backoff(&self, deaths: u32) -> Duration {
        let mult = 1u64 << deaths.min(6);
        // as_millis 给的是 u128：荒谬的 base（如 Duration::MAX）走 `as u64`
        // 会静默截断出一个毫无意义的小窗口；饱和到 u64::MAX 让 min(cap) 兜底
        let base_ms = u64::try_from(self.backoff_base.as_millis()).unwrap_or(u64::MAX);
        Duration::from_millis(base_ms.saturating_mul(mult)).min(self.backoff_cap)
    }
}

/// 管理器视角下某个在册单元的状态（对账输入）。
#[derive(Debug, Clone)]
pub struct UnitView<S> {
    pub spec: S,
    pub alive: bool,
    pub deaths: u32,
    pub next_restart_at_ms: TimestampMs,
}

/// 重建缘由，决定死亡计数怎么处理。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestartReason {
    /// 配置变了：运维的主动动作，计数清零重新开始
    SpecChanged,
    /// 单元自己死了：计数累加
    UnitDied,
}

/// 一轮对账要执行的动作。
#[derive(Debug)]
pub struct ReconcilePlan<S: UnitSpec> {
    pub stop: Vec<S::Key>,
    pub restart: Vec<(S::Key, S, RestartReason)>,
    pub start: Vec<(S::Key, S)>,
    /// 因并发上限被挡下的 key：本轮不动它们，也不烧死亡计数
    pub over_cap: Vec<S::Key>,
}

// 手写而不 derive：derive 会给 S 加 Default 约束，而单元规格没理由有默认值
impl<S: UnitSpec> Default for ReconcilePlan<S> {
    fn default() -> Self {
        Self {
            stop: Vec::new(),
            restart: Vec::new(),
            start: Vec::new(),
            over_cap: Vec::new(),
        }
    }
}

impl<S: UnitSpec> ReconcilePlan<S> {
    /// 无事可做。管理器据此跳过整轮执行，稳态下不刷无意义日志。
    pub fn is_noop(&self) -> bool {
        self.stop.is_empty()
            && self.restart.is_empty()
            && self.start.is_empty()
            && self.over_cap.is_empty()
    }
}

/// 算出这一轮该做什么。纯函数：不读时钟、不碰运行时。
///
/// # 六种情形与判定顺序（顺序本身是不变量，不能调整）
///
/// 1. 期望集里没有 → 停
/// 2. 规格变了 → 重建（**排在退避与放弃之前**：运维改配置往往正是为了救一个
///    反复死掉的单元，这是解除放弃的唯一手段，顺序调后就没了）
/// 3. 还活着 → 不动，但计入上限占用
/// 4. 死亡次数够了 → 放弃（**排在退避之前**：否则被放弃的单元还会在窗口
///    到期时被再建一次）
/// 5. 还在退避窗口内 → 等
/// 6. 其余 → 按死亡重建
///
/// 退避判定用 `now_ms < next_restart_at_ms` 的**严格小于**：时刻恰等于窗口
/// 终点时放行，否则整数时钟下会白等一整轮。
///
/// # 上限（`max_units`）的记账口径
///
/// 只扣**活着的**单元：已死的不占资源，把它们算进去会让上限在故障时反而
/// 收紧，恰好卡住恢复（僵尸条目永久堵死自己的重建）。余额**先给重建后给
/// 新建**：重建的是运维已在观察的对象，让它一直起不来比少起一个新单元更
/// 难解释。被挡下的进 `over_cap`，等容量释放后的巡检再试。
pub fn plan_reconcile<S: UnitSpec>(
    desired: &HashMap<S::Key, S>,
    running: &HashMap<S::Key, UnitView<S>>,
    now_ms: TimestampMs,
    max_deaths: u32,
    max_units: Option<usize>,
) -> ReconcilePlan<S> {
    let mut plan = ReconcilePlan::default();
    let mut kept_alive = 0usize;

    for (id, view) in running {
        match desired.get(id) {
            None => plan.stop.push(id.clone()),
            Some(d) if !d.same_as(&view.spec) => {
                plan.restart
                    .push((id.clone(), d.clone(), RestartReason::SpecChanged));
            }
            // 这一支不是空动作：它承担上限记账，删掉会让余额按「全部重算」发放
            Some(_) if view.alive => kept_alive += 1,
            Some(_) if view.deaths >= max_deaths => {}
            Some(_) if now_ms < view.next_restart_at_ms => {}
            Some(d) => plan
                .restart
                .push((id.clone(), d.clone(), RestartReason::UnitDied)),
        }
    }

    for (id, spec) in desired {
        if !running.contains_key(id) {
            plan.start.push((id.clone(), spec.clone()));
        }
    }

    // HashMap 遍历序随机。排序既让日志可读，也让上限裁剪的取舍稳定——
    // 否则同一份输入会在不同轮次里挑中不同的单元放行
    plan.stop.sort_by_key(|k| k.to_string());
    plan.restart.sort_by_key(|(k, _, _)| k.to_string());
    plan.start.sort_by_key(|(k, _)| k.to_string());

    if let Some(cap) = max_units {
        let budget = cap.saturating_sub(kept_alive);
        let admitted_restarts = plan.restart.len().min(budget);
        let admitted_starts = plan.start.len().min(budget - admitted_restarts);

        plan.over_cap = plan
            .restart
            .split_off(admitted_restarts)
            .into_iter()
            .map(|(id, _, _)| id)
            .chain(
                plan.start
                    .split_off(admitted_starts)
                    .into_iter()
                    .map(|(id, _)| id),
            )
            .collect();
    }

    plan
}

#[cfg(test)]
mod tests {
    //! 用例矩阵：每条守一项语义，六种情形与上限记账各有对应。
    //! 两种典型面形态贯穿其中：**无上限、巡检基数 30s** 的轮询面，
    //! **有上限、巡检基数 5s** 的会话面。

    use super::*;

    const MAX_DEATHS: u32 = 5;

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

    fn desired(specs: &[TestSpec]) -> HashMap<String, TestSpec> {
        specs.iter().map(|s| (s.key(), s.clone())).collect()
    }

    fn running(
        views: Vec<(TestSpec, bool, u32, TimestampMs)>,
    ) -> HashMap<String, UnitView<TestSpec>> {
        views
            .into_iter()
            .map(|(spec, alive, deaths, next_restart_at_ms)| {
                (
                    spec.key(),
                    UnitView {
                        spec,
                        alive,
                        deaths,
                        next_restart_at_ms,
                    },
                )
            })
            .collect()
    }

    /// 停 / 重建 / 新建三类动作同轮各就各位
    #[test]
    fn plan_covers_stop_restart_and_start() {
        let d = desired(&[spec("keep", 1), spec("changed", 2), spec("new", 1)]);
        let r = running(vec![
            (spec("keep", 1), true, 0, 0),
            (spec("changed", 1), true, 0, 0),
            (spec("gone", 1), true, 0, 0),
        ]);

        let plan = plan_reconcile(&d, &r, 1_000, MAX_DEATHS, None);

        assert_eq!(plan.stop, vec!["gone".to_owned()]);
        assert_eq!(plan.restart.len(), 1);
        assert_eq!(plan.restart[0].0, "changed");
        assert_eq!(plan.restart[0].2, RestartReason::SpecChanged);
        assert_eq!(plan.start.len(), 1);
        assert_eq!(plan.start[0].0, "new");
    }

    /// 退避窗口是严格小于：499 还要等，500 就放行
    #[test]
    fn dead_unit_waits_out_the_backoff_window() {
        let d = desired(&[spec("a", 1)]);
        let r = running(vec![(spec("a", 1), false, 1, 500)]);

        assert!(plan_reconcile(&d, &r, 499, MAX_DEATHS, None).is_noop());

        let plan = plan_reconcile(&d, &r, 500, MAX_DEATHS, None);
        assert_eq!(plan.restart.len(), 1);
        assert_eq!(plan.restart[0].2, RestartReason::UnitDied);
    }

    /// 达到放弃阈值后一直不动，哪怕退避窗口早已过去
    #[test]
    fn given_up_unit_stays_down() {
        let d = desired(&[spec("a", 1)]);
        let r = running(vec![(spec("a", 1), false, MAX_DEATHS, 0)]);

        assert!(plan_reconcile(&d, &r, 9_999_999, MAX_DEATHS, None).is_noop());
    }

    /// 改配置是解除放弃的唯一手段：同时越过退避窗口与放弃阈值
    #[test]
    fn spec_change_revives_a_given_up_unit() {
        let d = desired(&[spec("a", 2)]);
        let r = running(vec![(spec("a", 1), false, MAX_DEATHS, i64::MAX)]);

        let plan = plan_reconcile(&d, &r, 0, MAX_DEATHS, None);
        assert_eq!(plan.restart.len(), 1);
        assert_eq!(plan.restart[0].2, RestartReason::SpecChanged);
    }

    /// 上限只扣活着的、余额先给重建（僵尸不占名额）
    #[test]
    fn cap_counts_only_live_units_and_prefers_restarts() {
        let d = desired(&[spec("live", 1), spec("dead", 1), spec("new", 1)]);
        let r = running(vec![
            (spec("live", 1), true, 0, 0),
            (spec("dead", 1), false, 1, 0),
        ]);

        // 上限 2：活单元占一格，余额 1 给重建，新建被挡
        let plan = plan_reconcile(&d, &r, 1_000, MAX_DEATHS, Some(2));
        assert_eq!(plan.restart.len(), 1);
        assert_eq!(plan.restart[0].0, "dead");
        assert!(plan.start.is_empty());
        assert_eq!(plan.over_cap, vec!["new".to_owned()]);

        // 上限 1：活单元占满，重建与新建都挡下，且不烧死亡计数
        let plan = plan_reconcile(&d, &r, 1_000, MAX_DEATHS, Some(1));
        assert!(plan.restart.is_empty());
        assert!(plan.start.is_empty());
        assert_eq!(plan.over_cap, vec!["dead".to_owned(), "new".to_owned()]);
    }

    /// 不设上限就一个都不裁，新建按 key 升序
    #[test]
    fn no_cap_never_trims() {
        let specs: Vec<TestSpec> = (0..100).map(|i| spec(&format!("s{i:03}"), 1)).collect();
        let plan = plan_reconcile(&desired(&specs), &HashMap::new(), 0, MAX_DEATHS, None);

        assert_eq!(plan.start.len(), 100);
        assert!(plan.over_cap.is_empty());
        assert_eq!(plan.start[0].0, "s000");
        assert_eq!(plan.start[99].0, "s099");
    }

    /// 轮询面的退避曲线（30s 巡检基数）：30s → 60s → 240s → 300s 封顶；
    /// 极端死亡次数不移位溢出
    #[test]
    fn restart_backoff_doubles_from_a_thirty_second_base_and_caps() {
        let policy = RestartPolicy {
            backoff_base: Duration::from_secs(30),
            backoff_cap: Duration::from_secs(300),
            ..RestartPolicy::default()
        };

        assert_eq!(policy.restart_backoff(0), Duration::from_secs(30));
        assert_eq!(policy.restart_backoff(1), Duration::from_secs(60));
        assert_eq!(policy.restart_backoff(3), Duration::from_secs(240));
        assert_eq!(policy.restart_backoff(4), Duration::from_secs(300));
        assert_eq!(policy.restart_backoff(64), Duration::from_secs(300));
        assert_eq!(policy.restart_backoff(u32::MAX), Duration::from_secs(300));
    }

    /// 会话面的退避曲线（5s 巡检基数）：5s → 10s → 40s → 300s 封顶
    #[test]
    fn restart_backoff_doubles_from_a_five_second_base_and_caps() {
        let policy = RestartPolicy {
            backoff_base: Duration::from_secs(5),
            backoff_cap: Duration::from_secs(300),
            ..RestartPolicy::default()
        };

        assert_eq!(policy.restart_backoff(0), Duration::from_secs(5));
        assert_eq!(policy.restart_backoff(1), Duration::from_secs(10));
        assert_eq!(policy.restart_backoff(3), Duration::from_secs(40));
        assert_eq!(policy.restart_backoff(30), Duration::from_secs(300));
    }

    /// 荒谬策略只饱和不回绕：base/cap 给到 Duration::MAX 时，u128→u64 的
    /// 静默截断曾可能得出一个「几乎为零」的窗口，饱和后恒为可表达的最大值
    #[test]
    fn restart_backoff_saturates_on_absurd_policies() {
        let policy = RestartPolicy {
            backoff_base: Duration::MAX,
            backoff_cap: Duration::MAX,
            ..RestartPolicy::default()
        };

        assert_eq!(policy.restart_backoff(0), Duration::from_millis(u64::MAX));
        assert_eq!(
            policy.restart_backoff(u32::MAX),
            Duration::from_millis(u64::MAX)
        );
    }
}
