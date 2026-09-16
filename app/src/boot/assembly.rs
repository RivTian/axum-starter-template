//! 装配：把一份配置变成一组**造好但还没跑**的面。
//!
//! # 两段式
//!
//! ```text
//! assemble(config, build, executors) -> Result<Assembled, BootError>   // 纯构造，零 spawn
//! Assembled::launch(self)            -> Running                        // 唯一 spawn 点
//! ```
//!
//! 分成两段不是为了好看，是为了让"装配失败"和"运行中失败"成为**类型能回答的问题**：手上
//! 是 [`Assembled`] 就说明还没有任何任务在跑，于是失败路径直接 drop 即可，不需要收割；手上
//! 是 [`Running`] 才意味着 `abort_boot` / 关停编排那一套要接手。没有这条边界的话，
//! "要不要收割"就只能靠读代码顺序判断——而代码顺序是会被插入一行改掉的。
//!
//! # 存储是最后一件被打开的东西
//!
//! [`assemble`] 里的可失败步骤按这个顺序排，而顺序本身是一条不变量：
//!
//! | # | 步骤 | 失败了要收拾什么 |
//! | --- | --- | --- |
//! | ① | 把两个面各自绑到配置指定的 runtime 上 | 什么都不用 |
//! | ② | `bind()` 监听端口 | 什么都不用（`TcpListener` 随错误一起没了） |
//! | ③ | 打开存储 | 由 `StorageOwner::open` 自己收拾——它保证"要么返回一个开着的池，要么一个都不剩" |
//!
//! ③ **之后再没有任何可失败的步骤**。于是 `assemble` 里不存在"存储已经开着、但我要返回
//! `Err`"这条路径——这正是 `bind_failure_aborts_boot_with_the_original_error` 守的东西：
//! 端口被占时不该有人先跑一遍数据库迁移再回滚。
//!
//! 反过来说，这条不变量是**靠排序**维持的，不是靠类型。往 ③ 后面加可失败步骤的人必须
//! 同时回答"存储谁来关"。这句话写在这里，是因为它在代码里看不出来。
//!
//! # 为什么 [`Assembled`] 里装的是 `PlaneFuture` 而不是 `Router`
//!
//! `app` 不认识 `axum`（HTTP 展示层只有 `api` 能依赖它）。但 [`PlaneFuture`] 是 `core`
//! 的类型，而 `HttpPlane::build(listener, router, ack, cancel)` 的返回值正是它——于是装配层
//! 可以在 `api` 内部把 `Router` 造完、立刻把它关进一个 future 里，自己手上只剩一个 `core`
//! 类型。**一个没被 poll 过的 future 什么都没启动**，所以这一步仍然是「纯构造，零 spawn」。
//!
//! # `launch` 为什么不返回 `Result`
//!
//! 它唯一可能失败的调用是 [`TaskSupervisor::spawn_on`]，而那只在**同名面重复登记**时失败。
//! 这里登记的是 [`HttpPlane::SPEC`] 与 [`TickerPlane::SPEC`] 两个互不相同的常量，各最多一次。
//!
//! 万一它真的失败了（有人往 [`Assembled::planes`] 里塞了第三个同名面），这里**不另设失败
//! 通道**：没能 spawn 的那个 future 在原地被 drop，它捕获的 `AckSender` 随之消失，提交门
//! 会以 `ack_dropped` 中止启动——一条已经存在、已经被测试的失败路径。同时留一条 `error!`
//! 说明是登记出的问题，否则日志里只看得见"某个面没回执"。

use std::net::{SocketAddr, TcpListener};
use std::sync::Arc;

use service_api::{AppState, HttpPlane, bind, business_routes, router};
use service_core::build_info::BuildInfo;
use service_core::config::{Config, ConfigPublisher};
use service_core::lifecycle::LifecyclePublisher;
use service_core::storage::{Storage, StorageError};
use service_core::task::{
    AckReceiver, AckSender, PlaneFuture, TaskName, TaskSpec, TaskSupervisor, ack_channel,
};
use service_storage::StorageOwner;
use service_worker::TickerPlane;
use tokio::runtime::Handle;
use tokio_util::sync::CancellationToken;

use crate::boot::gate::AckSet;
use crate::rt::Executors;

