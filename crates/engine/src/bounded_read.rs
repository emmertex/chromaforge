//! Reading a whole file the user or a preset named, with a size cap that holds (issue #2570).
//!
//! `metadata().len()` then `fs::read` is not a cap: a FIFO, a device or a `/proc` file reports
//! 0 bytes, so opening a FIFO hangs the caller and reading `/dev/zero` grows until the process
//! aborts. [`read_bounded`] only opens regular files (a link to one is fine) and reads at most
//! `max + 1` bytes, so a file that grew after the check is refused too.
//!
//! Adapted from upstream #3470 (`crates/engine/src/bounded_read.rs`).

use std::io::{Read, Write};
use std::path::Path;

use crate::io_sched::job::{ByteKind, Side};

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
    read_bounded_as(path, max, ByteKind::Read)
}

/// [`read_bounded`], its bytes counted for the thread's I/O job as `kind` (a file on a copy's
/// destination is read back: [`ByteKind::ReadBack`]).
pub(crate) fn read_bounded_as(path: &Path, max: u64, kind: ByteKind) -> Result<Vec<u8>, ReadError> {
    on(kind.side());
    let meta = std::fs::metadata(path).map_err(ReadError::Io)?;
    if !meta.is_file() {
        return Err(ReadError::NotAFile);
    }
    if meta.len() > max {
        return Err(ReadError::TooLarge(meta.len()));
    }
    let mut file = counting_as(kind, std::fs::File::open(path).map_err(ReadError::Io)?);
    let mut out = Vec::new();
    // (the counting reader hides the file's own size hint from `read_to_end`: reserve it here, best effort)
    let hint = usize::try_from(meta.len().min(max.saturating_add(1))).unwrap_or(0);
    let _ = out.try_reserve_exact(hint);
    file.read_to_end_at_most(&mut out, max.saturating_add(1)).map_err(ReadError::Io)?;
    if out.len() as u64 > max {
        return Err(ReadError::TooLarge(out.len() as u64));
    }
    Ok(out)
}

/// Most bytes one `read` of a [`CountingReader`] asks for, so a slow card can't sit inside one
/// huge read without its progress showing (§5.2 of the I/O spec).
pub(crate) const MAX_READ_CHUNK: usize = 4 << 20;

/// A reader that caps every `read` at [`MAX_READ_CHUNK`] and counts what it reads for the I/O
/// job the thread runs (its bytes by kind and last-progress time, which the scheduler's stall rule
/// reads); before each read it tells the job which of a copy's volumes the read is on.
pub(crate) struct CountingReader<R> {
    inner: R,
    kind: ByteKind,
    job: Option<std::sync::Arc<crate::io_sched::JobState>>,
}

/// `inner`, a file on the job's source, counted for the thread's current I/O job (none: just
/// capped).
pub(crate) fn counting<R>(inner: R) -> CountingReader<R> {
    counting_as(ByteKind::Read, inner)
}

/// [`counting`], its bytes counted as `kind` (a copy's destination read back:
/// [`ByteKind::ReadBack`]).
pub(crate) fn counting_as<R>(kind: ByteKind, inner: R) -> CountingReader<R> {
    CountingReader { inner, kind, job: crate::memory::current_job() }
}

/// A writer that counts what it writes for the thread's I/O job as [`ByteKind::Written`] (on the
/// destination), and tells the job before each write that it is on the destination.
pub(crate) struct CountingWriter<W> {
    inner: W,
    job: Option<std::sync::Arc<crate::io_sched::JobState>>,
}

/// `inner`, a file on the job's destination, counted for the thread's current I/O job.
pub(crate) fn counting_writer<W>(inner: W) -> CountingWriter<W> {
    CountingWriter { inner, job: crate::memory::current_job() }
}

impl<W: Write> Write for CountingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if let Some(job) = &self.job {
            job.set_on(Side::Dest);
        }
        let n = self.inner.write(buf)?;
        if n > 0
            && let Some(job) = &self.job
        {
            job.add(ByteKind::Written, n);
        }
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if let Some(job) = &self.job {
            job.set_on(Side::Dest);
        }
        self.inner.flush()
    }
}

/// `n` bytes of `kind` moved by the thread's current I/O job where no counting reader or writer
/// sees them: counted as progress on their side's volume.
pub(crate) fn count_bytes(kind: ByteKind, n: u64) {
    if n > 0
        && let Some(job) = crate::memory::current_job()
    {
        job.add(kind, usize::try_from(n).unwrap_or(usize::MAX));
    }
}

/// The thread's current I/O job's next call is on `side` (a copy's open, metadata call or flush:
/// I/O that moves no counted byte, which can hang all the same). The scheduler then judges a
/// copy that stops there on that volume only (§5.2).
pub(crate) fn on(side: Side) {
    if let Some(job) = crate::memory::current_job() {
        job.set_on(side);
    }
}

