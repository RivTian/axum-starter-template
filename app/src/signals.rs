//! 信号：全仓库**唯一**调用 `tokio::signal` 的地方。
//!
//! 这里只做转发：把操作系统的信号翻成 [`ProcessSignal`] 的三个变体，一个判断都
//! 不做。"第二次停止请求要收紧预算"、"SIGHUP 要重读配置"这类语义全在 `lifecycle::stop`
//! 与 `lifecycle::r#loop` 里，那两处都是纯逻辑，有用例。
//!
//! 这条切分是「不做进程测试」能成立的前提之一：编排入口 `run` 收的是
//! `impl Stream<Item = ProcessSignal>`，于是用例可以塞一个合成流，把"连按两次 Ctrl-C"
//! 这种场景在进程内跑完。真正没有自动化证据的，只剩下面这几十行是否接对了线——
//! 而它们没有分支可接错，且 `cargo run` 第一次 Ctrl-C 就会暴露。
//!
//! # 为什么叫 `ProcessSignal` 而不是 `StopRequest`
//!
//! 叫 `StopRequest` 会更贴近它最常见的用途，但 SIGHUP 必须走**同一条**流：重载是
//! 编排循环的一件事，而编排循环只有一个输入口；把重载单开一条路，"重载期间收到 SIGTERM"
//! 的顺序就没有地方被定义了。于是这个枚举里有了一个不表示"停止"的变体，再叫
//! `StopRequest` 就是名字在说谎。
//!
//! # 平台
//!
//! 只实现 unix。非 unix 平台上这个文件**编译不过**，而不是给一个没验证过的实现——
//! 不声称没验证过的平台。补齐的办法在下面的 `compile_error!` 里写着，而且很便宜：
//! `run` 收的是 `impl Stream`，换一个产生器就行，编排层一行都不用动。

use std::pin::Pin;
use std::task::{Context, Poll};

use futures_core::Stream;

/// 操作系统送进来的一条请求。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProcessSignal {
    /// SIGTERM：编排系统要求停止。
    Terminate,
    /// SIGINT：终端里的 Ctrl-C。
    Interrupt,
    /// SIGHUP：重读配置。**不**停止进程。
    Reload,
}

impl ProcessSignal {
    /// 日志里用的稳定短名。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Terminate => "terminate",
            Self::Interrupt => "interrupt",
            Self::Reload => "reload",
        }
    }

    /// 这条请求要不要停止进程。
    ///
    /// 放在这里而不是在编排循环里 `match`：新增一个变体时，编译器会在这个无 `_` 臂的
    /// `match` 上拦住人，逼他回答"它算不算停止"——[`ExitKind`] 用的也是这一招。
    ///
    /// [`ExitKind`]: service_core::task::ExitKind
    #[must_use]
    pub const fn is_stop(self) -> bool {
        match self {
            Self::Terminate | Self::Interrupt => true,
            Self::Reload => false,
        }
    }
}

/// 订阅操作系统信号。
///
/// # Errors
///
/// 注册信号处理器失败时返回底层的 [`std::io::Error`]。这不做兜底：一个收不到 SIGTERM
/// 的服务在编排系统眼里就是个必须被 `SIGKILL` 的进程，那会跳过全部收尾。
///
/// # Panics
///
/// 不 panic。但它必须在 tokio runtime 上下文里调用——`tokio::signal` 要把处理器注册到
/// 信号驱动上。
#[cfg(unix)]
pub fn os_signal_stream() -> std::io::Result<impl Stream<Item = ProcessSignal>> {
    use tokio::signal::unix::{SignalKind, signal};

    Ok(OsSignals {
        sources: [
            (signal(SignalKind::terminate())?, ProcessSignal::Terminate),
            (signal(SignalKind::interrupt())?, ProcessSignal::Interrupt),
            (signal(SignalKind::hangup())?, ProcessSignal::Reload),
        ],
    })
}

#[cfg(not(unix))]
compile_error!(
    "os_signal_stream 目前只实现了 unix。\n\
     补齐方式：为目标平台写一个 `Stream<Item = ProcessSignal>` 的产生器即可，\n\
     编排层收的就是 `impl Stream`，不需要任何改动。\n\
     这里刻意不给一个未经编译验证的实现。"
);

/// 三路信号合成一条流。
#[cfg(unix)]
struct OsSignals {
    /// 固定顺序。两条信号落在同一次 poll 窗口里时先报前面那条——这是个必须有人来定的
    /// 平局规则，不是语义：`Terminate` 与 `Interrupt` 在 [`StopPolicy`] 眼里完全等价，
    /// 而 `Reload` 排最后是因为它不停止进程，晚一个 poll 没有代价。
    ///
    /// [`StopPolicy`]: crate::lifecycle::stop::StopPolicy
    sources: [(tokio::signal::unix::Signal, ProcessSignal); 3],
}

#[cfg(unix)]
impl Stream for OsSignals {
    type Item = ProcessSignal;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        for (source, kind) in &mut self.get_mut().sources {
            match source.poll_recv(cx) {
                Poll::Ready(Some(())) => return Poll::Ready(Some(*kind)),
                // 信号驱动没了（runtime 正在消失）。整条流就此结束，而不是继续挂着——
                // 挂着就意味着编排循环会永远等一个再也不会来的信号。编排循环本来就得
                // 处理"流结束"这一档：用例注入的合成流是有限的。
                Poll::Ready(None) => return Poll::Ready(None),
                Poll::Pending => {}
            }
        }
        Poll::Pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reload_is_not_a_stop_request() {
        // 这条用例守的是那个刻意改掉的名字：枚举里有一个变体不表示"停止"。
        assert!(ProcessSignal::Terminate.is_stop());
        assert!(ProcessSignal::Interrupt.is_stop());
        assert!(!ProcessSignal::Reload.is_stop());
    }

    #[test]
    fn every_variant_has_a_distinct_name() {
        let names = [
            ProcessSignal::Terminate.as_str(),
            ProcessSignal::Interrupt.as_str(),
            ProcessSignal::Reload.as_str(),
        ];
        let unique: std::collections::BTreeSet<_> = names.iter().collect();
        assert_eq!(unique.len(), names.len(), "日志短名撞车会让事件无法区分");
    }
}
