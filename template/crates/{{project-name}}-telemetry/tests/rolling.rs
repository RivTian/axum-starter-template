//! The log file: rotation by UTC date and archive names, archiving at start by the date
//! the old file was last written or appending to it on the same date, retention,
//! compression that never holds up writes, unfinished compressions, and a file or
//! directory deleted while the writer runs.
//! Time limits allow for debug builds: they bound what must stay fast and how long the
//! work may take to finish, separately.

use std::error::Error;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use flate2::read::GzDecoder;
use svc_telemetry::rolling::RollingGzipWriter;
use time::Date;
use time::macros::{date, datetime};

type TestResult = Result<(), Box<dyn Error>>;

/// How long compression may take to finish.
const FINISH: Duration = Duration::from_secs(60);

/// A directory of its own, removed when dropped.
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Result<Self, Box<dyn Error>> {
        let dir = std::env::temp_dir().join(format!("svc-telemetry-{}-{name}", std::process::id()));
        if dir.exists() {
            fs::remove_dir_all(&dir)?;
        }
        fs::create_dir_all(&dir)?;
        Ok(TempDir(dir))
    }

    fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    fn names(&self) -> Result<Vec<String>, Box<dyn Error>> {
        let mut names = Vec::new();
        for entry in fs::read_dir(&self.0)? {
            names.push(entry?.file_name().to_string_lossy().into_owned());
        }
        names.sort();
        Ok(names)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).ok();
    }
}

/// A date the test moves forward by hand.
#[derive(Clone)]
struct Clock(Arc<Mutex<Date>>);

impl Clock {
    fn new(date: Date) -> Self {
        Clock(Arc::new(Mutex::new(date)))
    }

    fn set(&self, date: Date) {
        if let Ok(mut current) = self.0.lock() {
            *current = date;
        }
    }

    fn source(&self) -> impl Fn() -> Date + Send + 'static {
        let clock = self.0.clone();
        move || clock.lock().map_or(date!(1970 - 01 - 01), |date| *date)
    }
}

/// Polls until `done` holds, for at most `limit`.
fn eventually(limit: Duration, done: impl Fn() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < limit {
        if done() {
            return true;
        }
        thread::sleep(Duration::from_millis(20));
    }
    done()
}

fn gunzip(path: &Path) -> Result<String, Box<dyn Error>> {
    let mut text = String::new();
    GzDecoder::new(File::open(path)?).read_to_string(&mut text)?;
    Ok(text)
}

#[test]
fn rotation_naming() -> TestResult {
    let dir = TempDir::new("rotation")?;
    let clock = Clock::new(date!(2026 - 09 - 29));
    {
        let mut writer = RollingGzipWriter::open_with_clock(&dir.0, "app", 14, clock.source())?;
        writer.write_all(b"first day\n")?;
        clock.set(date!(2026 - 09 - 30));
        writer.write_all(b"second day\n")?;
        assert_eq!(writer.failures(), 0);
    }
    let first = dir.join("app.2026-09-29.log.gz");
    assert!(eventually(FINISH, || first.exists()), "{:?}", dir.names()?);
    assert_eq!(gunzip(&first)?, "first day\n");
    assert_eq!(fs::read_to_string(dir.join("app.log"))?, "second day\n");

    // A second rotation on the same date gets the next sequence number.
    fs::remove_file(dir.join("app.log"))?;
    clock.set(date!(2026 - 09 - 29));
    {
        let mut writer = RollingGzipWriter::open_with_clock(&dir.0, "app", 14, clock.source())?;
        writer.write_all(b"again\n")?;
        clock.set(date!(2026 - 09 - 30));
        writer.write_all(b"later\n")?;
    }
    let second = dir.join("app.2026-09-29.1.log.gz");
    assert!(eventually(FINISH, || second.exists()), "{:?}", dir.names()?);
    assert_eq!(gunzip(&second)?, "again\n");
    Ok(())
}

#[test]
fn startup_archive_date() -> TestResult {
    let dir = TempDir::new("startup")?;
    let old = File::create(dir.join("app.log"))?;
    (&old).write_all(b"left over\n")?;
    old.set_modified(SystemTime::from(datetime!(2026-01-15 23:30 UTC)))?;
    drop(old);

    let clock = Clock::new(date!(2026 - 09 - 30));
    let _writer = RollingGzipWriter::open_with_clock(&dir.0, "app", 14, clock.source())?;
    let archive = dir.join("app.2026-01-15.log.gz");
    assert!(
        eventually(FINISH, || archive.exists()),
        "{:?}",
        dir.names()?
    );
    assert_eq!(gunzip(&archive)?, "left over\n");
    assert_eq!(fs::read_to_string(dir.join("app.log"))?, "");
    Ok(())
}

