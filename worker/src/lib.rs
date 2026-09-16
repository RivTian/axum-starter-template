//! 周期任务面。
//!
//! 这个 crate 只有一件事：给出一个**能照着写第二个**的后台任务面样板。
//!
//! # 它刻意不做的事
//!
//! - **不 spawn。** [`TickerPlane::build`] 返回一个 future，交给谁去跑是 `app` 的决定。
//!   清单里的 `tokio` 连 `rt` feature 都没开，所以"顺手 spawn 一下"在这里编译不过。
//! - **不建 runtime、不装 subscriber。** 前者同上；后者是：库层一旦能装 subscriber，
//!   就能在被别人 `use` 的时候擅自改全局日志。
//! - **不碰存储。** 邻接表明令禁止 `worker → storage`。后台任务要读写数据时，走
//!   `app` 注入的 `Arc<dyn core::Storage>`——那是门面类型，住在 `core`，加一条 build 边等于
//!   把具体后端拖进任务面。这个模板里还没有那样的任务，所以这里连注入口都没有预留：
//!   第一个真需要它的人再加一个参数，比现在猜一个形状准。
//! - **不定义新的错误类型。** 面的失败面是 [`PlaneError`](service_core::task::PlaneError)，
//!   由 `core` 统一给出。这一层再包一层只会让退出报告多一次转译。
//!
//! # 公共出口
//!
//! 只有 [`TickerPlane`] 一个。清单见 `README.md`，与这里的 `pub use` 两侧集合相等。

mod ticker;

pub use ticker::TickerPlane;

// 这一层没有 `#[cfg(test)] mod tests`——能断言的东西全都是外部可观察的，都在 `tests/` 里。
// 但 dev-dependency 会被链进 **lib 的 test 目标**，于是 `service-testkit` 在这里成了
// "写了却没人 use"，而本工作区 `-D warnings`。
//
// 处理方式与 `storage/tests/facade.rs` 里那行 `use sqlx as _;` 同源：`as _` 只满足链接检查，
// 不引入任何可命名的东西。另一条路是编几个用不上的单元用例把夹具用起来——那等于为了一条
// lint 往库里塞假断言，比这一行糟得多。
#[cfg(test)]
use service_testkit as _;
