//! 管线级测试：三段入口、路径锚点、内嵌默认、环境覆盖、校验/钳位、敏感信息。
//!
//! 所有用例都注入 `MapEnv`，不碰进程环境：同一进程里并行跑测试也不会互相干扰。

use std::path::Path;

use tempfile::TempDir;

use {{crate_prefix_snake}}_config::Anchor;
use {{crate_prefix_snake}}_config::ConfigSource;
use {{crate_prefix_snake}}_config::EMBEDDED_TEMPLATE;
use {{crate_prefix_snake}}_config::ENV_PREFIX;
use {{crate_prefix_snake}}_config::ErrorKind;
use {{crate_prefix_snake}}_config::FileConfig;
use {{crate_prefix_snake}}_config::MapEnv;
use {{crate_prefix_snake}}_config::NoticeKind;
use {{crate_prefix_snake}}_config::SourceOrigin;
use {{crate_prefix_snake}}_config::load;
fn anchor(temp: &TempDir) -> Anchor {
    Anchor::from_dir(temp.path())
}

fn no_env() -> MapEnv {
    MapEnv::new()
}

#[test]
fn embedded_template_matches_schema_defaults_field_by_field() {
    let from_template: FileConfig =
        toml::from_str(EMBEDDED_TEMPLATE).expect("内嵌模板必须是合法 TOML");
    assert_eq!(
        from_template,
        FileConfig::default(),
        "内嵌模板与 Default 必须逐字段一致：改 schema 时两个地方要一起改"
    );
}

#[test]
fn missing_config_file_is_written_from_embedded_template() {
    let temp = TempDir::new().expect("临时目录");
    let source = ConfigSource::locate(None, &no_env(), &anchor(&temp));
    assert_eq!(source.origin, SourceOrigin::AnchorDefault);

    let loaded = load(&source, &anchor(&temp), &no_env()).expect("零配置必须可起");
    assert!(loaded.from_embedded_template);
    assert!(loaded.notices.is_empty(), "{:?}", loaded.notices);
    let written = std::fs::read_to_string(&source.path).expect("默认配置应已落盘");
    assert_eq!(written, EMBEDDED_TEMPLATE);
    assert_eq!(
        loaded.config.storage.url.expose().as_str(),
        format!("sqlite:{}/data/service.db?mode=rwc", temp.path().display()).as_str()
    );
}

#[test]
fn cli_beats_env_beats_anchor_default() {
    let temp = TempDir::new().expect("临时目录");
    let anchor = anchor(&temp);
    let default_path = temp.path().join("config.toml");
    let env_path = temp.path().join("env.toml");
    let cli_path = temp.path().join("cli.toml");
    std::fs::write(&default_path, "[log]\nfilter = \"info\"\n").expect("写默认");
    std::fs::write(&env_path, "[log]\nfilter = \"warn\"\n").expect("写 env");
    std::fs::write(&cli_path, "[log]\nfilter = \"error\"\n").expect("写 cli");
    let env = MapEnv::new().with(
        format!("{ENV_PREFIX}_CONFIG"),
        env_path.to_string_lossy().to_string(),
    );

    let source = ConfigSource::locate(Some(cli_path.as_path()), &env, &anchor);
    assert_eq!(source.origin, SourceOrigin::Cli);
    let loaded = load(&source, &anchor, &env).expect("cli 路径必须可读");
    assert_eq!(loaded.config.log.filter, "error");

    let source = ConfigSource::locate(None, &env, &anchor);
    assert_eq!(source.origin, SourceOrigin::Env);
    let loaded = load(&source, &anchor, &env).expect("env 路径必须可读");
    assert_eq!(loaded.config.log.filter, "warn");

    let source = ConfigSource::locate(None, &no_env(), &anchor);
    assert_eq!(source.origin, SourceOrigin::AnchorDefault);
    let loaded = load(&source, &anchor, &no_env()).expect("默认路径必须可读");
    assert_eq!(loaded.config.log.filter, "info");
}

#[test]
fn anchor_is_the_only_base_for_relative_paths() {
    let temp = TempDir::new().expect("临时目录");
    let anchor = anchor(&temp);
    let source = ConfigSource::locate(None, &no_env(), &anchor);
    std::fs::write(&source.path, "[storage]\nurl = \"sqlite:data/x.db\"\n").expect("写配置");

    let loaded = load(&source, &anchor, &no_env()).expect("加载");
    assert_eq!(
        loaded.config.storage.url.expose().as_str(),
        format!("sqlite:{}/data/x.db", temp.path().display()).as_str(),
        "相对路径必须从锚点派生"
    );
}

