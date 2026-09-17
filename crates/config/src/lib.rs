//! 配置：schema、三段入口、路径锚点、唯一加载管线、热度分档与热重载。
//!
//! 边界（详见 crate README）：
//! - 不依赖 tokio / tracing / 兄弟 crate 之外的任何东西：这是纯函数库 + 文件监听回调；
//!   把配置变成日志级别、把退避参数交给 supervisor，是装配层的事。
//! - 从文件到 `Config` 只有一条管线（`pipeline::load`）；启动与重载共用它，避免两条管线漂移。

mod anchor;
mod embedded;
mod env;
mod pipeline;
mod reload;
mod schema;
mod secret;
mod source;
mod tier;
mod watch;

pub use anchor::Anchor;
pub use embedded::TEMPLATE as EMBEDDED_TEMPLATE;
pub use env::ENV_PREFIX;
pub use env::EnvSource;
pub use env::MapEnv;
pub use env::ProcessEnv;
pub use pipeline::Loaded;
pub use pipeline::load;
pub use reload::ReloadOutcome;
pub use reload::ReloadReport;
pub use reload::Reloader;
pub use schema::Config;
pub use schema::FileConfig;
pub use schema::HttpConfig;
pub use schema::LogConfig;
pub use schema::Notice;
pub use schema::NoticeKind;
pub use schema::StorageConfig;
pub use schema::SupervisorConfig;
pub use secret::Secret;
pub use source::ConfigSource;
pub use source::SourceOrigin;
pub use tier::Tier;
pub use tier::classify;
pub use tier::leaf_paths;
pub use watch::FileWatcher;
pub use watch::watch_file;

// 公共 API 里到处是 core::Error，调用方不该为了接一个错误再去加一条直接依赖。
pub use {{crate_prefix_snake}}_core::Error;
pub use {{crate_prefix_snake}}_core::ErrorKind;
