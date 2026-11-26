//! Reading a whole file the user or a preset named, with a size cap that holds (issue #2570).
//!
//! `metadata().len()` then `fs::read` is not a cap: a FIFO, a device or a `/proc` file reports
//! 0 bytes, so opening a FIFO hangs the caller and reading `/dev/zero` grows until the process
//! aborts. [`read_bounded`] only opens regular files (a link to one is fine) and reads at most
//! `max + 1` bytes, so a file that grew after the check is refused too.
//!
//! Adapted from upstream #3470 (`crates/engine/src/bounded_read.rs`).

use std::io::Read;
use std::path::Path;

/// Most bytes of a photo, video or other media file read into memory (upstream #3703).
pub const MAX_MEDIA_BYTES: u64 = 8 << 30;

/// Why [`read_bounded`] refused a file.
#[derive(Debug)]
pub enum ReadError {
    /// The file can't be found, opened or read.
    Io(std::io::Error),
    /// A folder, FIFO, device or socket, not a regular file.
    NotAFile,
    /// Larger than the cap; the size is the length the file reports.
    TooLarge(u64),
}

impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReadError::Io(e) => write!(f, "{e}"),
            ReadError::NotAFile => f.write_str("not a regular file"),
            ReadError::TooLarge(n) => write!(f, "{n} bytes is too large"),
        }
    }
}

/// The bytes of the regular file at `path`, at most `max` of them.
pub fn read_bounded(path: &Path, max: u64) -> Result<Vec<u8>, ReadError> {
    let meta = std::fs::metadata(path).map_err(ReadError::Io)?;
    if !meta.is_file() {
        return Err(ReadError::NotAFile);
    }
    if meta.len() > max {
        return Err(ReadError::TooLarge(meta.len()));
    }
    let file = counting(std::fs::File::open(path).map_err(ReadError::Io)?);
    let mut out = Vec::new();
    // (the counting reader hides the file's own size hint from `read_to_end`: reserve it here, best effort)
    let hint = usize::try_from(meta.len().min(max.saturating_add(1))).unwrap_or(0);
    let _ = out.try_reserve_exact(hint);
    file.take(max.saturating_add(1)).read_to_end(&mut out).map_err(ReadError::Io)?;
    if out.len() as u64 > max {
        return Err(ReadError::TooLarge(out.len() as u64));
    }
    Ok(out)
}

/// Most bytes one `read` of a [`CountingReader`] asks for, so a slow card can't sit inside one
/// huge read without its progress showing (§5.2 of the I/O spec).
pub(crate) const MAX_READ_CHUNK: usize = 4 << 20;

/// A reader that caps every `read` at [`MAX_READ_CHUNK`] and counts what it reads for the I/O
/// job the thread runs (its bytes and last-progress time, which the scheduler's stall rule reads).
pub(crate) struct CountingReader<R> {
    inner: R,
    job: Option<std::sync::Arc<crate::io_sched::JobState>>,
}

/// `inner`, counted for the thread's current I/O job (none: just capped).
pub(crate) fn counting<R>(inner: R) -> CountingReader<R> {
    CountingReader { inner, job: crate::memory::current_job() }
}

impl<R: Read> Read for CountingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let cap = buf.len().min(MAX_READ_CHUNK);
        let n = self.inner.read(buf.get_mut(..cap).unwrap_or_default())?;
        if n > 0
            && let Some(job) = &self.job
        {
            job.add_bytes(n);
        }
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("lcc-bounded-read-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn reads_a_small_file_and_refuses_a_large_one() {
        let dir = dir("small");
        let path = dir.join("a.bin");
        std::fs::write(&path, [7u8; 10]).unwrap();
        assert_eq!(read_bounded(&path, 10).unwrap(), vec![7u8; 10]);
        assert!(matches!(read_bounded(&path, 9), Err(ReadError::TooLarge(10))));
        assert!(matches!(read_bounded(&dir, 10), Err(ReadError::NotAFile)));
        assert!(matches!(read_bounded(&dir.join("missing"), 10), Err(ReadError::Io(_))));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A FIFO reports 0 bytes and opening it waits for a writer: it must be refused, not opened.
    #[cfg(unix)]
    #[test]
    fn a_fifo_is_refused_without_waiting() {
        let dir = dir("fifo");
        let fifo = dir.join("preset.xmp");
        let made = std::process::Command::new("mkfifo").arg(&fifo).status().unwrap();
        assert!(made.success());
        let (tx, rx) = std::sync::mpsc::channel();
        let path = fifo.clone();
        std::thread::spawn(move || {
            let _ = tx.send(read_bounded(&path, 1024).map(|b| b.len()));
        });
        let got = rx.recv_timeout(std::time::Duration::from_secs(5)).expect("reading a FIFO must not hang");
        assert!(matches!(got, Err(ReadError::NotAFile)), "{got:?}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A character device reports 0 bytes but never ends: it must be refused, not read to the end.
    #[cfg(unix)]
    #[test]
    fn a_device_is_refused() {
        assert!(matches!(read_bounded(Path::new("/dev/zero"), 1024), Err(ReadError::NotAFile)));
    }
}
