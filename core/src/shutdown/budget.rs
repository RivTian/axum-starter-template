//! 关停预算。
//!
//! **一个绝对 deadline 沿链传递，不是每层各拿一份同名时长。**
//!
//! 反面形态是每一段都收到"3 秒"这个 `Duration`，然后各自 `sleep(3s)`。调用方以为自己给了
//! 3 秒，实际最坏情况是段数 × 3 秒；附加 runtime 越多，超出得越离谱，而配置里那个 `3s`
//! 看起来一点问题都没有。
//!
//! 这里的构造是累积绝对时刻，并且每一段都再夹一次总边界 `T`：
//!
//! ```text
//! T  = now + total_grace                    // 唯一的硬边界
//! d1 = min(T, now + harvest_budget)
//! d2 = min(T, d1  + reap_budget)
//! d3 = min(T, d2  + storage_close_budget)
//! d4 = min(T, d3  + runtime_shutdown_budget)
//! ```
//!
//! 于是"内层严格不超外层"是构造出来的，不是靠调用方自觉。

use std::time::{Duration, Instant};

/// 关停的各段时长预算（来自配置）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Budgets {
    /// 整个关停过程的硬上界。所有分段都不得越过它。
    #[serde(with = "humantime_serde")]
    pub total_grace: Duration,
    /// 第一段：等 `Graceful` 面自己收尾。
    #[serde(with = "humantime_serde")]
    pub harvest: Duration,
    /// 第二段：abort 之后回收。
    #[serde(with = "humantime_serde")]
    pub reap: Duration,
    /// 第三段：关存储。
    #[serde(with = "humantime_serde")]
    pub storage_close: Duration,
    /// 第四段：关 runtime（在 `block_on` 返回之后同步执行）。
    #[serde(with = "humantime_serde")]
    pub runtime_shutdown: Duration,
    /// 收到第二次停止请求时，把总边界收紧到 `now + escalate`。
    #[serde(with = "humantime_serde")]
    pub escalate: Duration,
}

impl Default for Budgets {
    /// 各段之和（25s）**小于**总边界（30s）。
    ///
    /// 留出余量是刻意的：分段预算是"这一段最多等多久"，不是"这一段一定要用满"。
    /// 让它们正好加满 `total_grace`，等于默认配置下最后一段永远只剩 0 秒——
    /// 那时 `min(T, ...)` 的夹取会把一个本来合理的默认值变成"最后一段必定超时"。
    ///
    /// 具体数值只是一个能跑的起点，不代表任何部署事实：调这几个数是
    /// 使用者上线前必然要做的事，README 会说明各段的含义。
    fn default() -> Self {
        Self {
            total_grace: Duration::from_secs(30),
            harvest: Duration::from_secs(10),
            reap: Duration::from_secs(5),
            storage_close: Duration::from_secs(5),
            runtime_shutdown: Duration::from_secs(5),
            escalate: Duration::from_secs(2),
        }
    }
}

/// 关停各段的绝对截止时刻。
///
/// 一旦构造出来就是只读的。第二次停止请求不会"刷新"它，只会产生一个更紧的新计划
/// （见 [`Plan::escalate`]）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Plan {
    /// 唯一的硬边界。
    pub total: Instant,
    /// 第一段截止时刻。
    pub harvest: Instant,
    /// 第二段截止时刻。
    pub reap: Instant,
    /// 第三段截止时刻。
    pub storage_close: Instant,
    /// 第四段截止时刻。
    pub runtime_shutdown: Instant,
    /// 构造过程中出现过时长溢出。
    ///
    /// **fail-closed**：溢出时不假装算出了一个合理的计划，而是把这个事实带下去，
    /// 让报告如实说明"预算配置本身有问题"。
    pub invalid: bool,
}

impl Plan {
    /// 从 `now` 出发构造一份计划。
    ///
    /// 任一步 `checked_add` 溢出时，该段退回到 `total`，并置 `invalid`。
    /// 退回到 `total` 而不是退回到 `now`：后者会让关停在配置写错时**立刻放弃**，
    /// 把一个配置问题升级成数据问题。
    #[must_use]
    pub fn new(now: Instant, budgets: &Budgets) -> Self {
        let mut invalid = false;

        let mut add = |base: Instant, delta: Duration| -> Option<Instant> {
            match base.checked_add(delta) {
                Some(at) => Some(at),
                None => {
                    invalid = true;
                    None
                }
            }
        };

        let total = add(now, budgets.total_grace).unwrap_or(now);
        // 每一段都夹一次 `total`：这就是"内层不超外层"的实现。
        let harvest = add(now, budgets.harvest).unwrap_or(total).min(total);
        let reap = add(harvest, budgets.reap).unwrap_or(total).min(total);
        let storage_close = add(reap, budgets.storage_close).unwrap_or(total).min(total);
        let runtime_shutdown = add(storage_close, budgets.runtime_shutdown)
            .unwrap_or(total)
            .min(total);

        Self {
            total,
            harvest,
            reap,
            storage_close,
            runtime_shutdown,
            invalid,
        }
    }

