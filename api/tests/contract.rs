//! HTTP 层的契约用例。
//!
//! 这个文件只用公共 API：`router()`、`AppState::new()`、`bind()`、`HttpPlane`，加上 `core`
//! 的那几个句柄。看不见任何 `pub(crate)` 的东西——能断言的就是客户端能看见的那些：状态码、
//! content-type、body 的键集合。
//!
//! # 为什么存储替身写在这里，而不是从 `storage` 借一个
//!
//! 分层邻接表禁止 `api → storage`，**连 dev 边都没有**。所以这里自己写一个
//! [`FakeStorage`]。这不是将就：真后端没法按需在**探测进行到一半时**改变进程阶段，而
//! `/readyz` 那条「跨越 draining 的探测一律不就绪」恰恰只能在那个瞬间被观察到。替身的
//! `on_probe` 钩子就是为它存在的。
//!
//! # 用例分两类
//!
//! - **`oneshot`（绝大多数）**：路由树、中间件栈、信封三样都能在内存里验完，不起端口。
//!   起端口会把「端口被占」变成一类与被测内容无关的随机失败。
//! - **真 socket（两条）**：`bind` 的失败时机、面的 ack/serve/cancel 生命周期。这两件事
//!   `oneshot` 摸不到——`from_std` 注册到哪个 runtime、`with_graceful_shutdown` 收到取消
//!   之后还接不接连接，都只有真 fd 能回答。
//!
//! # 日志断言为什么都是「存在」而不是「不存在」
//!
//! `LogCapture` 是**进程级**的互斥资源：一条用例捕获期间，并行跑的别的用例发的事件也会
//! 落进来。于是「某事件不存在」这种断言会被别人污染成假阳性，而「某事件存在且带着这个
//! 字段」只会被污染成更容易通过——用例仍然只在自己那条路径真的走通时才绿。要断言「不该
//! 出现的东西没出现」，一律去看**响应体**，那是这条用例独占的。
//!
//! 末尾那两行 `as _` 是给 `unused_crate_dependencies` 交代的：集成测试目标会链接被测
//! crate 的全部普通依赖，`tower-http` 与 `tracing` 这个文件自己不念，但它们确实被库用着。

use std::fmt;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use axum::response::Response;
use axum::routing::{get, post};
use serde::{Deserialize, Serialize};
use service_api::{AppState, ENVELOPE_KEYS, HttpPlane, Json, Path, Query, bind, router};
use service_core::build_info::BuildInfo;
use service_core::config::{Config, ConfigPublisher, HttpConfig};
use service_core::lifecycle::{LifecyclePublisher, Phase};
use service_core::storage::{Storage, StorageError, StorageFuture};
use service_core::task::{ShutdownClass, TaskName, ack_channel};
use service_testkit::LogCapture;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;
use tower_http as _;
use tracing as _;

/// 读响应体的上限。比任何一条用例的 body 都大得多；它只是不让一个跑飞的用例把内存吃光。
const BODY_READ_LIMIT: usize = 64 * 1024;

/// 超时用例的预算。真实时间，不是暂停时钟——`oneshot` 走的是完整的中间件栈，
/// 而 `tokio::time::pause()` 要求 current-thread runtime，两者在这条路径上凑不到一起。
/// 30ms 与 handler 里那个 60 秒之间差三个数量级，不存在"机器慢了就红"的余地。
const TIMEOUT_BUDGET: Duration = Duration::from_millis(30);

/// `/slow` 睡多久。故意远大于 [`TIMEOUT_BUDGET`]：这条 handler 永远跑不完。
const SLOW: Duration = Duration::from_secs(60);

/// 限流用例里的 body 上限。`{"name":"ok"}` 是 15 字节，其余三条用例都塞得下。
const SMALL_BODY_LIMIT: usize = 64;

/// panic 用例的负载。取一个别处不会出现的字符串，于是日志断言不会被别的用例污染。
const PANIC_PAYLOAD: &str = "contract-test panic payload 4f2a";

/// 夹具交给 [`BuildInfo`] 的服务名。
///
/// 故意取一个**不等于**任何 crate 名的串：`/v1/info` 的 `service` 要是哪天又退回去读
/// `CARGO_PKG_NAME`，这里当场红。取 `service_api` 之类的名字就测不出这件事——两个来源
/// 恰好同值时，断言两边都过。
const RIG_SERVICE: &str = "contract-rig";

// ── 存储替身 ────────────────────────────────────────────────────────────────