#[test]
fn anchor_default_path_is_next_to_the_executable() {
    // 生产入口用 current_exe 的父目录；这里只断言"它是绝对目录，且配置文件默认在它下面"。
    let anchor = Anchor::from_current_exe().expect("当前可执行文件的位置必须可确定");
    assert!(anchor.dir().is_absolute(), "{:?}", anchor.dir());
    let source = ConfigSource::locate(None, &no_env(), &anchor);
    assert_eq!(source.path, anchor.dir().join("config.toml"));
}

#[test]
fn unknown_keys_are_errors_and_missing_sections_are_defaults() {
    let temp = TempDir::new().expect("临时目录");
    let anchor = anchor(&temp);
    let source = ConfigSource::locate(None, &no_env(), &anchor);

    std::fs::write(&source.path, "[htp]\nbind = \"127.0.0.1:0\"\n").expect("写配置");
    let err = load(&source, &anchor, &no_env()).expect_err("拼错段名必须报错");
    assert_eq!(err.kind(), ErrorKind::Config);
    assert!(err.to_string().contains("htp"), "{err}");

    std::fs::write(&source.path, "[log]\nfilter = \"debug\"\n").expect("写配置");
    let loaded = load(&source, &anchor, &no_env()).expect("缺段等价默认");
    assert_eq!(loaded.config.log.filter, "debug");
    assert_eq!(loaded.config.supervisor.restart_backoff_ms, 500);
    assert_eq!(loaded.config.http.bind.to_string(), "127.0.0.1:0");
}

#[test]
fn env_value_overlay_applies_and_unknown_prefixed_env_is_rejected() {
    let temp = TempDir::new().expect("临时目录");
    let anchor = anchor(&temp);
    let source = ConfigSource::locate(None, &no_env(), &anchor);
    std::fs::write(&source.path, "[log]\nfilter = \"info\"\n").expect("写配置");

    let env = MapEnv::new()
        .with(format!("{ENV_PREFIX}_LOG__FILTER"), "debug")
        .with(
            format!("{ENV_PREFIX}_SUPERVISOR__RESTART_BACKOFF_MS"),
            "900",
        );
    let loaded = load(&source, &anchor, &env).expect("覆盖应当成功");
    assert_eq!(loaded.config.log.filter, "debug");
    assert_eq!(loaded.config.supervisor.restart_backoff_ms, 900);

    let env = MapEnv::new().with(format!("{ENV_PREFIX}_LOG__FILTRE"), "debug");
    let err = load(&source, &anchor, &env).expect_err("拼错键必须报错");
    assert!(err.to_string().contains("LOG__FILTRE"), "{err}");
}

#[test]
fn values_that_cannot_run_are_rejected_and_out_of_range_is_clamped() {
    let temp = TempDir::new().expect("临时目录");
    let anchor = anchor(&temp);
    let source = ConfigSource::locate(None, &no_env(), &anchor);

    std::fs::write(&source.path, "[http]\nbind = \"127.0.0.1\"\n").expect("写配置");
    let err = load(&source, &anchor, &no_env()).expect_err("非法 bind 必须拒绝");
    assert!(err.to_string().contains("http.bind"), "{err}");

    std::fs::write(&source.path, "[supervisor]\nrestart_backoff_ms = 0\n").expect("写配置");
    let err = load(&source, &anchor, &no_env()).expect_err("0 退避必须拒绝");
    assert!(err.to_string().contains("restart_backoff_ms"), "{err}");

    std::fs::write(&source.path, "[storage]\nbusy_timeout_ms = 10\n").expect("写配置");
    let loaded = load(&source, &anchor, &no_env()).expect("越界只钳位");
    assert_eq!(loaded.config.storage.busy_timeout_ms, 100);
    assert!(
        loaded
            .notices
            .iter()
            .any(|notice| notice.kind == NoticeKind::Clamped)
    );
}

