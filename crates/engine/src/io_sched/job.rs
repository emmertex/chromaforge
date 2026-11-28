//! One running job's shared state (§5.2 of the I/O spec): what it moved, by kind and volume (a
//! copy's source reads, destination writes and read-back), when it last made progress, which of a
//! copy's volumes its current I/O call is on, whether it waits at the work gate or computes, the
//! gate bytes it holds, and whether the scheduler abandoned it. Shared (`Arc`) between the job's thread, the call that runs it and,
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

/// Which of a copy's two volumes an I/O call is on (a job that isn't a copy has only its source).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Side {
    Source,
    Dest,
}

impl Side {
    fn index(self) -> usize {
        match self {
            Side::Source => 0,
            Side::Dest => 1,
        }
    }

    /// The other volume of a copy.
    pub(crate) fn other(self) -> Side {
        match self {
            Side::Source => Side::Dest,
            Side::Dest => Side::Source,
        }
    }
}

/// What a counted byte was: read from the source, written to the destination, or read back from
/// the destination (a copy's verify, Copy as DNG's read of the stored raw and of its DNG).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ByteKind {
    Read,
    Written,
    ReadBack,
}

impl ByteKind {
    fn index(self) -> usize {
        match self {
            ByteKind::Read => 0,
            ByteKind::Written => 1,
            ByteKind::ReadBack => 2,
        }
    }

    /// The volume the byte moved on.
    pub(crate) fn side(self) -> Side {
        match self {
            ByteKind::Read => Side::Source,
            ByteKind::Written | ByteKind::ReadBack => Side::Dest,
        }
    }
}

/// A job's bytes by kind.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Moved {
    pub(crate) read: u64,
    pub(crate) written: u64,
    pub(crate) read_back: u64,
}

impl Moved {
    /// Every byte (a copy's read, its write and its read-back each count).
    pub(crate) fn total(&self) -> u64 {
        self.read.saturating_add(self.written).saturating_add(self.read_back)
    }

    /// The bytes that moved on `side`'s volume.
    pub(crate) fn on(&self, side: Side) -> u64 {
        match side {
            Side::Source => self.read,
            Side::Dest => self.written.saturating_add(self.read_back),
        }
    }
}

/// [`JobState::on`]'s value before any call is marked.
const ON_UNKNOWN: u8 = 0;

/// A job's state, installed in its thread ([`crate::memory::with_job`]) while it runs.
pub(crate) struct JobState {
    pub(crate) id: u64,
    pub(crate) volume: VolumeKey,
    clock: Arc<dyn Clock>,
    /// The registry the gate's "cannot fit" rule reads (weak: the registry holds abandoned jobs).
    shared: Weak<Shared>,
    /// Bytes by [`ByteKind`].
    bytes: [AtomicU64; 3],
    /// Monotonic time bytes last moved, per [`Side`] (0: never).
    last_bytes: [AtomicU64; 2],
    /// The side the job's current (or last) I/O call is on: [`ON_UNKNOWN`], or a [`Side`]'s
    /// index + 1. Set by the job's code before each call, so a call that hangs names its volume.
    on: AtomicU8,
    /// Monotonic time of the last progress: the start, every byte read, the end of a computing span.
    last_progress: AtomicU64,
    /// When the current computing span began, plus one (0: not computing).
    computing_since: AtomicU64,
    at_gate: AtomicBool,
    /// Work-gate bytes the job's permits hold now.
    permit_bytes: AtomicU64,
    status: AtomicU8,
    /// Cancel asked the job to skip its optional last step (a copy's DNG conversion): the gate
    /// refuses its next wait ([`crate::memory::GateError::Skipped`]).
    skip: AtomicBool,
    /// Set when the gate refused the job because abandoned jobs hold memory (typed, for the
    /// metrics; nothing matches the error text).
    gate_failure: Mutex<Option<VolumeKey>>,
    /// The work gate whose permits it holds or waits for, woken when the job is abandoned.
    gate: Mutex<Option<Arc<crate::memory::GateCore>>>,
}

