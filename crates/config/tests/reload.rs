//! 热重载测试：叶子枚举、三档分类、cold 回滚、last-good、报告。
//!
//! 这里也是"冷段漏登记会红"的落点：`every_leaf_path_is_explicitly_classified` 会把新加的、
//! 没登记档位的配置字段直接判红。

use std::sync::Arc;

use {{crate_prefix_snake}}_config::{
    Anchor, ConfigSource, MapEnv, NoticeKind, Reloader, Tier, classify, leaf_paths, load,
};
use tempfile::TempDir;

fn setup(temp: &TempDir) -> (Anchor, ConfigSource, Arc<MapEnv>, Reloader) {
    let anchor = Anchor::from_dir(temp.path());
    let env = Arc::new(MapEnv::new());
    let source = ConfigSource::locate(None, env.as_ref(), &anchor);
    let loaded = load(&source, &anchor, env.as_ref()).expect("初始加载");
    let reloader = Reloader::new(source.clone(), anchor.clone(), env.clone(), loaded.config);
    (anchor, source, env.clone(), reloader)
}

fn write_config(source: &ConfigSource, body: &str) {
    std::fs::write(&source.path, body).expect("写配置");
}

#[test]
fn every_leaf_path_is_explicitly_classified() {
    let temp = TempDir::new().expect("临时目录");
    let (_, _, _, reloader) = setup(&temp);
    let tree = toml::Value::try_from(reloader.running()).expect("运行配置可序列化");
    let leaves = leaf_paths(&tree);
    assert!(!leaves.is_empty(), "至少要有一个叶子路径");
    let unknown: Vec<_> = leaves
        .iter()
        .filter(|path| classify(path) == Tier::Unknown)
        .collect();
    assert!(
        unknown.is_empty(),
        "这些配置叶子没有登记热度档位（tier.rs 的 HOT_PATHS / SEMI_PATHS / COLD_PREFIXES）：{unknown:?}"
    );
}

#[test]
fn hot_change_applies_without_restart() {
    let temp = TempDir::new().expect("临时目录");
    let (_, source, _, mut reloader) = setup(&temp);
    write_config(&source, "[log]\nfilter = \"debug\"\n");

    let outcome = reloader.reload().expect("热段重载应当成功");
    assert_eq!(outcome.report.applied, vec!["log.filter"]);
    assert!(outcome.report.restart_required.is_empty());
    assert_eq!(outcome.config.log.filter, "debug");
    assert_eq!(reloader.running().log.filter, "debug");
}

#[test]
fn semi_change_is_reported_as_next_use() {
    let temp = TempDir::new().expect("临时目录");
    let (_, source, _, mut reloader) = setup(&temp);
    write_config(
        &source,
        "[supervisor]\nrestart_backoff_ms = 900\nrestart_backoff_cap_ms = 30000\n",
    );

    let outcome = reloader.reload().expect("半热段重载应当成功");
    assert_eq!(
        outcome.report.next_use,
        vec!["supervisor.restart_backoff_ms"]
    );
    assert_eq!(outcome.config.supervisor.restart_backoff_ms, 900);
    assert!(outcome.report.restart_required.is_empty());
}

#[test]
fn cold_change_is_rolled_back_and_listed_for_restart() {
    let temp = TempDir::new().expect("临时目录");
    let (_, source, _, mut reloader) = setup(&temp);
    let running_url = reloader.running().storage.url.expose().to_owned();
    write_config(
        &source,
        "[http]\nbind = \"127.0.0.1:18080\"\n[storage]\nurl = \"sqlite:other.db\"\n",
    );

    let outcome = reloader.reload().expect("cold 段重载不报错，但要回滚");
    let mut expected = vec!["http.bind", "storage.url"];
    expected.sort();
    assert_eq!(outcome.report.restart_required, expected);
    assert!(outcome.report.applied.is_empty());
    assert!(outcome.report.unclassified.is_empty());
    // 回滚：生效配置里的 cold 值必须还是运行值。
    assert_eq!(outcome.config.http.bind.to_string(), "127.0.0.1:0");
    assert_eq!(
        outcome.config.storage.url.expose().as_str(),
        running_url.as_str()
    );
    assert_eq!(reloader.running().http.bind.to_string(), "127.0.0.1:0");
}

#[test]
fn invalid_reload_keeps_last_good() {
    let temp = TempDir::new().expect("临时目录");
    let (_, source, _, mut reloader) = setup(&temp);
    write_config(&source, "[log]\nfilter = \"debug\"\n");
    reloader.reload().expect("先做一次成功重载");
    assert_eq!(reloader.running().log.filter, "debug");

    write_config(&source, "[log]\nfilter = ");
    let err = reloader.reload().expect_err("坏文件必须失败");
    assert!(err.to_string().contains("不合法"), "{err}");
    assert_eq!(
        reloader.running().log.filter,
        "debug",
        "重载失败必须保留 last-good，不能半套用"
    );

    write_config(&source, "[htp]\nbind = \"127.0.0.1:0\"\n");
    let err = reloader.reload().expect_err("未知键必须失败");
    assert!(err.to_string().contains("htp"), "{err}");
    assert_eq!(reloader.running().log.filter, "debug");
}

#[test]
fn deleting_the_file_falls_back_to_the_embedded_defaults() {
    let temp = TempDir::new().expect("临时目录");
    let (_, source, _, mut reloader) = setup(&temp);
    write_config(
        &source,
        "[log]\nfilter = \"debug\"\n[storage]\nurl = \"sqlite:other.db\"\n",
    );
    let outcome = reloader.reload().expect("第一次重载");
    assert_eq!(outcome.report.applied, vec!["log.filter"]);
    assert_eq!(outcome.report.restart_required, vec!["storage.url"]);

    std::fs::remove_file(&source.path).expect("删掉配置文件");
    let outcome = reloader.reload().expect("文件缺失时落盘默认模板并继续");
    // 运行值里的 log.filter 是 debug（上次热改的结果）；删文件后候选回到默认 info → 可热，直接生效。
    assert_eq!(outcome.report.applied, vec!["log.filter"]);
    assert_eq!(outcome.config.log.filter, "info");
    assert!(source.path.exists(), "重载也必须把默认模板落盘");
}

#[test]
fn notices_are_reported_on_reload_too() {
    let temp = TempDir::new().expect("临时目录");
    let (_, source, _, mut reloader) = setup(&temp);
    write_config(&source, "[storage]\nbusy_timeout_ms = 10\n");
    let outcome = reloader.reload().expect("越界只钳位");
    assert!(
        outcome
            .notices
            .iter()
            .any(|notice| notice.kind == NoticeKind::Clamped),
        "{:?}",
        outcome.notices
    );
    // busy_timeout_ms 是不可热段：钳位后的候选值也会被回滚成运行值，并列入待重启清单。
    assert_eq!(outcome.config.storage.busy_timeout_ms, 5000);
    assert_eq!(
        outcome.report.restart_required,
        vec!["storage.busy_timeout_ms"]
    );
}

#[test]
fn unchanged_file_produces_an_empty_report() {
    let temp = TempDir::new().expect("临时目录");
    let (_, source, _, mut reloader) = setup(&temp);
    let _ = load(&source, &Anchor::from_dir(temp.path()), &MapEnv::new()).expect("加载");
    let outcome = reloader.reload().expect("重载");
    assert!(!outcome.report.changed(), "{:?}", outcome.report);
}
