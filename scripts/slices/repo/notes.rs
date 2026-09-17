//! 切片二：第一个仓储。
//!
//! 门面约定（来自 crates/storage）：`Db` 的所有权在装配层（它要负责最后关池），
//! 仓储只克隆一个池句柄；错误统一包成 `Error`，别把 `sqlx` 的错误类型泄漏到上层。

use sqlx::SqlitePool;
use svc_core::{Error, ErrorKind};

use crate::Db;

/// `notes` 表的仓储。
#[derive(Clone)]
pub struct NotesRepo {
    pool: SqlitePool,
}

impl NotesRepo {
    /// 从 `Db` 门面拿一个池句柄（`Db` 本身的所有权仍归装配层）。
    pub fn new(db: &Db) -> Self {
        Self {
            pool: db.pool().clone(),
        }
    }

    /// 写入一条并返回自增主键。
    pub async fn insert(&self, body: &str) -> Result<i64, Error> {
        let result = sqlx::query("INSERT INTO notes (body) VALUES (?)")
            .bind(body)
            .execute(&self.pool)
            .await
            .map_err(|err| Error::with_source(ErrorKind::Storage, "写入 notes 失败", err))?;
        Ok(result.last_insert_rowid())
    }

    /// 统计条数。
    pub async fn count(&self) -> Result<i64, Error> {
        sqlx::query_scalar("SELECT COUNT(*) FROM notes")
            .fetch_one(&self.pool)
            .await
            .map_err(|err| Error::with_source(ErrorKind::Storage, "统计 notes 失败", err))
    }
}