    /// 第二次停止请求：**收紧**总边界，绝不放宽。
    ///
    /// 语义是"加速"，不是"重新计时"。把 `now + escalate` 与原 `total` 取 `min`，
    /// 于是重复按 Ctrl-C 只会让关停更快，永远不会把已经临近的截止时间往后推。
    #[must_use]
    pub fn escalate(self, now: Instant, budgets: &Budgets) -> Self {
        let tightened = now
            .checked_add(budgets.escalate)
            .map_or(self.total, |at| at.min(self.total));

        Self {
            total: tightened,
            harvest: self.harvest.min(tightened),
            reap: self.reap.min(tightened),
            storage_close: self.storage_close.min(tightened),
            runtime_shutdown: self.runtime_shutdown.min(tightened),
            invalid: self.invalid,
        }
    }
}

/// 关停走到哪一段。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Stage {
    /// 等 `Graceful` 面自己收尾。
    Harvest,
    /// abort 之后回收。
    Reap,
}

impl Stage {
    /// 报告与日志里使用的稳定短名。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Harvest => "harvest",
            Self::Reap => "reap",
        }
    }
}

/// 一个到点仍未回收的任务。
///
/// **是结构化记录，不是计数，也不是日志字符串。** 一个数字只能告诉人"出事了"；
/// 要定位问题，必须知道是哪个面、在哪个 runtime、卡在哪一段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnreapedTask {
    /// 面的名字。
    pub name: Option<crate::task::TaskName>,
    /// 它跑在哪个 runtime 上。
    pub runtime: crate::task::RuntimeId,
    /// 它登记时声明的关停类别。
    pub class: crate::task::ShutdownClass,
    /// 卡在哪一段。
    pub stalled_at: Stage,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn budgets(total: u64, harvest: u64, reap: u64, storage: u64, rt: u64) -> Budgets {
        Budgets {
            total_grace: Duration::from_secs(total),
            harvest: Duration::from_secs(harvest),
            reap: Duration::from_secs(reap),
            storage_close: Duration::from_secs(storage),
            runtime_shutdown: Duration::from_secs(rt),
            escalate: Duration::from_secs(1),
        }
    }

    #[test]
    fn stages_are_monotonically_ordered() {
        let now = Instant::now();
        let plan = Plan::new(now, &budgets(30, 5, 5, 5, 5));
        assert!(plan.harvest <= plan.reap);
        assert!(plan.reap <= plan.storage_close);
        assert!(plan.storage_close <= plan.runtime_shutdown);
        assert!(plan.runtime_shutdown <= plan.total);
        assert!(!plan.invalid);
    }

    #[test]
    fn inner_stage_never_outlives_the_outer_deadline() {
        // 回归用例：各段之和远超总预算时，总边界仍然是硬的。
        // 属性式遍历一组组合，而不是只测一个"好看的"配置。
        let now = Instant::now();
        let cases = [
            (1_u64, 10_u64, 10_u64, 10_u64, 10_u64),
            (5, 100, 1, 1, 1),
            (2, 1, 1, 1, 100),
            (0, 5, 5, 5, 5),
            (30, 5, 5, 5, 5),
        ];
        for (i, (total, h, r, s, rt)) in cases.into_iter().enumerate() {
            let plan = Plan::new(now, &budgets(total, h, r, s, rt));
            let bound = now + Duration::from_secs(total);
            for (stage, at) in [
                ("harvest", plan.harvest),
                ("reap", plan.reap),
                ("storage_close", plan.storage_close),
                ("runtime_shutdown", plan.runtime_shutdown),
            ] {
                assert!(
                    at <= bound,
                    "TC{i} ({stage}) 越过了总边界：各段之和不得突破 total_grace"
                );
            }
        }
    }

    #[test]
    fn overflow_is_reported_not_swallowed() {
        // fail-closed：溢出必须被带出来，而不是悄悄得到一个"看起来正常"的计划。
        let now = Instant::now();
        let plan = Plan::new(
            now,
            &Budgets {
                total_grace: Duration::MAX,
                harvest: Duration::MAX,
                reap: Duration::MAX,
                storage_close: Duration::MAX,
                runtime_shutdown: Duration::MAX,
                escalate: Duration::from_secs(1),
            },
        );
        assert!(plan.invalid, "时长溢出必须置 invalid");
    }

    #[test]
    fn escalation_tightens_and_never_extends() {
        let now = Instant::now();
        let plan = Plan::new(now, &budgets(30, 5, 5, 5, 5));

        // 第二次请求发生在 1 秒后，escalate = 1s ⇒ 新边界 ≈ now + 2s，远早于原 now + 30s。
        let tightened = plan.escalate(now + Duration::from_secs(1), &budgets(30, 5, 5, 5, 5));
        assert!(tightened.total < plan.total, "第二次请求必须收紧总边界");
        assert!(tightened.harvest <= plan.harvest);
        assert!(tightened.runtime_shutdown <= plan.runtime_shutdown);

        // 很晚才来的第二次请求不得把边界往后推。
        let late = plan.escalate(now + Duration::from_secs(29), &budgets(30, 5, 5, 5, 5));
        assert!(late.total <= plan.total, "escalate 永远不该放宽既有边界");
    }
}