#[test]
fn secret_precedence_is_env_then_file_then_literal() {
    let temp = TempDir::new().expect("临时目录");
    let anchor = anchor(&temp);
    let source = ConfigSource::locate(None, &no_env(), &anchor);
    std::fs::write(temp.path().join("url.txt"), "sqlite:from-file.db\n").expect("写 secret 文件");

    std::fs::write(&source.path, "[storage]\nurl = \"sqlite:literal.db\"\n").expect("写配置");
    let loaded = load(&source, &anchor, &no_env()).expect("加载");
    assert_eq!(
        loaded.config.storage.url.expose().as_str(),
        format!("sqlite:{}/literal.db", temp.path().display()).as_str(),
        "明文的相对路径同样从锚点派生"
    );

    std::fs::write(
        &source.path,
        "[storage]\nurl = \"sqlite:literal.db\"\nurl_file = \"url.txt\"\n",
    )
    .expect("写配置");
    let loaded = load(&source, &anchor, &no_env()).expect("加载");
    assert_eq!(
        loaded.config.storage.url.expose().as_str(),
        format!("sqlite:{}/from-file.db", temp.path().display()).as_str()
    );
    assert!(!format!("{:?}", loaded.config.storage.url).contains("from-file"));
    assert!(
        loaded
            .notices
            .iter()
            .any(|notice| notice.kind == NoticeKind::ShadowedSecretSource)
    );

    std::fs::write(
        &source.path,
        "[storage]\nurl = \"sqlite:literal.db\"\nurl_file = \"url.txt\"\nurl_env = \"TEST_SVC_URL\"\n",
    )
    .expect("写配置");
    let env = MapEnv::new().with("TEST_SVC_URL", "sqlite:from-env.db");
    let loaded = load(&source, &anchor, &env).expect("加载");
    assert_eq!(
        loaded.config.storage.url.expose().as_str(),
        format!("sqlite:{}/from-env.db", temp.path().display()).as_str()
    );
}

#[test]
fn missing_secret_source_is_a_startup_error() {
    let temp = TempDir::new().expect("临时目录");
    let anchor = anchor(&temp);
    let source = ConfigSource::locate(None, &no_env(), &anchor);
    std::fs::write(
        &source.path,
        "[storage]\nurl = \"\"\nurl_env = \"TEST_SVC_MISSING\"\n",
    )
    .expect("写配置");
    let err = load(&source, &anchor, &no_env()).expect_err("缺失的 secret 来源必须拒绝启动");
    assert_eq!(err.kind(), ErrorKind::Config);
    assert!(err.to_string().contains("url_env"), "{err}");
}

#[test]
fn relative_url_file_is_anchored_and_missing_file_is_an_error() {
    let temp = TempDir::new().expect("临时目录");
    let anchor = anchor(&temp);
    let source = ConfigSource::locate(None, &no_env(), &anchor);
    std::fs::write(&source.path, "[storage]\nurl_file = \"nope.txt\"\n").expect("写配置");
    let err = load(&source, &anchor, &no_env()).expect_err("缺文件必须报错");
    assert!(err.to_string().contains("url_file"), "{err}");
    assert!(err.to_string().contains("nope.txt"), "{err}");
}

#[test]
fn default_log_filter_is_normalized_when_blank() {
    let temp = TempDir::new().expect("临时目录");
    let anchor = anchor(&temp);
    let source = ConfigSource::locate(None, &no_env(), &anchor);
    std::fs::write(&source.path, "[log]\nfilter = \"   \"\n").expect("写配置");
    let loaded = load(&source, &anchor, &no_env()).expect("空过滤器等价默认");
    assert_eq!(loaded.config.log.filter, "info");
}

#[test]
fn read_failure_that_is_not_missing_file_is_an_error() {
    let temp = TempDir::new().expect("临时目录");
    let anchor = anchor(&temp);
    // 把目录当配置文件读：不是 NotFound，必须报错而不是悄悄用默认值。
    let source = ConfigSource {
        path: temp.path().to_path_buf(),
        origin: SourceOrigin::Cli,
    };
    let err = load(&source, &anchor, &no_env()).expect_err("目录不能当配置读");
    assert!(err.to_string().contains("读取配置文件"), "{err}");
}

#[test]
fn embedded_template_is_written_next_to_anchor_not_cwd() {
    let temp = TempDir::new().expect("临时目录");
    let anchor = anchor(&temp);
    let source = ConfigSource::locate(None, &no_env(), &anchor);
    let _ = load(&source, &anchor, &no_env()).expect("加载");
    let expected = temp.path().join("config.toml");
    assert!(Path::new(&expected).exists(), "默认配置应写在锚点下");
}
