//! 配置的类型。
//!
//! 六个段，与 [`heat::classify`](super::heat) 解构的六个字段一一对应。**这不是巧合，是
//! 约束**：`classify` 对 `Config` 做无 `..` 的穷尽解构，所以这里每加一个字段，那边就编译
//! 不过，加字段的人必须当场回答"它是热的还是冷的"。
//!
//! 每段都是 `#[serde(default, deny_unknown_fields)]`：
//!
//! - `default` 让配置文件可以只写要改的那几行。一份三行的 `service.toml` 应该能跑起来。
//! - `deny_unknown_fields` 让拼错的字段**报错**而不是被忽略。默默忽略 `bind_add` 的后果
//!   是服务监听在默认地址上，而运维盯着自己写的那行配置查半天。
//!
//! # 默认值里没有部署事实
//!
//! 默认值只够让 `cargo run` 之后有东西可看：回环地址、一个端口、安装根下的数据文件。
//! 没有任何主机名、外部地址或凭据。

use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;

use crate::shutdown::Budgets;

/// 主 runtime 在日志、线程名与报告里使用的名字。
///
/// 它**不是**一个可以写在面的 `runtime` 键里的值——绑定到主 runtime 的写法是**不写那个键**
/// （`resolve(None)` 走的就是这条）。给同一件事留两种写法，就得在两处各维护一遍"它们是同义的"。
///
/// 它同时是一个**保留字**：`[runtime.extra.main]` 会被 [`Config::validate`] 拒绝。否则关停
/// 日志里会出现两个叫 `main` 的 runtime，而"主 runtime 最后关"这条纪律的证据正是那行日志。
pub const MAIN_RUNTIME_NAME: &str = "main";

/// 服务的全部配置。
///
/// `PartialEq`：重载事务要比较新旧两份（冷段的"类型全等"判据依赖它）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// HTTP 面。
    pub http: HttpConfig,
    /// 存储。
    pub storage: StorageConfig,
    /// 后台 worker 面。
    pub worker: WorkerConfig,
    /// Tokio runtime。
    pub runtime: RuntimeConfig,
    /// 关停预算。
    pub shutdown: Budgets,
    /// 日志与追踪。
    pub telemetry: TelemetryConfig,
}

impl Config {
    /// 全部「面 → runtime」绑定，按段名列出。`None` 表示主 runtime。
    ///
    /// 这是**唯一**一处枚举它们的地方：[`validate`](Self::validate) 的两条 runtime 规则
    /// （引用的必须存在、声明的必须被引用）都靠它，`app` 打"哪个面在哪个 runtime 上"
    /// 也靠它。两份清单迟早会差一项，而差出来的那一项正好是没人检查的那个面。
    ///
    /// 解构无 `..`，返回值是**定长数组**：新增一个面（`Config` 多一个段）时，这里既过不了
    /// 解构也过不了长度，加面的人必须当场回答"它跑在哪"。
    #[must_use]
    pub fn plane_runtimes(&self) -> [(&'static str, Option<&str>); 2] {
        let Self {
            http,
            storage: _,
            worker,
            runtime: _,
            shutdown: _,
            telemetry: _,
        } = self;

        [
            ("http.runtime", http.runtime.as_deref()),
            ("worker.runtime", worker.runtime.as_deref()),
        ]
    }
}

/// HTTP 面。
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HttpConfig {
    /// 监听地址。**冷**：换地址要重新 bind，只能重启。
    ///
    /// 默认回环而不是 `0.0.0.0`：一份刚生成的模板不该在第一次 `cargo run` 时就对着
    /// 整个局域网开着。要对外监听是使用者的显式决定。
    pub bind_addr: SocketAddr,

    /// 单个请求的处理超时。**热**：每个请求现读。
    ///
    /// 超时返回 504。实现是自己写的中间件，不是 tower-http 的——理由见 `api` 的 README。
    #[serde(with = "humantime_serde")]
    pub handler_timeout: Duration,

    /// 请求体上限（字节）。**半热**：装在 router 上，面重启才换。
    pub body_limit_bytes: usize,

    /// 这个面跑在哪个附加 runtime 上。`None` = 主 runtime。**冷**。
    ///
    /// 绑定写在面自己的段里，而不是 `[runtime]` 下的一张集中表：集中表读起来是
    /// "runtime 有哪些面"，而实际要回答的问题永远是"这个面在哪跑"。后者要的是把答案放在
    /// 问题旁边。
    pub runtime: Option<String>,
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            bind_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8080),
            handler_timeout: Duration::from_secs(15),
            body_limit_bytes: 1024 * 1024,
            runtime: None,
        }
    }
}

