//! Tokio runtime 装配：按 `[runtime]` 建主 runtime 与附加 runtime，把 `Handle` 集合
//! 交给装配层。造 runtime 不是「所有 crate 共享」的内容，所以在 app 而不在 core。
//!
//! # 三条跨 runtime 规则
//!
//! 1. **通道与令牌天然跨 runtime**：`tokio::sync` 与 `CancellationToken` 不持有 runtime，
//!    根令牌的级联、事件总线、配置句柄都不需要为多 runtime 做任何事；
//! 2. **IO 资源随创建时的 runtime 注册**：`TcpListener` / 连接池在哪个 runtime 里创建，
//!    就靠哪个 runtime 的驱动醒（`from_std` 在没有 IO 驱动的上下文里直接 panic）。
//!    共享资源只在主 runtime 创建，主 runtime 最后关；面私有的 IO 在面的 future 里创建
//!    （HTTP 走「同步 bind + future 内 from_std」）；
//! 3. **阻塞与预算按 runtime 分**：`spawn_blocking` 落在当前 runtime 的 blocking 池；
//!    不得在 runtime 线程上 `Handle::block_on`（tokio 直接 panic）；`block_in_place`
//!    只在多线程 runtime 的 worker 线程上合法，模板不用它做跨 runtime 调用。
//!
//! 推论：附加 runtime 只能是多线程形态。这里没有一条线程会对附加 runtime 调
//! `block_on`，而 `new_current_thread()` 的 runtime 要有人 `block_on` 才轮询任务，
//! 光 `Handle::spawn` 进去的任务永不执行。要单线程隔离就写 `worker_threads = 1`。
//!
//! # 为什么附加 runtime 在 `block_on` 之外同步关
//!
//! 在 async 上下文里 drop 一个 `Runtime` 会 panic（tokio："Cannot drop a runtime in a
//! context where blocking is not allowed"）；`shutdown_background` 虽不 panic 但不等
//! 任务收尾，顺序就看不出来了。生产项目踩的是同一条约束：risingwave 的包装类型注释
//! 「父 runtime 里不能直接 drop 嵌套 runtime」、databend 的「同 runtime 的 worker 上
//! join 会死锁」、influxdb3 把专用 runtime 放到独立 OS 线程再 `shutdown_timeout`。
//! 模板选最简单的一种：`main` 在 `block_on` 返回之后逐个 `shutdown_timeout`。

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use tokio::runtime::{Builder, Handle, Runtime};

use {{crate_prefix_snake}}_core::config::{ConfigError, ExtraRuntimeConfig, RuntimeConfig};
use {{crate_prefix_snake}}_core::{AppError, AppResult};

/// 进程持有的全部 runtime。由 `main` 拥有；`block_on` 之外同步关。
pub struct Runtimes {
    main: Runtime,
    /// 按配置顺序（`BTreeMap` 的键序）；关停时逆序
    extra: Vec<(String, Runtime)>,
}

impl Runtimes {
    /// 按配置建主 runtime 与全部附加 runtime。线程名 `<name>-<seq>`：日志的
    /// `threadName` 字段与 `ps -M` 里能认出线程属于谁。
    pub fn build(cfg: &RuntimeConfig) -> std::io::Result<Self> {
        let mut builder = Builder::new_multi_thread();
        builder.enable_all();
        // 配置里的 0 = 「按 CPU 数定」。先解析成具体数字再建，为的是启动日志：
        // `worker_threads: 0` 在现场会被读成「没有 worker 线程」，而这条日志的用处
        // 正是回答「这个进程现在是什么形状」。口径与 tokio 的默认一致。
        // 代价是 TOKIO_WORKER_THREADS 不再有机会插手——runtime 形状由 [runtime] 段
        // 决定是本模板的既定口径，环境变量绕过配置文件才是意外
        let worker_threads = match cfg.worker_threads {
            0 => std::thread::available_parallelism().map_or(1, |n| n.get()),
            n => n,
        };
        builder.worker_threads(worker_threads);
        builder.max_blocking_threads(cfg.max_blocking_threads);
        builder.thread_name_fn(thread_namer("main"));
        let main = builder.build()?;
        tracing::info!(
            worker_threads,
            max_blocking_threads = cfg.max_blocking_threads,
            "main runtime built"
        );

        let mut extra = Vec::with_capacity(cfg.extra.len());
        for (name, spec) in &cfg.extra {
            let runtime = build_extra(name, spec)?;
            tracing::info!(
                runtime = %name,
                worker_threads = spec.worker_threads,
                max_blocking_threads = spec.max_blocking_threads,
                "extra runtime built"
            );
            extra.push((name.clone(), runtime));
        }
        Ok(Self { main, extra })
    }

