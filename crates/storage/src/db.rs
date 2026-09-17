//! `Db`：SQLite 连接池的唯一所有者。

use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

use {{crate_prefix_snake}}_core::{Error, ErrorKind};

/// 迁移集在编译期嵌入二进制：部署时不需要额外搬运 `.sql` 文件。
///
/// 目录在编译期必须存在（sqlx 会报错），所以 `migrations/` 里有一份 README 占位——
/// 但它不是迁移；骨架里迁移集是空的，"加一个仓储"的第一步就是往这里放第一个 `.sql`。
static MIGRATIONS: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// SQLite 连接池门面。
///
/// 关闭所有权很明确：装配层持有它，并把 [`Db::close`] 注册成"最后关闭的资源"
/// （任务全部收尾之后才关池；顺序错了会看到连接在关停中途被掐断的报错）。
#[derive(Debug)]
pub struct Db {
    pool: SqlitePool,
}

impl Db {
    /// 打开连接池并配置 busy_timeout。
    ///
    /// 只支持 `sqlite:` scheme（这是骨架的存储引擎事实）：别的 scheme 明确报错，不做"看起来支持"的抽象。
    pub async fn open(url: &str, busy_timeout: Duration) -> Result<Self, Error> {
        // 明确拦下别的 scheme：sqlx 的 SQLite 解析器会把不认识的前缀当文件名，
        // 到连接时才报"打不开"，错误信息又长又偏题。这里先给出准确的失败。
        if !url.starts_with("sqlite:") {
            return Err(Error::new(
                ErrorKind::Startup,
                "storage.url 只支持 `sqlite:` scheme（骨架的存储引擎事实；换引擎见 crates/storage/README.md）",
            ));
        }
        ensure_parent_dir(url)?;
        let options = SqliteConnectOptions::from_str(url).map_err(|err| {
            // 不把 url 放进 message：它可能带凭据；原因链里有需要的信息就够了。
            Error::with_source(
                ErrorKind::Startup,
                "storage.url 不是合法的 SQLite 连接串（骨架只支持 sqlite:）",
                err,
            )
        })?;
        let options = options.busy_timeout(busy_timeout);
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(options)
            .await
            .map_err(|err| Error::with_source(ErrorKind::Startup, "打开 SQLite 连接池失败", err))?;
        Ok(Self { pool })
    }

    /// 连接池本身（仓储实现用它）。
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// 运行全部迁移。骨架里迁移集为空——这一步仍然会跑，并建立 `_sqlx_migrations` 记账表。
    ///
    /// 只在启动的"提交点"之前调用；失败即拒绝启动（退出码 2），不会带着半套 schema 提供服务。
    pub async fn migrate(&self) -> Result<(), Error> {
        MIGRATIONS
            .run(&self.pool)
            .await
            .map_err(|err| Error::with_source(ErrorKind::Startup, "运行数据库迁移失败", err))?;
        tracing::info!(
            total = MIGRATIONS.iter().count(),
            "database migrations applied"
        );
        Ok(())
    }

    /// 关闭连接池（等待连接归还）。资源预算由调用方（supervisor 的资源阶段）负责。
    pub async fn close(self) -> Result<(), Error> {
        self.pool.close().await;
        Ok(())
    }
}

/// 首次启动时数据库文件可能还不存在：SQLite 不会替我们创建父目录，所以这里补上。
///
/// 只对"文件形式的 sqlite URL"生效（`:memory:`、`file:`、`sqlite://` 都不碰）。
fn ensure_parent_dir(url: &str) -> Result<(), Error> {
    let Some(rest) = url.strip_prefix("sqlite:") else {
        return Ok(());
    };
    if rest.starts_with("//") || rest.starts_with(":memory:") || rest.starts_with("file:") {
        return Ok(());
    }
    let path = rest.split('?').next().unwrap_or(rest);
    if path.is_empty() {
        return Ok(());
    }
    let Some(parent) = Path::new(path).parent() else {
        return Ok(());
    };
    if parent.as_os_str().is_empty() {
        return Ok(());
    }
    std::fs::create_dir_all(parent).map_err(|err| {
        Error::with_source(
            ErrorKind::Startup,
            format!("创建数据库目录 `{}` 失败", parent.display()),
            err,
        )
    })
}
