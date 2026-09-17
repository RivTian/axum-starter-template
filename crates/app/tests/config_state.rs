//! 配置发布路径：三档副作用、last-good、watch 视图 == 生效值。

use std::sync::Arc;
use std::time::Duration;

use tempfile::TempDir;

use {{crate_prefix_snake}}_app::{ConfigState, Telemetry};
use {{crate_prefix_snake}}_config::Anchor;
use {{crate_prefix_snake}}_config::ConfigSource;
use {{crate_prefix_snake}}_config::EnvSource;
use {{crate_prefix_snake}}_config::MapEnv;
use {{crate_prefix_snake}}_config::Reloader;
use {{crate_prefix_snake}}_config::load;
use {{crate_prefix_snake}}_runtime::{Backoff, shared_backoff};

struct Fixture {
    _temp: TempDir,
    source: ConfigSource,
    reloader: Reloader,
    state: ConfigState,
    backoff: {{crate_prefix_snake}}_runtime::SharedBackoff,
}

fn fixture() -> Fixture {
    let temp = TempDir::new().expect("临时目录");
    let env: Arc<dyn EnvSource> = Arc::new(MapEnv::new());
    let anchor = Anchor::from_dir(temp.path());
    let source = ConfigSource::locate(None, env.as_ref(), &anchor);
    let loaded = load(&source, &anchor, env.as_ref()).expect("首载");
    let telemetry = Arc::new(Telemetry::new(&loaded.config.log.filter).expect("日志过滤器"));
    let backoff = shared_backoff(Backoff::default());
    let state = ConfigState::new(loaded.config.clone(), telemetry, backoff.clone());
    let reloader = Reloader::new(source.clone(), anchor, env, loaded.config);
    Fixture {
        _temp: temp,
        source,
        reloader,
        state,
        backoff,
    }
}

fn write(source: &ConfigSource, body: &str) {
    std::fs::write(&source.path, body).expect("写配置");
}

#[test]
fn hot_change_applies_and_is_published_to_watchers() {
    let mut fixture = fixture();
    let mut view = fixture.state.subscribe();
    write(&fixture.source, "[log]\nfilter = \"debug\"\n");

    let outcome = fixture.reloader.reload().expect("重载");
    let report = fixture.state.apply(outcome).expect("应用");

    assert_eq!(report.applied, vec!["log.filter"]);
    assert_eq!(fixture.state.current().log.filter, "debug");
    assert!(
        view.has_changed().expect("watch 通道"),
        "发布要能被订阅者看到"
    );
    assert_eq!(view.borrow_and_update().log.filter, "debug");
}

#[test]
fn semi_change_updates_the_shared_backoff() {
    let mut fixture = fixture();
    write(
        &fixture.source,
        "[supervisor]\nrestart_backoff_ms = 900\nrestart_backoff_cap_ms = 30000\n",
    );

    let outcome = fixture.reloader.reload().expect("重载");
    let report = fixture.state.apply(outcome).expect("应用");

    assert_eq!(report.next_use, vec!["supervisor.restart_backoff_ms"]);
    let backoff = *fixture.backoff.read().expect("退避参数锁");
    assert_eq!(backoff.base, Duration::from_millis(900));
    assert_eq!(backoff.cap, Duration::from_secs(30));
    assert_eq!(fixture.state.current().supervisor.restart_backoff_ms, 900);
}

#[test]
fn cold_change_is_rolled_back_in_the_published_view() {
    let mut fixture = fixture();
    let running = fixture.state.current().http.bind;
    write(&fixture.source, "[http]\nbind = \"127.0.0.1:18080\"\n");

    let outcome = fixture.reloader.reload().expect("重载");
    let report = fixture.state.apply(outcome).expect("应用");

    assert_eq!(report.restart_required, vec!["http.bind"]);
    assert_eq!(
        fixture.state.current().http.bind,
        running,
        "cold 段回滚之后才发布：watch 视图里不能出现没生效的值"
    );
}

#[test]
fn invalid_log_filter_rejects_the_whole_reload() {
    let mut fixture = fixture();
    write(
        &fixture.source,
        "[log]\nfilter = \"only_the_level_part_is_allowed=banana\"\n",
    );

    // 管线本身能读出候选（字符串），但可热段的应用会失败。
    let outcome = fixture.reloader.reload();
    match outcome {
        Ok(outcome) => {
            let err = fixture
                .state
                .apply(outcome)
                .expect_err("非法的日志过滤器必须让整次重载作废");
            assert!(err.to_string().contains("log.filter"), "{err}");
        }
        Err(err) => {
            // 过滤器语法也可能在反序列化/校验阶段就被拦住——同样算"没生效"。
            assert!(err.to_string().contains("filter"), "{err}");
        }
    }
    assert_eq!(
        fixture.state.current().log.filter,
        "info",
        "失败之后必须保留 last-good"
    );
}

#[test]
fn published_view_equals_effective_config() {
    let mut fixture = fixture();
    write(
        &fixture.source,
        "[log]\nfilter = \"warn\"\n[http]\nbind = \"127.0.0.1:18081\"\n",
    );
    let outcome = fixture.reloader.reload().expect("重载");
    fixture.state.apply(outcome).expect("应用");

    // 发布值 == 管线里的生效值（cold 已回滚）。
    let effective = fixture.reloader.running().clone();
    assert_eq!(fixture.state.current(), effective);
    assert_eq!(fixture.state.current().log.filter, "warn");

    // 再改一次 hot：视图也要跟上。
    write(&fixture.source, "[log]\nfilter = \"error\"\n");
    let outcome = fixture.reloader.reload().expect("重载");
    fixture.state.apply(outcome).expect("应用");
    assert_eq!(fixture.state.current().log.filter, "error");
    assert_eq!(fixture.state.current(), *fixture.reloader.running());
}