/// 装配阶段的失败。
///
/// 三个变体一一对应上面那张表里的三步。**没有 `#[from]`**：`std::io::Error` 与
/// `StorageError` 都可能从别处冒出来，自动转换会让"这个错到底是哪一步产生的"变成读代码
/// 才知道的事，而 `kind()` 正是要把那个答案固定下来。
#[derive(Debug, thiserror::Error)]
pub(crate) enum BootError {
    /// 配置把某个面指派给了一个不存在的 runtime。
    ///
    /// 配置管线**已经**校验过这件事（`Config::plane_runtimes`），所以这条路径本不该发生。
    /// 但"本不该发生"不是 panic 的理由：这里把它变成一次普通的启动失败。
    #[error("plane `{plane}` is bound to runtime `{runtime}`, which does not exist")]
    UnknownRuntime {
        /// 哪个面。
        plane: TaskName,
        /// 配置里写的那个 runtime 名。
        runtime: String,
    },

    /// 监听端口绑不上：被占、地址不合法、或者权限不够（1024 以下）。
    #[error("cannot bind {addr}")]
    Bind {
        /// 试图绑定的地址。
        addr: SocketAddr,
        /// 操作系统给的原因。
        #[source]
        source: std::io::Error,
    },

    /// 存储打不开。
    #[error("cannot open storage")]
    Storage(#[source] StorageError),
}

impl BootError {
    /// 失败的大类。进 [`RunReport::startup_failed`] 的第一个参数，也进日志。
    ///
    /// [`RunReport::startup_failed`]: crate::lifecycle::report::RunReport
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Self::UnknownRuntime { .. } => "runtime",
            Self::Bind { .. } => "bind",
            Self::Storage(_) => "storage",
        }
    }
}

/// 一个造好了、但还没被 spawn 的面。
struct PreparedPlane {
    spec: TaskSpec,
    /// 已经解析好的目标 runtime。**在 `assemble` 里解析一次**，`launch` 不再查表——
    /// `resolve_happens_once_at_startup` 守的就是这件事。
    handle: Handle,
    future: PlaneFuture,
}

/// 手写 `Debug`：`dyn Future` 派生不出来。只打规格，够定位了。
impl std::fmt::Debug for PreparedPlane {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedPlane")
            .field("spec", &self.spec)
            .finish_non_exhaustive()
    }
}

/// 装配完成、尚未启动的一整套东西。
///
/// **持有它就说明一个任务都没在跑。** 这就是这条类型边界的全部内容。
#[derive(Debug)]
pub(crate) struct Assembled {
    planes: Vec<PreparedPlane>,
    acks: Vec<AckReceiver>,
    lifecycle: LifecyclePublisher,
    config: ConfigPublisher,
    storage_owner: StorageOwner,
    storage: Arc<dyn Storage>,
    cancel: CancellationToken,
}

/// 已经跑起来的一整套东西。
///
/// 字段对 crate 内公开：主编排循环要逐个用到它们，而给每个字段配一个 getter 只是把同一份
/// 可见性写两遍。
#[derive(Debug)]
pub(crate) struct Running {
    /// 在册的面。提交门与主循环都从这里拿退出记录。
    pub(crate) supervisor: TaskSupervisor,
    /// 还没收齐的回执。提交门的谓词就建在它上面。
    pub(crate) acks: AckSet,
    /// 相位发布端。全进程唯一的写者。
    pub(crate) lifecycle: LifecyclePublisher,
    /// 配置发布端。热重载走它。
    pub(crate) config: ConfigPublisher,
    /// 存储的所有权。关停序列第三段 `close(d3)` 要用。
    pub(crate) storage_owner: StorageOwner,
    /// 存储门面的一份引用。
    ///
    /// 留着它**不是**为了用，是为了数：关停到第三段时，
    /// `Arc::strong_count` 回到 1 才说明没有别的面还攥着这个池——那正是
    /// `CloseOutcome::SkippedUnproven` 想要的那份"证据"。
    pub(crate) storage: Arc<dyn Storage>,
    /// 根取消令牌。级联关停从它发起。
    pub(crate) cancel: CancellationToken,
}

