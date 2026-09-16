//! 配置：类型、三段式管线的纯函数部分、热度判别、发布与读取。
//!
//! # 这里没有的东西
//!
//! 读文件、解析 TOML、读环境变量——**一样都没有**。它们全在 `app`。
//!
//! 理由是两条纪律的合流：第三方 crate 的所在层受限，以及全进程只有一处进程边界。
//! `core` 不认识 TOML，也不认识 `std::env`。代价是 `app` 要多写一层适配；换来的是
//! 这个模块**整体**可以在内存里被遍历——展开、归类、校验、钳位、发布，每一步都是
//! 纯函数，不需要摆弄真实的文件系统或环境变量就能验完。
//!
//! 反序列化错误的传递也因此要绕一下：`app` 用 `serde_path_to_error` 拿到路径与消息，
//! 再构造 [`ConfigError::Deserialize`]。`core` 因此不依赖 `serde_path_to_error`，
//! 而错误形态仍然是统一的那一个。

mod error;
mod expand;
mod heat;
mod pipeline;
mod publish;
mod secret;
mod types;

pub use error::{ConfigError, FieldPath, SafeMessage};
pub use expand::{ExpandError, VarSource, expand};
pub use heat::{HeatDiff, ReloadDecision, evaluate_reload};
pub use pipeline::ClampRecord;
pub use publish::{Accepted, ConfigPublisher, ConfigReader, ConfigSnapshot, Generation};
pub use secret::{Secret, SecretLookup, SecretSource};
pub use types::{
    Config, HttpConfig, LogFormat, MAIN_RUNTIME_NAME, RuntimeConfig, RuntimeThreads, StorageConfig,
    TelemetryConfig, WorkerConfig,
};
