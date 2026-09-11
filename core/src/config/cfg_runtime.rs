//! Tokio runtime 子配置（`[runtime]` 段）
//!
//! 主 runtime 的线程预算，以及**可选**的附加 runtime（`[runtime.extra.<name>]`）。
//! 缺省没有任何附加 runtime：单 runtime 服务不为多 runtime 付一行配置。
//!
//! runtime 在 `block_on` 之前就建好了，整段不可热变更（`ConfigStore` 白名单把
//! `runtime.` 前缀归入 `requires_restart`）。各面的绑定字段 `<面>.runtime` 同理不可热。
//!
//! 定义在 core 而消费在 app：配置类型属于「所有 crate 共享、不含常驻任务」的内容；
//! 造 runtime 的代码在装配层。
//!
//! # 什么时候值得开附加 runtime
//!
//! I/O 密集的面一律不值得。只有量出下面三种情况之一才开：CPU 密集的 async 循环
//! 拖高别的面的延迟；阻塞型 FFI 打满 blocking 池；某个面有独立的 P99 目标要隔离。
//! 准入清单见生成项目 README「怎么给一个面单开 runtime」。
//!
//! # 为什么没有 `current_thread` 形态
//!
//! 附加 runtime 在本模板里只当 spawn 目标：没有任何线程对它调 `block_on`。
//! `new_current_thread()` 建出来的 runtime 没有自己的驱动线程，`Handle::spawn`
//! 只是把任务塞进队列，要等有人 `block_on` 才轮询——于是任务永不执行，`await`
//! 它的 `JoinHandle` 直接挂死。要让它可用得额外配一条专用 OS 驱动线程和一条
//! 独立关停路径，而 `worker_threads = 1` 的多线程 runtime 已经给到同样的隔离
//! （独占一个 worker、不与别人窃取），还自带驱动。所以只留一种形态。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::clamp::clamp_field;

/// `[runtime]` 段完整配置。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RuntimeConfig {
    /// 主 runtime 的 worker 线程数；0 = 按 CPU 数定（启动日志报的是解析后的具体值）
    pub worker_threads: usize,
    /// 主 runtime 的 blocking 池上限（`spawn_blocking` 与 sqlx 的 SQLite 驱动都吃这个预算）
    pub max_blocking_threads: usize,
    /// 附加 runtime：每个子表一个，键是名字（面的配置段用 `runtime = "<name>"` 绑定）。
    /// `BTreeMap` 而不是 `HashMap`：构建与关停顺序要确定，日志才可复现
    pub extra: BTreeMap<String, ExtraRuntimeConfig>,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            worker_threads: 0,
            // tokio 自己的缺省值；显式写出来是让配置文件成为文档
            max_blocking_threads: 512,
            extra: BTreeMap::new(),
        }
    }
}

impl RuntimeConfig {
    pub(super) fn sanitize(&mut self) {
        clamp_field(
            &mut self.worker_threads,
            0,
            1024,
            "runtime",
            "worker_threads",
        );
        clamp_field(
            &mut self.max_blocking_threads,
            1,
            4096,
            "runtime",
            "max_blocking_threads",
        );
        for (name, extra) in &mut self.extra {
            let section = format!("runtime.extra.{name}");
            clamp_field(
                &mut extra.worker_threads,
                1,
                1024,
                &section,
                "worker_threads",
            );
            clamp_field(
                &mut extra.max_blocking_threads,
                1,
                4096,
                &section,
                "max_blocking_threads",
            );
        }
    }

    /// 某个面声明的绑定是否指向已配置的附加 runtime。`None` 恒合法（主 runtime）。
    pub fn has_runtime(&self, name: Option<&str>) -> bool {
        name.is_none_or(|n| self.extra.contains_key(n))
    }
}

/// `[runtime.extra.<name>]`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ExtraRuntimeConfig {
    /// 附加 runtime 的缺省故意小：它是隔离手段，不是扩容手段。
    /// `1` 即「独占一个 worker、不与别人窃取」，是单线程隔离的写法
    pub worker_threads: usize,
    pub max_blocking_threads: usize,
}

impl Default for ExtraRuntimeConfig {
    fn default() -> Self {
        Self {
            worker_threads: 1,
            max_blocking_threads: 16,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_toml_yields_defaults_with_no_extra_runtimes() {
        let cfg: RuntimeConfig = toml::from_str("").unwrap();
        assert_eq!(cfg, RuntimeConfig::default());
        assert!(cfg.extra.is_empty());
        assert!(cfg.has_runtime(None));
        assert!(!cfg.has_runtime(Some("compute")));
    }

    /// `[runtime.extra.<name>]` 子表形态；省掉的字段取缺省
    #[test]
    fn extra_runtimes_parse_from_subtables() {
        let cfg: RuntimeConfig =
            toml::from_str("worker_threads = 4\n[extra.compute]\nworker_threads = 2\n[extra.io]\n")
                .unwrap();
        assert_eq!(cfg.worker_threads, 4);
        assert_eq!(cfg.extra["compute"].worker_threads, 2);
        assert_eq!(cfg.extra["io"].worker_threads, 1);
        assert_eq!(cfg.extra["io"].max_blocking_threads, 16);
        assert!(cfg.has_runtime(Some("compute")));
    }

    #[test]
    fn sanitize_clamps_main_and_extra_budgets() {
        let mut cfg: RuntimeConfig =
            toml::from_str("max_blocking_threads = 0\n[extra.c]\nworker_threads = 0\n").unwrap();
        cfg.sanitize();
        assert_eq!(cfg.max_blocking_threads, 1);
        assert_eq!(cfg.extra["c"].worker_threads, 1);
    }

    /// `deny_unknown_fields`：写错字段名当场报错，不静默忽略。
    /// `flavor` 曾经存在，现在删了——老配置会红，这是想要的
    #[test]
    fn unknown_extra_field_is_rejected() {
        assert!(
            toml::from_str::<RuntimeConfig>("[extra.c]\nflavor = \"current_thread\"\n").is_err()
        );
    }
}
