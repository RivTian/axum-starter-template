//! 这个进程的**身份**：它叫什么、是哪个版本。
//!
//! `/v1/info` 报的、日志第一行报的、测试断言的，必须是同一个值。
//!
//! 一种容易写歪的做法：`api` 的用例断言 `api` 自己的 `CARGO_PKG_VERSION`，而端点返回的
//! 是另一个 crate 的。它一直是绿的——只因为 workspace 里所有成员恰好共享同一个版本号。
//! 那天有人给某个成员单独改了版本，这条用例才会开始说真话，而在那之前它什么都没在守。
//!
//! 这里的做法：常量只有一处，[`BuildInfo::current`] 是它唯一的出口，测试和端点都从这里取。
//! 于是"两边不一致"这件事在类型上就无从发生。

/// 进程的身份。
///
/// `Copy` 且全是 `&'static str`：它进 `AppState`，而 `AppState` 会被每个请求克隆一次。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BuildInfo {
    /// 服务名，也就是 `[[bin]]` 的名字。
    ///
    /// **不是**某个 crate 的 `CARGO_PKG_NAME`——那是 workspace 的内部构造（`<前缀>-core`
    /// 之类），把它发到 `/v1/info` 上等于把仓库布局写进对外契约。运维要的是"这台机器上
    /// 跑的是哪个服务"，那个答案只有一个来源：可执行文件自己的名字。
    pub service: &'static str,
    /// 版本号。来自 workspace 的 `version` 字段（各成员 `version.workspace = true`）。
    pub version: &'static str,
}

impl BuildInfo {
    /// 编译进本二进制的身份。
    ///
    /// # 为什么版本是读出来的、服务名却要传进来
    ///
    /// 因为它们能被读到的地方不同。`CARGO_PKG_VERSION` 在任何目标里都有值，而
    /// `CARGO_BIN_NAME` **只在 `[[bin]]` 目标里可见**（实测）——这个文件属于
    /// lib 目标，在这里 `env!` 它是编译错误。于是它只能由 `main.rs` 一路传下来，和
    /// `ProcessEnv::capture` 收 `bin_name` 是同一条理由、同一个值。
    ///
    /// 参数类型是 `&'static str` 而不是 `&str`：它的来源只能是 `env!` 展开出来的那个
    /// 字面量。允许一个借来的串，就等于允许有人拿 `argv[0]` 填这里——而磁盘上被改过名的
    /// 可执行文件不该改变这个服务的身份。
    ///
    /// `const fn`：调用点不会有任何运行期开销，也没有"第一次调用时初始化"的余地——
    /// 那是全局可变状态的入口，本模板不留这种口子。
    #[must_use]
    pub const fn current(service: &'static str) -> Self {
        Self {
            service,
            version: env!("CARGO_PKG_VERSION"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_has_a_single_source() {
        // 回归用例：断言的来源必须就是端点会用的那个常量。
        // 这条用例**不**写死版本号字面量——写死之后它守的就变成"有没有人改过版本号"，
        // 而那不是这里要守的东西。
        let info = BuildInfo::current("svc");
        assert_eq!(
            info.version,
            env!("CARGO_PKG_VERSION"),
            "BuildInfo 的版本必须直接来自本 crate 的编译期常量"
        );
        assert!(!info.version.is_empty());
    }

    #[test]
    fn the_service_name_is_whatever_the_caller_passed() {
        // 服务名**不**从这个 crate 的任何常量里取。这条用例守的就是那件事：换一个入参
        // 就换一个身份，中间没有一个"其实是 CARGO_PKG_NAME"的兜底。
        assert_eq!(BuildInfo::current("svc").service, "svc");
        assert_ne!(BuildInfo::current("svc"), BuildInfo::current("other"));
    }

    #[test]
    fn build_info_is_stable_across_calls() {
        assert_eq!(BuildInfo::current("svc"), BuildInfo::current("svc"));
    }
}
