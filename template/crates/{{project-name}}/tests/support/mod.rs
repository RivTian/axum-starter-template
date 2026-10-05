//! Helpers shared by the process-level tests: start the binary, follow its JSON log lines,
//! send real signals, and make plain HTTP/1.1 requests. Every wait has a limit, so a
//! binary that hangs fails the test instead of blocking it.

pub(crate) mod bin;

use std::error::Error;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

pub(crate) type TestResult<T = ()> = Result<T, Box<dyn Error>>;

/// How long a started service may take to log a line the test waits for.
pub(crate) const WAIT: Duration = Duration::from_secs(10);

/// The binary's file name, which is the project name and the service name.
pub(crate) fn service_name() -> String {
    Path::new(bin::BIN)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The environment variable prefix, derived like the template derives it from the project
/// name: upper case, with `_` for `-` (project names are lower-case letters, digits and
/// single hyphens).
pub(crate) fn env_prefix() -> String {
    service_name().to_ascii_uppercase().replace('-', "_")
}

/// A command for the binary without `RUST_LOG` and the project's variables from the
/// environment the tests run in.
pub(crate) fn command(args: &[&str]) -> Command {
    let mut command = Command::new(bin::BIN);
    command.args(args);
    without_our_variables(&mut command);
    command
}

/// Like [`command`], run by the shell with at most `files` open file descriptors. Unix only.
#[cfg(unix)]
pub(crate) fn command_with_file_limit(files: u32, args: &[&str]) -> Command {
    let mut command = Command::new("sh");
    let script = format!("ulimit -n {files} && exec \"$@\"");
    command.args(["-c", &script, "sh", bin::BIN]).args(args);
    without_our_variables(&mut command);
    command
}

fn without_our_variables(command: &mut Command) {
    let prefix = format!("{}_", env_prefix());
    for (name, _) in std::env::vars_os() {
        if name == "RUST_LOG" || name.to_string_lossy().starts_with(&prefix) {
            command.env_remove(name);
        }
    }
}

/// The exit code, stdout and stderr of a command that ends by itself.
pub(crate) struct Finished {
    pub(crate) code: Option<i32>,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
}

/// Runs a command that ends by itself, such as `check-config` or `probe`.
pub(crate) fn finish(mut command: Command) -> TestResult<Finished> {
    let child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut service = Running::watch(child)?;
    let code = service.exit(WAIT)?;
    Ok(Finished {
        code,
        stdout: service.stdout_text(),
        stderr: service.stderr(),
    })
}

/// A directory for one test's files, removed when dropped.
pub(crate) struct Scratch {
    dir: PathBuf,
}

impl Scratch {
    pub(crate) fn new(test: &str) -> TestResult<Self> {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "{}-process-{}-{}-{test}",
            service_name(),
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir)?;
        Ok(Scratch { dir })
    }

    /// Writes a file and returns its path.
    pub(crate) fn file(&self, name: &str, text: &str) -> TestResult<PathBuf> {
        let path = self.dir.join(name);
        std::fs::write(&path, text)?;
        Ok(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

/// A configuration file for `run`: a free port, the given drain timings, and more lines.
pub(crate) fn config(
    scratch: &Scratch,
    drain_delay: &str,
    drain_timeout: &str,
    extra: &str,
) -> TestResult<PathBuf> {
    scratch.file(
        "service.toml",
        &format!(
            "[server]\nhttp_addr = \"127.0.0.1:0\"\n\n[lifecycle]\ndrain_delay = \"{drain_delay}\"\ndrain_timeout = \"{drain_timeout}\"\n{extra}"
        ),
    )
}

/// A process being watched: its stdout lines (JSON when they parse) and its stderr.
pub(crate) struct Running {
    child: Child,
    stdout: Arc<Mutex<Vec<String>>>,
    stderr: Arc<Mutex<String>>,
    readers: Vec<thread::JoinHandle<()>>,
}

impl Running {
    /// Starts `run --log-format json` with the configuration file, more arguments and
    /// environment variables.
    pub(crate) fn start(config: &Path, args: &[&str], env: &[(&str, &str)]) -> TestResult<Self> {
        let config = config.to_string_lossy().into_owned();
        let mut all = vec!["run", "--config", config.as_str(), "--log-format", "json"];
        all.extend_from_slice(args);
        let mut command = command(&all);
        command.envs(env.iter().copied());
        Running::spawn(command)
    }

    /// Like [`Running::start`], with at most `files` open file descriptors. Unix only.
    #[cfg(unix)]
    pub(crate) fn start_with_file_limit(config: &Path, files: u32) -> TestResult<Self> {
        let config = config.to_string_lossy().into_owned();
        let args = ["run", "--config", config.as_str(), "--log-format", "json"];
        Running::spawn(command_with_file_limit(files, &args))
    }

    fn spawn(mut command: Command) -> TestResult<Self> {
        let child = command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        Running::watch(child)
    }

    fn watch(mut child: Child) -> TestResult<Self> {
        let stdout = Arc::new(Mutex::new(Vec::new()));
        let stderr = Arc::new(Mutex::new(String::new()));
        let out = child.stdout.take().ok_or("no stdout")?;
        let err = child.stderr.take().ok_or("no stderr")?;
        let lines = stdout.clone();
        let text = stderr.clone();
        let readers = vec![
            thread::spawn(move || {
                for line in BufReader::new(out).lines().map_while(Result::ok) {
                    if let Ok(mut lines) = lines.lock() {
                        lines.push(line);
                    }
                }
            }),
            thread::spawn(move || {
                let mut all = String::new();
                BufReader::new(err).read_to_string(&mut all).ok();
                if let Ok(mut text) = text.lock() {
                    text.push_str(&all);
                }
            }),
        ];
        Ok(Running {
            child,
            stdout,
            stderr,
            readers,
        })
    }

    /// The stdout lines that are JSON, in order.
    pub(crate) fn lines(&self) -> Vec<Value> {
        self.stdout
            .lock()
            .map(|lines| {
                lines
                    .iter()
                    .filter_map(|line| serde_json::from_str(line).ok())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The lines with this message.
    pub(crate) fn with_message(&self, message: &str) -> Vec<Value> {
        self.lines()
            .into_iter()
            .filter(|line| line["fields"]["message"] == message)
            .collect()
    }

    fn stdout_text(&self) -> String {
        self.stdout
            .lock()
            .map(|lines| {
                lines.iter().fold(String::new(), |mut text, line| {
                    text.push_str(line);
                    text.push('\n');
                    text
                })
            })
            .unwrap_or_default()
    }

    pub(crate) fn stderr(&self) -> String {
        self.stderr
            .lock()
            .map(|text| text.clone())
            .unwrap_or_default()
    }

    /// Waits for the first line that `wanted` accepts.
    pub(crate) fn wait_for_line(
        &self,
        what: &str,
        wanted: impl Fn(&Value) -> bool,
    ) -> TestResult<Value> {
        let start = Instant::now();
        loop {
            if let Some(line) = self.lines().into_iter().find(|line| wanted(line)) {
                return Ok(line);
            }
            if start.elapsed() > WAIT {
                return Err(format!(
                    "no {what} within {WAIT:?}; stdout:\n{}\nstderr:\n{}",
                    self.stdout_text(),
                    self.stderr()
                )
                .into());
            }
            thread::sleep(Duration::from_millis(20));
        }
    }

    /// Waits for the first line with this message.
    pub(crate) fn wait_for(&self, message: &str) -> TestResult<Value> {
        self.wait_for_line(message, |line| line["fields"]["message"] == message)
    }

    /// Waits for the phase change to `phase`.
    pub(crate) fn wait_for_phase(&self, phase: &str) -> TestResult<Value> {
        self.wait_for_line(&format!("phase {phase}"), |line| {
            line["fields"]["message"] == "phase changed" && line["fields"]["phase"] == phase
        })
    }

    /// Waits until the server runs, and returns the address it listens on.
    pub(crate) fn running(&self) -> TestResult<SocketAddr> {
        let listening = self.wait_for("listening")?;
        self.wait_for_phase("running")?;
        Ok(listening["fields"]["http.addr"]
            .as_str()
            .ok_or("no http.addr")?
            .parse()?)
    }

    /// Stops the service and waits for it: with SIGINT on Unix; on Windows, where no console
    /// event can be sent to it from here, by ending the process.
    pub(crate) fn stop(&mut self) -> TestResult {
        #[cfg(unix)]
        self.signal("INT")?;
        #[cfg(windows)]
        self.child.kill()?;
        self.exit(WAIT)?;
        Ok(())
    }

    /// Sends a real signal with `kill -s <name>`. Unix only.
    #[cfg(unix)]
    pub(crate) fn signal(&self, name: &str) -> TestResult {
        let status = Command::new("kill")
            .args(["-s", name, &self.child.id().to_string()])
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("kill -s {name} failed: {status}").into())
        }
    }

    /// Waits for the process to exit and for its output to be read; the exit code, or
    /// `None` when a signal ended it.
    pub(crate) fn exit(&mut self, limit: Duration) -> TestResult<Option<i32>> {
        let start = Instant::now();
        let status = loop {
            if let Some(status) = self.child.try_wait()? {
                break status;
            }
            if start.elapsed() > limit {
                self.child.kill().ok();
                return Err(format!(
                    "still running after {limit:?}; stdout:\n{}\nstderr:\n{}",
                    self.stdout_text(),
                    self.stderr()
                )
                .into());
            }
            thread::sleep(Duration::from_millis(20));
        };
        for reader in self.readers.drain(..) {
            reader.join().ok();
        }
        Ok(status.code())
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            self.child.kill().ok();
            self.child.wait().ok();
        }
    }
}

/// Sends one HTTP/1.1 request with an optional JSON body; the status code and the body.
pub(crate) fn request(
    addr: SocketAddr,
    method: &str,
    path: &str,
    body: Option<&str>,
) -> TestResult<(u16, String)> {
    let mut stream = TcpStream::connect(addr)?;
    stream.set_read_timeout(Some(WAIT))?;
    let body = body.unwrap_or_default();
    let length = body.len();
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nhost: {addr}\r\nconnection: close\r\ncontent-type: application/json\r\ncontent-length: {length}\r\n\r\n{body}"
    )?;
    let mut raw = String::new();
    stream.read_to_string(&mut raw)?;
    let status = raw
        .split_whitespace()
        .nth(1)
        .ok_or("no status line")?
        .parse()?;
    let body = raw
        .split_once("\r\n\r\n")
        .map(|(_, body)| body.to_string())
        .unwrap_or_default();
    Ok((status, body))
}
