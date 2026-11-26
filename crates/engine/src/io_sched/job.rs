//! One running job's shared state (§5.2 of the I/O spec): what its reads moved, when it last made
//! progress, whether it waits at the work gate or computes, the gate bytes it holds, and whether
//! the scheduler abandoned it. Shared (`Arc`) between the job's thread, the call that runs it and,
//! once abandoned, the registry, so a job's silence can be judged with no thread sampling it.
// (on wasm32 no job runs: the gate only consults this when a job is installed)
#![cfg_attr(target_arch = "wasm32", allow(dead_code))]

use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, Weak};

use lcc_catalog::folders::VolumeKey;

use super::clock::Clock;
use super::shared::Shared;

const RUNNING: u8 = 0;
const FINISHED: u8 = 1;
const ABANDONED: u8 = 2;

/// A job's state, installed in its thread ([`crate::memory::with_job`]) while it runs.
pub(crate) struct JobState {
    pub(crate) id: u64,
    pub(crate) volume: VolumeKey,
    clock: Arc<dyn Clock>,
    /// The registry the gate's "cannot fit" rule reads (weak: the registry holds abandoned jobs).
    shared: Weak<Shared>,
    bytes: AtomicU64,
    /// Monotonic time bytes last moved (0: never).
    last_bytes: AtomicU64,
    /// Monotonic time of the last progress: the start, every byte read, the end of a computing span.
    last_progress: AtomicU64,
    /// When the current computing span began, plus one (0: not computing).
    computing_since: AtomicU64,
    at_gate: AtomicBool,
    /// Work-gate bytes the job's permits hold now.
    permit_bytes: AtomicU64,
    status: AtomicU8,
    /// Set when the gate refused the job because abandoned jobs hold memory (typed, for the
    /// metrics; nothing matches the error text).
    gate_failure: Mutex<Option<VolumeKey>>,
}

impl JobState {
    pub(crate) fn new(id: u64, volume: VolumeKey, clock: Arc<dyn Clock>, shared: Weak<Shared>) -> JobState {
        let now = clock.mono();
        JobState {
            id,
            volume,
            clock,
            shared,
            bytes: AtomicU64::new(0),
            last_bytes: AtomicU64::new(0),
            last_progress: AtomicU64::new(now),
            computing_since: AtomicU64::new(0),
            at_gate: AtomicBool::new(false),
            permit_bytes: AtomicU64::new(0),
            status: AtomicU8::new(RUNNING),
            gate_failure: Mutex::new(None),
        }
    }

    pub(crate) fn now(&self) -> u64 {
        self.clock.mono()
    }

    pub(crate) fn shared(&self) -> Option<Arc<Shared>> {
        self.shared.upgrade()
    }

    /// `n` bytes moved: counted, and the job's clock restarts.
    pub(crate) fn add_bytes(&self, n: usize) {
        let now = self.clock.mono();
        self.bytes.fetch_add(u64::try_from(n).unwrap_or(u64::MAX), Ordering::Relaxed);
        self.last_bytes.fetch_max(now, Ordering::Relaxed);
        self.last_progress.fetch_max(now, Ordering::Relaxed);
    }

    /// When bytes last moved (0: never).
    pub(crate) fn last_bytes(&self) -> u64 {
        self.last_bytes.load(Ordering::Relaxed)
    }

    pub(crate) fn bytes(&self) -> u64 {
        self.bytes.load(Ordering::Relaxed)
    }

    pub(crate) fn last_progress(&self) -> u64 {
        self.last_progress.load(Ordering::Relaxed)
    }

    /// Start a computing span; `false` if one is already running (the outer one stays).
    pub(crate) fn begin_computing(&self) -> bool {
        let now = self.clock.mono().saturating_add(1);
        self.computing_since.compare_exchange(0, now, Ordering::AcqRel, Ordering::Relaxed).is_ok()
    }

    /// End the computing span: the job's clock restarts now.
    pub(crate) fn end_computing(&self) {
        self.last_progress.fetch_max(self.clock.mono(), Ordering::Relaxed);
        self.computing_since.store(0, Ordering::Release);
    }

    /// How long the current computing span has lasted at `now`, if one runs.
    pub(crate) fn computing_for(&self, now: u64) -> Option<u64> {
        match self.computing_since.load(Ordering::Acquire) {
            0 => None,
            s => Some(now.saturating_sub(s - 1)),
        }
    }

    pub(crate) fn set_at_gate(&self, waiting: bool) {
        self.at_gate.store(waiting, Ordering::Relaxed);
    }

    pub(crate) fn at_gate(&self) -> bool {
        self.at_gate.load(Ordering::Relaxed)
    }

    pub(crate) fn permit_add(&self, bytes: usize) {
        self.permit_bytes.fetch_add(u64::try_from(bytes).unwrap_or(u64::MAX), Ordering::AcqRel);
    }

    pub(crate) fn permit_sub(&self, bytes: usize) {
        let b = u64::try_from(bytes).unwrap_or(u64::MAX);
        let _ = self.permit_bytes.try_update(Ordering::AcqRel, Ordering::Acquire, |v| Some(v.saturating_sub(b)));
    }

    pub(crate) fn permit_bytes(&self) -> u64 {
        self.permit_bytes.load(Ordering::Acquire)
    }

    pub(crate) fn is_abandoned(&self) -> bool {
        self.status.load(Ordering::Acquire) == ABANDONED
    }

    /// The job's thread claims its result; `false` if the scheduler abandoned it first.
    pub(crate) fn try_finish(&self) -> bool {
        self.status.compare_exchange(RUNNING, FINISHED, Ordering::AcqRel, Ordering::Acquire).is_ok()
    }

    /// The scheduler abandons the job; `false` if its thread finished first.
    pub(crate) fn try_abandon(&self) -> bool {
        self.status.compare_exchange(RUNNING, ABANDONED, Ordering::AcqRel, Ordering::Acquire).is_ok()
    }

    pub(crate) fn record_gate_failure(&self, volume: Option<VolumeKey>) {
        *self.gate_failure.lock().unwrap_or_else(PoisonError::into_inner) = volume;
    }

    pub(crate) fn gate_failure(&self) -> Option<VolumeKey> {
        self.gate_failure.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}
