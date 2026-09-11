//! 应用级错误基座。
//!
//! 变体按**失败面**分，不按业务分：IO / 配置 / 持久化 / 其余。业务 crate 自己的错误
//! 类型（如 `StorageError`）在各自 crate 内定义，向上只经 `From` 升格到这里。

use thiserror::Error;

pub type AppResult<T> = Result<T, AppError>;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("config error: {0}")]
    Config(#[from] crate::config::ConfigError),

    /// 持久化层错误的承接口。
    ///
    /// 载荷是 `anyhow::Error` 而不是存储层的具体错误类型：core 位于依赖图下游，
    /// 不能反向依赖 storage crate。转换由 storage 侧提供 `From` 实现完成
    /// （孤儿规则允许，依赖方向不变）。
    ///
    /// 不用 `#[from]`：`Internal` 已经吃掉了 `anyhow::Error`，两个 `From<anyhow::Error>`
    /// 会冲突；这里要求调用方显式选择语义。
    #[error("storage error: {0}")]
    Storage(#[source] anyhow::Error),

    /// 兜底：没有更具体语义的内部错误。
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}
