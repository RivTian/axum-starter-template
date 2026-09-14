use super::*;
use std::collections::BTreeMap;
use std::ffi::OsString;

fn context(dir: &std::path::Path) -> LoadContext {
    LoadContext::new("service.toml".into(), dir, BTreeMap::new(), 2).unwrap()
}
#[test]
fn shipped_sample_and_omitted_defaults_are_equivalent() {
    let dir = tempfile::tempdir().unwrap();
    let context = context(dir.path());
    let sample = load::parse(include_str!("../../../config/service.toml"), &context).unwrap();
    let omitted = load::parse("", &context).unwrap();
    assert_eq!(sample, omitted);
    assert_eq!(sample.boot.worker_threads, 2);
}
#[test]
fn paths_use_the_captured_configuration_directory_not_the_process_cwd() {
    let dir = tempfile::tempdir().unwrap();
    let context = context(dir.path());
    let cfg = load::parse("[storage]\npath='data/file.sqlite3'", &context).unwrap();
    assert_eq!(
        cfg.boot.storage.path(),
        dir.path().join("data/file.sqlite3")
    );
    let ipv6 = load::parse("[http]\nlisten='[::1]:0'", &context).unwrap();
    assert!(ipv6.boot.storage.path().is_absolute());
}
#[test]
fn env_strings_cannot_inject_toml_or_recursively_expand() {
    let dir = tempfile::tempdir().unwrap();
    let mut context = context(dir.path());
    let value = "quotes\"\n[http]\nlisten='PRIVATE_TOKEN'\n${OTHER}";
    context.environment.insert("DB_PATH".into(), value.into());
    let cfg = load::parse("[storage]\npath='${DB_PATH}'", &context).unwrap();
    assert_eq!(cfg.boot.storage.path(), dir.path().join(value));
    let literal = load::parse("[storage]\npath='cash$$.sqlite3'", &context).unwrap();
    assert_eq!(
        literal.boot.storage.path(),
        dir.path().join("cash$.sqlite3")
    );
    let fallback = load::parse("[storage]\npath='${MISSING:-fallback.sqlite3}'", &context).unwrap();
    assert_eq!(
        fallback.boot.storage.path(),
        dir.path().join("fallback.sqlite3")
    );
}
#[test]
fn invalid_fields_values_and_future_runtime_topology_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let context = context(dir.path());
    for invalid in [
        "unknown=true",
        "[runtime.extra.compute]\nworker_threads=1",
        "[ticker]\nruntime='other'",
        "[ticker]\ninterval_ms=0",
        "[runtime]\nworker_threads=0",
        "[storage]\nmax_connections=0",
        "[storage]\npath='/'",
        "[storage]\nmin_connections=5\nmax_connections=4",
        "[http]\nlisten='not-an-address'",
        "[http]\nready_probe_timeout_ms=2000",
        "[lifecycle]\ngrace_ms=1000",
        "[lifecycle]\nabort_reap_ms=9223372036854775807",
        "[lifecycle]\nreload_timeout_ms=0",
        "[lifecycle]\nreload_timeout_ms=60001",
        "[ticker]\ninterval_ms='${PERIOD:-1000}'",
        "[storage]\npath='${MISSING}'",
        "[storage]\npath='${BAD-NAME}'",
        "[storage]\npath='${BROKEN'",
    ] {
        assert!(
            load::parse(invalid, &context).is_err(),
            "accepted {invalid}"
        );
    }
}
#[test]
fn errors_do_not_echo_configuration_values() {
    let dir = tempfile::tempdir().unwrap();
    let context = context(dir.path());
    for invalid in [
        "[http]\nlisten='PRIVATE_TOKEN'",
        "[ticker]\ninterval_ms='PRIVATE_TOKEN'",
        "this is PRIVATE_TOKEN !!!",
    ] {
        let error = load::parse(invalid, &context).unwrap_err().to_string();
        assert!(!error.contains("PRIVATE_TOKEN"));
    }
}
#[test]
fn missing_oversized_and_non_utf8_files_fail_before_resources_exist() {
    let dir = tempfile::tempdir().unwrap();
    let context = context(dir.path());
    assert!(load(&context).is_err());
    std::fs::write(&context.path, vec![b' '; 65 * 1024]).unwrap();
    assert!(load(&context).is_err());
    std::fs::write(&context.path, [0xff, 0xfe]).unwrap();
    assert!(load(&context).is_err());
}
#[cfg(unix)]
#[test]
fn non_utf8_environment_values_are_explicit_errors() {
    use std::os::unix::ffi::OsStringExt;
    let dir = tempfile::tempdir().unwrap();
    let mut context = context(dir.path());
    context
        .environment
        .insert(OsString::from("BAD"), OsString::from_vec(vec![0xff]));
    assert!(load::parse("[storage]\npath='${BAD}'", &context).is_err());
}

#[test]
fn environment_expansion_has_a_separate_size_limit() {
    let dir = tempfile::tempdir().unwrap();
    let mut context = context(dir.path());
    context
        .environment
        .insert("HUGE".into(), "x".repeat(65 * 1024).into());
    assert!(load::parse("[storage]\npath='${HUGE}'", &context).is_err());
}