impl<R> CountingReader<R> {
    fn count(&self, n: usize) {
        if n > 0
            && let Some(job) = &self.job
        {
            job.add(self.kind, n);
        }
    }

    /// Tell the job its next read is on this reader's side.
    fn mark(&self) {
        if let Some(job) = &self.job {
            job.set_on(self.kind.side());
        }
    }
}

impl<R: Read> CountingReader<R> {
    /// Append at most `limit` bytes to `out`, to the end of the input or the limit, in steps of at
    /// most [`MAX_READ_CHUNK`] counted one by one. Each step is the inner reader's own
    /// `read_to_end`, so a `File` reads straight into `out`'s spare capacity: the default one,
    /// over [`Read::read`], zero-fills that capacity first (one more pass over every byte).
    pub(crate) fn read_to_end_at_most(&mut self, out: &mut Vec<u8>, limit: u64) -> std::io::Result<usize> {
        let mut total = 0u64;
        while total < limit {
            let want = (MAX_READ_CHUNK as u64).min(limit - total);
            self.mark();
            let before = out.len();
            let step = (&mut self.inner).take(want).read_to_end(out);
            // (what a failing step appended was read too)
            let n = out.len().saturating_sub(before);
            self.count(n);
            step?;
            if n == 0 {
                break;
            }
            total = total.saturating_add(n as u64);
        }
        Ok(usize::try_from(total).unwrap_or(usize::MAX))
    }
}

impl<R: Read> Read for CountingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let cap = buf.len().min(MAX_READ_CHUNK);
        self.mark();
        let n = self.inner.read(buf.get_mut(..cap).unwrap_or_default())?;
        self.count(n);
        Ok(n)
    }

    fn read_to_end(&mut self, buf: &mut Vec<u8>) -> std::io::Result<usize> {
        self.read_to_end_at_most(buf, u64::MAX)
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

    /// A reader that records the size of every read asked of it.
    struct Sizes<'a> {
        left: usize,
        asked: &'a std::sync::Mutex<Vec<usize>>,
    }

    impl Read for Sizes<'_> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.asked.lock().unwrap().push(buf.len());
            let n = buf.len().min(self.left);
            buf.get_mut(..n).unwrap().fill(9);
            self.left -= n;
            Ok(n)
        }
    }

    /// Reading to the end through the counting reader counts every byte for the thread's job, no
    /// read asks for more than one chunk, and the limit holds; a whole file read by `read_bounded`
    /// is counted too (in more than one step).
    #[test]
    fn read_to_end_counts_every_byte_in_chunks() {
        let clock: std::sync::Arc<dyn crate::io_sched::clock::Clock> = std::sync::Arc::new(crate::io_sched::clock::FakeClock::default());
        let key = lcc_catalog::folders::volume_key("/a", &lcc_catalog::folders::Mounts::none());
        let job = std::sync::Arc::new(crate::io_sched::JobState::new(1, key, clock, std::sync::Weak::new()));
        let size = 2 * MAX_READ_CHUNK + 3;
        let asked = std::sync::Mutex::new(Vec::new());
        let mut out = Vec::new();
        crate::memory::with_job(Some(job.clone()), || counting(Sizes { left: size, asked: &asked }).read_to_end(&mut out)).unwrap();
        assert_eq!(out.len(), size);
        assert!(out.iter().all(|&b| b == 9));
        assert_eq!(job.bytes(), size as u64);
        assert!(asked.lock().unwrap().iter().all(|&n| n <= MAX_READ_CHUNK), "{:?}", asked.lock().unwrap());
        // the limit, mid-chunk
        let mut out = Vec::new();
        let n = counting(Sizes { left: size, asked: &asked }).read_to_end_at_most(&mut out, MAX_READ_CHUNK as u64 + 5).unwrap();
        assert_eq!((n, out.len()), (MAX_READ_CHUNK + 5, MAX_READ_CHUNK + 5));

        let dir = dir("counted");
        let path = dir.join("a.bin");
        std::fs::write(&path, vec![3u8; size]).unwrap();
        let before = job.bytes();
        let got = crate::memory::with_job(Some(job.clone()), || read_bounded(&path, size as u64)).unwrap();
        assert_eq!(got.len(), size);
        assert_eq!(job.bytes() - before, size as u64);
        assert!(matches!(read_bounded(&path, size as u64 - 1), Err(ReadError::TooLarge(_))));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A character device reports 0 bytes but never ends: it must be refused, not read to the end.
    #[cfg(unix)]
    #[test]
    fn a_device_is_refused() {
        assert!(matches!(read_bounded(Path::new("/dev/zero"), 1024), Err(ReadError::NotAFile)));
    }
}