    /// Handle 集合，Clone 廉价；进 `RuntimeState` 供任务注册解析。
    pub fn executors(&self) -> Executors {
        Executors {
            main: self.main.handle().clone(),
            extra: Arc::new(
                self.extra
                    .iter()
                    .map(|(name, rt)| (name.clone(), rt.handle().clone()))
                    .collect(),
            ),
        }
    }

    pub fn block_on<F: Future>(&self, f: F) -> F::Output {
        self.main.block_on(f)
    }

    /// 同步关停；只能在 `block_on` 返回之后调。附加 runtime 逆序先关，主 runtime 最后：
    /// 共享资源（存储池）活在主 runtime 上，面可能在最后一刻还在用它们。
    pub fn shutdown(self, grace: Duration) {
        for (name, runtime) in self.extra.into_iter().rev() {
            tracing::info!(runtime = %name, grace = ?grace, "shutting down extra runtime");
            runtime.shutdown_timeout(grace);
            tracing::info!(runtime = %name, "extra runtime stopped");
        }
        tracing::info!(grace = ?grace, "shutting down main runtime");
        self.main.shutdown_timeout(grace);
        tracing::info!("main runtime stopped");
    }
}

/// 只建多线程形态，理由见模块头第 3 条推论。`worker_threads = 1` 即单线程隔离。
fn build_extra(name: &str, spec: &ExtraRuntimeConfig) -> std::io::Result<Runtime> {
    let mut builder = Builder::new_multi_thread();
    builder.worker_threads(spec.worker_threads);
    builder.enable_all();
    builder.max_blocking_threads(spec.max_blocking_threads);
    builder.thread_name_fn(thread_namer(name));
    builder.build()
}

/// `<name>-<seq>` 的线程命名器。每个 runtime 一个计数器，序号从 0 起。
fn thread_namer(name: &str) -> impl Fn() -> String + Send + Sync + 'static {
    let name = name.to_owned();
    let seq = Arc::new(AtomicUsize::new(0));
    move || format!("{name}-{}", seq.fetch_add(1, Ordering::Relaxed))
}

/// 各 runtime 的 `Handle` 集合（Clone 廉价）。
#[derive(Clone)]
pub struct Executors {
    main: Handle,
    extra: Arc<HashMap<String, Handle>>,
}

impl Executors {
    /// 按面的 `runtime` 绑定解析：`None` → 主 runtime；`Some(name)` → 同名附加 runtime。
    ///
    /// 未配置的名字是错误而不是回落：配置管线已经在加载期拦过一次，这里是绕过
    /// 管线直接装配时的最后一道网。回落会把「配置写错」藏成「性能不达预期」
    pub fn resolve(&self, name: Option<&str>) -> AppResult<&Handle> {
        match name {
            None => Ok(&self.main),
            Some(name) => self.extra.get(name).ok_or_else(|| {
                AppError::Config(ConfigError::Invalid(format!(
                    "runtime binding {name:?} is not declared under [runtime.extra]"
                )))
            }),
        }
    }

    /// 测试用：以当前 `#[tokio::test]` runtime 充当主 runtime，无附加 runtime。
    #[cfg(test)]
    pub fn from_current() -> Self {
        Self {
            main: Handle::current(),
            extra: Arc::new(HashMap::new()),
        }
    }
}

