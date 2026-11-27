//! The process's mount list for [`lcc_catalog::folders::volume_key`] (§5.1 of the I/O spec).
//!
//! A call with several jobs asks for a fresh read on a helper thread and waits about a second for
//! it; the process keeps the last list read, which a call whose read times out, and every
//! single-job call, uses as it is. At most one helper is outstanding: while one is stuck (a mount
//! of a dead network volume in progress), calls use the cached list at once, so stuck helpers
//! never pile up; the helper clears its "outstanding" flag through a drop guard, so a panic while
//! parsing can't freeze the cache.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use lcc_catalog::folders::Mounts;

use super::clock::{Clock, SEC};

/// How long a call waits for a fresh mount list.
pub(crate) const MOUNT_WAIT: u64 = SEC;

/// Reads the mount list; `None` when it can't be read (the cached list is kept).
pub(crate) type MountReader = Arc<dyn Fn() -> Option<Mounts> + Send + Sync>;

pub(crate) struct MountCache {
    last: Arc<Mutex<Option<Arc<Mounts>>>>,
    outstanding: Arc<AtomicBool>,
    reader: MountReader,
}

/// Clears the "outstanding" flag when the helper ends, however it ends.
struct Outstanding(Arc<AtomicBool>);

impl Drop for Outstanding {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

impl MountCache {
    pub(crate) fn new(reader: MountReader) -> MountCache {
        MountCache { last: Arc::default(), outstanding: Arc::default(), reader }
    }

    /// The last list read (no read now); [`Mounts::none`] before any.
    pub(crate) fn cached(&self) -> Arc<Mounts> {
        self.last.lock().unwrap_or_else(PoisonError::into_inner).clone().unwrap_or_else(|| Arc::new(Mounts::none()))
    }

    /// A fresh list, read on a helper thread, waiting up to `wait` ns of `clock`; the cached list
    /// when the read takes longer, when a helper is already outstanding, or when none can start.
    pub(crate) fn fresh(&self, clock: &dyn Clock, wait: u64) -> Arc<Mounts> {
        if self.outstanding.swap(true, Ordering::AcqRel) {
            return self.cached();
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let guard = Outstanding(self.outstanding.clone());
        let (reader, last) = (self.reader.clone(), self.last.clone());
        let spawned = std::thread::Builder::new().name("lc-io-mounts".into()).spawn(move || {
            let _guard = guard;
            if let Some(m) = reader() {
                if m.truncated() {
                    log::warn!("mount list cut at {} entries", m.len());
                }
                let m = Arc::new(m);
                *last.lock().unwrap_or_else(PoisonError::into_inner) = Some(m.clone());
                let _ = tx.send(m);
            }
        });
        if let Err(e) = spawned {
            // (the closure, and the guard in it, were dropped: the flag is clear again)
            log::warn!("can't read the mount list: {e}");
            return self.cached();
        }
        let start = clock.mono();
        loop {
            match rx.recv_timeout(std::time::Duration::from_millis(5)) {
                Ok(m) => return m,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return self.cached(),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    if clock.mono().saturating_sub(start) >= wait {
                        log::debug!("the mount list took more than {} ms: the last one is used", wait / 1_000_000);
                        return self.cached();
                    }
                }
            }
        }
    }

    /// Whether a helper is outstanding (tests).
    #[cfg(test)]
    pub(crate) fn outstanding(&self) -> bool {
        self.outstanding.load(Ordering::Acquire)
    }
}

/// The mount list of a mountinfo file (Linux `/proc/self/mountinfo`), read through
/// [`lcc_sysmem::limits::read_capped`] with at most `cap` bytes: a longer file is cut at its last
/// whole line, and the list says it was cut ([`Mounts::truncated`]).
#[cfg_attr(not(any(target_os = "linux", target_os = "android")), allow(dead_code))]
pub(crate) fn mounts_from_mountinfo(path: &std::path::Path, cap: u64) -> Option<Mounts> {
    let Some(read) = lcc_sysmem::limits::read_capped(path, cap) else {
        log::debug!("{}: can't be read", path.display());
        return None;
    };
    let mut m = Mounts::from_mountinfo(&read.text);
    if read.truncated {
        log::warn!("{} longer than {cap} bytes: the rest is left out", path.display());
        m.mark_truncated();
    }
    Some(m)
}

/// This platform's mount list: Linux from `/proc/self/mountinfo` (procfs, no volume touched),
/// macOS from the names in `/Volumes` (names only, no `stat` of the entries); elsewhere none.
pub(crate) fn read_platform_mounts() -> Option<Mounts> {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        mounts_from_mountinfo(std::path::Path::new("/proc/self/mountinfo"), lcc_sysmem::limits::READ_CAP)
    }
    #[cfg(target_os = "macos")]
    {
        let dir = std::fs::read_dir("/Volumes").map_err(|e| log::debug!("/Volumes: {e}")).ok()?;
        let mut names = Vec::new();
        let mut failed = 0usize;
        for entry in dir.take(lcc_catalog::folders::MAX_MOUNTS.saturating_add(1)) {
            match entry {
                Ok(e) => names.push(e.file_name().to_string_lossy().into_owned()),
                Err(_) => failed = failed.saturating_add(1),
            }
        }
        if failed > 0 {
            log::debug!("/Volumes: {failed} entries unreadable");
        }
        Some(Mounts::from_volume_names(names))
    }
    #[cfg(not(any(target_os = "linux", target_os = "android", target_os = "macos")))]
    {
        None
    }
}
