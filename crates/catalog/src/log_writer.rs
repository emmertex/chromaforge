//! The thread that writes the catalog log for [`crate::store::FsStore`] (natively).
//!
//! An append hands its bytes over and returns; the thread writes them in order and makes each batch
//! durable (`fsync`) before the next. The caller (the UI thread, for the app) never waits for the
//! drive: on Windows a write waits for a sync of the same file in flight, and on a busy disk every
//! flag or step paid it (OneSlips' test note 2 of 2026-10-25: the window froze 1.4 s).
//!
//! What is durable is published as [`crate::store::LogProgress::durable_len`], the length of the log that is
//! written whole and synced: the journal counts an append as acknowledged only once it lies within
//! it ([`crate::journal::Journal::durable_seq`]). A write or sync that fails is published as
//! [`crate::store::LogProgress::failed`]; appends handed over after it are left out until the log is cut back or
//! replaced, so the file never holds a gap. Cuts, replacements and flushes run after everything
//! handed over before them; the caller waits for them. Dropping the writer writes and syncs
//! everything handed over (shutdown flushes).

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use crate::safe_file::sync_file;
use crate::store::LogProgress as Progress;

/// How long a caller waits for the thread at most (a cut, a replace, a flush).
const WAIT: Duration = Duration::from_secs(120);

enum Msg {
    Append(Vec<u8>),
    Truncate(u64, Sender<io::Result<()>>),
    Replace(Vec<u8>, Sender<io::Result<()>>),
    /// Reply once everything before is written and synced.
    Flush(Sender<()>),
}

pub(crate) struct LogWriter {
    pub(crate) name: String,
    tx: Option<Sender<Msg>>,
    progress: Arc<Mutex<Progress>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

fn stopped() -> io::Error {
    io::Error::other("the catalog log writer stopped")
}

impl LogWriter {
    /// A writer for `dir/name`, whose first `len` bytes are durable already. `None` when no thread
    /// can start: the store then writes itself.
    pub(crate) fn start(dir: &Path, name: &str, len: u64) -> Option<LogWriter> {
        let (tx, rx) = channel::<Msg>();
        let progress = Arc::new(Mutex::new(Progress { durable_len: len, failed: None }));
        let st = progress.clone();
        let path = dir.join(name);
        let tmp = dir.join(format!("{name}.tmp"));
        let dir = dir.to_path_buf();
        let thread = std::thread::Builder::new().name("catalog-log".into()).spawn(move || run(&rx, &st, Paths { dir, path, tmp }, len)).ok()?;
        Some(LogWriter { name: name.to_string(), tx: Some(tx), progress, thread: Some(thread) })
    }

    fn send(&self, m: Msg) -> io::Result<()> {
        self.tx.as_ref().ok_or_else(stopped)?.send(m).map_err(|_| stopped())
    }

    /// Hand `data` over (written and synced right after, in order).
    pub(crate) fn append(&self, data: &[u8]) -> io::Result<()> {
        self.send(Msg::Append(data.to_vec()))
    }

    /// Cut the log to `len` bytes, after everything handed over; waits for it.
    pub(crate) fn truncate(&self, len: u64) -> io::Result<()> {
        let (tx, rx) = channel();
        self.send(Msg::Truncate(len, tx))?;
        rx.recv_timeout(WAIT).map_err(|_| stopped())?
    }

    /// Replace the log with `data` (atomically, durably), after everything handed over; waits.
    pub(crate) fn replace(&self, data: &[u8]) -> io::Result<()> {
        let (tx, rx) = channel();
        self.send(Msg::Replace(data.to_vec(), tx))?;
        rx.recv_timeout(WAIT).map_err(|_| stopped())?
    }

    /// Wait until everything handed over is written and synced (or has failed).
    pub(crate) fn flush(&self) -> io::Result<()> {
        let (tx, rx) = channel();
        self.send(Msg::Flush(tx))?;
        rx.recv_timeout(WAIT).map_err(|_| stopped())
    }

