//! The real binary: configuration problems and `check-config`, the command line, a run from
//! start to a clean stop, signals, the exit codes, `probe`, and a closed stdout.

mod support;

use std::process::Stdio;
use std::time::Duration;

use support::{
    Running, Scratch, TestResult, WAIT, command, config, env_prefix, finish, request, service_name,
};

#[test]
fn every_configuration_problem_is_reported_at_once_and_both_commands_agree() -> TestResult {
    let scratch = Scratch::new("problems")?;
    let file = scratch.file("bad.toml", "[server]\nnope = 1\n[log]\nfilter = \"[\"\n")?;
    let file = file.to_string_lossy().into_owned();
    let timeout = format!("{}_LIFECYCLE__DRAIN_TIMEOUT", env_prefix());
    for subcommand in ["run", "check-config"] {
        let args = [subcommand, "--config", &file, "--http-addr", "nowhere"];
        let mut command = command(&args);
        command.env(&timeout, "5 seconds");
        let done = finish(command)?;
        assert_eq!(done.code, Some(78), "{subcommand}: {}", done.stderr);
        let lines: Vec<&str> = done.stderr.lines().collect();
        assert_eq!(lines.len(), 4, "{}", done.stderr);
        let name = service_name();
        assert!(
            lines
                .iter()
                .all(|line| line.starts_with(&format!("{name}: invalid configuration: ")))
        );
        for key in [
            "lifecycle.drain_timeout",
            "log.filter",
            "server.http_addr",
            "server.nope",
        ] {
            assert!(
                done.stderr.contains(&format!(" {key} (")),
                "{key}: {}",
                done.stderr
            );
        }
    }
    Ok(())
}

#[test]
fn check_config_prints_every_key_with_its_source() -> TestResult {
    let scratch = Scratch::new("check")?;
    let file = scratch.file("app.toml", "[events]\ncapacity = 8\n")?;
    let prefix = env_prefix();
    let mut command = command(&["check-config", "--log-filter", "debug"]);
    command
        .env(format!("{prefix}_CONFIG"), &file)
        .env(format!("{prefix}_SERVICE_HOST"), "10.0.0.1")
        .env("RUST_LOG", "");
    let done = finish(command)?;
    assert_eq!(done.code, Some(0), "{}", done.stderr);
    let mut lines = done.stdout.lines();
    assert_eq!(lines.next(), Some("configuration is valid"));
    let rows: Vec<Vec<&str>> = lines
        .map(|line| {
            line.split("  ")
                .map(str::trim)
                .filter(|c| !c.is_empty())
                .collect()
        })
        .collect();
    let row = |key: &str| {
        rows.iter()
            .find(|row| row[0].trim() == key)
            .map(|row| row[1..].join("|"))
    };
    let shown = file.display().to_string();
    assert_eq!(row("events.capacity"), Some(format!("8|file {shown}")));
    assert_eq!(
        row("log.filter"),
        Some("debug|cli --log-filter".to_string())
    );
    assert_eq!(
        row("server.http_addr"),
        Some("127.0.0.1:8080|default".to_string())
    );
    Ok(())
}

#[test]
fn an_empty_filter_is_an_error_and_an_empty_rust_log_is_unset() -> TestResult {
    let mut empty = command(&["check-config"]);
    empty.env(format!("{}_LOG__FILTER", env_prefix()), "");
    let done = finish(empty)?;
    assert_eq!(done.code, Some(78));
    assert!(done.stderr.contains("log.filter"), "{}", done.stderr);
    let mut unset = command(&["check-config"]);
    unset.env("RUST_LOG", "");
    assert_eq!(finish(unset)?.code, Some(0));
    Ok(())
}

#[test]
fn the_command_line_version_help_and_usage_errors() -> TestResult {
    let version = finish(command(&["--version"]))?;
    assert_eq!(version.code, Some(0));
    let line = version.stdout.trim();
    let prefix = format!("{} 0.1.0 (", service_name());
    assert!(line.starts_with(&prefix) && line.ends_with(')'), "{line}");
    let sha = &line[prefix.len()..line.len() - 1];
    assert!(
        sha == "unknown" || (sha.len() == 7 && sha.chars().all(|c| c.is_ascii_hexdigit())),
        "{line}"
    );
    let help = finish(command(&["run", "--help"]))?;
    assert_eq!(help.code, Some(0));
    assert!(
        help.stdout.contains(&format!("{}_CONFIG", env_prefix())),
        "{}",
        help.stdout
    );
    assert_eq!(finish(command(&["run", "--no-such-flag"]))?.code, Some(64));
    assert_eq!(finish(command(&[]))?.code, Some(64));
    Ok(())
}

