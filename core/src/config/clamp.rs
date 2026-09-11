//! 配置段共用的钳位小件
//!
//! 各段的 `sanitize` 都调它，钳位的 warn 文案只在此处定义一次——各段各写一份
//! 迟早在措辞上分家，运维按日志 grep 时会漏掉一半。

/// 夹取单个字段并在越界时告警
///
/// 泛型而非给每个字段写一遍：几种整数类型都要用，`Ord + Copy + Display` 是全部
/// 需要的能力，不必引任何 num trait。`section` 是所属 TOML 段名（如 `"ticker"`），
/// 进 warn 文案的方括号里，让一条日志自己说清它属于哪一段
pub(super) fn clamp_field<T: Ord + Copy + std::fmt::Display>(
    v: &mut T,
    lo: T,
    hi: T,
    section: &str,
    name: &str,
) {
    let c = (*v).clamp(lo, hi);
    if c != *v {
        tracing::warn!("[{section}] {name} = {v} out of range [{lo}, {hi}], using {c}");
        *v = c;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 越界夹进区间、区间内原样保留；几种整数类型都走同一份泛型
    #[test]
    fn clamp_field_clamps_only_out_of_range_values() {
        let mut low = 0u64;
        clamp_field(&mut low, 1_000, 600_000, "t", "low");
        assert_eq!(low, 1_000);

        let mut high = 99u32;
        clamp_field(&mut high, 0, 5, "t", "high");
        assert_eq!(high, 5);

        let mut fine = 8usize;
        clamp_field(&mut fine, 1, 512, "t", "fine");
        assert_eq!(fine, 8);
    }
}