#[test]
fn every_current_cold_field_is_rejected_and_has_a_stable_report_path() {
    let dir = tempfile::tempdir().unwrap();
    let context = context(dir.path());
    let base = load::parse("", &context).unwrap();
    for (text, field) in [
        ("[http]\nlisten='127.0.0.2:0'", "http"),
        ("[storage]\npath='other.sqlite3'", "storage"),
        ("[runtime]\nworker_threads=3", "runtime.worker_threads"),
        (
            "[runtime]\nmax_blocking_threads=8",
            "runtime.max_blocking_threads",
        ),
        (
            "[lifecycle]\nstartup_timeout_ms=11000",
            "lifecycle.startup_timeout_ms",
        ),
        (
            "[lifecycle]\nreload_timeout_ms=3000",
            "lifecycle.reload_timeout_ms",
        ),
        ("[lifecycle]\ngrace_ms=6000", "lifecycle.grace_ms"),
        ("[lifecycle]\nabort_reap_ms=2000", "lifecycle.abort_reap_ms"),
        (
            "[lifecycle]\nstorage_close_ms=4000",
            "lifecycle.storage_close_ms",
        ),
        (
            "[lifecycle]\nruntime_shutdown_ms=2000",
            "lifecycle.runtime_shutdown_ms",
        ),
    ] {
        let next = load::parse(text, &context).unwrap();
        assert_ne!(base.boot, next.boot);
        assert_eq!(base.boot.changed_fields(&next.boot), [field]);
    }
}

#[test]
fn optional_runtime_bindings_are_normalized_and_must_reference_used_declared_budgets() {
    let dir = tempfile::tempdir().unwrap();
    let context = context(dir.path());
    let default = load::parse("", &context).unwrap();
    let explicit =
        load::parse("[http]\nruntime='main'\n[ticker]\nruntime='main'", &context).unwrap();
    assert_eq!(default, explicit);
    let extra = load::parse("[ticker]\nruntime='compute'\n[runtime.extra.compute]\nworker_threads=1\nmax_blocking_threads=2", &context).unwrap();
    assert_eq!(extra.boot.ticker_runtime, Binding::Extra("compute".into()));
    assert_eq!(extra.boot.http_runtime, Binding::Main);
    assert_eq!(extra.boot.thread_totals(), Some((3, 18)));
    assert_eq!(
        default.boot.changed_fields(&extra.boot),
        ["runtime.extra", "ticker.runtime"]
    );
    for invalid in [
        "[http]\nruntime='typo'",
        "[runtime.extra.unused]\nworker_threads=1\nmax_blocking_threads=1",
        "[runtime.extra.main]\nworker_threads=1\nmax_blocking_threads=1",
        "[ticker]\nruntime='UPPER'",
        "[ticker]\nruntime='bad--name'",
        "[ticker]\nruntime='compute'\n[runtime.extra.compute]\nworker_threads=0\nmax_blocking_threads=1",
        "[ticker]\nruntime='compute'\n[runtime.extra.compute]\nworker_threads=1\nmax_blocking_threads=0",
        "[ticker]\nruntime='compute'\n[runtime.extra.compute]\nworker_threads=1",
        "[runtime]\nworker_threads=256\n[ticker]\nruntime='compute'\n[runtime.extra.compute]\nworker_threads=1\nmax_blocking_threads=1",
        "[runtime]\nmax_blocking_threads=256\n[ticker]\nruntime='compute'\n[runtime.extra.compute]\nworker_threads=1\nmax_blocking_threads=1",
    ] {
        assert!(
            load::parse(invalid, &context).is_err(),
            "accepted {invalid}"
        );
    }
}

#[test]
fn runtime_name_length_and_topology_size_limits_are_checked_without_building_threads() {
    let dir = tempfile::tempdir().unwrap();
    let context = context(dir.path());
    let name = "a".repeat(33);
    assert!(load::parse(&format!("[ticker]\nruntime='{name}'\n[runtime.extra.{name}]\nworker_threads=1\nmax_blocking_threads=1"), &context).is_err());
    let mut many = String::new();
    for index in 0..9 {
        many += &format!("[runtime.extra.x{index}]\nworker_threads=1\nmax_blocking_threads=1\n");
    }
    let error = load::parse(&many, &context).unwrap_err().to_string();
    assert!(error.contains("at most 8"), "{error}");
}

#[test]
fn configuration_location_precedence_uses_only_explicit_inputs() {
    use std::ffi::OsStr;
    let key = OsStr::new("DEMO_CONFIG");
    let environment = BTreeMap::from([(key.to_os_string(), OsString::from("env.toml"))]);
    assert_eq!(
        select_path(Some("cli.toml".into()), &environment, key),
        ("cli.toml".into(), "cli")
    );
    assert_eq!(
        select_path(None, &environment, key),
        ("env.toml".into(), "environment")
    );
    assert_eq!(
        select_path(None, &BTreeMap::new(), key),
        (DEFAULT_PATH.into(), "default")
    );
    // An explicitly empty environment path must fail validation, not silently
    // choose another file. Path source selection itself has no I/O side effects.
    let empty = BTreeMap::from([(key.to_os_string(), OsString::new())]);
    let (path, source) = select_path(None, &empty, key);
    assert_eq!(source, "environment");
    assert!(LoadContext::new(path, std::path::Path::new("/"), empty, 1).is_err());
}
