//! Runtime 拓扑：全仓库**唯一**建 Tokio runtime 的地方。
//!
//! # 两个类型，两种权限
//!
//! - [`RuntimeSet`]：**所有权**。它持有 `Runtime`，因此它是唯一能关停 runtime 的东西。
//!   它是 `!Send` 的（靠一个 `PhantomData<*const ()>`），于是编译器就拦住了那类最阴的
//!   缺陷：把 `Runtime` 带进 async 上下文里 drop——那会直接 panic，而且 panic 发生在
//!   关停路径上，现场往往已经没人看了。
//! - [`Executors`]：**只读句柄**。装配层拿的是它。`Handle` 可以随便 clone、随便跨线程，
//!   但它关不掉 runtime。「谁能 spawn」和「谁能关」在类型上就是两回事。
//!
//! # 关停顺序
//!
//! 附加 runtime **逆序**先关，主 runtime 最后。逆序是因为后建的更可能依赖先建的；
//! 主 runtime 最后，是因为共享资源（存储池、配置通道）都挂在它上面。
//!
//! 全部发生在 `block_on` 返回之后，也就是**同步**上下文里。这不是风格问题：
//! `Runtime::shutdown_timeout` 会阻塞当前线程等工作线程退出，在 async 上下文里调它
//! 等于让 runtime 等自己。

use std::collections::BTreeMap;
use std::io;
use std::marker::PhantomData;
use std::time::{Duration, Instant};

use service_core::config::{MAIN_RUNTIME_NAME, RuntimeConfig, RuntimeThreads};
use tokio::runtime::{Builder, Handle, Runtime};
use tracing::{error, warn};

/// 建 runtime 失败。
#[derive(Debug, thiserror::Error)]
pub(crate) enum RuntimeError {
    /// `Builder::build()` 失败。名字带上是因为多 runtime 时"哪一个没建起来"是第一问题。
    #[error("failed to build runtime `{name}`")]
    Build {
        /// runtime 名。来自配置的键，已经过校验（只含标识符字符）。
        name: String,
        /// 底层 I/O 错误。**手写 `#[source]` 而不是 `#[from]`**：`#[from]` 只用于
        /// 本 workspace 内部的错误类型。
        #[source]
        source: io::Error,
    },
}

/// 一次 runtime 关停的记录。
///
/// 只记「给了多少预算」和「实际用了多久」，**不记「有没有超时」**：
/// `Runtime::shutdown_timeout` 不返回任何东西，"超时了没"只能靠比时间去猜，而预算为零
/// 且任务已清空时这个猜法必然误报。与其给一个有时说谎的布尔，不如把两个真实数字交出去。
#[derive(Debug, Clone)]
pub(crate) struct RuntimeShutdown {
    /// runtime 名。
    pub(crate) name: String,
    /// 这一步分到的预算。
    pub(crate) budget: Duration,
    /// 实际耗时。
    pub(crate) elapsed: Duration,
}

/// 一个带名字的 runtime。
///
/// `Option<Runtime>` 不是为了表达"可能没有"，而是为了让 [`Drop`] 能和显式关停共存：
/// 正常路径把它 `take()` 走，`Drop` 于是什么都不做。
#[derive(Debug)]
struct NamedRuntime {
    name: String,
    handle: Handle,
    runtime: Option<Runtime>,
}

impl NamedRuntime {
    fn build(name: &str, threads: RuntimeThreads) -> Result<Self, RuntimeError> {
        let mut builder = Builder::new_multi_thread();
        builder
            .enable_all()
            // 线程名带上 runtime 名：`ps -L` / 调试器里能直接看出一个线程属于哪个面的
            // 那一组。tokio 默认的 `tokio-runtime-worker` 在多 runtime 下全都一个样。
            .thread_name(format!("{name}-worker"))
            .worker_threads(resolve_worker_threads(name, threads.worker_threads));
        if let Some(limit) = threads.max_blocking_threads {
            builder.max_blocking_threads(limit);
        }
        let runtime = builder.build().map_err(|source| RuntimeError::Build {
            name: name.to_owned(),
            source,
        })?;
        Ok(Self {
            name: name.to_owned(),
            handle: runtime.handle().clone(),
            runtime: Some(runtime),
        })
    }

