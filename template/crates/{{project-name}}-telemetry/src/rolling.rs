//! The log file `<dir>/<base>.log`. When the UTC date changes it is renamed to
//! `<base>.<YYYY-MM-DD>.log` (`<base>.<YYYY-MM-DD>.<N>.log` if that name is taken), a
//! thread of its own compresses the archive into `.log.gz`, and only the newest
//! `max_archives` compressed archives are kept. The writer belongs to the log writer
//! thread, so it needs no lock.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread;

use flate2::Compression;
use flate2::write::GzEncoder;
use svc_util::prelude::*;
use time::{Date, OffsetDateTime};

use crate::error::LOG_OUTPUT_UNAVAILABLE;

/// How many archives may wait for compression; more wait for the next start.
const QUEUE: usize = 4;

/// The rotating, compressing writer of the log file.
pub struct RollingGzipWriter {
    dir: PathBuf,
    base: String,
    current: PathBuf,
    file: File,
    file_date: Date,
    today: Box<dyn Fn() -> Date + Send>,
    compressor: SyncSender<PathBuf>,
    failures: u64,
}

impl RollingGzipWriter {
    /// Opens `<dir>/<base>.log`, creating the directory. A non-empty file left by an
    /// earlier run is archived under the UTC date it was last written, and archives left
    /// uncompressed are compressed.
    ///
    /// # Errors
    ///
    /// [`LOG_OUTPUT_UNAVAILABLE`] when the directory or the file cannot be created, or the
    /// compression thread cannot start.
    pub fn open(dir: &Path, base: &str, max_archives: usize) -> Result<Self> {
        Self::open_with_clock(dir, base, max_archives, || OffsetDateTime::now_utc().date())
    }

    /// Like [`RollingGzipWriter::open`], with the current date taken from `today`.
    ///
    /// # Errors
    ///
    /// As for [`RollingGzipWriter::open`].
    pub fn open_with_clock(
        dir: &Path,
        base: &str,
        max_archives: usize,
        today: impl Fn() -> Date + Send + 'static,
    ) -> Result<Self> {
        fs::create_dir_all(dir).or_err_with(LOG_OUTPUT_UNAVAILABLE, || {
            format!("cannot create the log directory {}", dir.display())
        })?;
        let current = dir.join(format!("{base}.log"));
        let written = fs::metadata(&current).ok().filter(|meta| meta.len() > 0);
        if let Some(meta) = written {
            let date = meta.modified().map_or_else(
                |_| today(),
                |modified| OffsetDateTime::from(modified).date(),
            );
            fs::rename(&current, archive_path(dir, base, date))
                .or_err_with(LOG_OUTPUT_UNAVAILABLE, || {
                    format!("cannot archive the log file {}", current.display())
                })?;
        }
        let file = open_append(&current).or_err_with(LOG_OUTPUT_UNAVAILABLE, || {
            format!("cannot open the log file {}", current.display())
        })?;
        let (compressor, queue) = mpsc::sync_channel(QUEUE);
        let archiver = Archiver {
            dir: dir.to_path_buf(),
            base: base.to_string(),
            max_archives,
        };
        thread::Builder::new()
            .name("log-compressor".to_string())
            .spawn(move || archiver.run(&queue))
            .or_err(
                LOG_OUTPUT_UNAVAILABLE,
                "cannot start the log compression thread",
            )?;
        Ok(RollingGzipWriter {
            dir: dir.to_path_buf(),
            base: base.to_string(),
            current,
            file,
            file_date: today(),
            today: Box::new(today),
            compressor,
            failures: 0,
        })
    }

    /// How many writes or rotations have failed; only the first failure is printed.
    #[must_use]
    pub fn failures(&self) -> u64 {
        self.failures
    }

    fn rotate(&mut self, today: Date) -> io::Result<()> {
        let date = self.file_date;
        self.file_date = today;
        self.file.flush()?;
        let archive = archive_path(&self.dir, &self.base, date);
        fs::rename(&self.current, &archive)?;
        self.file = open_append(&self.current)?;
        // A full queue leaves the archive for the next start to compress.
        self.compressor.try_send(archive).ok();
        Ok(())
    }

    fn fail(&mut self, error: &io::Error) {
        self.failures += 1;
        if self.failures == 1 {
            let file = self.current.display();
            let later = "later failures are counted, not printed";
            warn(&format!(
                "cannot write the log file {file}: {error} ({later})"
            ));
        }
    }
}

impl Write for RollingGzipWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let today = (self.today)();
        if today != self.file_date
            && let Err(error) = self.rotate(today)
        {
            self.fail(&error);
        }
        if let Err(error) = self.file.write_all(buf) {
            self.fail(&error);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if let Err(error) = self.file.flush() {
            self.fail(&error);
        }
        Ok(())
    }
}

/// Problems of the log file go to stderr, since the log itself is what fails. A closed
/// stderr is ignored.
fn warn(message: &str) {
    writeln!(io::stderr().lock(), "{message}").ok();
}

fn open_append(path: &Path) -> io::Result<File> {
    OpenOptions::new().create(true).append(true).open(path)
}

