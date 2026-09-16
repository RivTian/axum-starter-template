//! HTTP 面：绑定端口，跑 accept 循环，收到取消就优雅关停。
//!
//! # `bind` 为什么是**同步的**，而且在 `launch()` 之前
//!
//! "端口被占"是**启动期**的错误。它必须在进程决定"我起来了"之前被发现，否则会变成这样
//! 一次事故：supervisor 登记了 http 面，面在自己的 future 里 bind 失败，进程于是走
//! `abort_boot`——而这中间，编排器已经看到进程存在了一会儿。
//!
//! 所以 `bind()` 是一个普通的同步函数，`app` 在 `launch()` **之前**调它，失败就直接退出，
//! 退出码是启动失败那一档。一个自然的想法是让面通过回执把真实 `SocketAddr` 回传给装配层
//! （端口写 0 时需要知道内核给了哪个）——这里不需要：`bind()` 单独成为一个出口
//! 之后，`app` 手里本来就有 `TcpListener`，`local_addr()` 直接读得到，回执不需要带负载。
//!
//! # `from_std` 为什么在 future **里面**
//!
//! `TcpListener::from_std` 会把 fd 注册到**调用时所在的那个** runtime 的 reactor 上。在
//! `build()` 里做，注册的就是装配线程的 runtime；而这个 future 可能被 `spawn_on` 到另一个
//! runtime 上（多 runtime 绑定时就会这样）。那样得到的是一个"看起来正常、但永远不会就绪"的
//! listener——最难查的一类问题。放进 future 体内，注册的必然是真正跑它的那个 runtime。
//!
//! # 绑定 ≠ 就绪
//!
//! 回执一发出去，提交门就可能放行，而这个面**立刻**开始 accept。也就是说：`Phase::Starting`
//! 期间，socket 已经在接连接了。
//!
//! 这是有意的，而且是被 `/readyz` 的语义逼出来的：它要在没起来时回 `503 starting`。要做到
//! 这一点，请求就得先被 accept、被路由、跑进 handler。推迟 accept 的话，探针连接会排在
//! backlog 里，客户端看到的是**挂住**，而不是一个说明原因的 503——恰恰是那一档最需要被
//! 观察到的时候观察不到。
//!
//! 所以"提交之前不处理业务"这句话在这一层的落点是 `/readyz` 回 503，**不是**推迟 accept。
//! 模板里没有业务路由，所以没有第二个落点；等你加了第一条业务路由，README 的"加业务路由"
//! 一节说明怎么给它加相位门。
//!
//! # 为什么是 `Graceful`
//!
//! `with_graceful_shutdown` 收到信号后不再 accept，但会等已经在跑的请求做完。等多久由
//! 关停序列的 harvest 预算兜底——预算用完，supervisor `abort()` 它。所以这个面**值得等**，
//! 属于 `ShutdownClass::Graceful`。

use std::net::{SocketAddr, TcpListener};

use axum::Router;
use service_core::task::{AckSender, PlaneError, PlaneFuture, ShutdownClass, TaskName, TaskSpec};
use tokio_util::sync::CancellationToken;

/// 绑定监听端口。**同步**，在 `launch()` 之前调用。
///
/// 顺带把 socket 设成非阻塞：`tokio::net::TcpListener::from_std` 要求调用方保证这一点，
/// 而那一步在面的 future 里。把它留到那时候做，就等于把一个可失败的步骤推到提交门之后——
/// 而那正是这套设计最努力要避免的一类错。
///
/// # Errors
///
/// 端口被占、地址不合法、权限不足（绑 1024 以下的端口）都在这里返回。
pub fn bind(addr: SocketAddr) -> std::io::Result<TcpListener> {
    let listener = TcpListener::bind(addr)?;
    listener.set_nonblocking(true)?;
    Ok(listener)
}

/// HTTP 面。
///
/// 和 `TickerPlane` 一样是个纯命名空间：没有字段，也不需要被构造出来。面的状态全在
/// `build()` 返回的那个 future 里。
#[derive(Debug)]
pub struct HttpPlane;

impl HttpPlane {
    /// 登记规格。
    ///
    /// `SPEC` 住在这一层而不是让 `app` 在登记时现编一个：`ShutdownClass` 问的是
    /// "这个面需不需要优雅收尾"，而那只有写这个面的人知道。
    pub const SPEC: TaskSpec = TaskSpec::new(TaskName::Http, ShutdownClass::Graceful);

    /// 造 future。**不 spawn**，也不做任何可失败的事，所以不返回 `Result`。
    ///
    /// 参数里有 `ack`：一个更朴素的签名是 `build(listener, state, cancel)`，这里多了一个
    /// 回执发送端，并且把 `state` 换成了已经装好的 `Router`。两处都有理由：
    ///
    /// - **回执**：提交门的判据是"登记过的面全部回执"，定义域是登记集。让某个面免于回执,
    ///   就等于把"哪些面算数"变成一份要人维护的名单。
    /// - **`Router` 而不是 `AppState`**：面只负责"跑这棵树"，树长什么样是装配的决定。
    ///   传 `AppState` 的话，使用者就没有地方把自己的业务路由塞进来——一个加不了路由的
    ///   HTTP 模板没有意义。
    #[must_use]
    pub fn build(
        listener: TcpListener,
        router: Router,
        ack: AckSender,
        cancel: CancellationToken,
    ) -> PlaneFuture {
        Box::pin(run(listener, router, ack, cancel))
    }
}

async fn run(
    listener: TcpListener,
    router: Router,
    ack: AckSender,
    cancel: CancellationToken,
) -> Result<(), PlaneError> {
    // 注册到**跑这个 future 的**那个 runtime 上。见模块文档。
    let listener = tokio::net::TcpListener::from_std(listener).map_err(|error| {
        tracing::error!(
            name: "listener_registration_failed",
            target: "http",
            %error,
            "the listener could not be registered with this runtime"
        );
        PlaneError::failed(
            TaskName::Http,
            "the listener could not be registered with this runtime",
        )
    })?;

    // 注册成功了才回执：失败时装配层同时看到"缺一个回执"和"面返回了 Err"，两条独立证据。
    ack.send();

    axum::serve(listener, router)
        .with_graceful_shutdown(async move { cancel.cancelled().await })
        .await
        .map_err(|error| {
            // `PlaneError` 里只放 `&'static str`（错误会进日志与退出报告，不带外部数据）,
            // 真正的 io 错误在这里记一次。
            tracing::error!(
                name: "accept_loop_failed",
                target: "http",
                %error,
                "the accept loop ended with an io error"
            );
            PlaneError::failed(TaskName::Http, "the accept loop ended with an io error")
        })
}