    fn shutdown_until(&mut self, deadline: Instant) -> RuntimeShutdown {
        let budget = deadline.saturating_duration_since(Instant::now());
        let started = Instant::now();
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_timeout(budget);
        }
        RuntimeShutdown {
            name: self.name.clone(),
            budget,
            elapsed: started.elapsed(),
        }
    }
}

impl Drop for NamedRuntime {
    fn drop(&mut self) {
        let Some(runtime) = self.runtime.take() else {
            return;
        };
        // 到这里说明关停路径没走完就展开了（panic，或者提前 return）。
        // 在 async 上下文里 drop 一个 `Runtime` 会 panic——而这是 drop，再 panic 一次
        // 就是 abort。所以先探一下，探到了就降级为不阻塞的后台关停。
        if Handle::try_current().is_ok() {
            error!(
                name: "runtime_dropped_in_async_context",
                runtime = %self.name,
                "runtime was dropped inside an async context; falling back to background shutdown"
            );
            runtime.shutdown_background();
            return;
        }
        // 同步上下文：交给 `Runtime` 自己的 `Drop`，它会等工作线程退出。
        drop(runtime);
    }
}

/// 全进程的 runtime 集合。**持有所有权，不可 `Send`。**
#[derive(Debug)]
pub(crate) struct RuntimeSet {
    main: NamedRuntime,
    extras: Vec<NamedRuntime>,
    /// 让整个类型 `!Send` + `!Sync`。
    ///
    /// 这不是装饰：它是"`Runtime` 不得进入 async 上下文"这条纪律的**编译期**载体。
    /// 没有它，一个 `tokio::spawn(async move { drop(set) })` 能通过编译，然后在运行时
    /// 的关停路径上 panic。
    _not_send: PhantomData<*const ()>,
}

impl RuntimeSet {
    /// 按配置建出主 runtime 与全部附加 runtime。
    ///
    /// # Errors
    ///
    /// 任何一个 runtime 建不起来就整体失败——**不**降级成"少一个也能跑"。
    /// 面已经在配置里被指派给了某个 runtime，少一个就是少一个面。
    pub(crate) fn build(cfg: &RuntimeConfig) -> Result<Self, RuntimeError> {
        let main = NamedRuntime::build(MAIN_RUNTIME_NAME, cfg.main())?;
        let mut extras = Vec::with_capacity(cfg.extra.len());
        for (name, threads) in &cfg.extra {
            extras.push(NamedRuntime::build(name, *threads)?);
        }
        Ok(Self {
            main,
            extras,
            _not_send: PhantomData,
        })
    }

    /// 派生只读句柄。
    pub(crate) fn executors(&self) -> Executors {
        Executors {
            main: self.main.handle.clone(),
            extras: self
                .extras
                .iter()
                .map(|r| (r.name.clone(), r.handle.clone()))
                .collect(),
        }
    }

    /// 在主 runtime 上把一个 future 跑到完成。
    ///
    /// 用 `Handle::block_on` 而不是 `Runtime::block_on`，纯粹是为了不必在这里拿到
    /// `&Runtime`——那会逼着上面那个 `Option` 漏到调用处。
    pub(crate) fn block_on<F: Future>(&self, future: F) -> F::Output {
        self.main.handle.block_on(future)
    }

    /// 这个集合里一共有几个 runtime。
    pub(crate) fn len(&self) -> usize {
        1 + self.extras.len()
    }

