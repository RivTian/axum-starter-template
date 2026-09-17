//! 统一的错误分类。
//!
//! 每个 crate 的失败都在这里落成一个 [`Error`]：`kind` 让装配层决定"这是配置失败、
//! 启动失败还是运行失败"，`message` 是给人看的一句话，`source` 保留底层原因链。
//!
//! 两种纪律：
//! - 业务错误类型不进这里；`Error` 只承载分类 + 上下文。
//! - 不要把敏感取值（口令、带凭据的 URL）写进 `message`：`Display`/`Debug` 会把它带进日志。

use std::fmt;

/// 错误来源分类。装配层用它映射退出码与日志级别。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// 配置：定位、读取、解析、校验、路径解析失败。启动期发生 → 拒绝启动。
    Config,
    /// 启动期资源：开池、迁移、绑定监听、注册任务失败 → 拒绝启动。
    Startup,
    /// 任务面运行期失败：任务返回的错误。
    Task,
    /// 存储操作失败：查询、迁移执行。
    Storage,
}

impl ErrorKind {
    /// 稳定的短名：只用于日志字段与测试断言，不是用户文案。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Config => "config",
            Self::Startup => "startup",
            Self::Task => "task",
            Self::Storage => "storage",
        }
    }
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// 统一错误：分类 + 一句话 + 原因链。
pub struct Error {
    kind: ErrorKind,
    message: String,
    source: Option<BoxError>,
}

impl Error {
    /// 无底层原因的错误。
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            source: None,
        }
    }

    /// 带底层原因的错误：`message` 说清"在做什么"，`source` 保留为什么。
    pub fn with_source(
        kind: ErrorKind,
        message: impl Into<String>,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        Self {
            kind,
            message: message.into(),
            source: Some(Box::new(source)),
        }
    }

    /// 分类。
    pub fn kind(&self) -> ErrorKind {
        self.kind
    }

    /// 给人看的一句话（不含原因链）。
    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn config(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Config, message)
    }

    pub fn startup(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Startup, message)
    }

    pub fn task(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Task, message)
    }

    pub fn storage(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Storage, message)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.kind, self.message)
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug = f.debug_struct("Error");
        debug.field("kind", &self.kind);
        debug.field("message", &self.message);
        if let Some(source) = &self.source {
            debug.field("source", source);
        }
        debug.finish()
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_deref()
            .map(|source| source as &(dyn std::error::Error + 'static))
    }
}

/// 统一结果别名：crate 之间只交换 `core::Result`。
pub type Result<T, E = Error> = std::result::Result<T, E>;

#[cfg(test)]
mod tests {
    use super::*;

    fn io_error() -> std::io::Error {
        std::io::Error::new(std::io::ErrorKind::NotFound, "no such file")
    }

    #[test]
    fn display_renders_kind_and_message_only() {
        let error = Error::with_source(ErrorKind::Config, "read config", io_error());
        assert_eq!(error.to_string(), "config: read config");
        assert_eq!(error.kind(), ErrorKind::Config);
        assert_eq!(error.message(), "read config");
    }

    #[test]
    fn source_chain_is_preserved() {
        let error = Error::with_source(ErrorKind::Storage, "open database", io_error());
        let source = std::error::Error::source(&error).expect("source must be present");
        assert_eq!(source.to_string(), "no such file");
    }

    #[test]
    fn debug_exposes_source_for_diagnostics() {
        let error = Error::with_source(ErrorKind::Startup, "bind listener", io_error());
        let rendered = format!("{error:?}");
        assert!(rendered.contains("Startup"), "{rendered}");
        assert!(rendered.contains("bind listener"), "{rendered}");
        assert!(rendered.contains("no such file"), "{rendered}");
    }

    #[test]
    fn kind_str_is_stable() {
        assert_eq!(ErrorKind::Config.as_str(), "config");
        assert_eq!(ErrorKind::Startup.as_str(), "startup");
        assert_eq!(ErrorKind::Task.as_str(), "task");
        assert_eq!(ErrorKind::Storage.as_str(), "storage");
    }
}
