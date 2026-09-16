//! 契约用例：只用公共 API。
//!
//! 与 `src/` 里的单元用例分工明确——那边可以看私有字段、可以直连原库核对表清单；这边只能
//! 用 `StorageOwner` 和 `Arc<dyn Storage>`。这条分工本身就是一条断言：**如果这个文件里的
//! 用例需要 `sqlx`，说明门面漏了。**
//!
//! 下面那行 `use sqlx as _;` 不是打脸，是给 `unused_crate_dependencies` 交代：集成测试目标
//! 会链接本 crate 的**全部**普通依赖，一个没用到就是一条警告，而本工作区 `-D warnings`。
//! `as _` 只满足链接检查，不引入任何可命名的东西——"本文件用不到 sqlx"这条断言原样成立。
//! 这条声明出现在哪个 `tests/*.rs` 里，就说明那个文件刻意没碰驱动。

use std::sync::Arc;
use std::time::{Duration, Instant};

use sqlx as _;

use service_core::config::StorageConfig;
use service_core::storage::Storage;
use service_storage::StorageOwner;
use service_testkit::TempDb;
use tokio_util::sync::CancellationToken;

fn config_at(db: &TempDb) -> StorageConfig {
    StorageConfig {
        path: db.path().to_path_buf(),
        max_readers: 2,
        busy_timeout: Duration::from_millis(200),
        acquire_timeout: Duration::from_secs(2),
    }
}

fn in_five_seconds() -> Instant {
    Instant::now() + Duration::from_secs(5)
}

#[tokio::test]
async fn open_creates_the_database_then_health_passes_then_close_is_clean() {
    let db = TempDb::new().expect("临时库");
    assert!(!db.exists(), "夹具刻意不建文件");

    let (owner, storage) = StorageOwner::open(&config_at(&db), &CancellationToken::new())
        .await
        .expect("首次打开");

    assert!(
        db.exists(),
        "`open` 应当把不存在的库建出来——首次部署走的就是这条路径"
    );
    storage.health().await.expect("刚建好的库是健康的");

    let outcome = owner.close(in_five_seconds()).await;
    assert!(outcome.is_clean(), "关闭结局：{}", outcome.as_str());
}

#[tokio::test]
async fn the_facade_debug_says_nothing_but_the_backend() {
    let db = TempDb::new().expect("临时库");
    let (owner, storage) = StorageOwner::open(&config_at(&db), &CancellationToken::new())
        .await
        .expect("打开");

    // `Arc<dyn Storage>` 会被 `app` 放进结构体，而结构体迟早会被 `{:?}` 进日志。
    let rendered = format!("{storage:?}");
    assert!(
        !rendered.contains(&db.path().display().to_string()),
        "库路径泄漏进了 Debug：{rendered}"
    );

    owner.close(in_five_seconds()).await;
}

#[tokio::test]
async fn the_facade_outlives_nothing_it_should_not() {
    // 门面是 `Arc`：关掉 owner 之后它仍然存在，但探活必须失败。这是停机窗口里的真实形状
    // ——请求处理器手里还攥着一份门面，而池已经关了。
    let db = TempDb::new().expect("临时库");
    let (owner, storage) = StorageOwner::open(&config_at(&db), &CancellationToken::new())
        .await
        .expect("打开");

    let still_held: Arc<dyn Storage> = Arc::clone(&storage);
    owner.close(in_five_seconds()).await;

    let err = still_held
        .health()
        .await
        .expect_err("池关了之后探活必须失败");
    assert_eq!(
        err.kind_str(),
        "unavailable",
        "停机窗口里应当是 503 那一类，不是 500：{err}"
    );
}

/// 门面可替换性的**肯定式**证据：同一组断言，两个形态完全不同的后端都要过。
///
/// 这条用例是 `--features test-utils` 那一趟门禁存在的理由。它跑不到的话，第二个实现会
/// 静默腐烂，而一个永远不编译的实现举不了任何证。
#[cfg(feature = "test-utils")]
#[tokio::test]
async fn both_backends_satisfy_the_same_contract() {
    use service_storage::InMemoryStorage;

    let db = TempDb::new().expect("临时库");
    let (owner, sqlite) = StorageOwner::open(&config_at(&db), &CancellationToken::new())
        .await
        .expect("打开 sqlite");

    let memory: Arc<dyn Storage> = Arc::new(InMemoryStorage::new());

    let cases: &[(&str, &Arc<dyn Storage>)] = &[("sqlite", &sqlite), ("memory", &memory)];
    for (i, (name, storage)) in cases.iter().enumerate() {
        storage
            .health()
            .await
            .unwrap_or_else(|err| panic!("TC{i} ({name}) 健康的后端探活失败：{err}"));

        // `Debug` 只给后端名——两个实现都必须遵守，而且给出的名字要能区分它们。
        let rendered = format!("{storage:?}");
        assert_eq!(&rendered, name, "TC{i} ({name}) 的 Debug 不是后端名");
    }

    owner.close(in_five_seconds()).await;
}

/// 故障注入：这两个变体在 SQLite 实现上只能靠制造真事故触发，有了假实现才能随时切换。
#[cfg(feature = "test-utils")]
#[tokio::test]
async fn injected_faults_come_back_as_the_facade_error_kinds() {
    use service_storage::InMemoryStorage;

    let fake = Arc::new(InMemoryStorage::new());
    let storage: Arc<dyn Storage> = Arc::clone(&fake) as Arc<dyn Storage>;

    fake.fail_unavailable();
    assert_eq!(
        storage
            .health()
            .await
            .expect_err("注入之后必须失败")
            .kind_str(),
        "unavailable"
    );

    fake.fail_internal();
    assert_eq!(
        storage
            .health()
            .await
            .expect_err("注入之后必须失败")
            .kind_str(),
        "internal"
    );

    fake.recover();
    storage.health().await.expect("恢复之后应当健康");
}
