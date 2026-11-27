//! The scheduler's clocks, injectable so tests can step time: a monotonic clock (which doesn't
//! advance while the computer sleeps, on macOS and Linux) and the wall clock (which does). A wall
//! clock that jumps past the monotonic one marks a wake from sleep (§5.2 of the I/O spec). On
//! Windows the monotonic clock (QueryPerformanceCounter) counts the sleep too, so there a tick
//! that comes far later than it was due marks the wake instead ([`INSTANT_COUNTS_SLEEP`]).
// (on wasm32 no job runs: the gate only consults this when a job is installed)
#![cfg_attr(target_arch = "wasm32", allow(dead_code))]

use std::sync::OnceLock;

/// Nanoseconds in a second.
pub(crate) const SEC: u64 = 1_000_000_000;

/// Whether the monotonic clock keeps counting while the computer sleeps (Windows), so a wake
/// shows as a late tick rather than as a wall-clock jump.
pub(crate) const INSTANT_COUNTS_SLEEP: bool = cfg!(windows);

/// Both clocks, in nanoseconds.
pub(crate) trait Clock: Send + Sync {
    /// Monotonic nanoseconds since a process epoch.
    fn mono(&self) -> u64;
    /// Wall-clock nanoseconds since the Unix epoch.
    fn wall(&self) -> u64;
}

/// The real clocks (`web_time`, so wasm32 builds too).
pub(crate) struct SystemClock;

impl Clock for SystemClock {
    fn mono(&self) -> u64 {
        static EPOCH: OnceLock<web_time::Instant> = OnceLock::new();
        let epoch = *EPOCH.get_or_init(web_time::Instant::now);
        u64::try_from(epoch.elapsed().as_nanos()).unwrap_or(u64::MAX)
    }

    fn wall(&self) -> u64 {
        web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map(|d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX)).unwrap_or(0)
    }
}

/// A clock a test steps by hand.
#[cfg(test)]
#[derive(Default)]
pub(crate) struct FakeClock {
    mono: std::sync::atomic::AtomicU64,
    wall: std::sync::atomic::AtomicU64,
}

#[cfg(test)]
impl FakeClock {
    /// Both clocks forward by `nanos`.
    pub(crate) fn advance(&self, nanos: u64) {
        use std::sync::atomic::Ordering::SeqCst;
        self.mono.fetch_add(nanos, SeqCst);
        self.wall.fetch_add(nanos, SeqCst);
    }

    /// Only the wall clock forward (a sleep: the monotonic clock stood still).
    pub(crate) fn sleep(&self, nanos: u64) {
        self.wall.fetch_add(nanos, std::sync::atomic::Ordering::SeqCst);
    }

    /// Set the monotonic clock to `nanos` (the wall clock moves with it).
    pub(crate) fn set(&self, nanos: u64) {
        let now = self.mono();
        if nanos > now {
            self.advance(nanos - now);
        }
    }
}

#[cfg(test)]
impl Clock for FakeClock {
    fn mono(&self) -> u64 {
        self.mono.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn wall(&self) -> u64 {
        self.wall.load(std::sync::atomic::Ordering::SeqCst)
    }
}