    /// 关停全部 runtime：附加的逆序先关，主 runtime 最后。
    ///
    /// 必须在 `block_on` **返回之后**调用——它会阻塞当前线程。
    pub(crate) fn shutdown(self, deadline: Instant) -> Vec<RuntimeShutdown> {
        // `Self` 没有 `Drop`（`Drop` 挂在 `NamedRuntime` 上），所以这里可以解构。
        let Self {
            mut main,
            mut extras,
            ..
        } = self;
        extras.reverse();
        let mut records = Vec::with_capacity(extras.len() + 1);
        for extra in &mut extras {
            records.push(extra.shutdown_until(deadline));
        }
        records.push(main.shutdown_until(deadline));
        records
    }
}

/// 只读执行器句柄。装配层拿到的是这个。
#[derive(Debug, Clone)]
pub(crate) struct Executors {
    main: Handle,
    extras: BTreeMap<String, Handle>,
}

impl Executors {
    /// 按面的 `runtime` 键取句柄。`None` 与 `"main"` 都指主 runtime。
    ///
    /// 返回 `Option` 而不是在取不到时 panic：配置校验**已经**拒掉了不存在的名字，
    /// 所以这里的 `None` 是一条本不该发生的路径——但"本不该发生"不是 panic 的理由，
    /// 装配层会把它变成一次普通的启动失败。
    pub(crate) fn resolve(&self, name: Option<&str>) -> Option<&Handle> {
        match name {
            None => Some(&self.main),
            Some(n) if n == MAIN_RUNTIME_NAME => Some(&self.main),
            Some(n) => self.extras.get(n),
        }
    }

    /// 只有主 runtime 的执行器集合。**测试专用。**
    ///
    /// 装配层的用例关心的是"面被绑到了哪个句柄上"，不关心多 runtime 拓扑——而真造一个
    /// [`RuntimeSet`] 要在 async 上下文之外进行（`Runtime::new` 在 runtime 里会 panic），
    /// 那会把每个装配用例的骨架都撑成两层。给个测试构造函数，比让用例绕这个弯便宜。
    #[cfg(test)]
    pub(crate) fn for_tests(main: Handle) -> Self {
        Self {
            main,
            extras: BTreeMap::new(),
        }
    }
}