/// 一个可以按需失败、并且能在**探测进行到一半时**做点别的事情的 `Storage`。
struct FakeStorage {
    healthy: AtomicBool,
    /// 每次 `health()` 被调用时先跑一遍。draining 窗口那条用例靠它把阶段推过去。
    on_probe: Option<Box<dyn Fn() + Send + Sync>>,
}

impl FakeStorage {
    fn healthy() -> Arc<Self> {
        Arc::new(Self {
            healthy: AtomicBool::new(true),
            on_probe: None,
        })
    }

    fn unavailable() -> Arc<Self> {
        let storage = Self::healthy();
        storage.healthy.store(false, Ordering::SeqCst);
        storage
    }

    fn with_probe_hook(hook: impl Fn() + Send + Sync + 'static) -> Arc<Self> {
        Arc::new(Self {
            healthy: AtomicBool::new(true),
            on_probe: Some(Box::new(hook)),
        })
    }
}

/// 手写 `Debug`：`Box<dyn Fn>` 派生不出来，而 `Storage` 要求 `Debug`
/// （门面的实现会进日志，一个不可打印的存储在排查时等于没有）。
impl fmt::Debug for FakeStorage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FakeStorage")
            .field("healthy", &self.healthy.load(Ordering::SeqCst))
            .field("hooked", &self.on_probe.is_some())
            .finish()
    }
}

impl Storage for FakeStorage {
    fn health(&self) -> StorageFuture<'_, Result<(), StorageError>> {
        Box::pin(async move {
            if let Some(hook) = &self.on_probe {
                hook();
            }
            // 真实后端的探测一定跨 `await`。不让出的话，"探测期间相变了"这个窗口在替身上
            // 根本不存在，而那正是被测的东西。
            tokio::task::yield_now().await;

            if self.healthy.load(Ordering::SeqCst) {
                Ok(())
            } else {
                Err(StorageError::Unavailable { backend: "fake" })
            }
        })
    }
}

// ── 业务路由（被测对象的一部分：它们必须和系统端点走同一套中间件）────────────

#[derive(Debug, Deserialize, Serialize)]
struct Payload {
    name: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct Filter {
    page: u32,
}

#[derive(Debug, Serialize)]
struct Echoed {
    got: u32,
}

async fn echo_json(Json(payload): Json<Payload>) -> Json<Payload> {
    Json(payload)
}

async fn echo_query(Query(filter): Query<Filter>) -> Json<Filter> {
    Json(filter)
}

async fn echo_path(Path(id): Path<u32>) -> Json<Echoed> {
    Json(Echoed { got: id })
}

/// 路由上只有一个参数，handler 却要两个——**程序员写错了**，不是客户端发错了。
/// axum 把它判成 500（`WrongNumberOfParameters`），这条路由存在就是为了证明包装提取器
/// 把那个 500 原样传下来了，而不是拍平成 400。
async fn path_misuse(Path((first, second)): Path<(u32, u32)>) -> Json<Echoed> {
    Json(Echoed {
        got: first.wrapping_add(second),
    })
}

/// 这条 handler 的**全部内容**就是 panic：它是 `CatchPanicLayer` 的被试。
#[expect(
    clippy::panic,
    reason = "被试本身。没有 panic 就没法验证 handler panic 不会杀掉进程、且会变成一个信封"
)]
async fn boom() -> Json<Echoed> {
    panic!("{PANIC_PAYLOAD}")
}

async fn slow() -> Json<Echoed> {
    tokio::time::sleep(SLOW).await;
    Json(Echoed { got: 0 })
}

fn business() -> Router<AppState> {
    Router::new()
        .route("/echo/json", post(echo_json))
        .route("/echo/query", get(echo_query))
        .route("/echo/path/{id}", get(echo_path))
        .route("/echo/path-misuse/{id}", get(path_misuse))
        .route("/boom", get(boom))
        .route("/slow", get(slow))
}

// ── 夹具 ────────────────────────────────────────────────────────────────────

/// 一个可以分享的阶段写端。
///
/// `LifecyclePublisher` 本身**不是** `Clone`（写端全进程唯一），但 `publish` 只要
/// `&self`，所以 `Arc` 是合法的共享方式——存储替身的钩子要的正是这个。
fn new_lifecycle() -> Arc<LifecyclePublisher> {
    let (publisher, _reader) = LifecyclePublisher::new();
    Arc::new(publisher)
}

struct Rig {
    lifecycle: Arc<LifecyclePublisher>,
    /// 读端是 `watch::Receiver`，写端 drop 之后 `borrow()` 照样读得到最后一份值，
    /// 所以这个字段严格说不是必需的。留着它是为了让「配置的真值有一个所有者」在夹具里
    /// 也成立——哪天有人给用例加一次热重载，不必先改夹具的形状。
    _config: ConfigPublisher,
    storage: Arc<FakeStorage>,
    router: Router,
}

