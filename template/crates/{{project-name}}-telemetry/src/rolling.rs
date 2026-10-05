//! The log file `<dir>/<base>.log`. When the UTC date changes it is renamed to
//! `<base>.<YYYY-MM-DD>.log` (`<base>.<YYYY-MM-DD>.<N>.log` if that name is taken), a
//! thread of its own compresses the archive into `.log.gz`, and only the newest
//! `max_archives` compressed archives are kept. A restart on the day the file was last
//! written appends to it, so restarts do not push older days out of the archives. A file
//! or directory deleted while the service runs is created again within a second. The
//! writer belongs to the log writer thread, so it needs no lock; it buffers, and the
//! thread flushes after each batch of lines. It closes the file before renaming it, which
//! Windows requires of an open file.

mod archive;

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::thread;
use std::time::{Duration, Instant};

use svc_util::prelude::*;
use time::{Date, OffsetDateTime};

use crate::error::LOG_OUTPUT_UNAVAILABLE;
use archive::{Archiver, archive_path};

/// How many archives may wait for compression; more wait for the next start.
const QUEUE: usize = 4;

/// How often the writer checks that the file it writes is still in the directory.
const RECHECK: Duration = Duration::from_secs(1);

/// The rotating, compressing writer of the log file.
pub struct RollingGzipWriter {
    dir: PathBuf,
    base: String,
    current: PathBuf,
    /// The open file; `None` while it is closed for a rotation or could not be reopened.
    file: Option<BufWriter<File>>,
    file_date: Date,
    checked: Instant,
    today: Box<dyn Fn() -> Date + Send>,
    compressor: SyncSender<PathBuf>,
    failures: Failures,
}

/// How many writes, rotations or re-creations of the log file have failed; shared, so the
/// count can be read after the writer has moved to its thread.
#[derive(Clone, Debug, Default)]
pub struct Failures(Arc<AtomicU64>);

impl Failures {
    /// The count so far.
    #[must_use]
    pub fn count(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
}

impl RollingGzipWriter {
    /// Opens `<dir>/<base>.log`, creating the directory. A non-empty file left by an
    /// earlier run is appended to when it was last written today (UTC), and otherwise
    /// archived under that date; archives left uncompressed are compressed.
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
        // A file whose time cannot be read counts as written today, and is appended to.
        let last_written = written.and_then(|meta| meta.modified().ok());
        let date = last_written.map(|modified| OffsetDateTime::from(modified).date());
        if let Some(date) = date.filter(|date| *date != today()) {
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
            file: Some(BufWriter::new(file)),
            file_date: today(),
            checked: Instant::now(),
            today: Box::new(today),
            compressor,
            failures: Failures::default(),
        })
    }

    /// How many writes, rotations or re-creations have failed; only the first failure is
    /// printed.
    #[must_use]
    pub fn failures(&self) -> u64 {
        self.failures.count()
    }

    /// The failure count, to be read after the writer has moved to its thread.
    #[must_use]
    pub fn failure_count(&self) -> Failures {
        self.failures.clone()
    }

    /// The path of the file being written.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.current
    }

    fn rotate(&mut self, today: Date) -> io::Result<()> {
        let date = self.file_date;
        self.file_date = today;
        // Closed first: Windows does not rename a file that is open.
        if let Some(mut file) = self.file.take() {
            file.flush()?;
        }
        let archive = archive_path(&self.dir, &self.base, date);
        let renamed = match fs::rename(&self.current, &archive) {
            // A full queue leaves the archive for the next start to compress.
            Ok(()) => {
                self.compressor.try_send(archive).ok();
                Ok(())
            }
            // The file or its directory was deleted: there is nothing to archive.
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        };
        // Reopened whatever happened, so that the next lines are written somewhere.
        self.reopen()?;
        renamed
    }

    /// Creates the directory and the file again when either was deleted.
    fn recheck(&mut self) -> io::Result<()> {
        self.checked = Instant::now();
        if self.current.exists() {
            return Ok(());
        }
        // What the buffer still holds belonged to the deleted file.
        if let Some(file) = self.file.as_mut() {
            file.flush().ok();
        }
        self.reopen()?;
        let file = self.current.display();
        warn(&format!(
            "the log file {file} was deleted; it is written again"
        ));
        Ok(())
    }

    fn reopen(&mut self) -> io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        self.file = Some(BufWriter::new(open_append(&self.current)?));
        Ok(())
    }

    fn fail(&mut self, error: &io::Error) {
        if self.failures.0.fetch_add(1, Ordering::Relaxed) == 0 {
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
        if today != self.file_date {
            if let Err(error) = self.rotate(today) {
                self.fail(&error);
            }
        } else if self.checked.elapsed() >= RECHECK
            && let Err(error) = self.recheck()
        {
            self.fail(&error);
        }
        if self.file.is_none()
            && let Err(error) = self.reopen()
        {
            self.fail(&error);
        }
        if let Some(Err(error)) = self.file.as_mut().map(|file| file.write_all(buf)) {
            self.fail(&error);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if let Some(Err(error)) = self.file.as_mut().map(Write::flush) {
            self.fail(&error);
        }
        Ok(())
    }
}

/// Problems of the log file go to stderr, since the log itself is what fails. A closed
/// stderr is ignored.
pub(crate) fn warn(message: &str) {
    writeln!(io::stderr().lock(), "{message}").ok();
}

fn open_append(path: &Path) -> io::Result<File> {
    OpenOptions::new().create(true).append(true).open(path)
}