/// 决定一个 runtime 的工作线程数。
///
/// `available_parallelism()` 失败时退到 1，**并留下一条 `warn`**。
/// 静默退到 1 很坑：一个 32 核的机器上服务只用一个线程，而日志里什么都没有。
fn resolve_worker_threads(name: &str, configured: Option<usize>) -> usize {
    if let Some(n) = configured {
        // `.max(1)` 不是在这里做钳位：`worker_threads = 0` 由 `Config::clamp` 处理，
        // 并且会留下一条 `ClampRecord`。这里只是不让一个"本不该发生"的 0 变成
        // `Builder::worker_threads` 里的 panic——构造函数 panic 会把一次配置错误
        // 伪装成一次程序缺陷。
        return n.max(1);
    }
    match std::thread::available_parallelism() {
        Ok(n) => n.get(),
        Err(err) => {
            warn!(
                name: "available_parallelism_unavailable",
                runtime = %name,
                error = %err,
                "cannot determine available parallelism; falling back to a single worker thread"
            );
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::num::NonZeroUsize;

    use service_core::task::RuntimeId;

    fn cfg_with_extras(extras: &[(&str, Option<usize>)]) -> RuntimeConfig {
        let mut cfg = RuntimeConfig::default();
        for (name, threads) in extras {
            cfg.extra.insert(
                (*name).to_owned(),
                RuntimeThreads {
                    worker_threads: *threads,
                    max_blocking_threads: None,
                },
            );
        }
        cfg
    }

    #[test]
    fn a_default_config_creates_exactly_one_runtime() {
        // 多 runtime 这项能力的准入前提：不配 `[runtime.extra.*]` 就只有主 runtime，
        // 也就没有第二组工作线程。
        let set = RuntimeSet::build(&RuntimeConfig::default()).unwrap();
        assert_eq!(set.len(), 1);
        assert!(set.executors().extras.is_empty());
        drop(set.shutdown(Instant::now() + Duration::from_secs(5)));
    }

    #[test]
    fn single_runtime_shutdown_visits_exactly_one_runtime() {
        // 多 runtime 这项能力在关停上的那点代价：多一段 runtime 要逆序关闭。
        //
        // 这项能力在单 runtime 服务上的全部代价，就是 `shutdown` 里那个跑零次的
        // `for extra in &mut extras`。它便宜到什么程度，看的是**关停记录的条数**：
        // 一条，名字是主 runtime。多出任何一条都意味着有人为"将来可能要用"提前
        // 建了个 runtime——那是为将来预铺的形状，而且它会真的多占一组线程。
        let set = RuntimeSet::build(&RuntimeConfig::default()).unwrap();
        let records = set.shutdown(Instant::now() + Duration::from_secs(5));
        assert_eq!(records.len(), 1, "单 runtime 的关停只该访问一个 runtime");
        assert_eq!(records[0].name, MAIN_RUNTIME_NAME);
    }

    #[test]
    fn resolve_happens_once_at_startup() {
        // 「每个面的段里多一个可选 `runtime` 键」这点代价的落点：
        // 「解析在启动期一次性完成，运行期零分支」。
        //
        // 「一次性」在这里是两条可验的性质，不是一句形容：
        //
        // 1. [`Executors`] 是 [`RuntimeSet::executors`] 产出的**快照**。它只有 `&self`
        //    方法，没有任何一条能往里加东西——所以运行期不存在"第一次用到某个名字时
        //    才去建 runtime"的惰性路径。查不到就是查不到。
        // 2. 同一个名字反复解析，拿到的永远是同一个 runtime。装配层因此可以在启动期
        //    把 `Option<&str>` 换成一个 `Handle` 交给面，之后那个面每次 spawn 都不再
        //    经过任何按名字的分支。
        let set = RuntimeSet::build(&cfg_with_extras(&[("alpha", Some(1))])).unwrap();
        let before = set.len();
        let ex = set.executors();

        let first = RuntimeId::of(ex.resolve(Some("alpha")).unwrap());
        for round in 0..8 {
            assert_eq!(
                RuntimeId::of(ex.resolve(Some("alpha")).unwrap()),
                first,
                "第 {round} 次解析换了一个 runtime"
            );
        }

        // 解析一个不存在的名字既不 panic，也不顺手建一个——它是查表，不是工厂。
        assert!(ex.resolve(Some("nope")).is_none());
        assert_eq!(set.len(), before, "resolve 把集合改大了");

        // 再取一次快照，仍然是同一批 runtime：快照之间没有"谁比谁新"的问题。
        assert_eq!(
            RuntimeId::of(set.executors().resolve(Some("alpha")).unwrap()),
            first
        );
        drop(set.shutdown(Instant::now() + Duration::from_secs(5)));
    }

    /// 在给定 runtime 上跑一格，回报这一格落在哪个线程上。
    ///
    /// 用 `spawn` 而不是直接在 `block_on` 的 future 里读：`Handle::block_on` 的那个
    /// future 跑在**调用线程**（用例线程）上，读到的名字会是 libtest 的线程名。
    fn worker_thread_name(handle: &Handle) -> String {
        handle.block_on(async {
            handle
                .spawn(async {
                    std::thread::current()
                        .name()
                        .unwrap_or("<unnamed>")
                        .to_owned()
                })
                .await
                .expect("这一格不该 panic")
        })
    }

    #[test]
    fn single_runtime_spawns_no_extra_threads() {
        // 多 runtime 这项能力最实的一条承诺：不用它就是零额外线程。
        //
        // 「进程里一共有多少线程」没有可移植的读法（`/proc/self/task` 只有 Linux 有，
        // 别的路子都要 `libc` + `unsafe`，而 `unsafe_code = forbid`）。所以这里验的是
        // 构成它的两件事，两件都是可移植的：
        //
        // ① **有几组**：`Executors` 里除了主 runtime 没有别的句柄；
        // ② **每组多大**：`Handle::metrics().num_workers()`（tokio 1.53.1 的稳定 API，
        //    不在 `tokio_unstable` 后面）给的是这个 runtime 真实的工作线程数。主组恰好
        //    等于机器并行度——没有谁为了"将来可能要开附加 runtime"先切一块出去。
        //
        // 线程名是同一件事在运维侧的投影：工作线程叫 `<runtime 名>-worker`，所以
        // 「有没有第二组」在 `ps -L` 里直接看得见。下面把这个命名也钉住。
        let machine = std::thread::available_parallelism().map_or(1, NonZeroUsize::get);

        let set = RuntimeSet::build(&RuntimeConfig::default()).unwrap();
        let ex = set.executors();
        assert!(ex.extras.is_empty(), "缺省配置不该有第二组工作线程");
        assert_eq!(
            ex.main.metrics().num_workers(),
            machine,
            "主 runtime 该拿到整台机器的并行度"
        );
        assert_eq!(
            worker_thread_name(&ex.main),
            format!("{MAIN_RUNTIME_NAME}-worker")
        );
        drop(set.shutdown(Instant::now() + Duration::from_secs(5)));

        // 正对照：配了才有。这一组是**另一个名字**的线程，且主组一根都没少。
        // 没有这一半，上面那几条断言也可能是因为 `num_workers()` 恒等于机器并行度
        // 而绿的——那样它就什么都没验到。
        let set = RuntimeSet::build(&cfg_with_extras(&[("alpha", Some(2))])).unwrap();
        let ex = set.executors();
        assert_eq!(
            ex.main.metrics().num_workers(),
            machine,
            "多建一个 runtime 不该动主组的大小"
        );
        let alpha = ex.resolve(Some("alpha")).expect("配了就该解析得到");
        assert_eq!(alpha.metrics().num_workers(), 2, "附加组的大小该听配置的");
        assert_eq!(worker_thread_name(alpha), "alpha-worker");
        drop(set.shutdown(Instant::now() + Duration::from_secs(5)));
    }

    #[test]
    fn extras_shut_down_in_reverse_order_with_main_last() {
        // `extra` 是 BTreeMap，所以声明顺序 = 字典序 = 断言可写死。
        let cfg = cfg_with_extras(&[("alpha", Some(1)), ("beta", Some(1))]);
        let set = RuntimeSet::build(&cfg).unwrap();
        assert_eq!(set.len(), 3);
        let order: Vec<String> = set
            .shutdown(Instant::now() + Duration::from_secs(5))
            .into_iter()
            .map(|r| r.name)
            .collect();
        assert_eq!(order, vec!["beta", "alpha", MAIN_RUNTIME_NAME]);
    }

    #[test]
    fn executors_resolve_none_and_main_to_the_same_handle() {
        let set = RuntimeSet::build(&cfg_with_extras(&[("alpha", Some(1))])).unwrap();
        let ex = set.executors();
        let by_none = RuntimeId::of(ex.resolve(None).unwrap());
        let by_name = RuntimeId::of(ex.resolve(Some(MAIN_RUNTIME_NAME)).unwrap());
        assert_eq!(
            by_none, by_name,
            "`None` 和 `\"main\"` 必须是同一个 runtime"
        );
        assert_ne!(
            RuntimeId::of(ex.resolve(Some("alpha")).unwrap()),
            by_none,
            "附加 runtime 必须是另一个"
        );
        assert!(ex.resolve(Some("nope")).is_none(), "不存在的名字返回 None");
        drop(set.shutdown(Instant::now()));
    }

    #[test]
    fn a_configured_thread_count_wins_over_available_parallelism() {
        assert_eq!(resolve_worker_threads("main", Some(3)), 3);
        assert!(resolve_worker_threads("main", None) >= 1);
    }
}
