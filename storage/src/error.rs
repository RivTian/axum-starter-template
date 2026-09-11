//! 存储层错误
//!
//! 生命周期类变体（初始化 / 数据库操作 / 迁移）之外，另设四个**语义变体**：
//! 各后端把驱动原生错误（SQLITE_CONSTRAINT / PG 23xxx）归一化为语义，
//! 业务层与 HTTP 层只对语义分支，不感知方言。不按业务表拆错误——那是仓储
//! 实现的细节，泄漏到错误枚举里会让每加一张表就要改一次公共 API。
//!
//! 语义变体现在没有构造者（模板不带仓储）：它们是给仓储实现用的**归一化目标**，
//! 与 api 侧 `From<StorageError> for HttpError` 的映射一起构成「加一个仓储」时
//! 不必再设计的那部分。

use thiserror::Error;

use {{crate_prefix_snake}}_core::AppError;

pub type StorageResult<T> = Result<T, StorageError>;

#[derive(Debug, Error)]
pub enum StorageError {
    /// 初始化阶段失败：路径不可用、连接参数非法、密码读取失败等。
    /// 这类错误一律 fail-fast，不做降级。
    #[error("storage init failed: {0}")]
    Init(String),

    /// 数据库操作失败（连接、查询、事务）。
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),

    /// schema 迁移失败。
    #[error("migration failed: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),

    /// 目标不存在（HTTP 层映射 404）
    #[error("not found: {0}")]
    NotFound(String),

    /// 唯一约束冲突（HTTP 层映射 409）；载荷是约束名，绝不含行值
    #[error("unique violation: {0}")]
    UniqueViolation(String),

    /// 业务冲突：仓储在事务内主动判定的领域规则冲突（HTTP 层映射 409）。
    /// 与 [`Self::UniqueViolation`] 区分：后者由数据库唯一约束触发。
    #[error("conflict: {0}")]
    Conflict(String),

    /// 乐观锁冲突（HTTP 层映射 409）。两个版本号都要报准：
    /// 它们会显示给人看（「你以为是 2，其实是 1」）。
    #[error("version conflict: expected {expected}, actual {actual}")]
    VersionConflict { expected: i64, actual: i64 },
}

/// 向应用级错误升格。放在本 crate（而不是 core）是依赖方向决定的：
/// core 不知道 `StorageError`，孤儿规则允许在这里为 core 的类型实现 `From`。
impl From<StorageError> for AppError {
    fn from(error: StorageError) -> Self {
        AppError::Storage(anyhow::Error::new(error))
    }
}