impl Rig {
    fn new() -> Self {
        Self::assemble(
            Config::default(),
            FakeStorage::healthy(),
            new_lifecycle(),
            business(),
        )
    }

    fn with_config(config: Config) -> Self {
        Self::assemble(config, FakeStorage::healthy(), new_lifecycle(), business())
    }

    fn with_storage(storage: Arc<FakeStorage>) -> Self {
        Self::assemble(Config::default(), storage, new_lifecycle(), business())
    }

    /// 换一份业务路由装一次。只有一条用例用得上它：验**发布出去的那份默认值**。
    fn with_business(business: Router<AppState>) -> Self {
        Self::assemble(
            Config::default(),
            FakeStorage::healthy(),
            new_lifecycle(),
            business,
        )
    }

    fn assemble(
        config: Config,
        storage: Arc<FakeStorage>,
        lifecycle: Arc<LifecyclePublisher>,
        business: Router<AppState>,
    ) -> Self {
        let (config_publisher, config_reader) = ConfigPublisher::new(config);

        // 这里用方法语法 `.clone()` 而不是 `Arc::clone(&storage)`，也不能省掉类型标注。
        // `Arc::clone` 是关联函数，`Self` 从**期待类型**反推，于是期待 `Arc<dyn Storage>`
        // 时它连参数一起要求 `&Arc<dyn Storage>`——unsize 转换没有发生的位置。方法调用按
        // 接收者解析，结果在这个具名绑定上才转成 trait object。
        let shared: Arc<dyn Storage> = storage.clone();

        let state = AppState::new(
            shared,
            lifecycle.reader(),
            config_reader,
            BuildInfo::current(RIG_SERVICE),
        );

        Self {
            lifecycle,
            _config: config_publisher,
            storage,
            router: router(state, business),
        }
    }

    async fn get(&self, uri: &str) -> Result<Reply, String> {
        self.send(Method::GET, uri, None, Vec::new()).await
    }

    /// 发一次请求。
    ///
    /// 返回 `Result` 而不是在里面 `expect`：`clippy.toml` 那三条 `allow-*-in-tests` 是
    /// **结构性**豁免，只认 `#[test]` 函数体和 `#[cfg(test)]` 模块，集成测试的辅助函数两样
    /// 都不占。断言留在用例里本来也更对——失败位置指向那条用例，而不是指向这个夹具。
    async fn send(
        &self,
        method: Method,
        uri: &str,
        content_type: Option<&str>,
        body: Vec<u8>,
    ) -> Result<Reply, String> {
        let mut builder = Request::builder().method(method).uri(uri);
        if let Some(value) = content_type {
            builder = builder.header(header::CONTENT_TYPE, value);
        }
        let request = builder
            .body(Body::from(body))
            .map_err(|error| format!("请求构造失败：{error}"))?;

        let response = self
            .router
            .clone()
            .oneshot(request)
            .await
            .map_err(|error| format!("路由树不该返回错误：{error}"))?;

        Reply::read(response).await
    }

    /// 发一次 `application/json` 的 POST。四条提取器用例共用它。
    async fn post_json(&self, uri: &str, body: &str) -> Result<Reply, String> {
        self.send(
            Method::POST,
            uri,
            Some("application/json"),
            body.as_bytes().to_vec(),
        )
        .await
    }
}

/// 一次响应里，客户端真正能看见的东西。
struct Reply {
    status: StatusCode,
    content_type: Option<String>,
    body: Vec<u8>,
}

impl Reply {
    async fn read(response: Response) -> Result<Self, String> {
        let status = response.status();
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);

        let body = axum::body::to_bytes(response.into_body(), BODY_READ_LIMIT)
            .await
            .map_err(|error| format!("读响应体失败：{error}"))?;

