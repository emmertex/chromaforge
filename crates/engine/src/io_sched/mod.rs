//! The I/O scheduler (`adaptive-io-spec` PR 1): bulk file reads queued per volume, so a call over
//! several drives or cards reads them at once, and a volume that stops answering is contained
//! (its files fail "volume not responding"; the others go on; cancel returns).
//!
//! PR 1 runs one scheduler per call ([`run_jobs`]) with today's limits ([`crate::import::workers`]);
//! the abandoned-job registry and the volumes' history ([`shared`]) are process-wide. The work
//! gate's job-aware wait is [`crate::memory::WorkGate::acquire_for_job`]; byte counting is
//! [`crate::bounded_read`]'s capped counting reader.

#[cfg(not(target_arch = "wasm32"))]
use std::sync::atomic::{AtomicU64, AtomicUsize};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::{Arc, OnceLock};

pub(crate) mod clock;
pub(crate) mod job;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) mod mounts;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) mod run;
pub(crate) mod shared;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_copies;
#[cfg(test)]
mod tests_gate;

pub(crate) use job::JobState;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) use run::{IoJob, JobOutcome, run_jobs};

#[cfg(not(target_arch = "wasm32"))]
/// How often the calling thread runs the bookkeeping when no result arrives.
const TICK: std::time::Duration = std::time::Duration::from_millis(100);

#[cfg(not(target_arch = "wasm32"))]
/// Everything a scheduler call shares with others: the registry and history, the mount list,
/// the clocks. One for the process ([`IoContext::global`]); tests make their own.
pub(crate) struct IoContext {
    pub(crate) shared: Arc<shared::Shared>,
    pub(crate) mounts: mounts::MountCache,
    pub(crate) tick: std::time::Duration,
    /// How long a call waits for a fresh mount list (ns).
    pub(crate) mount_wait: u64,
    /// Thread spawns that may still succeed (tests inject failures with it; `usize::MAX` otherwise).
    pub(crate) spawn_budget: AtomicUsize,
    /// Bookkeeping ticks run (tests wait on it).
    pub(crate) ticks: AtomicU64,
    /// A tick that comes more than [`shared::WAKE_JUMP`] late is a wake from sleep too: on
    /// Windows, where the monotonic clock counts the sleep (so the wall clock doesn't jump past
    /// it); elsewhere the wall-clock jump alone tells (tests set it).
    pub(crate) late_tick_is_wake: bool,
}

#[cfg(not(target_arch = "wasm32"))]
impl IoContext {
    pub(crate) fn new(clock: Arc<dyn clock::Clock>, reader: mounts::MountReader, tick: std::time::Duration) -> IoContext {
        IoContext {
            shared: Arc::new(shared::Shared::new(clock)),
            mounts: mounts::MountCache::new(reader),
            tick,
            mount_wait: mounts::MOUNT_WAIT,
            spawn_budget: AtomicUsize::new(usize::MAX),
            ticks: AtomicU64::new(0),
            late_tick_is_wake: clock::INSTANT_COUNTS_SLEEP,
        }
    }

    /// The process's context: real clocks, this platform's mount list.
    pub(crate) fn global() -> &'static Arc<IoContext> {
        static CTX: OnceLock<Arc<IoContext>> = OnceLock::new();
        CTX.get_or_init(|| Arc::new(IoContext::new(Arc::new(clock::SystemClock), Arc::new(mounts::read_platform_mounts), TICK)))
    }
}
