//! Process-wide scheduler state (§5.2 of the I/O spec): the registry of abandoned jobs that have
//! not returned, and each volume's history ("ever moved bytes", idle time, longest finished job,
//! when a job last showed the volume works, its recovery job), shared by every call.
//!
//! Lock order: the work gate's lock, then this one. Nothing holding this lock drops a gate
//! `Permit` (entries hold job states, never permits).
// (on wasm32 no job runs: the gate only consults this when a job is installed)
#![cfg_attr(target_arch = "wasm32", allow(dead_code))]

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use lcc_catalog::folders::VolumeKey;

use super::clock::{Clock, SEC};
use super::job::JobState;

/// No bytes and no finished job for this long stalls a volume that has ever moved bytes.
pub(crate) const STALL_WITH_BYTES: u64 = 30 * SEC;
/// The least stall time of a volume whose jobs never reported bytes (the completion rule), and
/// of one waking from idle.
pub(crate) const STALL_GRACE: u64 = 120 * SEC;
/// The completion rule: this many times the volume's longest finished job.
pub(crate) const COMPLETION_FACTOR: u64 = 4;
/// A volume with no running job for more than this gets [`STALL_GRACE`] until it moves bytes.
pub(crate) const IDLE_AFTER: u64 = 60 * SEC;
/// A wall clock that jumps more than this past the monotonic one between two ticks is a wake.
pub(crate) const WAKE_JUMP: u64 = 5 * SEC;
/// A computing span stops pausing a clock past this many stall times.
pub(crate) const COMPUTING_FACTOR: u64 = 10;
/// After cancel, a volume showing no progress for this long has its running jobs abandoned.
pub(crate) const CANCEL_SILENCE: u64 = 5 * SEC;
/// A blocked volume lets a recovery job start at most this often.
pub(crate) const RECOVERY_EVERY: u64 = 60 * SEC;
/// A stuck recovery job stops being "the" recovery job after this long.
pub(crate) const RECOVERY_STUCK: u64 = 600 * SEC;

/// Why a job was abandoned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AbandonKind {
    /// Its volume stalled: stalled on its own clock from the start (unless computing).
    Stall,
    /// The user cancelled: stalled only once its own clock passes its stall time.
    Cancel,
}

/// An abandoned job that has not returned.
struct Entry {
    volume: VolumeKey,
    kind: AbandonKind,
    abandoned_at: u64,
    /// Its volume's stall time when it was abandoned.
    stall_time: u64,
    job: Arc<JobState>,
}

/// A volume's recovery job, while it runs or is abandoned and hasn't returned.
struct Recovery {
    job_id: u64,
    started_at: u64,
}

#[derive(Default)]
struct VolumeHistory {
    /// A job on it ever reported bytes in this process.
    ever_bytes: bool,
    /// The longest finished job (monotonic ns, gate wait excluded).
    longest_finished: u64,
    /// Running jobs on it, every call together (abandoned ones excluded).
    running: usize,
    /// When `running` last fell to 0.
    idle_since: Option<u64>,
    /// Woke from idle and hasn't moved bytes since: at least [`STALL_GRACE`].
    grace: bool,
    /// When a job last showed the volume works (moved bytes, or finished well).
    last_works: Option<u64>,
    recovery: Option<Recovery>,
    last_recovery_start: Option<u64>,
}

#[derive(Default)]
struct State {
    entries: HashMap<u64, Entry>,
    volumes: HashMap<VolumeKey, VolumeHistory>,
}

/// The registry and the volumes' history.
pub(crate) struct Shared {
    pub(crate) clock: Arc<dyn Clock>,
    state: Mutex<State>,
    /// The last wake from sleep, plus one (0: none).
    last_wake: AtomicU64,
    next_id: AtomicU64,
}

