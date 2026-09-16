//! 面向 handler 的共享状态。
//!
//! # 为什么是一个结构体而不是四个 `Extension`
//!
//! `Extension<T>` 的取值在**运行期**才失败：漏装一个，编译照过，第一次请求打到那个
//! handler 才 500。`State<AppState>` 是编译期的——少给一件零件，`Router::with_state`
//! 就通不过。这一层要的是"零业务全局态"，而把全局态换成一个运行期才验的 map，只是把
//! 全局变量改了个名字。
//!
//! # 为什么它必须是 `Clone` 而且**便宜**
//!
//! axum 对每个请求克隆一次 state。四个字段分别是 `Arc`、两个 `watch::Receiver` 句柄和
//! 一个 `Copy` 的结构体——克隆代价是几次计数加一，没有一次分配。任何人往这里加字段时
//! 都得先过这一关：一个 `String` 字段意味着每个请求一次堆分配。
//!
//! # 起始时刻在这里，不在 `app`
//!
//! `/v1/info` 的 uptime 要有个起点。把它做成进程级常量（`OnceLock` / `static`）会直接
//! 违背"一个进程里可以跑两套独立装配"：那时两套的 uptime 会指向同一个时刻，而它们本来
//! 就该各有各的。所以起点在 [`AppState::new`] 里盖章，一套装配一个。
//!
//! `started` 是私有字段，于是这个结构体**无法**用结构体字面量在 crate 外构造——想拿到
//! 一个 `AppState` 就只能走 `new`，也就只能拿到一个真实的起点。

use std::sync::Arc;
use std::time::{Duration, Instant};

use service_core::build_info::BuildInfo;
use service_core::config::ConfigReader;
use service_core::lifecycle::LifecycleReader;
use service_core::storage::Storage;

/// handler 能看到的全部东西。
///
/// 四个公开字段就是 handler 需要的全部；`started` 是私有的，理由见模块文档。
#[derive(Clone, Debug)]
pub struct AppState {
    /// 存储门面。**是 `dyn`，不是具体后端**——`api → storage` 在邻接表里是禁止的，
    /// 这一层连 sqlx 的名字都念不出来。
    pub storage: Arc<dyn Storage>,

    /// 生命周期读端。`/readyz` 的第一道判据。
    pub lifecycle: LifecycleReader,

    /// 配置读端。**热字段每请求现读**（`handler_timeout` 就是这么取的），
    /// 不在这里固化成一个值——固化了就等于把热字段悄悄降级成半热。
    pub config: ConfigReader,

    /// 进程身份。`/v1/info` 的 `service` 与 `version` 的唯一来源
    /// （不在这一层另写一个字符串常量）。
    pub build: BuildInfo,

    /// 这一套装配的起始时刻。私有，见模块文档。
    started: Instant,
}

impl AppState {
    /// 组装一套状态，并把**此刻**记为这套装配的起点。
    #[must_use]
    pub fn new(
        storage: Arc<dyn Storage>,
        lifecycle: LifecycleReader,
        config: ConfigReader,
        build: BuildInfo,
    ) -> Self {
        Self {
            storage,
            lifecycle,
            config,
            build,
            started: Instant::now(),
        }
    }

    /// 从 [`AppState::new`] 到现在过了多久。
    #[must_use]
    pub fn uptime(&self) -> Duration {
        self.started.elapsed()
    }
}
