//! The I/O scheduler: bulk file reads and copies queued per volume,
//! so a call over several drives or cards reads them at once, and a volume that stops answering
//! is contained (its files fail "volume not responding"; the others go on; cancel returns).
//!
//! One pool for the process ([`pool`], PR 3): every call's jobs go to one scheduler ([`sched`])
//! run by a supervisor thread, with today's limits ([`crate::import::workers`]) shared by every
//! call on a volume; the abandoned-job registry and the volumes' history ([`shared`]) are
//! process-wide. The work gate's job-aware wait is
//! [`crate::memory::WorkGate::acquire_for_job`]; byte counting is [`crate::bounded_read`]'s capped
//! counting reader.

#[cfg(all(not(target_arch = "wasm32"), test))]
use std::sync::atomic::AtomicBool;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::atomic::{AtomicU64, AtomicUsize};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::{Arc, Mutex, OnceLock};

pub(crate) mod clock;
pub(crate) mod job;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) mod mounts;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) mod pool;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) mod run;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) mod sched;
pub(crate) mod shared;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_copies;
#[cfg(test)]
mod tests_gate;
#[cfg(test)]
mod tests_pool;

pub(crate) use job::JobState;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) use pool::run_jobs;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) use run::{IoJob, JobOutcome};

#[cfg(not(target_arch = "wasm32"))]
/// How often the pool runs the bookkeeping tick (progress, the stall and cancel rules), however
/// often job ends and submits wake it for admission.
const TICK: std::time::Duration = std::time::Duration::from_millis(100);

#[cfg(not(target_arch = "wasm32"))]
/// Everything a scheduler call shares with others: the pool, the registry and history, the mount
/// list, the clocks. One for the process ([`IoContext::global`]); tests make their own.
pub(crate) struct IoContext {
    pub(crate) shared: Arc<shared::Shared>,
    pub(crate) mounts: mounts::MountCache,
    pub(crate) tick: std::time::Duration,
    /// How long a call waits for a fresh mount list (ns).
    pub(crate) mount_wait: u64,
    /// Worker thread spawns that may still succeed (tests inject failures with it; `usize::MAX`
    /// otherwise).
    pub(crate) spawn_budget: Arc<AtomicUsize>,
    /// Supervisor thread spawns that may still succeed (likewise).
    pub(crate) supervisor_budget: AtomicUsize,
    /// Bookkeeping ticks run (tests wait on it).
    pub(crate) ticks: Arc<AtomicU64>,
    /// A tick that comes more than [`shared::WAKE_JUMP`] late is a wake from sleep too: on
    /// Windows, where the monotonic clock counts the sleep (so the wall clock doesn't jump past
    /// it); elsewhere the wall-clock jump alone tells (tests set it).
    pub(crate) late_tick_is_wake: bool,
    /// Jobs the pool holds with no outcome yet (tests wait on it).
    pub(crate) pending: Arc<AtomicUsize>,
    pool: Mutex<pool::PoolState>,
    next_call: AtomicU64,
    /// Held while the context exists: the supervisor ends once it is gone.
    alive: Arc<()>,
    /// Makes the supervisor panic at its next step (tests).
    #[cfg(test)]
    pub(crate) panic_supervisor: Arc<AtomicBool>,
}

#[cfg(not(target_arch = "wasm32"))]
impl IoContext {
    pub(crate) fn new(clock: Arc<dyn clock::Clock>, reader: mounts::MountReader, tick: std::time::Duration) -> IoContext {
        IoContext {
            shared: Arc::new(shared::Shared::new(clock)),
            mounts: mounts::MountCache::new(reader),
            tick,
            mount_wait: mounts::MOUNT_WAIT,
            spawn_budget: Arc::new(AtomicUsize::new(usize::MAX)),
            supervisor_budget: AtomicUsize::new(usize::MAX),
            ticks: Arc::default(),
            late_tick_is_wake: clock::INSTANT_COUNTS_SLEEP,
            pending: Arc::default(),
            pool: Mutex::new(pool::PoolState::Down),
            next_call: AtomicU64::new(1),
            alive: Arc::new(()),
            #[cfg(test)]
            panic_supervisor: Arc::default(),
        }
    }

    /// The process's context: real clocks, this platform's mount list.
    pub(crate) fn global() -> &'static Arc<IoContext> {
        static CTX: OnceLock<Arc<IoContext>> = OnceLock::new();
        CTX.get_or_init(|| Arc::new(IoContext::new(Arc::new(clock::SystemClock), Arc::new(mounts::read_platform_mounts), TICK)))
    }
}
