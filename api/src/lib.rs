//! HTTP 展示层。
//!
//! 这一层的全部工作可以压成一句话：**把进程内部的东西翻译成 HTTP，并且只用一种说法。**
//!
//! "只用一种说法"是这里唯一真正难守的纪律。一个服务里错误响应有两种形状（一种是 JSON
//! 信封、一种是框架默认的 text/plain），客户端就得写两套解析——而它们会在最难复现的路径
//! 上冒出来：路由不存在、方法不对、body 超限、handler panic、handler 超时。这五条全都
//! 不经过任何业务代码，也就全都不会出现在业务测试里。
//!
//! 所以这个 crate 的结构基本上是围着那五条展开的：
//!
//! | 漏点 | 默认行为 | 这里做了什么 |
//! | --- | --- | --- |
//! | 路由不存在 | axum 回 `404` + 空 body | 顶层 `fallback` |
//! | 方法不对 | axum 回 `405` + 空 body | `method_not_allowed_fallback` |
//! | 提取器失败 | axum 回 `text/plain` | 包装提取器 [`Json`] / [`Path`] / [`Query`] |
//! | handler panic | tower-http 回 `text/plain` 的 `Service panicked` | `CatchPanicLayer::custom` |
//! | handler 超时 | tower-http 回 `408` + **空** body | 自己写的中间件，回 `504` 信封 |
//!
//! 后两条值得单独记一笔：它们是**同一类**缺陷——第三方中间件各自带了一套"出错时回什么"，
//! 而它们都不知道本服务有信封这回事。超时那条是实测 tower-http 时撞见的，panic 那条更
//! 隐蔽——要等到某个 handler 真的 panic 那天才会暴露。所以这里留下一条可复用的判据：
//! **凡是能自己产出响应的第三方层，都要先问它产出的是不是信封。**
//!
//! # 事件的「名字」和「那句话」是两件东西
//!
//! 这一层是模板里发事件最多的一层（5xx 的构造处、panic 捕获处、超时处、面退出处），于是
//! 这条纪律写在这里：**每个事件都要显式 `name:`，那个名字是 snake_case 标识符；消息位置
//! 留给给人读的那句话。**
//!
//! 不写 `name:` 的话，`tracing` 自动生成的名字是 `event api/src/system.rs:90`——**带行号**。
//! 断言只能退而求其次去匹配消息文本，于是改一句话的措辞就会红一批用例；而行号会烂，是
//! 最不该拿来定位的那种东西。把标识符塞进消息位置看着更省事，实际上是把两个角色压成了
//! 一个：日志里没有人话，断言里没有稳定的键。
//!
//! 可以被断言的事件名，见 `README.md` 的事件表。
//!
//! # 它刻意不做的事
//!
//! - **不碰 `storage`。** 邻接表禁止 `api → storage`，连 dev 边都没有。这一层持有的
//!   是 `Arc<dyn core::Storage>`，而 `core` 不依赖 sqlx——"门面不泄漏后端"因此由依赖图强制。
//!   代价是契约测试要自己写一个 `Storage` 替身；收益是那个替身能按需失败，比真后端好用。
//! - **不装 subscriber、不建 runtime、不 spawn。** 同 `worker`。
//! - **不读配置文件、不读环境变量。** 配置从 `AppState` 里的读端来，那是 `app` 注入的。
//! - **不预铺 `From<StorageError> for HttpError`。** `From` 会被 `?` 隐式调用，
//!   于是"这个存储错误该回几"会散落到每个 handler 里。取而代之的是显式的
//!   [`HttpError::from_storage`]，它内部那个 `match` 没有 `_` 臂。
//!
//! # 公共出口
//!
//! 清单见 `README.md`，与下面的 `pub use` 两侧集合相等。本 crate 没有 feature，
//! 所以只有一份清单。

mod business;
mod error;
mod extract;
mod middleware;
mod response;
mod router;
mod serve;
mod state;
mod system;

// 改名导出：在本 crate 里它叫 `business::routes`（模块名已经给足了上下文），到了 `app`
// 那边一眼看去只剩 `routes`——而装配代码里同时还有 runtime、面、存储要装，一个叫 `routes`
// 的东西说不清是谁的路由。
pub use business::routes as business_routes;
pub use error::HttpError;
pub use extract::{Json, Path, Query};
pub use response::ENVELOPE_KEYS;
pub use router::router;
pub use serve::{HttpPlane, bind};
pub use state::AppState;

// dev-dependency 会被链进 **lib 的 test 目标**，而本 crate 的单元测试（`error.rs`、
// `response.rs`、`system.rs`、`middleware/panic.rs` 里的那几个 `mod tests`）用不到夹具与
// HTTP 客户端——它们要么是纯函数，要么只看一个 `Response` 的头。`-D warnings` 下这就是
// 两条 `unused_crate_dependencies`（`tokio-util` 与 `tokio` 是普通依赖，库里就用到了）。
//
// `as _` 只满足链接检查，不引入任何可命名的东西。另一条路是编几个用不上的单元用例把它们
// 用起来——那等于为了一条 lint 往库里塞假断言。
#[cfg(test)]
use {service_testkit as _, tower as _};
