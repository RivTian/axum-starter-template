//! handler 分组：只声明各组子模块（crate 外不可见）。
//!
//! 每组暴露 `pub(crate) fn router() -> Router<AppState>`，在 `build_router` 里 merge。

pub(crate) mod system;