/// 造出全部共享资源与各个面，**不启动任何任务**。
///
/// `config` 按值收下：它立刻被交给 [`ConfigPublisher`]，之后全进程只经由 `watch` 读它。
/// 装配期间自己要用的那几个字段从 `current()` 的快照上读——这样"装配读的配置"和"面运行时
/// 读的配置"从第一刻起就是同一份，不会出现两份各自演化的副本。
///
/// # Errors
///
/// 见 [`BootError`]。三条路径都发生在存储被打开**之前或之中**，于是没有一条需要调用方
/// 去关存储。
pub(crate) async fn assemble(
    config: Config,
    build: BuildInfo,
    executors: &Executors,
) -> Result<Assembled, BootError> {
    let cancel = CancellationToken::new();
    // 两个发布端各自带回来的那份读端在这里用不上：下面每处要读端都从发布端现取
    // （`reader()`），这样"谁在读"在调用点就看得见。丢掉它们是安全的——两处 `publish` 都用
    // `send_replace`，没有接收端时不报错。
    let (lifecycle, _) = LifecyclePublisher::new();
    let (config_publisher, _) = ConfigPublisher::new(config);

    let snapshot = config_publisher.current();
    let cfg = snapshot.config();

    // ① 先把两个面各自的 runtime 解析出来。查表在这里做一次，`launch` 只认句柄。
    let http_runtime = resolve(executors, TaskName::Http, cfg.http.runtime.as_deref())?;
    let ticker_runtime = resolve(executors, TaskName::Ticker, cfg.worker.runtime.as_deref())?;

    // ② 端口。同步 bind，失败就是一次启动失败——不能拖到面的 future 里去（见 `api::bind`
    //    的模块文档：那样编排器会先看见一个"活着"的进程）。
    let listener = bind(cfg.http.bind_addr).map_err(|source| BootError::Bind {
        addr: cfg.http.bind_addr,
        source,
    })?;
    // 端口写 0 时内核给的是哪个，只有这里知道。日志里留一条，否则测试环境下没人找得到它。
    let bound = listener.local_addr().unwrap_or(cfg.http.bind_addr);
    // 两个地址都记：只记 `addr` 的话，"配了 0、内核给了 54321"和"配了 54321"在日志里
    // 长得一模一样。消息不能省——一条只有字段的记录在人眼读的那份输出里是一行裸键值对。
    tracing::info!(
        name: "listener_bound",
        addr = %bound,
        configured = %cfg.http.bind_addr,
        "the http listener is bound"
    );

    // ③ 存储。**最后一件可失败的事**（见模块文档）。
    let (storage_owner, storage) = StorageOwner::open(&cfg.storage, &cancel)
        .await
        .map_err(BootError::Storage)?;

    // 从这里往下全部不可失败。
    let mut planes = Vec::with_capacity(TaskName::ALL.len());
    let mut acks = Vec::with_capacity(TaskName::ALL.len());

    let (http_ack, http_ack_rx) = ack_channel(TaskName::Http);
    acks.push(http_ack_rx);
    planes.push(PreparedPlane {
        spec: HttpPlane::SPEC,
        handle: http_runtime,
        future: http_plane(
            listener,
            Arc::clone(&storage),
            &lifecycle,
            &config_publisher,
            build,
            http_ack,
            cancel.clone(),
        ),
    });

    // worker 面是**条件登记**的：`enabled = false` 时它根本不进 supervisor，也就不在提交门
    // 的谓词里。另一种写法是照样登记、让面自己在 `enabled` 为假时立刻返回——那会得到一个
    // 在册、回执、随即 `Returned` 的面，而 first-failure 会把那次正常返回当成关停理由。
    if cfg.worker.enabled {
        let (ticker_ack, ticker_ack_rx) = ack_channel(TaskName::Ticker);
        acks.push(ticker_ack_rx);
        planes.push(PreparedPlane {
            spec: TickerPlane::SPEC,
            handle: ticker_runtime,
            future: TickerPlane::build(
                config_publisher.reader(),
                lifecycle.reader(),
                ticker_ack,
                cancel.clone(),
            ),
        });
    } else {
        tracing::info!(
            name: "plane_disabled",
            plane = %TaskName::Ticker,
            "the plane is disabled by configuration and was not registered"
        );
    }

    Ok(Assembled {
        planes,
        acks,
        lifecycle,
        config: config_publisher,
        storage_owner,
        storage,
        cancel,
    })
}

/// 把配置里的 runtime 名换成句柄。
fn resolve(
    executors: &Executors,
    plane: TaskName,
    configured: Option<&str>,
) -> Result<Handle, BootError> {
    executors
        .resolve(configured)
        .cloned()
        .ok_or_else(|| BootError::UnknownRuntime {
            plane,
            runtime: configured.unwrap_or_default().to_owned(),
        })
}

/// 造 HTTP 面的 future。
///
/// 单独成一个函数，是为了让 `Router` 的**生存期**只覆盖这一个函数体：它在这里被造出来、
/// 立刻被 `HttpPlane::build` 关进 future 里，`assemble` 的函数体里因此没有任何一个 axum
/// 类型的局部变量。
fn http_plane(
    listener: TcpListener,
    storage: Arc<dyn Storage>,
    lifecycle: &LifecyclePublisher,
    config: &ConfigPublisher,
    build: BuildInfo,
    ack: AckSender,
    cancel: CancellationToken,
) -> PlaneFuture {
    let state = AppState::new(storage, lifecycle.reader(), config.reader(), build);
    HttpPlane::build(listener, router(state, business_routes()), ack, cancel)
}