#[cfg(test)]
mod tests {
    use {{crate_prefix_snake}}_testkit::LogCapture;

    use super::*;

    fn config_with_extra(name: &str, worker_threads: usize) -> RuntimeConfig {
        let mut cfg = RuntimeConfig::default();
        cfg.extra.insert(
            name.to_owned(),
            ExtraRuntimeConfig {
                worker_threads,
                ..ExtraRuntimeConfig::default()
            },
        );
        cfg
    }

    /// 让一个任务报告自己跑在哪个线程上：这是「绑定生效」的直接证据
    fn thread_name_on(runtimes: &Runtimes, handle: &Handle) -> String {
        runtimes.block_on(async {
            handle
                .spawn(async { std::thread::current().name().map(str::to_owned) })
                .await
                .expect("probe task")
                .unwrap_or_default()
        })
    }

    /// 绑到附加 runtime 的任务跑在以该 runtime 命名的线程上；缺省绑定跑在 main-* 上。
    /// 同步测试：Runtime 不能在 async 上下文里建与关
    #[test]
    fn tasks_land_on_the_runtime_they_are_bound_to() {
        let runtimes = Runtimes::build(&config_with_extra("compute", 2)).expect("build runtimes");
        let executors = runtimes.executors();

        let on_compute = thread_name_on(&runtimes, executors.resolve(Some("compute")).unwrap());
        assert!(on_compute.starts_with("compute-"), "{on_compute}");

        let on_main = thread_name_on(&runtimes, executors.resolve(None).unwrap());
        assert!(on_main.starts_with("main-"), "{on_main}");

        runtimes.shutdown(Duration::from_millis(100));
    }

    /// `worker_threads = 1` 是单线程隔离的写法，任务照样跑得起来：
    /// 这条测试守着「不要再引入 current_thread 形态」——它建出来的 runtime
    /// 没有驱动线程，同样的探针会永远挂在 `await` 上
    #[test]
    fn a_single_worker_runtime_still_runs_its_tasks() {
        let runtimes = Runtimes::build(&config_with_extra("io", 1)).expect("build runtimes");
        let executors = runtimes.executors();
        let name = thread_name_on(&runtimes, executors.resolve(Some("io")).unwrap());
        assert_eq!(name, "io-0", "单 worker 的 runtime 只有一条 worker 线程");
        runtimes.shutdown(Duration::from_millis(100));
    }

    /// 未声明的名字是错误，不静默回落到主 runtime
    #[test]
    fn unknown_binding_is_an_error_not_a_fallback() {
        let runtimes = Runtimes::build(&RuntimeConfig::default()).expect("build runtimes");
        let executors = runtimes.executors();
        let error = executors
            .resolve(Some("typo"))
            .expect_err("未声明的名字应报错");
        assert!(error.to_string().contains("typo"), "{error}");
        runtimes.shutdown(Duration::from_millis(100));
    }

    /// 关停顺序在日志里可见：附加 runtime 逆序先关，主 runtime 最后
    #[test]
    fn shutdown_order_is_extra_runtimes_reversed_then_main() {
        let mut cfg = RuntimeConfig::default();
        cfg.extra.insert("a".into(), ExtraRuntimeConfig::default());
        cfg.extra.insert("b".into(), ExtraRuntimeConfig::default());
        let runtimes = Runtimes::build(&cfg).expect("build runtimes");

        let capture = LogCapture::default();
        capture.capture(|| runtimes.shutdown(Duration::from_millis(100)));
        let log = capture.contents();

        let at = |needle: &str| {
            log.find(needle)
                .unwrap_or_else(|| panic!("缺 {needle}: {log}"))
        };
        let b_down = at("runtime=b grace");
        let a_down = at("runtime=a grace");
        let main_down = at("shutting down main runtime");
        let main_stopped = at("main runtime stopped");
        assert!(
            b_down < a_down && a_down < main_down && main_down < main_stopped,
            "{log}"
        );
    }
}
