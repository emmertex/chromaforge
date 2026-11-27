//! The scheduler's job types (§5.2 of the I/O spec): a job, its typed outcome, its metrics
//! (R9), and the single-job path, which runs on the calling thread as today. Several jobs go
//! through the pool ([`super::pool`]), whose scheduler is [`super::sched`].

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use lcc_catalog::folders::{VolumeKey, volume_key};

use super::IoContext;
use super::job::{JobState, Moved, Side};
use super::shared::Admission;

/// One unit of file work: the file it reads (its volume decides its queue), where it writes, if
/// anywhere (a copy: the destination folder, whose volume it also needs a slot on), and the work.
pub(crate) struct IoJob<T> {
    pub(crate) source: String,
    pub(crate) dest: Option<String>,
    /// Cancel asks it to skip its optional last step instead of abandoning it at the work gate (a
    /// copy's Copy as DNG step: it then keeps its raw copy and returns).
    pub(crate) skippable: bool,
    pub(crate) run: Box<dyn FnOnce() -> T + Send + 'static>,
}

impl<T> IoJob<T> {
    /// A job that reads `source` only.
    pub(crate) fn read(source: String, run: Box<dyn FnOnce() -> T + Send + 'static>) -> IoJob<T> {
        IoJob { source, dest: None, skippable: false, run }
    }
}

/// What became of a job. Every outcome but `Done` comes from the scheduler, never from the job.
#[derive(Debug)]
pub(crate) enum JobOutcome<T> {
    /// The job ran to its end.
    Done(T),
    /// Never started: the call was cancelled first.
    NotStarted,
    /// Started, then left behind by the user's cancel (its result, if it comes, is dropped).
    Cancelled,
    /// A volume it needs stopped responding: abandoned while running (`abandoned`: its outcome is
    /// unknown, it may still finish), or failed while queued or at once because the volume is
    /// blocked (never started).
    VolumeNotResponding { volume: VolumeKey, abandoned: bool },
    /// The job panicked (the message says how).
    Panicked(String),
    /// Never started: the pool's supervisor stopped (a panic) with it queued.
    SchedulerStopped,
}

impl<T> JobOutcome<T> {
    /// Whether a call's progress counts this outcome as a file done: every outcome but a job that
    /// cancel kept from starting ([`JobOutcome::NotStarted`]) or left behind
    /// ([`JobOutcome::Cancelled`]). The one rule for [`super::pool::run_jobs`]' `done` and every
    /// streaming caller's own count.
    pub(crate) fn counts_as_done(&self) -> bool {
        !matches!(self, JobOutcome::NotStarted | JobOutcome::Cancelled)
    }
}

/// What one job did (R9): bytes it moved, its time, its wait at the work gate.
#[derive(Clone, Debug)]
pub(crate) struct JobMetrics {
    pub(crate) volume: VolumeKey,
    /// A copy's destination volume, when not the source's.
    pub(crate) dest: Option<VolumeKey>,
    /// Every byte it moved (`moved`'s total).
    pub(crate) bytes: u64,
    /// Its bytes by kind: read (on its source), written and read back (on its destination).
    pub(crate) moved: Moved,
    /// Nanoseconds from start to end (or to abandonment).
    pub(crate) wall: u64,
    /// Nanoseconds of that spent waiting at work gates.
    pub(crate) gate_wait: u64,
    /// The gate refused it because abandoned jobs on this volume hold memory.
    pub(crate) gate_failure: Option<VolumeKey>,
}

impl JobMetrics {
    /// A job that did nothing (yet).
    pub(crate) fn none(job: &JobState) -> JobMetrics {
        let moved = job.moved();
        JobMetrics { volume: job.volume.clone(), dest: None, bytes: moved.total(), moved, wall: 0, gate_wait: 0, gate_failure: job.gate_failure() }
    }
}

/// A call's results: an outcome per job, in input order, and what each job that ran did.
pub(crate) struct Report<T> {
    pub(crate) outcomes: Vec<JobOutcome<T>>,
    /// (logged per volume by the call; read by the tests and, from PR 5, the limit controller)
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) metrics: Vec<Option<JobMetrics>>,
}

/// Run `run` as `job` (whose destination volume is `dest`) on this thread, under the panic guard.
pub(crate) fn execute<T>(
    run: Box<dyn FnOnce() -> T + Send + 'static>,
    job: &Arc<JobState>,
    dest: Option<VolumeKey>,
) -> (Result<T, String>, JobMetrics) {
    let (t0, w0) = (job.now(), crate::memory::gate_wait_nanos());
    let result = crate::memory::with_job(Some(job.clone()), || crate::guard::catch("reading the file", run));
    let moved = job.moved();
    let metrics = JobMetrics {
        volume: job.volume.clone(),
        dest,
        bytes: moved.total(),
        moved,
        wall: job.now().saturating_sub(t0),
        gate_wait: crate::memory::gate_wait_nanos().saturating_sub(w0),
        gate_failure: job.gate_failure(),
    };
    (result, metrics)
}