/// 存储。全段**冷**：换库、换池大小都要重建连接池。
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct StorageConfig {
    /// 数据库文件。相对路径锚到安装根，不是锚到配置文件所在目录。
    pub path: PathBuf,

    /// 读连接数。
    ///
    /// 写连接数**不在这里**：SQLite 同一时刻只允许一个写者，写池固定为 1 是它的物理
    /// 性质，不是可以调的旋钮。把它做成配置项，等于邀请使用者去调一个调了就出错的数。
    pub max_readers: u32,

    /// SQLite 的 `busy_timeout`：拿不到锁时在驱动内部重试多久。
    #[serde(with = "humantime_serde")]
    pub busy_timeout: Duration,

    /// 从池里取一条连接的等待上限。
    ///
    /// 与 `busy_timeout` 是两件事：前者是"池里没有空闲连接"，后者是"连接拿到了但库被
    /// 锁着"。两个都配、名字都带 timeout，是这一层最容易混淆的一对，所以两处注释都写明。
    #[serde(with = "humantime_serde")]
    pub acquire_timeout: Duration,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            path: PathBuf::from("data/service.sqlite3"),
            max_readers: 4,
            busy_timeout: Duration::from_secs(5),
            acquire_timeout: Duration::from_secs(10),
        }
    }
}

/// 后台 worker 面。
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WorkerConfig {
    /// 是否启用。**半热**：决定这个面在不在，只能靠面重启切换。
    pub enabled: bool,

    /// tick 间隔。**热**：每一 tick 结束后现读下一次的间隔。
    ///
    /// 热在这里是有意义的——它是模板里唯一一个"改了立刻能观察到效果"的旋钮，
    /// 改一次 `tick_interval` 再发一次 SIGHUP，是观察热重载是否生效最省事的办法。
    #[serde(with = "humantime_serde")]
    pub tick_interval: Duration,

    /// 这个面跑在哪个附加 runtime 上。`None` = 主 runtime。**冷**。理由同 [`HttpConfig::runtime`]。
    pub runtime: Option<String>,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            tick_interval: Duration::from_secs(30),
            runtime: None,
        }
    }
}

/// 一个 runtime 的线程旋钮。主 runtime 与每个附加 runtime 用的是**同一个形状**。
///
/// 同形是有意的：`app` 的 `rt` 模块因此只有一条建 runtime 的代码路径。两个形状会让
/// "主 runtime 的 `worker_threads = 0` 被夹掉了、附加的没有"这类偏差有地方藏。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RuntimeThreads {
    /// 工作线程数。`None` = 交给 Tokio 按可用并行度定。
    ///
    /// `Option` 而不是"0 表示自动"：后者要求读代码的人知道这条约定，而 `None` 自解释。
    pub worker_threads: Option<usize>,

    /// 阻塞线程池上限。`None` = Tokio 默认（512）。
    pub max_blocking_threads: Option<usize>,
}

/// Tokio runtime。全段**冷**：runtime 建好之后线程数不可改，面的归属也不可改。
///
/// **没有 `flavor` 字段**。附加 runtime 只有多线程一种形态：`current_thread`
/// runtime 没有线程对它 `block_on` 时，`Handle::spawn` 进去的任务永不执行——那是一个
/// 不报错、不打日志、只是永远不动的失败。残留的老 `flavor` 键由 `deny_unknown_fields`
/// 当场报错。
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RuntimeConfig {
    /// 主 runtime 的工作线程数。见 [`RuntimeThreads::worker_threads`]。
    pub worker_threads: Option<usize>,

    /// 主 runtime 的阻塞线程池上限。见 [`RuntimeThreads::max_blocking_threads`]。
    pub max_blocking_threads: Option<usize>,

    /// 附加 runtime，键即名字（`[runtime.extra.io]`）。缺省是**空表**。
    ///
    /// `BTreeMap` 而不是 `HashMap`：建 runtime、关 runtime、打日志都要遍历它，而
    /// `HashMap` 的遍历次序每次进程都不同。一个"关停顺序"的断言不该因为哈希种子而翻红。
    ///
    /// 空表时这个能力的全部代价就是这一行 `Default`。
    pub extra: BTreeMap<String, RuntimeThreads>,
}

