//! 后端工厂：按 `[storage].backend` 选择实现
//!
//! 双后端的差异被限制在本模块内：连接、迁移、PRAGMA、容量读数、维护命令。
//! 上层拿到的是 `Arc<dyn Storage>`，换后端不需要改任何调用方。

use std::sync::Arc;

use {{crate_prefix_snake}}_core::config::{StorageBackend, StorageConfig};

use crate::error::{StorageError, StorageResult};
use crate::repo::Storage;

mod postgres;
mod sqlite;

/// 已应用的最大迁移版本。两后端逐字相同。
///
/// `success = TRUE` 不能省：该表同时记录失败的迁移（无事务 DDL 的后端上，
/// 迁移中途失败会留下 `success = FALSE` 的行）。不加过滤会把一次失败的迁移
/// 报成当前版本，健康快照于是宣称 schema 已到位——而它并没有。
const MAX_MIGRATION_VERSION: &str =
    "SELECT MAX(version) FROM _sqlx_migrations WHERE success = TRUE";

/// 存储层唯一入口：连接 → 迁移 → 自检 → 返回门面。
///
/// 任一步失败即返回 `Err`：绝不带着半套 schema 或不可用的连接池对外服务。
/// 存储不可用时继续启动，只会把故障推迟到第一个业务请求，那时定位成本更高。
pub async fn init_storage(cfg: &StorageConfig) -> StorageResult<Arc<dyn Storage>> {
    let connected: StorageResult<Arc<dyn Storage>> = match cfg.backend {
        StorageBackend::Sqlite => sqlite::SqliteStorage::connect(cfg)
            .await
            .map(|s| Arc::new(s) as Arc<dyn Storage>),
        StorageBackend::Postgres => postgres::PgStorage::connect(cfg)
            .await
            .map(|s| Arc::new(s) as Arc<dyn Storage>),
    };
    let storage = connected.inspect_err(log_startup_hint)?;

    // 自检不是可选项：连接池建起来不代表能查询（权限、schema 可见性等）。
    // 自检失败必须显式关池再上抛：连接池的后台任务不随 Arc 析构立即停止，
    // 直接返回 Err 会在调用方重试启动时不断堆积连接
    let health = match storage.health().await {
        Ok(health) => health,
        Err(error) => {
            storage.close().await;
            return Err(error);
        }
    };
    tracing::info!(
        healthy = health.healthy,
        backend = health.backend,
        db_size_bytes = health.db_size_bytes,
        // Option 直接 Debug 会在日志里落成字符串 "None"；迁移号从 1 起，
        // 0 正好当「一条迁移都没跑过」的哨兵，字段类型也稳定是数字
        migration_version = health.migration_version.unwrap_or(0),
        "storage initialized"
    );

    Ok(storage)
}

/// 对常见启动失败在 fail-fast 前给出可执行的修复指引。
///
/// 这几类错误的原始信息只说「不匹配」「缺失」，看到的人往往会去手改
/// `_sqlx_migrations`——那会让问题彻底不可逆。
fn log_startup_hint(err: &StorageError) {
    match err {
        StorageError::Migrate(sqlx::migrate::MigrateError::VersionMismatch(version)) => {
            tracing::error!(
                "迁移文件校验和与库内记录不一致（version={version}）：该版本迁移文件在被应用后发生过改动\
                 （注释、空白变动同样会改变校验和）。开发环境删除本地数据库文件后重启即可；\
                 生产环境请恢复原始迁移文件或从备份恢复数据库，切勿手工修改 _sqlx_migrations。"
            );
        }
        StorageError::Migrate(sqlx::migrate::MigrateError::VersionMissing(version)) => {
            tracing::error!(
                "数据库中存在本程序不认识的迁移版本（version={version}）：通常是把旧版本或其他项目的\
                 数据库文件指给了本程序。请确认 [storage.sqlite].path / [storage.postgres].dbname \
                 指向正确的库；确需使用旧数据请先完成数据迁移，不要直接接管。"
            );
        }
        StorageError::Migrate(sqlx::migrate::MigrateError::Dirty(version)) => {
            tracing::error!(
                "存在半应用的迁移（version={version}）：上次迁移执行到一半被中断（掉电、进程被杀），\
                 schema 既不是旧版也不是新版，此时启动等于拿坏 schema 对外服务。\
                 请先人工核对该版本迁移的 DDL 实际执行到哪一步、补齐或回退到一致状态，\
                 再删除 _sqlx_migrations 中该行；生产环境建议直接从备份恢复。"
            );
        }
        _ => {}
    }
}
