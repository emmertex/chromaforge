//! One coalesced background refresh, shared by the caches that serve their last list at once and
//! read a new one off the calling thread ([`crate::devices::DeviceWatch`], the I/O scheduler's
//! mount list): at most one helper thread runs at a time, so a helper stuck on a dead volume never
//! has others pile up behind it, and its "running" flag clears however the helper ends — when its
//! work returns, when it panics, or at once when the thread can't start — so the cache never
//! freezes.
// (on wasm32 nothing spawns: the device list is empty there and the scheduler isn't built)
#![cfg_attr(target_arch = "wasm32", allow(dead_code))]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// The flag of one coalesced refresh.
#[derive(Default)]
pub(crate) struct Coalesced {
    running: Arc<AtomicBool>,
}

/// What [`Coalesced::spawn`] did.
#[derive(Debug)]
pub(crate) enum Started {
    /// The helper runs.
    Spawned,
    /// One still ran: nothing started (the caller serves what it has).
    Busy,
    /// The thread couldn't start; the flag is clear again.
    Failed(std::io::Error),
}

/// Clears the flag when dropped: when the helper ends however it ends, or with the closure when
/// the thread can't start.
struct Running(Arc<AtomicBool>);

impl Drop for Running {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

impl Coalesced {
    /// Run `work` on a new thread named `name`, unless the last one still runs.
    pub(crate) fn spawn(&self, name: &str, work: impl FnOnce() + Send + 'static) -> Started {
        if self.running.swap(true, Ordering::AcqRel) {
            return Started::Busy;
        }
        let guard = Running(self.running.clone());
        let spawned = std::thread::Builder::new().name(name.into()).spawn(move || {
            let _guard = guard;
            work();
        });
        match spawned {
            Ok(_) => Started::Spawned,
            // (the closure, and the guard in it, were dropped: the flag is clear again)
            Err(e) => Started::Failed(e),
        }
    }

    /// Whether a helper is running.
    pub(crate) fn running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn until(f: impl Fn() -> bool) -> bool {
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(20) {
            if f() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        false
    }

    /// One helper at a time; the flag clears when it returns and when it panics.
    #[test]
    fn one_helper_at_a_time_and_the_flag_always_clears() {
        let c = Coalesced::default();
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        assert!(matches!(
            c.spawn("t", move || {
                let _ = rx.recv();
            }),
            Started::Spawned
        ));
        assert!(c.running());
        assert!(matches!(c.spawn("t", || {}), Started::Busy));
        drop(tx);
        assert!(until(|| !c.running()));
        assert!(matches!(c.spawn("t", || panic!("a helper that panics")), Started::Spawned));
        assert!(until(|| !c.running()), "a panic clears the flag");
        assert!(matches!(c.spawn("t", || {}), Started::Spawned));
    }
}