impl RuntimeConfig {
    /// 主 runtime 的线程旋钮，形状与附加 runtime 相同。
    #[must_use]
    pub const fn main(&self) -> RuntimeThreads {
        RuntimeThreads {
            worker_threads: self.worker_threads,
            max_blocking_threads: self.max_blocking_threads,
        }
    }

    /// 写回主 runtime 的线程旋钮。
    ///
    /// 存在的唯一理由是让钳位对主 runtime 与附加 runtime 走同一段代码
    /// （见 `pipeline.rs` 的 `clamp_threads`）。
    pub const fn set_main(&mut self, threads: RuntimeThreads) {
        self.worker_threads = threads.worker_threads;
        self.max_blocking_threads = threads.max_blocking_threads;
    }
}

/// 日志输出格式。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogFormat {
    /// 人读的单行文本。默认——模板的第一次运行是在终端里。
    #[default]
    Text,
    /// 每行一个 JSON 对象。
    Json,
}

impl LogFormat {
    /// 报告与日志里使用的稳定短名。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Json => "json",
        }
    }
}

/// 日志与追踪。全段**冷**。
///
/// 冷的理由是实现上的，值得写下来：`EnvFilter` 在初始化时装进 subscriber，而这个模板
/// **不**装 `reload::Layer`。没有 reload handle 就没有改过滤器的途径，
/// 所以它只能是冷的——把它标成热的会让重载报告说"已生效"，而实际什么都没变。
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TelemetryConfig {
    /// `EnvFilter` 指令串，例如 `info,tower_http=debug`。
    ///
    /// 语法由 `tracing-subscriber` 定义，而 `core` 不依赖它（库层不碰 subscriber），
    /// 因此这里只能验"非空"。真正的语法错误在 `app` 初始化 subscriber 时暴露。
    pub filter: String,

    /// 输出格式。
    pub format: LogFormat,
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            filter: String::from("info"),
            format: LogFormat::Text,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_contain_no_deployment_facts() {
        // 回归用例：默认值里不能出现任何主机名、外部地址或凭据。
        let cfg = Config::default();
        assert!(
            cfg.http.bind_addr.ip().is_loopback(),
            "默认监听地址必须是回环——刚生成的模板不该对外开着"
        );
        assert!(
            cfg.storage.path.is_relative(),
            "默认数据库路径必须是相对的，才能锚到安装根"
        );
    }

    #[test]
    fn default_budgets_leave_headroom_under_the_hard_bound() {
        let b = Config::default().shutdown;
        let sum = b.harvest + b.reap + b.storage_close + b.runtime_shutdown;
        assert!(
            sum < b.total_grace,
            "各段之和 {sum:?} 必须小于总边界 {:?}，否则最后一段默认就超时",
            b.total_grace
        );
    }

    #[test]
    fn an_empty_document_yields_the_defaults() {
        // 三行配置能跑起来，靠的就是这条。
        let parsed: Config = toml_like_empty();
        assert_eq!(parsed, Config::default());
    }

    /// 模拟"空文档"的反序列化。
    ///
    /// `core` 不依赖任何格式解析库，所以这里用 serde 自带的 `MapDeserializer` 喂一个
    /// 空映射，走一遍 `#[serde(default)]` 的路径。验的是默认值补全，不是 TOML 的行为——
    /// 后者是 `app` 的用例。
    fn toml_like_empty() -> Config {
        let empty = serde::de::value::MapDeserializer::<_, serde::de::value::Error>::new(
            std::iter::empty::<(&str, &str)>(),
        );
        Config::deserialize(empty).unwrap_or_else(|e| panic!("空文档不该失败: {e}"))
    }

    #[test]
    fn log_format_names_are_stable() {
        assert_eq!(LogFormat::Text.as_str(), "text");
        assert_eq!(LogFormat::Json.as_str(), "json");
        assert_eq!(LogFormat::default(), LogFormat::Text);
    }
}
