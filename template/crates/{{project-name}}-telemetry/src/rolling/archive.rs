//! Archive names, and the thread that compresses archives and prunes the old ones.

use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;

use flate2::Compression;
use flate2::write::GzEncoder;
use time::Date;

use super::warn;

/// `<base>.<date>.log`, or `<base>.<date>.<N>.log` with the first free `N` from 1 when
/// that archive, compressed or not, already exists.
pub(super) fn archive_path(dir: &Path, base: &str, date: Date) -> PathBuf {
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
pub(super) struct Archiver {
    pub(super) dir: PathBuf,
    pub(super) base: String,
    pub(super) max_archives: usize,
}

impl Archiver {
    /// Compresses what earlier runs left, then every archive the writer sends, pruning
    /// after each one. A temporary file of a compression that an earlier run did not finish
    /// is deleted first; its archive is still there and is compressed again.
    pub(super) fn run(&self, queue: &Receiver<PathBuf>) {
        for unfinished in self.named(".log.gz.tmp") {
            if let Err(error) = fs::remove_file(&unfinished) {
                let file = unfinished.display();
                warn(&format!(
                    "cannot delete the unfinished archive {file}: {error}"
                ));
            }
        }
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
