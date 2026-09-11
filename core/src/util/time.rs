//! 时间基元：UTC epoch 毫秒。
//!
//! 不引 chrono：任务框架与退避窗口只需要一个可比较、可做差的整数时刻，
//! `SystemTime` 已经够用。给叶子 crate 增加依赖面，需要比「顺手」更强的理由。
//!
//! 时长一律走单调钟（`Instant`），墙钟只用于展示与落库——这里给的是墙钟。

/// UTC epoch 毫秒时间戳。
pub type TimestampMs = i64;

/// 当前 UTC epoch 毫秒。
///
/// 系统时钟早于 1970 只在硬件钟损坏时发生，此处返回 0 而不是 panic：
/// 这条路径会跑在任务循环里，panic 会被管理器记成单元死亡，坏钟会放大成重建风暴。
/// 上溢侧同理饱和到 `i64::MAX`（`as` 截断会静默回绕成负数，语义上是错的）。
pub fn now_ms() -> TimestampMs {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::now_ms;

    /// 量级自检：专抓「秒当毫秒用」这类错——它不报错，只让所有时刻差三个数量级
    #[test]
    fn now_ms_is_in_a_plausible_range() {
        let t = now_ms();
        assert!(t > 1_577_836_800_000, "早于 2020-01-01: {t}");
        assert!(t < 4_102_444_800_000, "晚于 2100-01-01: {t}");
    }
}
