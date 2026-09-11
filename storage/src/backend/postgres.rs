//! PostgreSQL 后端
//!
//! 单池即可：PostgreSQL 自身处理并发写，不需要 SQLite 那套单写者约束。
//!
//! 目标库须已存在——不自动 `CREATE DATABASE`。建库涉及权限、编码、表空间等
//! 部署决策，由程序代劳只会在权限不足时给出更难懂的错误。
//!
//! 连接选项由离散字段构造而非 DSN 字符串：DSN 会把密码带进任何一条包含
//! 连接串的错误信息或日志里。

use std::time::Duration;

use async_trait::async_trait;
use sqlx::postgres::{PgConnectOptions, PgPool, PgPoolOptions, PgSslMode};

use {{crate_prefix_snake}}_core::config::StorageConfig;

use super::MAX_MIGRATION_VERSION;
use crate::error::{StorageError, StorageResult};
use crate::model::StorageHealth;
use crate::repo::Storage;

pub(crate) struct PgStorage {
    pool: PgPool,
}

/// 手写而非 derive：`PgPool` 的 `Debug` 会连同连接选项一起打印，主机、库名、
/// 用户名随之进入任何一条 `{:?}` 日志。只暴露后端名。
impl std::fmt::Debug for PgStorage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PgStorage").finish_non_exhaustive()
    }
}

impl PgStorage {
    /// 连接 → 迁移 → 就绪。
    pub(crate) async fn connect(cfg: &StorageConfig) -> StorageResult<Self> {
        let pg = &cfg.postgres;

        if !pg.password.is_empty() {
            tracing::warn!(
                "[storage.postgres].password 使用了明文密码字段，仅限实验环境；\
                 生产请改用 password_env 或 password_file"
            );
        }

        let mut options = PgConnectOptions::new()
            .host(&pg.host)
            .port(pg.port)
            .database(&pg.dbname)
            .username(&pg.user)
            .ssl_mode(parse_ssl_mode(&pg.sslmode)?);

        if let Some(password) = pg
            .resolve_password()
            .map_err(|e| StorageError::Init(format!("failed to resolve postgres password: {e}")))?
        {
            options = options.password(&password);
        }

        let pool_size = pg.pool_size.max(1);
        let pool = PgPoolOptions::new()
            .max_connections(pool_size)
            .acquire_timeout(Duration::from_millis(pg.connect_timeout_ms))
            .connect_with(options.clone())
            .await;

        let pool = match pool {
            Ok(pool) => pool,
            Err(error) => return Err(recover_root_cause(error, options).await),
        };

        // sqlx 内部用 advisory lock 串行化迁移，多实例同时启动是安全的。
        // 失败先关池：迁移失败常触发外部重试，泄漏的池会持续占用数据库连接数
        if let Err(error) = sqlx::migrate!("migrations/postgres").run(&pool).await {
            pool.close().await;
            return Err(error.into());
        }

        tracing::info!(
            host = %pg.host,
            port = pg.port,
            dbname = %pg.dbname,
            pool = pool_size,
            "PostgreSQL storage connected"
        );

        Ok(Self { pool })
    }
}

#[async_trait]
impl Storage for PgStorage {
    fn backend_name(&self) -> &'static str {
        "postgres"
    }

    async fn health(&self) -> StorageResult<StorageHealth> {
        // 整数字面量 `1` 在 PostgreSQL 中是 INT4，按 i64 解码会报类型不匹配，
        // 故显式转 bigint 与 SQLite 侧的读数类型对齐
        sqlx::query_scalar::<_, i64>("SELECT 1::bigint")
            .fetch_one(&self.pool)
            .await?;

        let db_size_bytes: i64 = sqlx::query_scalar("SELECT pg_database_size(current_database())")
            .fetch_one(&self.pool)
            .await?;

        let migration_version: Option<i64> = sqlx::query_scalar(MAX_MIGRATION_VERSION)
            .fetch_one(&self.pool)
            .await?;

        Ok(StorageHealth {
            backend: self.backend_name(),
            healthy: true,
            db_size_bytes,
            migration_version,
        })
    }

    async fn run_maintenance(&self) -> StorageResult<()> {
        // 不做 VACUUM：PostgreSQL 有 autovacuum，手工全库 VACUUM 会长时间占用 IO
        sqlx::query("ANALYZE").execute(&self.pool).await?;
        tracing::debug!("PostgreSQL maintenance completed");
        Ok(())
    }

    async fn close(&self) {
        self.pool.close().await;
    }
}

/// 把池层的超时错误换成真正的连接失败原因。
///
/// 建池失败时 sqlx 上抛的是 `PoolTimedOut`——「pool timed out while waiting for an
/// open connection」。这句话对排障毫无价值：主机写错、端口不通、库不存在、密码错误，
/// 全都长这一个样，而 source 链里也没有更深的信息。
///
/// 于是这里直接**单独建立一条连接**把根因取出来。这条连接不进池、随即丢弃，
/// 只在已经确定要返回错误的路径上执行，正常启动不付出任何代价。
/// 若单连接反而成功了（瞬时抖动、池竞态），保留原始错误，不谎报成功。
async fn recover_root_cause(pool_error: sqlx::Error, options: PgConnectOptions) -> StorageError {
    use sqlx::ConnectOptions as _;

    if !matches!(pool_error, sqlx::Error::PoolTimedOut) {
        return pool_error.into();
    }
    match options.connect().await {
        Err(root) => root.into(),
        Ok(_) => pool_error.into(),
    }
}

/// 只支持三种模式：`verify-ca` / `verify-full` 需要 CA 证书路径配置，
/// 在没有对应配置项之前接受它们等于给出一个做不到的承诺。
fn parse_ssl_mode(mode: &str) -> StorageResult<PgSslMode> {
    match mode {
        "disable" => Ok(PgSslMode::Disable),
        "prefer" => Ok(PgSslMode::Prefer),
        "require" => Ok(PgSslMode::Require),
        other => Err(StorageError::Init(format!(
            "invalid [storage.postgres].sslmode: {other:?} (expected disable | prefer | require)"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ssl_mode_accepts_the_three_supported_values() {
        assert!(matches!(parse_ssl_mode("disable"), Ok(PgSslMode::Disable)));
        assert!(matches!(parse_ssl_mode("prefer"), Ok(PgSslMode::Prefer)));
        assert!(matches!(parse_ssl_mode("require"), Ok(PgSslMode::Require)));
    }

    #[test]
    fn parse_ssl_mode_rejects_unsupported_values_with_a_hint() {
        let error = parse_ssl_mode("verify-full").unwrap_err();
        assert!(error.to_string().contains("sslmode"));
    }
}
