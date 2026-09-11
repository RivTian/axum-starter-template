//! # `{{crate_prefix}}-storage`
//!
//! 持久化层：对上暴露 [`Storage`] 门面，对下封装 SQLite / PostgreSQL 双后端。
//!
//! ## 依赖方向
//!
//! `storage -> core`，单向且不可逆。core 不得依赖本 crate；
//! 需要向上传递错误时由本 crate 提供 `From` 实现（见 [`error`]）。
//!
//! ## 门面纪律
//!
//! 公共 API 中不出现任何 sqlx 类型（`SqlitePool`、`PgPool`、`Executor` 等）。
//! 上层拿到的是 `Arc<dyn Storage>`，看不见也管不着连接池。
//!
//! ## 扩展方式
//!
//! 按垂直切片增量扩展，一次一个业务用例，详见 crate 根目录 README。
//! 模板不预建任何表：`Storage` 只有生命周期方法，仓储访问器随首个消费者进场。

mod backend;
pub mod error;
pub mod model;
pub mod repo;

pub use backend::init_storage;
pub use error::{StorageError, StorageResult};
pub use model::StorageHealth;
pub use repo::Storage;