#[test]
fn a_run_serves_drains_on_sigterm_and_stops_cleanly() -> TestResult {
    let scratch = Scratch::new("run")?;
    let file = config(&scratch, "1s", "5s", "")?;
    let mut service = Running::start(&file, &["--log-filter", "info"], &[])?;
    let addr = service.running()?;
    let starting = service.wait_for("starting")?;
    assert!(starting["fields"]["git_sha"].is_string());
    let overrides = service.wait_for("configuration")?;
    let shown = overrides["fields"]["overrides"]
        .as_str()
        .unwrap_or_default();
    assert!(
        shown.contains("log.filter=info (cli --log-filter)"),
        "{shown}"
    );
    assert_eq!(request(addr, "GET", "/readyz", None)?.0, 200);
    let (status, _) = request(addr, "POST", "/v1/todos", Some(r#"{"title":"Buy milk"}"#))?;
    assert_eq!(status, 201);
    service.signal("TERM")?;
    service.wait_for_phase("draining")?;
    // While draining, readiness says no but requests are still served.
    assert_eq!(request(addr, "GET", "/readyz", None)?.0, 503);
    assert_eq!(request(addr, "GET", "/v1/todos", None)?.0, 200);
    assert_eq!(service.exit(WAIT)?, Some(0));
    let last = service.lines().pop().unwrap_or_default();
    assert_eq!(
        (&last["fields"]["message"], &last["fields"]["exit_code"]),
        (&"stopped".into(), &0.into())
    );
    Ok(())
}

#[test]
fn a_second_signal_cuts_the_drain_short() -> TestResult {
    let scratch = Scratch::new("abort")?;
    let file = config(&scratch, "30s", "5s", "")?;
    let mut service = Running::start(&file, &[], &[])?;
    service.running()?;
    service.signal("TERM")?;
    service.wait_for_phase("draining")?;
    service.signal("INT")?;
    assert_eq!(service.exit(WAIT)?, Some(130));
    Ok(())
}

#[test]
fn sighup_is_logged_and_ignored() -> TestResult {
    let scratch = Scratch::new("hup")?;
    let file = config(&scratch, "0s", "5s", "")?;
    let mut service = Running::start(&file, &[], &[])?;
    service.running()?;
    service.signal("HUP")?;
    service.wait_for("reloading is not supported; ignored")?;
    service.signal("INT")?;
    assert_eq!(service.exit(WAIT)?, Some(0));
    Ok(())
}

#[test]
fn a_port_in_use_fails_the_startup_with_69() -> TestResult {
    let taken = std::net::TcpListener::bind("127.0.0.1:0")?;
    let scratch = Scratch::new("port")?;
    let file = config(&scratch, "0s", "5s", "")?;
    let addr = taken.local_addr()?.to_string();
    let mut service = Running::start(&file, &["--http-addr", &addr], &[])?;
    assert_eq!(service.exit(WAIT)?, Some(69));
    let failed = service.with_message("service failed");
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0]["fields"]["error.type"], "BindError");
    Ok(())
}

#[test]
fn a_log_directory_that_cannot_be_created_fails_with_73() -> TestResult {
    let scratch = Scratch::new("logs")?;
    let blocker = scratch.file("not-a-dir", "")?;
    let dir = blocker.join("logs").to_string_lossy().into_owned();
    let extra = format!("[log.file]\nenabled = true\ndir = \"{dir}\"\n");
    let file = config(&scratch, "0s", "5s", &extra)?;
    let mut service = Running::start(&file, &[], &[])?;
    assert_eq!(service.exit(WAIT)?, Some(73));
    assert!(
        service.stderr().contains("cannot start logging"),
        "{}",
        service.stderr()
    );
    Ok(())
}

#[test]
fn probe_tries_every_address_and_says_why_it_failed() -> TestResult {
    let scratch = Scratch::new("probe")?;
    let file = config(&scratch, "0s", "5s", "")?;
    let mut service = Running::start(&file, &[], &[])?;
    let addr = service.running()?;
    // `localhost` may resolve to ::1 first; the service listens on 127.0.0.1 only.
    let url = format!("http://localhost:{}/readyz", addr.port());
    let ok = finish(command(&["probe", &url]))?;
    assert_eq!(
        (ok.code, ok.stdout.as_str(), ok.stderr.as_str()),
        (Some(0), "", "")
    );
    service.signal("INT")?;
    service.exit(WAIT)?;
    let down = finish(command(&["probe", &url]))?;
    assert_eq!(down.code, Some(1));
    assert!(
        down.stderr
            .starts_with(&format!("{}: probe failed: {url}: ", service_name())),
        "{}",
        down.stderr
    );
    Ok(())
}

#[test]
fn a_closed_stdout_is_not_a_crash() -> TestResult {
    for args in [&["check-config"][..], &["--help"][..]] {
        let mut child = command(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        // Close the read end at once: every write to stdout fails.
        drop(child.stdout.take());
        let status = wait(&mut child)?;
        assert_eq!(status, Some(0), "{args:?}");
    }
    Ok(())
}

fn wait(child: &mut std::process::Child) -> TestResult<Option<i32>> {
    let start = std::time::Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status.code());
        }
        if start.elapsed() > WAIT {
            child.kill().ok();
            return Err("still running".into());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
