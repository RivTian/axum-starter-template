//! 任务退出的分类。
//!
//! 两条纪律在这里合流：
//!
//! 1. **「标识缺失」与「退出原因」是正交的两个字段。** 把"元数据查不到"和"任务
//!    panic 了"塞进同一个枚举，于是一个查不到名字的 panic 被报成了 `Invariant`——panic
//!    这个事实被吞掉了。这里 `name: Option<TaskName>` 与 `kind: ExitKind` 各管各的。
//!
//! 2. **「发生了什么」与「该怎么办」是正交的两个问题。** 前者是 [`ExitKind`]，后者是
//!    [`ExitKind::is_terminal`]。拆开之后，新增一个变体时编译器会强迫你回答"它算不算
//!    触发关停"，而不是让它悄悄落进某个 `_` 臂。

use crate::task::{RuntimeId, TaskName};
use std::fmt;

// panic 策略的编译期守卫。
//
// 下面的 [`ExitKind`] 有四个变体，其中 [`ExitKind::Panicked`] 只在展开式 panic 下**可能被
// 构造**：`panic = "abort"` 时进程在 `JoinHandle` 拿到结果之前就已经死了。也就是说 abort
// 之下这个枚举仍然编译、仍然有四个变体，但其中一个永远取不到——分类从"四选一"悄悄变成
// "三选一"，而且没有任何一处会报错。同一件事还会连累两处：`CatchPanicLayer` 变成一个
// handler panic 就杀进程，`debug_assert!` 之外的不变量检查在 release 下失去可观察性。
//
// 让它在**编译期**红，而不是写一行"请记得用 unwind"：清单里的 profile 可以被改、可以被
// `RUSTFLAGS` 覆盖，而这三条纪律一旦只在 debug 下成立就不再是纪律。守卫放在 `core` 是因为
// 它是叶子层——任何一个二进制都必然把它编进去，绕不过。
#[cfg(panic = "abort")]
compile_error!(
    "this workspace requires `panic = \"unwind\"`: with `abort`, ExitKind::Panicked becomes \
     unreachable, CatchPanicLayer can no longer contain a handler panic, and release builds \
     lose invariant observability. See the `[profile.release]` comment in the root Cargo.toml."
);

/// 一个顶层任务退出时留下的完整记录。
#[derive(Debug)]
pub struct TaskExit {
    /// 面的名字。`None` 表示元数据查找失败——这**不影响** `kind` 的准确性。
    pub name: Option<TaskName>,
    /// 它实际跑在哪个 runtime 上（从 `Handle` 派生，见 [`RuntimeId`]）。
    ///
    /// 与 `name` 同为 `Option` 且**同源**：两者都来自同一次元数据查找，查不到就一起没有。
    /// 把 `runtime` 写成裸 `RuntimeId` 会逼出一个"未归属"的假值，而那个假值在报告里
    /// 与一个真实的 runtime 标识长得一模一样——正是第 1 条纪律要避免的事。
    pub runtime: Option<RuntimeId>,
    /// 它是怎么结束的。
    pub kind: ExitKind,
}

impl TaskExit {
    /// 渲染用的名字。元数据缺失时给一个显式的占位串，而不是假装知道。
    #[must_use]
    pub fn name_str(&self) -> &'static str {
        match self.name {
            Some(name) => name.as_str(),
            None => "<unknown>",
        }
    }

    /// 渲染用的 runtime 标识。缺失时与名字用同一个占位串，读日志的人一眼能看出是同一次缺失。
    #[must_use]
    pub fn runtime_str(&self) -> String {
        match self.runtime {
            Some(rt) => rt.to_string(),
            None => String::from("<unknown>"),
        }
    }
}

/// 任务结束的方式。
#[derive(Debug)]
pub enum ExitKind {
    /// 正常返回 `Ok(())`。
    Returned,
    /// 返回了 `Err(_)`。
    Failed(PlaneError),
    /// panic 了。
    ///
    /// 这个变体在 `panic = "abort"` 下**不可能**出现——整个进程会先死掉。
    /// 这正是 release profile 必须用 `unwind` 的原因之一：否则这里的四分类只在 debug 下成立，
    /// 那它就不是纪律，只是愿望。
    Panicked(PanicSummary),
    /// 被外部 `abort()` 取消，任务没有机会自己收尾。
    Cancelled,
}

impl ExitKind {
    /// 这次退出是否应当触发进程关停。
    ///
    /// **这个 `match` 没有 `_` 臂，这是刻意的。** 新增 `ExitKind` 变体时这里会编译不过，
    /// 迫使加变体的人显式回答"它算不算 terminal"。若写成 `_ => true`，新变体会默默继承
    /// 一个没人想过的答案。
    ///
    /// 当前四个变体**都**是 terminal（first-failure：任一顶层任务退出即进入关停）。
    /// 这是一个被写出来的结论，不是默认值。
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        match self {
            Self::Returned | Self::Failed(_) | Self::Panicked(_) | Self::Cancelled => true,
        }
    }

    /// 这次退出本身是否代表一个失败。
    ///
    /// 与 [`is_terminal`](Self::is_terminal) 正交：`Returned` 是 terminal 但不是失败
    /// （面正常收尾了，只是它不该在服务运行期间收尾）。关停报告用这一条判断是否 graceful。
    #[must_use]
    pub fn is_failure(&self) -> bool {
        match self {
            Self::Returned => false,
            Self::Failed(_) | Self::Panicked(_) | Self::Cancelled => true,
        }
    }

    /// 报告与日志里使用的稳定短名。
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Returned => "returned",
            Self::Failed(_) => "failed",
            Self::Panicked(_) => "panicked",
            Self::Cancelled => "cancelled",
        }
    }
}

