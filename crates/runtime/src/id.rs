//! 任务身份与 runtime 身份：`TaskKey`、`RuntimeId`、`RuntimeSet`。
//!
//! `RuntimeId` 是 `&'static str`：runtime 拓扑是**代码事实**（准入清单写在 `crates/runtime/README.md`），
//! 不从配置文件里读——配置能改的东西不该包含"任务跑在哪"。

use std::collections::BTreeMap;
use std::fmt;

use tokio::runtime::Handle;

use {{crate_prefix_snake}}_core::{Error, ErrorKind};

/// 任务的名字。同一进程内必须唯一（重复注册在 `register` 时即报错）。
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TaskKey(String);

impl TaskKey {
    pub fn new(key: impl Into<String>) -> Self {
        Self(key.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TaskKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// runtime 的名字。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RuntimeId(&'static str);

impl RuntimeId {
    /// 主 runtime：骨架里所有任务默认都在它上面。
    pub const MAIN: Self = Self("main");

    pub const fn new(name: &'static str) -> Self {
        Self(name)
    }

    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

impl fmt::Display for RuntimeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

/// 可用的 runtime 集合：id → `Handle`。
///
/// 装配层构造它（骨架里只有一个 `MAIN`）；注册任务时用 `RuntimeId` 指定 spawn 目标，
/// 未注册的名字是**启动错误**——静默回落会把"任务在哪个 runtime"变成猜测。
#[derive(Debug)]
pub struct RuntimeSet {
    main: Handle,
    aux: BTreeMap<RuntimeId, Handle>,
}

impl RuntimeSet {
    pub fn new(main: Handle) -> Self {
        Self {
            main,
            aux: BTreeMap::new(),
        }
    }

    /// 追加一个命名的附加 runtime（准入清单见 crate README）。
    pub fn with_aux(mut self, id: RuntimeId, handle: Handle) -> Self {
        self.aux.insert(id, handle);
        self
    }

    /// 解析 spawn 目标。
    pub fn resolve(&self, id: RuntimeId) -> Result<&Handle, Error> {
        if id == RuntimeId::MAIN {
            return Ok(&self.main);
        }
        self.aux.get(&id).ok_or_else(|| {
            Error::new(
                ErrorKind::Startup,
                format!(
                    "没有注册名为 `{id}` 的 runtime（已注册：{}）：runtime 拓扑是代码事实，不存在回落",
                    self.describe()
                ),
            )
        })
    }

    /// 已注册的 runtime 列表（错误消息与测试用）。
    pub fn describe(&self) -> String {
        let mut names = vec![RuntimeId::MAIN.to_string()];
        names.extend(self.aux.keys().map(ToString::to_string));
        names.join(", ")
    }

    /// 主 runtime 的 handle（装配层关停主 runtime 时用不到它，但测试与探针会用到）。
    pub fn main(&self) -> &Handle {
        &self.main
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_main_and_named_runtimes_only() {
        let main = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("runtime");
        let set = RuntimeSet::new(main.handle().clone());
        assert!(set.resolve(RuntimeId::MAIN).is_ok());
        let err = set
            .resolve(RuntimeId::new("io"))
            .expect_err("未注册的名字必须报错");
        assert_eq!(err.kind(), ErrorKind::Startup);
        assert!(err.to_string().contains("io"), "{err}");
        assert!(err.to_string().contains("main"), "{err}");
    }
}