        Ok(Self {
            status,
            content_type,
            body: body.to_vec(),
        })
    }

    fn json(&self) -> Option<serde_json::Value> {
        serde_json::from_slice(&self.body).ok()
    }

    /// body 是不是一个**严格**的信封：三个键不多不少，`message` 是字符串。
    /// 是就给出 `(error 短名, 信封里自报的 status)`。
    fn envelope(&self) -> Option<(String, u64)> {
        let value = self.json()?;
        let object = value.as_object()?;

        if object.len() != ENVELOPE_KEYS.len() {
            return None;
        }
        if !ENVELOPE_KEYS.iter().all(|key| object.contains_key(*key)) {
            return None;
        }
        object.get("message")?.as_str()?;

        Some((
            object.get("error")?.as_str()?.to_owned(),
            object.get("status")?.as_u64()?,
        ))
    }

    /// 信封里那句话。用于断言 5xx 没把内部细节带出来。
    fn message(&self) -> Option<String> {
        Some(self.json()?.get("message")?.as_str()?.to_owned())
    }

    /// body 顶层出现了哪些信封键名。成功响应必须一个都没有。
    fn envelope_keys_present(&self) -> Vec<&'static str> {
        let Some(value) = self.json() else {
            return Vec::new();
        };
        let Some(object) = value.as_object() else {
            return Vec::new();
        };
        ENVELOPE_KEYS
            .iter()
            .copied()
            .filter(|key| object.contains_key(*key))
            .collect()
    }
}

fn loopback_any() -> SocketAddr {
    SocketAddr::from((Ipv4Addr::LOCALHOST, 0))
}

/// 只改 HTTP 段的配置。
///
/// 写成结构体字面量而不是 `let mut c = Config::default(); c.http.x = y;`：后者是
/// `clippy::field_reassign_with_default`，而且读起来像「先造一个错的再把它修回来」。
/// `..Config::default()` 则明说了「其余五段照默认」。
fn config_with(http: HttpConfig) -> Config {
    Config {
        http,
        ..Config::default()
    }
}

/// 手写一行 HTTP/1.1 发进 socket，把整个响应读回来。
///
/// 模板里没有 HTTP 客户端，也不为一条用例引一个。手写请求行反而更诚实：它证明的是
/// **这个端口上真的有人在按 HTTP 应答**，与路由树的内部结构无关。
/// `Connection: close` 让服务端发完就关，于是 `read_to_string` 会自己结束。
async fn http_get(addr: SocketAddr, path: &str) -> Result<String, String> {
    let mut stream = TcpStream::connect(addr)
        .await
        .map_err(|error| format!("连不上 {addr}：{error}"))?;

    let request = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|error| format!("写请求失败：{error}"))?;

    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .await
        .map_err(|error| format!("读响应失败：{error}"))?;
    Ok(response)
}

// ── 系统端点 ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn the_shipped_business_router_adds_nothing() {
    // 这条用例验的是**发布出去的那个默认值**本身，不是本文件里那份探针路由。
    //
    // 两件事一起验，因为它们必须同时成立才算"这个落点可用"：
    //   · 默认值真的是空的——一份刚生成的模板不替使用者猜任何业务路径；
    //   · 空路由器 merge 进主树是空操作——系统端点照常，`cargo run` + `curl /healthz` 立刻通。
    //
    // 顺带也是 `business_routes` 这个出口的唯一调用点：没有它，那个 `pub use` 只被编译过，
    // 没被跑过。
    let rig = Rig::with_business(service_api::business_routes());

    let live = rig.get("/healthz").await.expect("请求不该在传输层失败");
    assert_eq!(
        live.status,
        StatusCode::OK,
        "业务路由为空时系统端点必须照常——否则模板生成出来第一步就跑不通"
    );

    // 探针路由里的一条。它在默认装配下必须不存在，而且 404 要走信封，
    // 不是 axum 的空 body。
    let absent = rig
        .get("/echo/query?n=1")
        .await
        .expect("请求不该在传输层失败");
    assert_eq!(absent.status, StatusCode::NOT_FOUND);
    assert_eq!(
        absent.envelope().map(|(code, _)| code),
        Some("not_found".to_owned()),
        "默认业务路由不该凭空多出路径，缺失的那条也必须回信封"
    );
}

#[tokio::test]
async fn the_three_system_endpoints_answer_three_different_questions() {
    // 排空期是这三条端点唯一会分道扬镳的时刻：进程仍然**健康**（不该被重启），但**不就绪**
    // （不该再收流量）。把它们压成一个端点，编排器就分不清"杀掉它"和"别再给它流量"。
    let rig = Rig::new();
    rig.lifecycle.publish(Phase::Running);
    rig.lifecycle.publish(Phase::Draining);

    let live = rig.get("/healthz").await.expect("请求不该在传输层失败");
    assert_eq!(
        live.status,
        StatusCode::OK,
        "排空中的进程是健康的——/healthz 变红会让编排器去重启一个正在正常关门的进程"
    );

    let ready = rig.get("/readyz").await.expect("请求不该在传输层失败");
    assert_eq!(ready.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        ready.envelope().map(|(code, _)| code),
        Some(Phase::Draining.as_str().to_owned()),
        "原因短名必须直接来自阶段名，不另起一套同义词"
    );

    let info = rig.get("/v1/info").await.expect("请求不该在传输层失败");
    assert_eq!(
        info.status,
        StatusCode::OK,
        "/v1/info 报的是「这是哪一版」，与阶段无关"
    );
}

