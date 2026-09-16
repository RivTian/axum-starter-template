//! 注册一个顶层任务时必须说清的事。
//!
//! [`TaskSpec`] **没有 `Default`**，而且字段全部必填。这是刻意的：关停预算是整个进程里最
//! 稀缺的资源（总宽限期 `T` 是硬边界），而"这个面需不需要优雅收尾"只有写这个面的人知道。
//! 给它一个默认值，就等于让所有后来新增的面默默继承一个没人想过的答案。

use crate::task::TaskName;

/// 一个面在关停时应当被怎样对待。
///
/// 这个分类**只决定是否分配 harvest 预算**，不决定是否回收。所有任务——不论哪一类——
/// 都会进入 reap 段并产出 [`TaskExit`](crate::task::TaskExit)。
///
/// 把"不等它"和"不管它"分开是有代价的经验：只 `abort()` 而不回收、不记录，等于把一个
/// 卡住的任务变成一条永远查不到的线索。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ShutdownClass {
    /// 有 in-flight 状态需要自己收尾，值得分配 harvest 预算。
    ///
    /// 典型：HTTP 面（要把已经在处理的请求答完）、写入型任务（要把缓冲刷出去）。
    Graceful,
    /// 没有需要收尾的状态，取消后直接进 reap。
    ///
    /// 典型：纯转发面、周期性只读探测。给它分配 harvest 是纯浪费——它收到取消信号之后
    /// 唯一会做的事就是立刻返回，而那点时间本可以留给真正需要的面。
    Abortable,
}

impl ShutdownClass {
    /// 是否为这个面分配 harvest 预算。
    ///
    /// 无 `_` 臂：新增分类时必须显式回答。
    #[must_use]
    pub fn gets_harvest_budget(self) -> bool {
        match self {
            Self::Graceful => true,
            Self::Abortable => false,
        }
    }

    /// 报告与日志里使用的稳定短名。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Graceful => "graceful",
            Self::Abortable => "abortable",
        }
    }
}

/// 注册一个顶层任务所需的全部元数据。
///
/// **刻意不实现 `Default`。** 类型本身就是证据：漏掉 `class` 会编译不过，而不是拿到一个
/// 看起来合理的默认值。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TaskSpec {
    /// 面的名字（枚举，不是字符串）。
    pub name: TaskName,
    /// 关停时怎么对待它。
    pub class: ShutdownClass,
}

impl TaskSpec {
    /// 构造一个任务规格。两个字段都必须显式给出。
    #[must_use]
    pub const fn new(name: TaskName, class: ShutdownClass) -> Self {
        Self { name, class }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abortable_task_gets_no_harvest_budget() {
        assert!(ShutdownClass::Graceful.gets_harvest_budget());
        assert!(
            !ShutdownClass::Abortable.gets_harvest_budget(),
            "Abortable 面若分到 harvest 预算，总宽限期会被无谓消耗"
        );
    }

    #[test]
    fn spec_requires_every_field() {
        // 这个用例的真正断言在编译期：TaskSpec 没有 Default，也没有部分构造的途径。
        let spec = TaskSpec::new(TaskName::Http, ShutdownClass::Graceful);
        assert_eq!(spec.name, TaskName::Http);
        assert_eq!(spec.class, ShutdownClass::Graceful);
    }
}