impl JobState {
    pub(crate) fn new(id: u64, volume: VolumeKey, clock: Arc<dyn Clock>, shared: Weak<Shared>) -> JobState {
        let now = clock.mono();
        JobState {
            id,
            volume,
            clock,
            shared,
            bytes: [AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0)],
            last_bytes: [AtomicU64::new(0), AtomicU64::new(0)],
            on: AtomicU8::new(ON_UNKNOWN),
            last_progress: AtomicU64::new(now),
            computing_since: AtomicU64::new(0),
            at_gate: AtomicBool::new(false),
            permit_bytes: AtomicU64::new(0),
            status: AtomicU8::new(RUNNING),
            skip: AtomicBool::new(false),
            gate_failure: Mutex::new(None),
            gate: Mutex::new(None),
        }
    }

    pub(crate) fn now(&self) -> u64 {
        self.clock.mono()
    }

    pub(crate) fn shared(&self) -> Option<Arc<Shared>> {
        self.shared.upgrade()
    }

    /// `n` bytes read from the source: [`JobState::add`] as [`ByteKind::Read`] (the tests' fake
    /// jobs).
    #[cfg(test)]
    pub(crate) fn add_bytes(&self, n: usize) {
        self.add(ByteKind::Read, n);
    }

    /// `n` bytes of `kind` moved: counted on its side's volume, the call is on that side, and the
    /// job's clock restarts.
    pub(crate) fn add(&self, kind: ByteKind, n: usize) {
        let now = self.clock.mono();
        if let Some(b) = self.bytes.get(kind.index()) {
            b.fetch_add(u64::try_from(n).unwrap_or(u64::MAX), Ordering::Relaxed);
        }
        if let Some(t) = self.last_bytes.get(kind.side().index()) {
            t.fetch_max(now, Ordering::Relaxed);
        }
        self.set_on(kind.side());
        self.last_progress.fetch_max(now, Ordering::Relaxed);
    }

    /// The job's next I/O call is on `side` (a copy's metadata calls, opens and flushes, which
    /// move no counted byte).
    pub(crate) fn set_on(&self, side: Side) {
        let v = u8::try_from(side.index() + 1).unwrap_or(ON_UNKNOWN);
        self.on.store(v, Ordering::Relaxed);
    }

    /// The side its current (or last) I/O call is on; `None` before any call was marked.
    pub(crate) fn on(&self) -> Option<Side> {
        match self.on.load(Ordering::Relaxed) {
            1 => Some(Side::Source),
            2 => Some(Side::Dest),
            _ => None,
        }
    }

    /// When bytes last moved, on either side (0: never).
    pub(crate) fn last_bytes(&self) -> u64 {
        self.last_bytes.iter().map(|t| t.load(Ordering::Relaxed)).max().unwrap_or(0)
    }

    /// When bytes last moved on `side` (0: never).
    pub(crate) fn last_bytes_on(&self, side: Side) -> u64 {
        self.last_bytes.get(side.index()).map_or(0, |t| t.load(Ordering::Relaxed))
    }

    /// Every byte the job moved ([`Moved::total`]).
    pub(crate) fn bytes(&self) -> u64 {
        self.moved().total()
    }

    /// Its bytes by kind.
    pub(crate) fn moved(&self) -> Moved {
        let get = |k: ByteKind| self.bytes.get(k.index()).map_or(0, |b| b.load(Ordering::Relaxed));
        Moved { read: get(ByteKind::Read), written: get(ByteKind::Written), read_back: get(ByteKind::ReadBack) }
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

    /// Cancel's request to skip the job's optional last step (a copy's DNG step). Its gate is
    /// woken, so a wait there for that step ends at once.
    pub(crate) fn request_skip(&self) {
        self.skip.store(true, Ordering::Release);
        self.wake_gate();
    }

    /// Wake the gate the job holds or waits for permits of, if any (the lock on it is let go
    /// first: the gate's waiters take the gate's lock, then this one).
    fn wake_gate(&self) {
        let gate = self.gate.lock().unwrap_or_else(PoisonError::into_inner).clone();
        if let Some(g) = gate {
            g.wake();
        }
    }

    pub(crate) fn skip_requested(&self) -> bool {
        self.skip.load(Ordering::Acquire)
    }

    /// The job's thread claims its result; `false` if the scheduler abandoned it first.
    pub(crate) fn try_finish(&self) -> bool {
        self.status.compare_exchange(RUNNING, FINISHED, Ordering::AcqRel, Ordering::Acquire).is_ok()
    }

    /// The job holds or waits for permits of `gate` (an abandon wakes it).
    pub(crate) fn attach_gate(&self, gate: &Arc<crate::memory::GateCore>) {
        *self.gate.lock().unwrap_or_else(PoisonError::into_inner) = Some(gate.clone());
    }

    /// The scheduler abandons the job; `false` if its thread finished first. Its gate is woken:
    /// the job ends a wait there at once, and other waiters look again at the memory it strands.
    pub(crate) fn try_abandon(&self) -> bool {
        let abandoned = self.status.compare_exchange(RUNNING, ABANDONED, Ordering::AcqRel, Ordering::Acquire).is_ok();
        if abandoned {
            self.wake_gate();
        }
        abandoned
    }

    pub(crate) fn record_gate_failure(&self, volume: Option<VolumeKey>) {
        *self.gate_failure.lock().unwrap_or_else(PoisonError::into_inner) = volume;
    }

    pub(crate) fn gate_failure(&self) -> Option<VolumeKey> {
        self.gate_failure.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}
