//! **你的业务路由写在这里。**
//!
//! 这个模块刻意是空的。它存在的理由有两条，都不是风格问题：
//!
//! 1. **业务路由必须和系统端点走同一套中间件。** [`routes`] 的返回值会被
//!    [`router`](crate::router) `merge` 进主路由树，然后整棵树一起挂 layer。你在别处另起
//!    一个 `Router` 再 `Router::merge` 到最外层，得到的会是一条绕过超时、绕过 body 上限、
//!    绕过统一错误信封的路由——而这种洞在灰度上线时通常要到出事才被发现。
//! 2. **装配层不认识 `axum`。** `app` 那一层连 `Router` 这个名字都念不出来（邻接表里
//!    `app → axum` 不存在，根清单里写着「HTTP 展示层只有 `api` 能依赖」）。于是"加一条
//!    路由"这件事必须在 `api` 内部有一个落点，否则第一个加业务路由的人就得去改根清单的
//!    依赖边——而那条边是一整条纪律的支点。
//!
//! # 怎么加
//!
//! ```text
//! pub fn routes() -> Router<AppState> {
//!     Router::new()
//!         .route("/v1/widgets", get(list_widgets).post(create_widget))
//!         .route("/v1/widgets/{id}", get(get_widget))
//! }
//!
//! async fn list_widgets(State(state): State<AppState>) -> Result<Json<Vec<Widget>>, HttpError> {
//!     // `state.storage` 是 `Arc<dyn Storage>`——门面，不是池。
//!     // `state.config.load()` 每次现读，于是热字段改了立刻生效。
//! }
//! ```
//!
//! 用 [`crate::Json`] / [`crate::Path`] / [`crate::Query`]，**不要**用 `axum::` 下的同名
//! 提取器：包装版在提取失败时也走统一错误信封，原版会返回一段裸文本。
//!
//! # 一条禁令：这里**不能**出现 `.fallback`
//!
//! 而且它不会报错——这才是要写下来的理由。
//!
//! axum 0.8.9 的 `Router::merge` 只在**两侧都已经有自定义 fallback** 时才 panic
//! （`axum-0.8.9/src/routing/mod.rs`，那个 `(false, false)` 臂）。而
//! [`router`](crate::router) 里 `.merge(business)` 排在 `.fallback(not_found)` **之前**：
//! 合并那一刻外层还是默认 fallback，走的是 `(true, false)` 臂——你的 fallback 先被**收下**，
//! 紧接着那句 `.fallback(not_found)` 再把它**覆盖**掉。
//!
//! 实测（axum 0.8.9，同样的 route → merge → fallback 顺序）：请求一条不存在的路径，拿到的
//! 是系统那条 404 信封，这里写的 fallback 一次都不会被调用。没有 panic，没有告警，没有
//! 编译错误。于是这条禁令只能靠**门禁**守：`make check` 会扫这个文件里的 `.fallback`。
//!
//! 路由树的补集本来就该由 [`router`](crate::router) 统一兜底——那样"路径不存在"和"路径存在
//! 但方法不对"才是同一套信封。

use axum::Router;

use crate::state::AppState;

/// 业务路由。
///
/// 默认是空的：一份刚生成的模板不该替使用者猜任何一条业务路径。空路由器 `merge` 进去
/// 是空操作，于是系统端点照常工作，`cargo run` + `curl /healthz` 立刻能跑通。
///
/// 「默认值确实是空的、且合并进主树之后系统端点还在」这条由 `api/tests/contract.rs` 的
/// `the_shipped_business_router_adds_nothing` 验；这里不放单元测试，因为构造 [`AppState`]
/// 需要存储替身与 `watch` 两端，那套夹具在集成测试里，搬进来等于复制一份。
// 这里**没有** `#[must_use]`：`Router` 自己就带着它，再加一个是 `double_must_use`。
pub fn routes() -> Router<AppState> {
    Router::new()
}
