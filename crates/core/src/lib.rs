//! 分层根：不依赖任何兄弟 crate，也不依赖任何第三方。
//!
//! 这里只放"每个 crate 都要用、且不引入依赖方向"的东西：统一的错误分类与 `Result` 别名。
//! 只被部分 crate 用到的东西应该沉到那些 crate 里更靠下的那一个，而不是上浮到这里。

mod error;

pub use error::Error;
pub use error::ErrorKind;
pub use error::Result;