#[test]
fn a_restart_on_the_same_day_appends() -> TestResult {
    let dir = TempDir::new("restart")?;
    fs::write(dir.join("app.log"), "before the restart\n")?;
    let clock = Clock::new(date!(2026 - 09 - 30));
    for _ in 0..3 {
        // Written earlier on the clock's date; the real time of the last write is today.
        let file = File::options().append(true).open(dir.join("app.log"))?;
        file.set_modified(SystemTime::from(datetime!(2026-09-30 08:00 UTC)))?;
        drop(file);
        let mut writer = RollingGzipWriter::open_with_clock(&dir.0, "app", 14, clock.source())?;
        writer.write_all(b"after a restart\n")?;
    }
    // No archive, so no restart pushes an older day out of the 14 kept.
    assert_eq!(dir.names()?, ["app.log"]);
    assert_eq!(
        fs::read_to_string(dir.join("app.log"))?,
        "before the restart\nafter a restart\nafter a restart\nafter a restart\n"
    );
    Ok(())
}

#[test]
fn unfinished_compressions_are_cleared_and_done_again() -> TestResult {
    let dir = TempDir::new("unfinished")?;
    // One whose archive is still there, and one whose archive is gone.
    fs::write(dir.join("app.2026-01-15.log"), "left over\n")?;
    fs::write(dir.join("app.2026-01-15.log.gz.tmp"), "half")?;
    fs::write(dir.join("app.2026-01-14.log.gz.tmp"), "half")?;
    let clock = Clock::new(date!(2026 - 09 - 30));
    let _writer = RollingGzipWriter::open_with_clock(&dir.0, "app", 14, clock.source())?;
    let done = || {
        dir.names()
            .is_ok_and(|names| names == ["app.2026-01-15.log.gz", "app.log"])
    };
    assert!(eventually(FINISH, done), "{:?}", dir.names()?);
    assert_eq!(gunzip(&dir.join("app.2026-01-15.log.gz"))?, "left over\n");
    Ok(())
}

#[test]
fn a_deleted_directory_is_created_again_within_a_second() -> TestResult {
    let dir = TempDir::new("deleted")?;
    let logs = dir.join("logs");
    let clock = Clock::new(date!(2026 - 09 - 30));
    let mut writer = RollingGzipWriter::open_with_clock(&logs, "app", 14, clock.source())?;
    writer.write_all(b"before\n")?;
    writer.flush()?;
    fs::remove_dir_all(&logs)?;
    thread::sleep(Duration::from_millis(1100));
    writer.write_all(b"after\n")?;
    writer.flush()?;
    assert_eq!(fs::read_to_string(logs.join("app.log"))?, "after\n");
    assert_eq!(writer.failures(), 0);
    Ok(())
}

#[test]
fn failures_are_counted_where_the_writer_cannot_follow() -> TestResult {
    let dir = TempDir::new("blocked")?;
    let logs = dir.join("logs");
    let clock = Clock::new(date!(2026 - 09 - 30));
    let mut writer = RollingGzipWriter::open_with_clock(&logs, "app", 14, clock.source())?;
    let failures = writer.failure_count();
    // The directory becomes a file, so it cannot be created again.
    fs::remove_dir_all(&logs)?;
    fs::write(&logs, "not a directory")?;
    thread::sleep(Duration::from_millis(1100));
    writer.write_all(b"lost\n")?;
    assert!(failures.count() >= 1, "{}", failures.count());
    assert_eq!(writer.failures(), failures.count());
    Ok(())
}

#[test]
fn retention() -> TestResult {
    let dir = TempDir::new("retention")?;
    for day in 1..=20 {
        fs::write(dir.join(&format!("app.2026-01-{day:02}.log.gz")), "")?;
    }
    let clock = Clock::new(date!(2026 - 09 - 30));
    let _writer = RollingGzipWriter::open_with_clock(&dir.0, "app", 14, clock.source())?;
    let archives = || {
        dir.names()
            .map(|names| {
                names
                    .into_iter()
                    .filter(|name| name.ends_with(".log.gz"))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    };
    assert!(
        eventually(FINISH, || archives().len() == 14),
        "{:?}",
        archives()
    );
    let expected: Vec<String> = (7..=20)
        .map(|day| format!("app.2026-01-{day:02}.log.gz"))
        .collect();
    assert_eq!(archives(), expected);
    Ok(())
}

#[test]
fn compression_off_thread() -> TestResult {
    let dir = TempDir::new("compression")?;
    let clock = Clock::new(date!(2026 - 09 - 29));
    let mut writer = RollingGzipWriter::open_with_clock(&dir.0, "app", 14, clock.source())?;
    let line = b"2026-09-29T12:00:00.000000Z  INFO svc_api::access: request finished status=200\n";
    let mut written = 0;
    while written < 8 * 1024 * 1024 {
        writer.write_all(line)?;
        written += line.len();
    }
    clock.set(date!(2026 - 09 - 30));
    writer.write_all(b"rotated\n")?;

    // While the 8 MiB archive is being compressed, writing goes on at full speed.
    let start = Instant::now();
    for _ in 0..1000 {
        writer.write_all(b"written during compression\n")?;
    }
    let writing = start.elapsed();
    assert!(
        writing < Duration::from_secs(2),
        "1000 lines took {writing:?}"
    );

    let archive = dir.join("app.2026-09-29.log.gz");
    let pending = dir.join("app.2026-09-29.log");
    assert!(
        eventually(FINISH, || archive.exists() && !pending.exists()),
        "{:?}",
        dir.names()?
    );
    assert_eq!(writer.failures(), 0);
    Ok(())
}