/// panic 的摘要。
///
/// 只保留能安全记录的部分。**不保留 `Box<dyn Any>`**：它既不是 `Send + Sync` 友好的，
/// 也会把 panic 负载的生命周期拖进报告里。
#[derive(Debug, Clone)]
pub struct PanicSummary {
    /// panic 消息（`&str` / `String` 负载能取到；其他类型取不到时为 `None`）。
    message: Option<Box<str>>,
}

impl PanicSummary {
    /// 从 `JoinError` 携带的 panic 负载中提取消息。
    #[must_use]
    pub fn from_payload(payload: &(dyn std::any::Any + Send)) -> Self {
        // std 的 panic 负载实际只有这两种形态。取不到就如实记 None，不编造。
        let message = payload
            .downcast_ref::<&'static str>()
            .map(|s| Box::from(*s))
            .or_else(|| {
                payload
                    .downcast_ref::<String>()
                    .map(|s| Box::from(s.as_str()))
            });
        Self { message }
    }

    /// panic 消息；取不到负载时为 `None`。
    #[must_use]
    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }
}

impl fmt::Display for PanicSummary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.message() {
            Some(msg) => f.write_str(msg),
            None => f.write_str("<non-string panic payload>"),
        }
    }
}

/// 一个顶层任务（面）返回的错误。
///
/// 面的 future 输出类型是 `Result<(), PlaneError>` 而不是 `()`——这一条是类型级的纪律：
/// `Output = ()` 会让"HTTP 面起不来"和"HTTP 面正常收尾"变成同一个形状，关停报告因此永远
/// 说不清发生了什么。
#[derive(Debug, thiserror::Error)]
pub enum PlaneError {
    /// 面在运行中失败。`context` 是 `Box<str>`，不含用户输入。
    #[error("{plane} plane failed: {context}")]
    Failed {
        /// 出错的面。
        plane: TaskName,
        /// 失败说明。
        context: Box<str>,
    },
}

impl PlaneError {
    /// 构造一个面失败。
    #[must_use]
    pub fn failed(plane: TaskName, context: impl Into<Box<str>>) -> Self {
        Self::Failed {
            plane,
            context: context.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_name_does_not_erase_the_panic() {
        // 回归用例：元数据查不到时，panic 这个事实必须原样保留。
        let exit = TaskExit {
            name: None,
            runtime: None,
            kind: ExitKind::Panicked(PanicSummary {
                message: Some(Box::from("boom")),
            }),
        };
        assert_eq!(exit.name_str(), "<unknown>");
        assert_eq!(
            exit.runtime_str(),
            "<unknown>",
            "name 与 runtime 同源，缺失时必须一起缺失"
        );
        assert!(
            matches!(exit.kind, ExitKind::Panicked(_)),
            "名字缺失不得把 kind 改写成别的东西"
        );
        assert!(exit.kind.is_failure());
    }

    #[test]
    fn terminal_and_failure_are_orthogonal() {
        // 正常返回：是 terminal（触发关停），但不是 failure（不影响 graceful 判定的"失败"项）。
        let cases: &[(ExitKind, bool, bool)] = &[
            (ExitKind::Returned, true, false),
            (
                ExitKind::Failed(PlaneError::failed(TaskName::Http, "x")),
                true,
                true,
            ),
            (
                ExitKind::Panicked(PanicSummary { message: None }),
                true,
                true,
            ),
            (ExitKind::Cancelled, true, true),
        ];
        for (i, (kind, terminal, failure)) in cases.iter().enumerate() {
            assert_eq!(
                kind.is_terminal(),
                *terminal,
                "TC{i} ({}) 的 is_terminal 不符",
                kind.as_str()
            );
            assert_eq!(
                kind.is_failure(),
                *failure,
                "TC{i} ({}) 的 is_failure 不符",
                kind.as_str()
            );
        }
    }

    #[test]
    fn panic_summary_extracts_both_std_payload_shapes() {
        let as_str: Box<dyn std::any::Any + Send> = Box::new("static msg");
        let as_string: Box<dyn std::any::Any + Send> = Box::new(String::from("owned msg"));
        let as_other: Box<dyn std::any::Any + Send> = Box::new(42_u32);

        assert_eq!(
            PanicSummary::from_payload(as_str.as_ref()).message(),
            Some("static msg")
        );
        assert_eq!(
            PanicSummary::from_payload(as_string.as_ref()).message(),
            Some("owned msg")
        );
        // 取不到就是 None——不编造一个看起来像消息的东西。
        assert_eq!(
            PanicSummary::from_payload(as_other.as_ref()).message(),
            None
        );
    }
}
