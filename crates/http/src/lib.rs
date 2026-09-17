//! HTTP 任务面：零路由起步的 `router()` 与可关停的 `serve(...)`。
//!
//! 边界（详见 crate README）：只管"怎么把请求接到 axum 上、怎么优雅停服"，
//! 没有业务路由、没有中间件目录、没有 AppState；不依赖 runtime crate（关停信号以 future 传入）。

mod serve;

pub use serve::router;
pub use serve::serve;

// 公共 API 里到处是 core::Error，调用方不该为了接一个错误再去加一条直接依赖。
pub use {{crate_prefix_snake}}_core::Error;
pub use {{crate_prefix_snake}}_core::ErrorKind;