#[tokio::test]
async fn readiness_names_each_way_of_not_being_ready() {
    // 三档全是 503。只看状态码它们一模一样——`error` 短名是唯一能把它们分开的东西，
    // 这正是信封里那个键存在的理由。
    let starting = Rig::new();
    let reply = starting.get("/readyz").await.expect("请求不该失败");
    assert_eq!(reply.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        reply.envelope().map(|(code, _)| code),
        Some("starting".to_owned()),
        "还没提交就该说 starting"
    );

    let broken = Rig::with_storage(FakeStorage::unavailable());
    broken.lifecycle.publish(Phase::Running);
    let reply = broken.get("/readyz").await.expect("请求不该失败");
    assert_eq!(reply.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        reply.envelope().map(|(code, _)| code),
        Some("storage_unavailable".to_owned())
    );

    let healthy = Rig::new();
    healthy.lifecycle.publish(Phase::Running);
    let reply = healthy.get("/readyz").await.expect("请求不该失败");
    assert_eq!(reply.status, StatusCode::OK);
}

#[tokio::test]
async fn a_probe_that_spans_a_phase_change_is_not_ready() {
    // 守的是「跨越 draining 的探测一律不就绪」。钩子在存储探测**内部**把阶段推到
    // `Draining`，于是只在探测前读一次阶段的实现会在这里回 200 ready——而那一瞬间编排器
    // 会据此又送来一批流量，送给一个正在关门的进程。
    let lifecycle = new_lifecycle();
    let flipper = Arc::clone(&lifecycle);
    let storage = FakeStorage::with_probe_hook(move || flipper.publish(Phase::Draining));

    let rig = Rig::assemble(Config::default(), storage, lifecycle, business());
    rig.lifecycle.publish(Phase::Running);

    let reply = rig.get("/readyz").await.expect("请求不该失败");
    assert_eq!(
        reply.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "探测期间相变了，这一次探测的结论就不再有效"
    );
    assert_eq!(
        reply.envelope().map(|(code, _)| code),
        Some(Phase::Draining.as_str().to_owned())
    );
}

#[tokio::test]
async fn info_echoes_the_build_info_it_was_given() {
    // 回归用例：端点报的两个字段必须就是装配时交给它的那一份，而不是 `api` 自己另写的
    // 一个字符串常量。用例里的 `BuildInfo::current` 与夹具塞进 `AppState` 的是同一个入参。
    //
    // `service` 这一半比 `version` 更要紧：版本号写歪了是报了个旧数字，服务名写歪了是把
    // workspace 的内部布局（`<前缀>-core` 之类）发到了对外契约上。[`RIG_SERVICE`] 特意不
    // 等于任何 crate 名，就是为了让那种回退当场可见。
    let rig = Rig::new();
    let expected = BuildInfo::current(RIG_SERVICE);

    let reply = rig.get("/v1/info").await.expect("请求不该失败");
    let body = reply.json().expect("/v1/info 的响应体必须是 JSON");

    assert_eq!(
        body.get("version").and_then(|v| v.as_str()),
        Some(expected.version)
    );
    assert_eq!(
        body.get("service").and_then(|v| v.as_str()),
        Some(expected.service)
    );
    assert!(
        body.get("uptime_seconds")
            .is_some_and(serde_json::Value::is_u64),
        "uptime 必须是个整数秒；实际 body：{body}"
    );
}

