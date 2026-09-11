//! 注入式指标：进程级自省计数的唯一形态
//!
//! `Metrics` 由 app 装配一份、以 `Arc` 注入写读两侧；进程内没有 `static` 计数器。
//! 这是测试并行化（不加 `--test-threads=1`）与「一个进程里跑两套装配」的前提之一。
//!
//! # 分面按消费者进场
//!
//! 本模块只随真实消费者长字段：模板里只有示例面 `ticker` 一个分面。加一个面就
//! 加一个 `XxxStats` 字段与对应的 `XxxSnapshot`，**只增不改名**——改名等于丢失
//! 与历史指标的对照关系。
//!
//! # 为什么全用 `Relaxed`
//!
//! 这几个数字之间**不构成任何不变量**：没有「A 加了 B 就必须也加」这类关系，
//! 读到的也只是给人看的近似值。用 `SeqCst` 唯一的效果是在热路径上加一道内存屏障，
//! 去换一个没有人能观察到的差别。

use std::sync::Mutex;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

use serde::Serialize;

use crate::util::TimestampMs;

/// 全部面的指标根（app 装配一份，`Arc<Metrics>` 注入）。
#[derive(Debug, Default)]
pub struct Metrics {
    /// 示例面 `ticker` 的进程级计数
    pub ticker: TickerStats,
}

/// 「最后一次错误」的存放形态：发生时刻与文本放在**同一把锁**下
///
/// 拆成一个原子量加一个锁的话，两者可以来自不同的两次失败，页面上就会出现
/// 「12 分钟前」配着刚刚那条错误的组合，而这种错位恰恰只在失败频繁时出现——
/// 也就是最需要看它的时候。
///
/// 临界区纯内存（一次赋值 / 一次克隆），绝不跨 await。
#[derive(Debug, Default)]
pub struct LastError(Mutex<Option<(TimestampMs, String)>>);

impl LastError {
    pub fn record(&self, at_ms: TimestampMs, message: String) {
        if let Ok(mut g) = self.0.lock() {
            *g = Some((at_ms, message));
        }
    }

    /// 读一次；锁中毒不让自省接口跟着倒（自省接口存在的理由就是在别处
    /// 出问题时还能打开）
    pub fn read(&self) -> (Option<TimestampMs>, Option<String>) {
        match self.0.lock() {
            Ok(g) => match g.as_ref() {
                Some((at, msg)) => (Some(*at), Some(msg.clone())),
                None => (None, None),
            },
            Err(_) => (None, Some("last_error 锁已中毒".to_owned())),
        }
    }
}

/// 示例面 `ticker` 的进程级计数。
#[derive(Debug, Default)]
pub struct TickerStats {
    /// 已完成的 tick 数
    ticks: AtomicU64,
    /// 最后一次 tick 的时刻；0 哨兵表示还没有过（对外读成 `None`）
    last_tick_at_ms: AtomicI64,
    /// 最后一次 tick 内的错误（示例面本身不会出错，字段是给真实面照抄的形状）
    pub last_error: LastError,
}

/// `TickerStats` 的只读快照（自省接口的输出形态）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TickerSnapshot {
    pub ticks: u64,
    pub last_tick_at_ms: Option<TimestampMs>,
    pub last_error_at_ms: Option<TimestampMs>,
    pub last_error: Option<String>,
}

impl TickerStats {
    /// 一次 tick 完成
    pub fn ticked(&self, at_ms: TimestampMs) {
        self.ticks.fetch_add(1, Ordering::Relaxed);
        self.last_tick_at_ms.store(at_ms, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> TickerSnapshot {
        let (last_error_at_ms, last_error) = self.last_error.read();
        let last = self.last_tick_at_ms.load(Ordering::Relaxed);
        TickerSnapshot {
            ticks: self.ticks.load(Ordering::Relaxed),
            last_tick_at_ms: (last != 0).then_some(last),
            last_error_at_ms,
            last_error,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_snapshot_reads_none_for_sentinels() {
        let m = Metrics::default();
        let s = m.ticker.snapshot();
        assert_eq!(s.ticks, 0);
        assert_eq!(s.last_tick_at_ms, None);
        assert_eq!(s.last_error, None);
    }

    #[test]
    fn ticked_advances_count_and_timestamp() {
        let m = Metrics::default();
        m.ticker.ticked(1_000);
        m.ticker.ticked(2_000);
        let s = m.ticker.snapshot();
        assert_eq!(s.ticks, 2);
        assert_eq!(s.last_tick_at_ms, Some(2_000));
    }

    /// 时刻与文本同锁：读到的永远是同一次失败的两半
    #[test]
    fn last_error_keeps_time_and_text_together() {
        let e = LastError::default();
        assert_eq!(e.read(), (None, None));
        e.record(42, "boom".into());
        assert_eq!(e.read(), (Some(42), Some("boom".into())));
    }
}
