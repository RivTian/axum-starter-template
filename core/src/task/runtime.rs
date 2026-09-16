//! 任务归属的 runtime 标识。
//!
//! 这个模块只做一件事：让「任务跑在哪个 runtime 上」成为**无法说谎**的事实。
//!
//! 反面形态是让调用方在 spawn 时自己带一个标签参数。那样一来，任务真跑在 A 而标签写 B
//! 时，编译通过、单元测试通过、日志和关停报告全都指向错误的 runtime——这类缺陷只能靠人
//! 读代码发现。
//!
//! 这里的做法是：标识**只能**从 `Handle` 派生，没有任何构造函数接受调用方给的值。
//! `TaskSupervisor` 在 spawn 时自己调用 [`RuntimeId::of`]，调用方连传都传不进来。

use std::fmt;

/// 一个 runtime 的身份，由 [`tokio::runtime::Handle`] 派生。
///
/// 内部是 tokio 自己的 `runtime::Id`（进程内唯一、非零）。之所以不自己发号，是因为自己发号
/// 就必须有一个"登记"的动作，而登记就是又一个可以登记错的地方。
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct RuntimeId(tokio::runtime::Id);

impl RuntimeId {
    /// 从 handle 派生。这是本类型**唯一**的构造途径。
    #[must_use]
    pub fn of(handle: &tokio::runtime::Handle) -> Self {
        Self(handle.id())
    }
}

impl fmt::Debug for RuntimeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // 透传 tokio 的表示，不自造格式：报告里出现的数字应当能和 tokio 自己的日志对上。
        write!(f, "RuntimeId({})", self.0)
    }
}

impl fmt::Display for RuntimeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}