#[tokio::test]
async fn success_bodies_never_use_an_envelope_key() {
    // 「信封的三个键名会不会撞上业务字段」这条断言只有在模板自己的成功响应刻意避开它们时
    // 才测的是真事。三条系统端点加两条回显路由一起验。
    let rig = Rig::new();
    rig.lifecycle.publish(Phase::Running);

    let mut replies = Vec::new();
    for uri in ["/healthz", "/readyz", "/v1/info", "/echo/path/7"] {
        replies.push((uri, rig.get(uri).await.expect("请求不该失败")));
    }
    replies.push((
        "/echo/json",
        rig.post_json("/echo/json", r#"{"name":"ok"}"#)
            .await
            .expect("请求不该失败"),
    ));

    for (uri, reply) in replies {
        assert_eq!(reply.status, StatusCode::OK, "{uri} 应当成功");
        assert_eq!(
            reply.envelope_keys_present(),
            Vec::<&str>::new(),
            "{uri} 的成功响应里出现了信封键名——客户端将无法用键集合区分成功与失败"
        );
    }
}

// ── 信封的五个漏点 ──────────────────────────────────────────────────────────

#[tokio::test]
async fn every_error_path_renders_the_same_three_key_envelope() {
    // 路由不存在 / 方法不对 / handler panic 三条都不经过任何业务代码，也就都不会出现在
    // 业务测试里。它们正是"错误响应有两种形状"最容易长出来的地方。
    let rig = Rig::new();

    let cases: Vec<(&str, Method, &str, StatusCode, &str)> = vec![
        (
            "根路径",
            Method::GET,
            "/",
            StatusCode::NOT_FOUND,
            "not_found",
        ),
        (
            "不存在的路径",
            Method::GET,
            "/nope",
            StatusCode::NOT_FOUND,
            "not_found",
        ),
        (
            "版本前缀下不存在的路径",
            Method::GET,
            "/v1/nope",
            StatusCode::NOT_FOUND,
            "not_found",
        ),
        (
            "已有路径的下一级",
            Method::GET,
            "/v1/info/extra",
            StatusCode::NOT_FOUND,
            "not_found",
        ),
        (
            "方法不对",
            Method::POST,
            "/healthz",
            StatusCode::METHOD_NOT_ALLOWED,
            "method_not_allowed",
        ),
        (
            "handler panic",
            Method::GET,
            "/boom",
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
        ),
    ];

    for (i, (label, method, uri, status, code)) in cases.into_iter().enumerate() {
        let reply = rig
            .send(method, uri, None, Vec::new())
            .await
            .expect("请求不该在传输层失败");

        assert_eq!(reply.status, status, "TC{i}（{label}）状态码不符");
        assert_eq!(
            reply.content_type.as_deref(),
            Some("application/json"),
            "TC{i}（{label}）的 content-type 不是 JSON——信封在这一档上有个洞"
        );

        let envelope = reply.envelope();
        assert_eq!(
            envelope.as_ref().map(|(short_name, _)| short_name.as_str()),
            Some(code),
            "TC{i}（{label}）的信封短名不符；body：{}",
            String::from_utf8_lossy(&reply.body)
        );
        assert_eq!(
            envelope.map(|(_, declared)| declared),
            Some(u64::from(status.as_u16())),
            "TC{i}（{label}）信封里自报的 status 与 HTTP 状态码对不上——\
             这条断言专门守「只在一个地方改了状态码」"
        );
    }
}

#[tokio::test]
async fn a_panic_payload_reaches_the_log_and_not_the_body() {
    let capture = LogCapture::start().expect("日志捕获应当可用");
    let rig = Rig::new();

    let reply = rig
        .get("/boom")
        .await
        .expect("panic 必须被拦下，而不是砍掉连接");
    assert_eq!(reply.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(
        !String::from_utf8_lossy(&reply.body).contains(PANIC_PAYLOAD),
        "panic 负载里可能带着任何局部变量的值，它不该出现在响应里"
    );

    let logged = capture
        .find("handler_panicked")
        .into_iter()
        .any(|event| event.field("panic") == Some(PANIC_PAYLOAD));
    assert!(
        logged,
        "负载必须落到日志里，否则它在任何地方都查不到；实际捕获到：{}",
        capture.summary()
    );
}

#[tokio::test]
async fn a_handler_that_overruns_its_budget_is_a_504_envelope() {
    // **504，不是 408。** 408 的语义是"客户端没及时把请求发完"，而这里超时的是本服务自己的
    // handler，客户端什么都没做错。有人把它"修"回 408 时，这条用例是第二道拦截。
    let capture = LogCapture::start().expect("日志捕获应当可用");

    let rig = Rig::with_config(config_with(HttpConfig {
        handler_timeout: TIMEOUT_BUDGET,
        ..HttpConfig::default()
    }));

    let reply = rig
        .get("/slow")
        .await
        .expect("超时必须返回一个响应，而不是挂住");
    assert_eq!(reply.status, StatusCode::GATEWAY_TIMEOUT);
    assert_eq!(reply.content_type.as_deref(), Some("application/json"));
    assert_eq!(
        reply.envelope(),
        Some(("handler_timeout".to_owned(), 504)),
        "tower-http 的超时层在这里会回一个空 body——那正是自己写这层中间件的理由"
    );

    let logged = capture
        .find("handler_timed_out")
        .into_iter()
        .any(|event| event.field("path") == Some("/slow"));
    assert!(
        logged,
        "超时必须留下一条带路径的记录；实际捕获到：{}",
        capture.summary()
    );
}

// ── 包装提取器 ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn the_four_extractor_failures_keep_four_different_statuses() {
    // 不许把四种失败拍平成一个 400。状态码直接取 axum 给的那一个，这条用例钉的是
    // "没有人在中间重新编一张表"。
    let rig = Rig::with_config(config_with(HttpConfig {
        body_limit_bytes: SMALL_BODY_LIMIT,
        ..HttpConfig::default()
    }));

    let oversized = "x".repeat(SMALL_BODY_LIMIT * 4);

    let syntax = rig
        .post_json("/echo/json", "{")
        .await
        .expect("请求不该失败");
    let semantic = rig
        .post_json("/echo/json", r#"{"other":1}"#)
        .await
        .expect("请求不该失败");
    let too_large = rig
        .post_json("/echo/json", &format!(r#"{{"name":"{oversized}"}}"#))
        .await
        .expect("请求不该失败");
    let no_content_type = rig
        .send(
            Method::POST,
            "/echo/json",
            None,
            br#"{"name":"ok"}"#.to_vec(),
        )
        .await
        .expect("请求不该失败");

    let observed = [
        ("JSON 语法错误", &syntax, StatusCode::BAD_REQUEST),
        ("字段不符", &semantic, StatusCode::UNPROCESSABLE_ENTITY),
        ("超过 body 上限", &too_large, StatusCode::PAYLOAD_TOO_LARGE),
        (
            "缺 content-type",
            &no_content_type,
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
        ),
    ];

    for (i, (label, reply, expected)) in observed.iter().enumerate() {
        assert_eq!(
            reply.status,
            *expected,
            "TC{i}（{label}）状态码不符；body：{}",
            String::from_utf8_lossy(&reply.body)
        );
        assert_eq!(
            reply.content_type.as_deref(),
            Some("application/json"),
            "TC{i}（{label}）走的还是 axum 的 text/plain——包装提取器没生效"
        );
        assert_eq!(
            reply.envelope().map(|(_, declared)| declared),
            Some(u64::from(expected.as_u16())),
            "TC{i}（{label}）不是一个信封"
        );
    }

    // 四档两两不等。上面逐条比对已经蕴含了这一点，但把它单独写出来是为了让"有人把四档
    // 合并成一个 400"这件事在失败信息里一眼可读。
    let statuses: Vec<StatusCode> = observed.iter().map(|(_, reply, _)| reply.status).collect();
    for (i, left) in statuses.iter().enumerate() {
        for right in statuses.iter().skip(i + 1) {
            assert_ne!(left, right, "四档提取器失败被拍平了：{statuses:?}");
        }
    }
}

#[tokio::test]
async fn a_query_rejection_is_an_envelope_too() {
    // `Query` 与 `Json` 走的是不同的 trait（`FromRequestParts` / `FromRequest`），
    // 只验 `Json` 的话，另外两个包装器有没有接上是没人知道的。
    let rig = Rig::new();

    let reply = rig
        .get("/echo/query?page=not-a-number")
        .await
        .expect("请求不该失败");
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    assert_eq!(reply.content_type.as_deref(), Some("application/json"));
    assert_eq!(
        reply.envelope().map(|(code, _)| code),
        Some("bad_request".to_owned())
    );

    let ok = rig.get("/echo/query?page=3").await.expect("请求不该失败");
    assert_eq!(ok.status, StatusCode::OK);
}

#[tokio::test]
async fn a_path_extractor_misuse_is_a_500_and_withholds_its_detail() {
    // 这条是"委托给 axum 的 `.status()`、不自己抄一张表"的全部理由所在。
    // `FailedToDeserializePathParams` 是手写的（不是宏生成的），它把「参数类型不符」判成
    // 400、把「handler 的元数与路由参数个数对不上」判成 **500**。手抄表几乎必然把这两种
    // 压平成同一个 400，于是一个程序员的错误会被报成客户端的错误。
    let rig = Rig::new();

    let client_fault = rig
        .get("/echo/path/not-a-number")
        .await
        .expect("请求不该失败");
    assert_eq!(
        client_fault.status,
        StatusCode::BAD_REQUEST,
        "路径参数类型不符是客户端的错"
    );

    let programmer_fault = rig.get("/echo/path-misuse/7").await.expect("请求不该失败");
    assert_eq!(
        programmer_fault.status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "handler 的 `Path<T>` 元数与路由对不上是**程序员**的错，不能报成 400"
    );
    assert_eq!(
        programmer_fault.envelope().map(|(code, _)| code),
        Some("internal".to_owned())
    );

    let message = programmer_fault
        .message()
        .expect("500 也必须是一个带 message 的信封");
    assert!(
        !message.to_lowercase().contains("parameter"),
        "5xx 的细节可能带着路由内部信息，必须被扔掉；实际是：{message}"
    );
    assert_ne!(
        client_fault.status, programmer_fault.status,
        "把这两档压平成同一个状态码，就没人能从监控上看出是自己写错了"
    );
}

// ── 面的生命周期（这一段需要真 socket）──────────────────────────────────────

#[test]
fn the_spec_says_graceful_and_the_plane_owns_that_answer() {
    // 规格由这一层给出而不是由装配层现编：「这个面需不需要优雅收尾」只有写这个面的人知道。
    assert_eq!(HttpPlane::SPEC.name, TaskName::Http);
    assert_eq!(HttpPlane::SPEC.class, ShutdownClass::Graceful);
    assert!(
        HttpPlane::SPEC.class.gets_harvest_budget(),
        "HTTP 面有 in-flight 请求，不给它 harvest 预算等于让每次关停都砍断正在处理的请求"
    );
}

#[test]
fn a_taken_port_fails_at_bind_time() {
    // `bind` 是**同步**的、在 `launch()` 之前调的，为的就是让这一类失败发生在进程宣称
    // "我起来了"之前。放进面的 future 里的话，编排器会先看到一个存在了一小会儿的进程，
    // 然后才看到它走启动失败路径——而那两者在监控上长得完全不一样。
    let first = bind(loopback_any()).expect("回环上的 0 号端口总能绑上");
    let addr = first.local_addr().expect("绑上了就一定读得到地址");

    assert!(
        bind(addr).is_err(),
        "端口被占必须在 bind 这一步就报出来，而不是留到 accept 循环里"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_plane_acks_then_serves_then_returns_cleanly_on_cancel() {
    let rig = Rig::new();
    rig.lifecycle.publish(Phase::Running);

    let listener = bind(loopback_any()).expect("回环上的 0 号端口总能绑上");
    let addr = listener.local_addr().expect("绑上了就一定读得到地址");

    let (ack_tx, ack_rx) = ack_channel(TaskName::Http);
    let cancel = CancellationToken::new();
    let plane = HttpPlane::build(listener, rig.router.clone(), ack_tx, cancel.child_token());
    let handle = tokio::spawn(plane);

    assert!(
        ack_rx.recv().await,
        "回执必须到——提交门等的就是它，等不到整个进程会停在 Starting 上"
    );

    // 真的走一遍 socket。这一段证明的是 `from_std` 把 fd 注册到了**跑这个 future 的**那个
    // runtime 上：注册错了的话 listener 会"看起来正常、但永远不会就绪"，而 `oneshot`
    // 那一路的用例全都照样绿。
    let response = http_get(addr, "/healthz")
        .await
        .expect("绑上的端口上应当有人按 HTTP 应答");
    assert!(
        response.starts_with("HTTP/1.1 200 OK"),
        "第一行不对：{response}"
    );
    assert!(
        response.contains(r#"{"live":true}"#),
        "应答的不是这棵路由树：{response}"
    );

    cancel.cancel();
    let outcome = tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("取消之后面应当自己返回，而不是等着被 abort")
        .expect("面不该 panic");
    assert!(outcome.is_ok(), "取消是正常收尾，不是失败：{outcome:?}");

    assert!(
        TcpStream::connect(addr).await.is_err(),
        "面返回之后 listener 就该没了；端口还在接连接说明 fd 被泄漏了"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_storage_replica_is_shared_not_copied() {
    // 夹具自身的一条守卫：`Rig` 交出去的 `Arc<FakeStorage>` 与 `AppState` 里那一份必须是
    // 同一个对象，否则"用例把存储改成不可用"这件事根本传不到 handler，而上面那条
    // `storage_unavailable` 用例会变成一个永远绿的空断言。
    let rig = Rig::with_storage(FakeStorage::healthy());
    rig.lifecycle.publish(Phase::Running);

    let before = rig.get("/readyz").await.expect("请求不该失败");
    assert_eq!(before.status, StatusCode::OK);

    rig.storage.healthy.store(false, Ordering::SeqCst);

    let after = rig.get("/readyz").await.expect("请求不该失败");
    assert_eq!(
        after.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "存储的状态没有传到 handler——夹具把替身复制了一份"
    );
}
