//! 顶层任务的名字。
//!
//! **是枚举，不是字符串。**
//!
//! 字符串名字的问题不是不好看，是改名时只改一处仍然能编译通过。用字符串写关池的前置条件
//! 写成 `exit.name == "http"`，HTTP 面后来改了名字，于是"HTTP 面已退出"永远判不成立，
//! 关池被静默跳过——没有任何测试会红，因为两边都是合法字符串。
//!
//! 枚举把这类改名变成编译错误。新增一个面时，所有 `match` 都会要求你回答它属于哪一类。

use std::fmt;

/// 顶层任务（"面"）的标识。
///
/// 新增一个面就在这里加一个变体。凡是对面做分类的地方都写**无 `_` 臂**的 `match`，
/// 于是"新增了面却忘了考虑它"会在编译期暴露，而不是在生产环境的关停路径上暴露。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TaskName {
    /// HTTP 服务面。
    Http,
    /// 周期性后台任务面。
    Ticker,
}

// 这里**没有** `ConfigReload`。初稿里有过，删掉的理由写在这儿，免得下一个人当成遗漏补回去：
// SIGHUP 重载不是一个"面"，它是编排循环里的一步。定下来的规矩是「SIGHUP 不引入任何外部
// 面」——重载要读的是同一份配置真值、要写的是同一个 `watch`、失败时要保留同一份 last-good，
// 这些状态全在编排循环手上。做成面就得把它们复制一份出去，然后回答"面里的 last-good 和
// 循环里的 last-good 谁说了算"。而一个已注册却没人 `spawn` 的面名，本身就是被禁止的
// "预铺猜测的形状"：它让 `TaskName::ALL` 说了谎，登记核对的用例也跟着失去意义。

impl TaskName {
    /// 全部变体。用于启动期登记与测试遍历。
    ///
    /// 手写数组而不是靠宏：数组漏写会被 `exhaustive_variant_list` 用例判红，
    /// 引入一个宏依赖只为省三行不划算。
    pub const ALL: &'static [Self] = &[Self::Http, Self::Ticker];

    /// 日志与报告里使用的稳定短名。
    ///
    /// 这是**单向**的：只用于渲染，不用于比较。任何地方都不该把它再解析回 `TaskName`。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Ticker => "ticker",
        }
    }
}

impl fmt::Display for TaskName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exhaustive_variant_list() {
        // `ALL` 漏写变体时这里红。用 match 而不是计数：加变体时编译器会先拦住我们。
        for name in TaskName::ALL {
            let covered = match name {
                TaskName::Http | TaskName::Ticker => true,
            };
            assert!(covered, "TC ({name}) 未被覆盖");
        }
        assert_eq!(TaskName::ALL.len(), 2, "新增变体后请同步更新 ALL");
    }

    #[test]
    fn short_names_are_unique() {
        let mut seen = std::collections::BTreeSet::new();
        for name in TaskName::ALL {
            assert!(
                seen.insert(name.as_str()),
                "TC ({name}) 的短名与另一个面重复——报告将无法区分它们"
            );
        }
    }
}