    pub(crate) fn progress(&self) -> Progress {
        self.progress.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

impl Drop for LogWriter {
    fn drop(&mut self) {
        // the thread works through everything handed over, syncs it, then ends
        drop(self.tx.take());
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Logs whose writes fail (tests: a full disk, a dropped share).
#[cfg(test)]
pub(crate) static FAIL_WRITES: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

#[cfg(test)]
fn write_fails(path: &Path) -> bool {
    FAIL_WRITES.lock().unwrap_or_else(PoisonError::into_inner).iter().any(|p| p == path)
}
#[cfg(not(test))]
fn write_fails(_: &Path) -> bool {
    false
}

struct Paths {
    dir: PathBuf,
    path: PathBuf,
    tmp: PathBuf,
}

/// The open log and how much of it was written (`written`) and synced (`synced`).
struct Log {
    file: Option<std::fs::File>,
    written: u64,
    synced: u64,
    failed: Option<String>,
}

impl Log {
    fn fail(&mut self, p: &Paths, why: String) {
        log::error!("catalog: {}: {why}", p.path.display());
        self.file = None;
        self.failed.get_or_insert(why);
    }

    fn append(&mut self, p: &Paths, data: &[u8]) {
        if self.failed.is_some() {
            return; // never a gap: left out until the log is cut back or replaced
        }
        if self.file.is_none() {
            match crate::safe_file::open_or_create_regular(&p.path, std::fs::OpenOptions::new().append(true)) {
                Ok((f, created)) => {
                    if created {
                        sync_dir(&p.dir);
                    }
                    self.file = Some(f);
                }
                Err(e) => return self.fail(p, e.to_string()),
            }
        }
        let Some(f) = self.file.as_mut() else { return };
        if write_fails(&p.path) {
            // (tests) half of it reaches the file, like a disk that filled up mid-write
            let _ = f.write_all(data.get(..data.len() / 2).unwrap_or_default());
            return self.fail(p, "no space left on the device (test)".into());
        }
        match f.write_all(data) {
            Ok(()) => self.written = self.written.saturating_add(data.len() as u64),
            Err(e) => self.fail(p, e.to_string()),
        }
    }

    fn sync(&mut self, p: &Paths) {
        if self.failed.is_some() || self.synced == self.written {
            return;
        }
        let Some(f) = self.file.as_ref() else { return };
        match sync_file(f, true) {
            Ok(()) => self.synced = self.written,
            Err(e) => self.fail(p, e.to_string()),
        }
    }

    fn truncate(&mut self, p: &Paths, len: u64) -> io::Result<()> {
        self.file = None;
        let cut = (|| {
            let Some(f) = crate::safe_file::open_regular(&p.path, std::fs::OpenOptions::new().write(true))? else { return Ok(0) };
            let now = f.metadata()?.len();
            let keep = len.min(now);
            f.set_len(keep)?;
            sync_file(&f, false)?;
            Ok(keep)
        })();
        cut.map(|keep| {
            self.written = keep;
            self.synced = keep;
            self.failed = None;
        })
    }

    fn replace(&mut self, p: &Paths, data: &[u8]) -> io::Result<()> {
        self.file = None;
        let written = (|| {
            let mut f = crate::store::create_staging(&p.tmp)?;
            f.write_all(data)?;
            sync_file(&f, false)?;
            drop(f);
            std::fs::rename(&p.tmp, &p.path)?;
            sync_dir(&p.dir);
            Ok(())
        })();
        match &written {
            Ok(()) => {
                self.written = data.len() as u64;
                self.synced = self.written;
                self.failed = None;
            }
            Err(_) => {
                let _ = std::fs::remove_file(&p.tmp);
            }
        }
        written
    }
}

fn sync_dir(dir: &Path) {
    // makes a new name durable on POSIX; opening a directory fails on Windows (where NTFS journals it)
    if let Ok(d) = std::fs::File::open(dir) {
        let _ = d.sync_all();
    }
}

fn run(rx: &Receiver<Msg>, progress: &Mutex<Progress>, p: Paths, len: u64) {
    let mut log = Log { file: None, written: len, synced: len, failed: None };
    let publish = |log: &Log| {
        *progress.lock().unwrap_or_else(PoisonError::into_inner) = Progress { durable_len: log.synced, failed: log.failed.clone() };
    };
    while let Ok(first) = rx.recv() {
        // everything waiting is one batch: one sync for many appends
        let mut batch = vec![first];
        while let Ok(m) = rx.try_recv() {
            batch.push(m);
        }
        for m in batch {
            match m {
                Msg::Append(data) => log.append(&p, &data),
                Msg::Truncate(len, reply) => {
                    log.sync(&p);
                    let r = log.truncate(&p, len);
                    publish(&log);
                    let _ = reply.send(r);
                }
                Msg::Replace(data, reply) => {
                    let r = log.replace(&p, &data);
                    publish(&log);
                    let _ = reply.send(r);
                }
                Msg::Flush(reply) => {
                    log.sync(&p);
                    publish(&log);
                    let _ = reply.send(());
                }
            }
        }
        log.sync(&p);
        publish(&log);
    }
}