impl Assembled {
    /// 全进程**唯一**的 `spawn` 点。
    ///
    /// 不返回 `Result` 的理由见模块文档。
    pub(crate) fn launch(self) -> Running {
        let Self {
            planes,
            acks,
            lifecycle,
            config,
            storage_owner,
            storage,
            cancel,
        } = self;

        let mut supervisor = TaskSupervisor::new();
        for plane in planes {
            let PreparedPlane {
                spec,
                handle,
                future,
            } = plane;
            let name = spec.name;
            if let Err(err) = supervisor.spawn_on(spec, &handle, future) {
                tracing::error!(
                    name: "plane_register_failed",
                    plane = %name,
                    error = %err,
                    "the plane could not be registered with the supervisor"
                );
            }
        }

        Running {
            supervisor,
            acks: AckSet::new(acks),
            lifecycle,
            config,
            storage_owner,
            storage,
            cancel,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;
    use std::time::Duration;

    use service_core::lifecycle::Phase;
    use service_testkit::TempInstallRoot;
    use tokio::runtime::Builder;

    /// 一份能真的装起来的配置：端口交给内核挑，数据库落在临时目录里。
    fn config_in(root: &TempInstallRoot) -> Config {
        let mut cfg = Config::default();
        cfg.http.bind_addr = SocketAddr::from((Ipv4Addr::LOCALHOST, 0));
        cfg.storage.path = root.root().join("data").join("test.sqlite3");
        cfg
    }

    /// 单 runtime 的 `Executors`——装配用例不关心多 runtime 拓扑。
    fn executors_here() -> Executors {
        Executors::for_tests(Handle::current())
    }

    /// 这几条用例要一个**真的会并发跑**的 runtime：`assemble_then_drop_leaves_no_task`
    /// 的判据是"没有任务在后台跑起来"，在 current-thread 上那是自动成立的，验不出东西。
    fn multi_thread() -> tokio::runtime::Runtime {
        Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn assemble_then_drop_leaves_no_task() {
        // 类型边界：`assemble` 返回之后手上是 `Assembled`，此时**一个任务都没在跑**。
        //
        // 判据不能是"我读了代码没看见 spawn"——那正是门禁扫描要替代的东西。这里用一条
        // 可观察的事实来验：把 `Assembled` 解构掉，各面的 future 随之 drop，它们捕获的
        // `AckSender` 也就没了，于是每一份回执都收到 `false`。反过来，只要有哪个面真被
        // spawn 了出去，它拥有的是自己那份 future，drop `Assembled` 动不了它——ticker 面
        // 更是**开头第一行**就发回执，那一份必然收到 `true`。
        let root = TempInstallRoot::new().unwrap();
        let assembled = assemble(
            config_in(&root),
            BuildInfo::current("test"),
            &executors_here(),
        )
        .await
        .expect("这份配置该能装起来");

        // 字段逐个绑出来再显式 drop，而不是 `let Assembled { acks, .. } = assembled;`。
        // 后者是一次**部分移动**：没被绑走的字段仍然挂在 `assembled` 这个局部变量上，要等
        // 它出作用域才析构。于是面的 future 会一直活到用例结束，下面几个断言验的就不是
        // "drop 之后"而是"drop 之前"——本用例第一版正是这样挂死的。
        let Assembled {
            planes,
            acks,
            lifecycle,
            config,
            storage_owner,
            storage,
            cancel,
        } = assembled;
        drop((planes, lifecycle, config, storage_owner, storage, cancel));

        assert_eq!(acks.len(), 2, "默认配置有 http 与 ticker 两个面");

        // 给"万一有任务被 spawn 了"一个真的跑起来的机会。ticker 面开头第一行就发回执，
        // 十毫秒足够它送到。这里用 `sleep` 而不是 `yield_now()`：本用例的 future 是被
        // `Runtime::block_on` 直接驱动的，不是一个被调度的任务，`yield_now()` 在那个
        // 位置上不保证能把自己唤回来。`sleep` 走时间驱动，`block_on` 自己就在驱动它。
        tokio::time::sleep(Duration::from_millis(10)).await;

        for rx in acks {
            let plane = rx.plane();
            // 零超时 = 只 poll 一次。发送端是在上面那个块结束时**同步**被 drop 掉的，
            // 于是"回执端已经关闭"必须当场成立，不需要等。写成无界 `await` 的话，
            // 断言不成立时的表现是整条用例挂死——那是最难读的一种失败。
            match tokio::time::timeout(Duration::ZERO, rx.recv()).await {
                Ok(false) => {}
                Ok(true) => panic!("{plane} 的回执发出来了——说明 `assemble` 里有人 spawn"),
                Err(_) => {
                    panic!(
                        "{plane} 的回执端还活着——`assemble` 造的 future 没随 `Assembled` 一起 drop"
                    )
                }
            }
        }
    }

    #[tokio::test]
    async fn bind_failure_aborts_boot_with_the_original_error() {
        // 先自己占住一个端口，再让装配去绑同一个——错误必须原样带出来，
        // 而且**不能**在这之前打开存储：数据目录如果被建了出来，就说明顺序反了。
        let root = TempInstallRoot::new().unwrap();
        let squatter = bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).unwrap();
        let taken = squatter.local_addr().unwrap();

        let mut cfg = config_in(&root);
        cfg.http.bind_addr = taken;
        cfg.storage.path = root.root().join("never-created").join("test.sqlite3");
        let data_dir = cfg.storage.path.parent().unwrap().to_path_buf();

        let err = assemble(cfg, BuildInfo::current("test"), &executors_here())
            .await
            .expect_err("端口已被占用，装配必须失败");

        assert_eq!(err.kind(), "bind");
        assert!(
            matches!(&err, BootError::Bind { addr, .. } if *addr == taken),
            "错误里要带着那个绑不上的地址：{err:?}"
        );
        assert!(
            !data_dir.exists(),
            "端口绑不上时不该有人先去建数据目录——存储是最后一件被打开的东西"
        );
    }

    #[tokio::test]
    async fn an_unknown_runtime_fails_assembly_instead_of_falling_back_to_main() {
        // 配置管线已经拦过一次；这里守的是第二道：真漏过来了也不能悄悄退到主 runtime。
        // 退回去的后果是一个"看起来正常"的进程，而它的面全挤在同一个 runtime 上。
        let root = TempInstallRoot::new().unwrap();
        let mut cfg = config_in(&root);
        cfg.worker.runtime = Some("does-not-exist".to_owned());

        let err = assemble(cfg, BuildInfo::current("test"), &executors_here())
            .await
            .expect_err("runtime 名不存在时装配必须失败");

        assert_eq!(err.kind(), "runtime");
        assert!(
            matches!(
                &err,
                BootError::UnknownRuntime { plane, runtime }
                    if *plane == TaskName::Ticker && runtime == "does-not-exist"
            ),
            "错误里要说清是哪个面、指向了哪个名字：{err:?}"
        );
    }

    #[tokio::test]
    async fn a_disabled_worker_is_never_registered() {
        // `enabled = false` 的面不进 supervisor，也就不进提交门的谓词。照样登记、让面自己
        // 立刻返回的写法会得到一个"在册 → 回执 → `Returned`"的面，而 first-failure 会把
        // 那次正常返回当成关停理由。
        let root = TempInstallRoot::new().unwrap();
        let mut cfg = config_in(&root);
        cfg.worker.enabled = false;

        let assembled = assemble(cfg, BuildInfo::current("test"), &executors_here())
            .await
            .expect("关掉 worker 之后仍然该能装起来");
        let running = assembled.launch();

        assert_eq!(running.supervisor.len(), 1);
        assert_eq!(
            running.acks.outstanding(),
            vec![TaskName::Http],
            "关掉的面不该留下一份永远等不到的回执"
        );
        running.cancel.cancel();
    }

    #[test]
    fn launch_is_the_only_place_that_spawns() {
        // 装配完 supervisor 还不存在；`launch` 之后它恰好装着两个面，且相位仍是 `Starting`
        // ——spawn 不等于提交。
        let rt = multi_thread();
        let root = TempInstallRoot::new().unwrap();

        rt.block_on(async {
            let assembled = assemble(
                config_in(&root),
                BuildInfo::current("test"),
                &executors_here(),
            )
            .await
            .expect("这份配置该能装起来");
            let running = assembled.launch();

            assert_eq!(running.supervisor.len(), 2);
            assert_eq!(
                running.lifecycle.current(),
                Phase::Starting,
                "起了任务不等于提交了"
            );
            assert!(
                Arc::strong_count(&running.storage) > 1,
                "HTTP 面的状态里也攥着一份——关停第三段要数的就是这个计数，\
                 它等于 1 才说明没人还在用这个池"
            );

            running.cancel.cancel();
        });
    }
}