/// `<base>.<date>.log`, or `<base>.<date>.<N>.log` with the first free `N` from 1 when
/// that archive, compressed or not, already exists.
fn archive_path(dir: &Path, base: &str, date: Date) -> PathBuf {
    let taken = |name: &str| dir.join(name).exists() || dir.join(format!("{name}.gz")).exists();
    let mut name = format!("{base}.{date}.log");
    let mut n = 0_u64;
    while taken(&name) {
        n += 1;
        name = format!("{base}.{date}.{n}.log");
    }
    dir.join(name)
}

/// The date and sequence number of an archive name, `<base>.<date>[.<N>].<suffix>`.
fn archive_key(name: &str, base: &str, suffix: &str) -> Option<(String, u64)> {
    let middle = name
        .strip_prefix(base)?
        .strip_prefix('.')?
        .strip_suffix(suffix)?;
    let (date, n) = match middle.split_once('.') {
        Some((date, n)) => (date, n.parse().ok()?),
        None => (middle, 0),
    };
    let is_date = date.len() == 10
        && date.char_indices().all(|(i, c)| {
            if i == 4 || i == 7 {
                c == '-'
            } else {
                c.is_ascii_digit()
            }
        });
    is_date.then(|| (date.to_string(), n))
}

/// The compression thread's view of the log directory.
struct Archiver {
    dir: PathBuf,
    base: String,
    max_archives: usize,
}

impl Archiver {
    /// Compresses what earlier runs left, then every archive the writer sends, pruning
    /// after each one.
    fn run(&self, queue: &Receiver<PathBuf>) {
        for archive in self.named(".log") {
            compress(&archive);
        }
        self.prune();
        for archive in queue {
            compress(&archive);
            self.prune();
        }
    }

    /// Archives with the given suffix, oldest first.
    fn named(&self, suffix: &str) -> Vec<PathBuf> {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut found: Vec<((String, u64), PathBuf)> = entries
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter_map(|path| {
                let name = path.file_name()?.to_str()?;
                Some((archive_key(name, &self.base, suffix)?, path))
            })
            .collect();
        found.sort();
        found.into_iter().map(|(_, path)| path).collect()
    }

    /// Deletes the compressed archives beyond the newest `max_archives`.
    fn prune(&self) {
        let archives = self.named(".log.gz");
        let excess = archives.len().saturating_sub(self.max_archives);
        for archive in archives.into_iter().take(excess) {
            if let Err(error) = fs::remove_file(&archive) {
                warn(&format!(
                    "cannot delete the log archive {}: {error}",
                    archive.display()
                ));
            }
        }
    }
}

/// `<archive>` becomes `<archive>.gz` through a temporary file; the original goes once the
/// compressed file is in place.
fn compress(archive: &Path) {
    let with_suffix = |suffix: &str| {
        let mut name = archive.as_os_str().to_owned();
        name.push(suffix);
        PathBuf::from(name)
    };
    let (gz, tmp) = (with_suffix(".gz"), with_suffix(".gz.tmp"));
    if let Err(error) = gzip(archive, &tmp)
        .and_then(|()| fs::rename(&tmp, &gz))
        .and_then(|()| fs::remove_file(archive))
    {
        warn(&format!(
            "cannot compress the log archive {}: {error}",
            archive.display()
        ));
    }
}

fn gzip(input: &Path, output: &Path) -> io::Result<()> {
    let mut input = File::open(input)?;
    let mut encoder = GzEncoder::new(
        BufWriter::new(File::create(output)?),
        Compression::default(),
    );
    io::copy(&mut input, &mut encoder)?;
    encoder.finish()?.flush()
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;
    use std::fs;

    use time::macros::date;

    use super::{archive_key, archive_path};

    #[test]
    fn archive_names_carry_the_date_and_a_sequence_number() -> Result<(), Box<dyn StdError>> {
        let dir = std::env::temp_dir().join(format!("svc-telemetry-names-{}", std::process::id()));
        fs::create_dir_all(&dir)?;
        let day = date!(2026 - 09 - 30);
        assert_eq!(
            archive_path(&dir, "app", day),
            dir.join("app.2026-09-30.log")
        );
        fs::write(dir.join("app.2026-09-30.log.gz"), "")?;
        assert_eq!(
            archive_path(&dir, "app", day),
            dir.join("app.2026-09-30.1.log")
        );
        fs::write(dir.join("app.2026-09-30.1.log"), "")?;
        assert_eq!(
            archive_path(&dir, "app", day),
            dir.join("app.2026-09-30.2.log")
        );
        fs::remove_dir_all(&dir)?;
        Ok(())
    }

    #[test]
    fn archive_keys_sort_by_date_then_sequence() {
        assert_eq!(
            archive_key("app.2026-09-30.log.gz", "app", ".log.gz"),
            Some(("2026-09-30".to_string(), 0))
        );
        assert_eq!(
            archive_key("app.2026-09-30.2.log.gz", "app", ".log.gz"),
            Some(("2026-09-30".to_string(), 2))
        );
        assert_eq!(archive_key("app.2026-09-30.log", "app", ".log.gz"), None);
        assert_eq!(archive_key("app.log", "app", ".log"), None);
        assert_eq!(
            archive_key("other.2026-09-30.log.gz", "app", ".log.gz"),
            None
        );
        assert_eq!(archive_key("app.2026-9-30.log.gz", "app", ".log.gz"), None);
        assert!(
            archive_key("app.2026-09-30.log.gz", "app", ".log.gz")
                < archive_key("app.2026-09-30.1.log.gz", "app", ".log.gz")
        );
    }
}