/// A single job, on the calling thread as today (no threads, no mount read, no stall rule; cancel
/// set before it starts means it doesn't start, but can't end it once it runs). It still checks
/// the registry, through the cached mount list: on a blocked volume (a copy: either of its two) it
/// runs as the recovery job if one may run now, else fails at once. Start and end follow the
/// registry's one rule each ([`super::shared::Shared::admission`], [`super::shared::Shared::job_finished`]), as the pool's
/// scheduler does.
pub(crate) fn run_single<T: Send + 'static>(
    ctx: &Arc<IoContext>,
    job: IoJob<T>,
    succeeded: fn(&T) -> bool,
    cancel: &AtomicBool,
    done: &AtomicUsize,
) -> Report<T> {
    if cancel.load(Ordering::Relaxed) {
        return Report { outcomes: vec![JobOutcome::NotStarted], metrics: vec![None] };
    }
    let shared = &ctx.shared;
    let mounts = ctx.mounts.cached();
    let key = volume_key(&job.source, &mounts);
    let dest = job.dest.as_deref().map(|d| volume_key(d, &mounts)).filter(|d| d != &key);
    let keys: Vec<VolumeKey> = std::iter::once(key.clone()).chain(dest.clone()).collect();
    let now = shared.clock.mono();
    let id = shared.next_id();
    let mut recovering: Vec<&VolumeKey> = Vec::new();
    for k in &keys {
        match shared.admission(k, id, now) {
            Admission::Free => {}
            Admission::Recovery => recovering.push(k),
            Admission::Refused => {
                for r in recovering {
                    shared.end_recovery(r, id);
                }
                done.fetch_add(1, Ordering::Relaxed);
                return Report { outcomes: vec![JobOutcome::VolumeNotResponding { volume: k.clone(), abandoned: false }], metrics: vec![None] };
            }
        }
    }
    let state = Arc::new(JobState::new(id, key.clone(), shared.clock.clone(), Arc::downgrade(shared)));
    for k in &keys {
        shared.job_started(k, now);
    }
    let (result, metrics) = execute(job.run, &state, dest);
    let _ = state.try_finish();
    let ok = result.as_ref().is_ok_and(succeeded);
    let end = shared.clock.mono();
    for k in &keys {
        let _ = shared.job_finished(k, end, took(&metrics), metrics.bytes > 0, ok, recovering.contains(&k).then_some(id));
    }
    done.fetch_add(1, Ordering::Relaxed);
    let outcome = match result {
        Ok(t) => JobOutcome::Done(t),
        Err(m) => JobOutcome::Panicked(m),
    };
    let metrics = vec![Some(metrics)];
    log_metrics(&metrics);
    Report { outcomes: vec![outcome], metrics }
}

/// A finished job's time on its volume: its wall time less its wait at the work gate.
pub(crate) fn took(m: &JobMetrics) -> u64 {
    m.wall.saturating_sub(m.gate_wait)
}

/// Per-volume totals of a call (R9): debug log, and stderr under `<PREFIX>_PROFILE`.
pub(crate) fn log_metrics(metrics: &[Option<JobMetrics>]) {
    let mut per: Vec<(&VolumeKey, u64, u64, u64, u64, u64)> = Vec::new();
    // (a copy counts on its source, with the bytes it read, and on its destination, with those it
    // wrote and read back)
    for (m, v, bytes) in metrics.iter().flatten().flat_map(|m| match &m.dest {
        Some(d) => vec![(m, &m.volume, m.moved.on(Side::Source)), (m, d, m.moved.on(Side::Dest))],
        None => vec![(m, &m.volume, m.bytes)],
    }) {
        let row = match per.iter().position(|r| r.0 == v) {
            Some(i) => per.get_mut(i),
            None => {
                per.push((v, 0, 0, 0, 0, 0));
                per.last_mut()
            }
        };
        if let Some(r) = row {
            r.1 = r.1.saturating_add(1);
            r.2 = r.2.saturating_add(bytes);
            r.3 = r.3.saturating_add(m.wall);
            r.4 = r.4.saturating_add(m.gate_wait);
            r.5 = r.5.saturating_add(u64::from(m.gate_failure.is_some()));
        }
    }
    let profile = lcc_brand::env_os("PROFILE").is_some();
    for (v, files, bytes, wall, gate, refused) in per {
        let line = format!(
            "io: volume {v}: {files} files, {bytes} bytes, {} ms in jobs, {} ms at the gate, {refused} refused by stranded memory",
            wall / 1_000_000,
            gate / 1_000_000
        );
        log::debug!("{line}");
        if profile {
            eprintln!("{line}");
        }
    }
}