impl Shared {
    pub(crate) fn new(clock: Arc<dyn Clock>) -> Shared {
        Shared { clock, state: Mutex::default(), last_wake: AtomicU64::new(0), next_id: AtomicU64::new(1) }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    /// A wake from sleep was seen at `now`: every volume and entry gets [`STALL_GRACE`] from it.
    pub(crate) fn note_wake(&self, now: u64) {
        self.last_wake.store(now.saturating_add(1), Ordering::Release);
    }

    /// Whether a wake less than [`STALL_GRACE`] ago holds every stall off.
    pub(crate) fn wake_holds(&self, now: u64) -> bool {
        match self.last_wake.load(Ordering::Acquire) {
            0 => false,
            w => now.saturating_sub(w - 1) < STALL_GRACE,
        }
    }

    /// The volume's stall time now: 30 s once a job on it has reported bytes, else the completion
    /// rule (at least 120 s); at least 120 s while it wakes from idle.
    pub(crate) fn stall_time(&self, volume: &VolumeKey) -> u64 {
        stall_time_of(self.lock().volumes.get(volume))
    }

    /// Whether `e` is stalled on its own clock at `now` (its permits then count as stranded).
    fn entry_stalled(&self, e: &Entry, now: u64) -> bool {
        if self.wake_holds(now) {
            return false;
        }
        if let Some(span) = e.job.computing_for(now) {
            return span > e.stall_time.saturating_mul(COMPUTING_FACTOR);
        }
        if e.kind == AbandonKind::Stall && e.job.last_progress() <= e.abandoned_at {
            return true;
        }
        now.saturating_sub(e.job.last_progress()) >= e.stall_time
    }

    /// A: the gate bytes held by abandoned jobs stalled on their own clocks, and the volume of the
    /// entry holding the most.
    pub(crate) fn stranded(&self, now: u64) -> (u128, Option<VolumeKey>) {
        let st = self.lock();
        let mut total = 0u128;
        let mut top: Option<(u64, &VolumeKey)> = None;
        for e in st.entries.values() {
            let b = e.job.permit_bytes();
            if b == 0 || !self.entry_stalled(e, now) {
                continue;
            }
            total = total.saturating_add(u128::from(b));
            if top.is_none_or(|(tb, _)| b > tb) {
                top = Some((b, &e.volume));
            }
        }
        (total, top.map(|(_, v)| v.clone()))
    }

    /// Whether `volume` is blocked: an entry on it is stalled on its own clock and no job on it has
    /// shown the volume works since that entry was abandoned (a read at that same instant counts as
    /// since: the stall that abandoned it needed seconds of silence before).
    pub(crate) fn blocked(&self, volume: &VolumeKey, now: u64) -> bool {
        let st = self.lock();
        let works = st.volumes.get(volume).and_then(|h| h.last_works);
        st.entries.values().any(|e| &e.volume == volume && works.is_none_or(|w| w < e.abandoned_at) && self.entry_stalled(e, now))
    }

    /// Make `job_id` the volume's recovery job if one may start now (none outstanding and none started in the last minute, or the outstanding one stuck for 10 minutes),
    /// in one step so two calls can't both start one.
    pub(crate) fn try_start_recovery(&self, volume: &VolumeKey, job_id: u64, now: u64) -> bool {
        let mut st = self.lock();
        let h = st.volumes.entry(volume.clone()).or_default();
        let may = match &h.recovery {
            Some(r) => now.saturating_sub(r.started_at) >= RECOVERY_STUCK,
            None => h.last_recovery_start.is_none_or(|t| now.saturating_sub(t) >= RECOVERY_EVERY),
        };
        if may {
            h.recovery = Some(Recovery { job_id, started_at: now });
            h.last_recovery_start = Some(now);
        }
        may
    }

    fn end_recovery_locked(st: &mut State, volume: &VolumeKey, job_id: u64) {
        if let Some(h) = st.volumes.get_mut(volume)
            && h.recovery.as_ref().is_some_and(|r| r.job_id == job_id)
        {
            h.recovery = None;
        }
    }

    /// The recovery job `job_id` returned (or failed): the slot is free.
    pub(crate) fn end_recovery(&self, volume: &VolumeKey, job_id: u64) {
        Self::end_recovery_locked(&mut self.lock(), volume, job_id);
    }

    /// A job starts on `volume` at `now`.
    pub(crate) fn job_started(&self, volume: &VolumeKey, now: u64) {
        let mut st = self.lock();
        let h = st.volumes.entry(volume.clone()).or_default();
        if h.running == 0 {
            let idle = h.idle_since.is_none_or(|t| now.saturating_sub(t) > IDLE_AFTER);
            h.grace = h.grace || idle;
        }
        h.running = h.running.saturating_add(1);
    }

    fn stop_running(h: &mut VolumeHistory, now: u64) {
        h.running = h.running.saturating_sub(1);
        if h.running == 0 {
            h.idle_since = Some(now);
        }
    }

    /// A job on `volume` moved bytes; `at` is when (its last progress).
    pub(crate) fn bytes_seen(&self, volume: &VolumeKey, at: u64) {
        let mut st = self.lock();
        let h = st.volumes.entry(volume.clone()).or_default();
        h.ever_bytes = true;
        h.grace = false;
        h.last_works = Some(h.last_works.map_or(at, |w| w.max(at)));
    }

    /// A job on `volume` finished at `now`, after `took` ns outside the gate; `moved` bytes; `ok`
    /// if it succeeded.
    pub(crate) fn job_ended(&self, volume: &VolumeKey, now: u64, took: u64, moved: bool, ok: bool) {
        let mut st = self.lock();
        let h = st.volumes.entry(volume.clone()).or_default();
        Self::stop_running(h, now);
        h.longest_finished = h.longest_finished.max(took);
        if moved {
            h.ever_bytes = true;
            h.grace = false;
        }
        if moved || ok {
            h.last_works = Some(h.last_works.map_or(now, |w| w.max(now)));
        }
    }

    /// Register `job` as abandoned at `now`, before the scheduler claims it (so its thread, if it
    /// returns at once, finds the entry to remove).
    pub(crate) fn add_entry(&self, job: &Arc<JobState>, kind: AbandonKind, now: u64) {
        let mut st = self.lock();
        let stall_time = stall_time_of(st.volumes.get(&job.volume));
        st.entries.insert(job.id, Entry { volume: job.volume.clone(), kind, abandoned_at: now, stall_time, job: job.clone() });
    }

    /// The scheduler failed to claim the job (its thread finished first): no entry after all.
    pub(crate) fn drop_entry(&self, job_id: u64) {
        let _ = self.lock().entries.remove(&job_id);
    }

    /// The job was abandoned: it no longer runs on its volume.
    pub(crate) fn abandoned(&self, volume: &VolumeKey, now: u64) {
        let mut st = self.lock();
        let h = st.volumes.entry(volume.clone()).or_default();
        Self::stop_running(h, now);
    }

    /// An abandoned job's thread returned: its entry goes, and its recovery slot if it held one.
    pub(crate) fn job_returned(&self, job: &JobState) {
        let mut st = self.lock();
        let _ = st.entries.remove(&job.id);
        Self::end_recovery_locked(&mut st, &job.volume, job.id);
    }

    /// Entries outstanding (tests).
    #[cfg(test)]
    pub(crate) fn entries(&self) -> usize {
        self.lock().entries.len()
    }
}

fn stall_time_of(h: Option<&VolumeHistory>) -> u64 {
    let Some(h) = h else { return STALL_GRACE };
    let base = if h.ever_bytes { STALL_WITH_BYTES } else { h.longest_finished.saturating_mul(COMPLETION_FACTOR).max(STALL_GRACE) };
    if h.grace { base.max(STALL_GRACE) } else { base }
}
