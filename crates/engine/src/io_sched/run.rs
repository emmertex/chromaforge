//! One call's scheduler (PR 1 and 2, §5.2–5.4 of the I/O spec): per-volume queues, round-robin
//! admission, today's limits, the stall rule, cancel, and the abandoned-job registry's block and
//! recovery rules. A copy holds a slot on its source and its destination (one when both are the
//! same volume); its reads are progress on its source, its writes and read-back on its
//! destination, its completion on both, and while its current call is on one of the two it is
//! waiting there, not on the other (so a hung destination is declared, not the card that feeds
//! it). The calling thread does the
//! bookkeeping, waking at every result and at least every tick; worker threads run the jobs and
//! send results over a channel, so a job that never returns can be left behind.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, PoisonError};

use lcc_catalog::folders::{VolumeKey, volume_key};

use super::IoContext;
use super::job::{JobState, Moved, Side};
use super::shared::{AbandonKind, Admission, CANCEL_SILENCE, COMPUTING_FACTOR, Ended, Shared, WAKE_JUMP};

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

/// A call's results: an outcome per job, in input order, and what each job that ran did.
pub(crate) struct Report<T> {
    pub(crate) outcomes: Vec<JobOutcome<T>>,
    /// (logged per volume by the call; read by the tests and, from PR 5, the limit controller)
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) metrics: Vec<Option<JobMetrics>>,
}

struct Task<T> {
    index: usize,
    run: Box<dyn FnOnce() -> T + Send + 'static>,
    job: Arc<JobState>,
    dest: Option<VolumeKey>,
}

struct Finished<T> {
    index: usize,
    result: Result<T, String>,
    metrics: JobMetrics,
}

/// Run `run` as `job` (whose destination volume is `dest`) on this thread, under the panic guard.
fn execute<T>(run: Box<dyn FnOnce() -> T + Send + 'static>, job: &Arc<JobState>, dest: Option<VolumeKey>) -> (Result<T, String>, JobMetrics) {
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

/// A worker: runs the jobs it is handed until the call ends; a worker whose job was abandoned
/// removes the registry entry when the job returns, and exits (its replacement already runs).
fn worker<T: Send + 'static>(tasks: Arc<Mutex<Receiver<Task<T>>>>, results: Sender<Finished<T>>, shared: Arc<Shared>) {
    loop {
        let task = tasks.lock().unwrap_or_else(PoisonError::into_inner).recv();
        let Ok(task) = task else { return };
        let (result, metrics) = execute(task.run, &task.job, task.dest);
        if !task.job.try_finish() {
            shared.job_returned(&task.job);
            return;
        }
        if results.send(Finished { index: task.index, result, metrics }).is_err() {
            return;
        }
    }
}

struct Slot<T> {
    /// Its source's volume (whose queue it waits in).
    vol: usize,
    /// A copy's destination volume, when not the source's.
    dest: Option<usize>,
    skippable: bool,
    id: u64,
    run: Option<Box<dyn FnOnce() -> T + Send + 'static>>,
    state: Option<Arc<JobState>>,
    /// Bytes seen at the last tick: every byte of a job on one volume, else a copy's bytes on its
    /// source and on its destination.
    seen: [u64; 2],
    /// When it started (monotonic ns).
    started: u64,
    /// The volume it runs as the recovery job of.
    recovery: Option<usize>,
    outcome: Option<JobOutcome<T>>,
    metrics: Option<JobMetrics>,
}

struct Vol {
    key: VolumeKey,
    /// Jobs whose source is on it, waiting, in input order.
    queue: VecDeque<usize>,
    /// Running jobs holding a slot on it (as source or destination).
    running: Vec<usize>,
    /// Jobs that need it (as source or destination) and have no outcome yet.
    active: usize,
    /// Admission sequence of the last job started (0: none yet).
    last_start: u64,
    /// Nanoseconds without progress while not paused.
    silent: u64,
    /// Nanoseconds without progress since cancel.
    cancel_silent: u64,
    /// Stalled, or its recovery failed: no more jobs in this call.
    dead: bool,
    /// Its recovery job, running.
    recovery: Option<usize>,
    /// A recovery job of this call answered without data ([`Ended::Answered`]): the next one may
    /// start at once.
    answered: bool,
}

struct Call<'a, T> {
    ctx: &'a Arc<IoContext>,
    slots: Vec<Slot<T>>,
    vols: Vec<Vol>,
    workers: usize,
    succeeded: fn(&T) -> bool,
    cancel: &'a AtomicBool,
    done: &'a AtomicUsize,
    tasks_tx: Option<Sender<Task<T>>>,
    tasks_rx: Arc<Mutex<Receiver<Task<T>>>>,
    results_tx: Sender<Finished<T>>,
    results_rx: Receiver<Finished<T>>,
    /// Worker threads alive and not abandoned.
    live: usize,
    /// Jobs running (dispatched, not finished nor abandoned).
    busy: usize,
    seq: u64,
    cancelling: bool,
    /// Jobs with no outcome yet.
    remaining: usize,
    /// Monotonic ns spent running jobs on the calling thread since the last tick (not lateness).
    inline_ns: u64,
}

/// The volume of `path`'s index in `vols`, added when new.
fn vol_index(vols: &mut Vec<Vol>, key: VolumeKey) -> usize {
    match vols.iter().position(|v| v.key == key) {
        Some(v) => v,
        None => {
            vols.push(Vol {
                key,
                queue: VecDeque::new(),
                running: Vec::new(),
                active: 0,
                last_start: 0,
                silent: 0,
                cancel_silent: 0,
                dead: false,
                recovery: None,
                answered: false,
            });
            vols.len() - 1
        }
    }
}

/// Run `jobs` per volume, at most `workers` at once on one volume (today's `workers()`; a copy holds
/// a slot on its source's and its destination's volume), and while some job doesn't need a volume
/// that has work, at most `max(2, workers)` at once with that volume held to one fewer (R4a:
/// one card copying into one SSD runs `workers` copies; two cards into it hold each card to one
/// fewer, not the SSD). Results in input order; `done` counts every outcome but [`JobOutcome::NotStarted`] and
/// [`JobOutcome::Cancelled`]; `succeeded` says whether a job's result shows its volume works (a
/// probe that returned Ok), for the recovery rules. A single job runs on the calling thread;
/// once `cancel` is set no job starts, a single one included.
pub(crate) fn run_jobs<T: Send + 'static>(
    ctx: &Arc<IoContext>,
    jobs: Vec<IoJob<T>>,
    workers: usize,
    succeeded: fn(&T) -> bool,
    cancel: &AtomicBool,
    done: &AtomicUsize,
) -> Report<T> {
    // (nothing to do, e.g. an Add import's batch with no copies: no mount read, no threads)
    if jobs.is_empty() {
        return Report { outcomes: Vec::new(), metrics: Vec::new() };
    }
    if jobs.len() == 1 {
        let mut jobs = jobs;
        return match jobs.pop() {
            Some(job) => run_single(ctx, job, succeeded, cancel, done),
            None => Report { outcomes: Vec::new(), metrics: Vec::new() },
        };
    }
    let mounts = ctx.mounts.fresh(ctx.shared.clock.as_ref(), ctx.mount_wait);
    let mut vols: Vec<Vol> = Vec::new();
    let mut slots = Vec::with_capacity(jobs.len());
    for (index, job) in jobs.into_iter().enumerate() {
        let vol = vol_index(&mut vols, volume_key(&job.source, &mounts));
        let dest = job.dest.as_deref().map(|d| vol_index(&mut vols, volume_key(d, &mounts))).filter(|&d| d != vol);
        if let Some(v) = vols.get_mut(vol) {
            v.queue.push_back(index);
            v.active += 1;
        }
        if let Some(d) = dest.and_then(|d| vols.get_mut(d)) {
            d.active += 1;
        }
        slots.push(Slot {
            vol,
            dest,
            skippable: job.skippable,
            id: ctx.shared.next_id(),
            run: Some(job.run),
            state: None,
            seen: [0; 2],
            started: 0,
            recovery: None,
            outcome: None,
            metrics: None,
        });
    }
    let (tasks_tx, tasks_rx) = std::sync::mpsc::channel();
    let (results_tx, results_rx) = std::sync::mpsc::channel();
    let remaining = slots.len();
    let mut call = Call {
        ctx,
        slots,
        vols,
        workers: workers.max(1),
        succeeded,
        cancel,
        done,
        tasks_tx: Some(tasks_tx),
        tasks_rx: Arc::new(Mutex::new(tasks_rx)),
        results_tx,
        results_rx,
        live: 0,
        busy: 0,
        seq: 0,
        cancelling: false,
        remaining,
        inline_ns: 0,
    };
    call.run();
    let (outcomes, metrics): (Vec<_>, Vec<_>) = call.slots.into_iter().map(|s| (s.outcome.unwrap_or(JobOutcome::NotStarted), s.metrics)).unzip();
    log_metrics(&metrics);
    Report { outcomes, metrics }
}

/// A single job, on the calling thread as today (no threads, no mount read, no stall rule; cancel
/// set before it starts means it doesn't start, but can't end it once it runs). It still checks
/// the registry, through the cached mount list: on a blocked volume (a copy: either of its two) it
/// runs as the recovery job if one may run now, else fails at once. Start and end follow the
/// registry's one rule each ([`Shared::admission`], [`Shared::job_finished`]), as the call's
/// scheduler does.
fn run_single<T: Send + 'static>(
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
fn took(m: &JobMetrics) -> u64 {
    m.wall.saturating_sub(m.gate_wait)
}

/// Whether a tick `dm` ns of the monotonic clock after the last one, `inline` of them spent running
/// a job on the calling thread, came so late that the computer slept in between: more than
/// [`WAKE_JUMP`] past the tick's period, where the monotonic clock counts sleep
/// ([`IoContext::late_tick_is_wake`]).
fn woke_late(ctx: &IoContext, dm: u64, inline: u64) -> bool {
    let period = u64::try_from(ctx.tick.as_nanos()).unwrap_or(u64::MAX);
    ctx.late_tick_is_wake && dm.saturating_sub(inline) > period.saturating_add(WAKE_JUMP)
}

/// Job `slot`'s side on volume `v`: `None` when it isn't a copy between two volumes (all its I/O
/// is on `v`).
fn side_on<T>(slot: &Slot<T>, v: usize) -> Option<Side> {
    slot.dest.map(|_| if slot.vol == v { Side::Source } else { Side::Dest })
}

/// Per-volume totals of a call (R9): debug log, and stderr under `<PREFIX>_PROFILE`.
fn log_metrics(metrics: &[Option<JobMetrics>]) {
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

impl<T: Send + 'static> Call<'_, T> {
    fn clock_now(&self) -> u64 {
        self.ctx.shared.clock.mono()
    }

    fn run(&mut self) {
        let clock = self.ctx.shared.clock.clone();
        let (mut prev_mono, mut prev_wall) = (clock.mono(), clock.wall());
        loop {
            self.admit(clock.mono());
            if self.remaining == 0 {
                break;
            }
            match self.results_rx.recv_timeout(self.ctx.tick) {
                Ok(f) => {
                    self.finish(f, clock.mono());
                    while let Ok(f) = self.results_rx.try_recv() {
                        self.finish(f, clock.mono());
                    }
                }
                Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => {}
            }
            let (now, wall) = (clock.mono(), clock.wall());
            let (dm, dw) = (now.saturating_sub(prev_mono), wall.saturating_sub(prev_wall));
            let late = woke_late(self.ctx, dm, std::mem::take(&mut self.inline_ns));
            if dw > dm.saturating_add(WAKE_JUMP) || late {
                log::info!("io: the computer woke from sleep: every volume gets a grace period");
                self.ctx.shared.note_wake(now);
            }
            (prev_mono, prev_wall) = (now, wall);
            // (a late tick's gap was the sleep, not the volumes' silence)
            self.tick(now, if late { 0 } else { dm });
            self.ctx.ticks.fetch_add(1, Ordering::Relaxed);
            if self.remaining == 0 {
                break;
            }
        }
        // idle workers see the channel close and exit
        self.tasks_tx = None;
    }

    fn set_outcome(&mut self, i: usize, outcome: JobOutcome<T>, metrics: Option<JobMetrics>) {
        let Some(slot) = self.slots.get_mut(i) else { return };
        if slot.outcome.is_some() {
            return;
        }
        if !matches!(outcome, JobOutcome::NotStarted | JobOutcome::Cancelled) {
            self.done.fetch_add(1, Ordering::Relaxed);
        }
        slot.outcome = Some(outcome);
        if metrics.is_some() {
            slot.metrics = metrics;
        }
        for v in [Some(slot.vol), slot.dest].into_iter().flatten() {
            if let Some(vol) = self.vols.get_mut(v) {
                vol.active = vol.active.saturating_sub(1);
            }
        }
        self.remaining = self.remaining.saturating_sub(1);
    }

    /// Whether volume `v` is held to one fewer than the call cap (R4a): it has work and some job
    /// with no outcome yet doesn't need it (a job on another volume, or a copy between two others).
    fn cut(&self, v: &Vol) -> bool {
        v.active > 0 && self.remaining > v.active
    }

    /// The call cap now (§5.3): `workers`, or at least 2 while some volume is cut (R4a).
    fn cap(&self) -> usize {
        if self.vols.iter().any(|v| self.cut(v)) { self.workers.max(2) } else { self.workers }
    }

    /// Volume `v`'s limit under call cap `cap`.
    fn limit(&self, v: usize, cap: usize) -> usize {
        match self.vols.get(v) {
            Some(vol) if self.cut(vol) => (cap - 1).max(1),
            _ => cap,
        }
    }

    /// Whether volume `v` can take one more running job now (not dead, not waiting on its
    /// recovery job, not blocked, under its limit).
    fn has_slot(&self, v: usize, cap: usize, blocked: &[bool]) -> bool {
        self.vols.get(v).is_some_and(|vol| {
            !vol.dead && vol.recovery.is_none() && !blocked.get(v).copied().unwrap_or(true) && vol.running.len() < self.limit(v, cap)
        })
    }

    /// The first queued job, in input order, that needs volume `v` (its own queue's front, or a
    /// copy into it waiting in another volume's queue).
    fn first_queued_needing(&self, v: usize) -> Option<usize> {
        let own = self.vols.get(v).and_then(|vol| vol.queue.front().copied());
        let into = self.vols.iter().filter_map(|vol| vol.queue.iter().copied().find(|&i| self.slots.get(i).is_some_and(|s| s.dest == Some(v)))).min();
        own.into_iter().chain(into).min()
    }

    /// Take job `i` out of its volume's queue.
    fn unqueue(&mut self, i: usize) {
        let Some(v) = self.slots.get(i).map(|s| s.vol) else { return };
        if let Some(vol) = self.vols.get_mut(v)
            && let Some(at) = vol.queue.iter().position(|&q| q == i)
        {
            let _ = vol.queue.remove(at);
        }
    }

    fn admit(&mut self, now: u64) {
        if self.cancelling || self.cancel.load(Ordering::Relaxed) {
            self.begin_cancel();
            return;
        }
        // blocked volumes: hold while their running jobs may show they work, else one recovery job
        // first (or fail at once when none may run)
        let mut blocked = vec![false; self.vols.len()];
        for v in 0..self.vols.len() {
            let Some(vol) = self.vols.get(v) else { continue };
            // (queued jobs that need it: those with no outcome that aren't running)
            if vol.dead || vol.active <= vol.running.len() || vol.recovery.is_some() {
                continue;
            }
            if !self.ctx.shared.blocked(&vol.key, now) {
                continue;
            }
            if let Some(b) = blocked.get_mut(v) {
                *b = true;
            }
            let cap = self.cap();
            if !self.vols.get(v).is_some_and(|vol| vol.running.is_empty()) || self.busy >= cap {
                continue;
            }
            let Some(i) = self.first_queued_needing(v) else { continue };
            // (a copy's other volume needs a free slot too: else it waits)
            let other = self.slots.get(i).map(|s| if s.vol == v { s.dest } else { Some(s.vol) }).unwrap_or(None);
            if let Some(o) = other
                && !self.vols.get(o).is_some_and(|ov| !ov.dead && ov.running.len() < self.limit(o, cap))
            {
                continue;
            }
            let (key, answered, id) = (
                self.vols.get(v).map(|vol| vol.key.clone()),
                self.vols.get(v).is_some_and(|vol| vol.answered),
                self.slots.get(i).map_or(0, |s| s.id),
            );
            let Some(key) = key else { continue };
            let shared = &self.ctx.shared;
            let may = if answered { shared.continue_recovery(&key, id, now) } else { shared.try_start_recovery(&key, id, now) };
            if may {
                self.start_recovery(v, i, now);
            } else {
                log::info!("io: volume {key} is not responding: its files fail");
                self.fail_queue(v);
            }
        }
        loop {
            let cap = self.cap();
            if self.busy >= cap {
                break;
            }
            // round robin over the volumes' queues; a queue's front starts when its source and
            // its destination both have a slot (input order within a volume)
            let pick = self
                .vols
                .iter()
                .enumerate()
                .filter(|(v, vol)| {
                    let Some(&i) = vol.queue.front() else { return false };
                    self.has_slot(*v, cap, &blocked) && self.slots.get(i).and_then(|s| s.dest).is_none_or(|d| self.has_slot(d, cap, &blocked))
                })
                .min_by_key(|(v, vol)| (vol.last_start, *v))
                .map(|(v, _)| v);
            let Some(v) = pick else { break };
            let Some(i) = self.vols.get_mut(v).and_then(|vol| vol.queue.pop_front()) else { break };
            if !self.dispatch(i, now) {
                if let Some(vol) = self.vols.get_mut(v) {
                    vol.queue.push_front(i);
                }
                break;
            }
        }
    }

    fn start_recovery(&mut self, v: usize, i: usize, now: u64) {
        self.unqueue(i);
        let Some(vol) = self.vols.get_mut(v) else { return };
        vol.recovery = Some(i);
        let key = vol.key.clone();
        if let Some(s) = self.slots.get_mut(i) {
            s.recovery = Some(v);
        }
        log::info!("io: volume {key} was not responding: trying one file first");
        if !self.dispatch(i, now) {
            if let Some(vol) = self.vols.get_mut(v) {
                vol.recovery = None;
            }
            // (back to its place in its own queue: input order)
            if let Some(s) = self.slots.get_mut(i) {
                s.recovery = None;
                self.ctx.shared.end_recovery(&key, s.id);
                let home = s.vol;
                if let Some(q) = self.vols.get_mut(home).map(|h| &mut h.queue) {
                    let at = q.iter().position(|&x| x > i).unwrap_or(q.len());
                    q.insert(at, i);
                }
            }
        }
    }

    /// Start job `i` on a free worker (spawning one if none is idle); `false` if no worker can take
    /// it now (a spawn failed while others exist: it runs later on one of them). With no worker at
    /// all it runs here, on the calling thread (the bookkeeping stops meanwhile).
    fn dispatch(&mut self, i: usize, now: u64) -> bool {
        let mut inline = false;
        if self.live <= self.busy {
            let injected = self.ctx.spawn_budget.try_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_sub(1)).is_err();
            let spawned = !injected && {
                let (rx, tx, shared) = (self.tasks_rx.clone(), self.results_tx.clone(), self.ctx.shared.clone());
                std::thread::Builder::new().name(format!("lc-io-{}", self.live)).spawn(move || worker(rx, tx, shared)).is_ok()
            };
            if spawned {
                self.live += 1;
            } else if self.live == 0 {
                log::warn!("io: no thread could be started: a file is read on the calling thread");
                inline = true;
            } else {
                return false;
            }
        }
        let Some(slot) = self.slots.get_mut(i) else { return false };
        let Some(run) = slot.run.take() else { return false };
        let (home, dest) = (slot.vol, slot.dest);
        let Some(key) = self.vols.get(home).map(|v| v.key.clone()) else { return false };
        let dest_key = dest.and_then(|d| self.vols.get(d)).map(|d| d.key.clone());
        let shared = &self.ctx.shared;
        let state = Arc::new(JobState::new(slot.id, key, shared.clock.clone(), Arc::downgrade(shared)));
        slot.state = Some(state.clone());
        slot.seen = [0; 2];
        slot.started = now;
        self.seq += 1;
        for v in [Some(home), dest].into_iter().flatten() {
            let Some(vol) = self.vols.get_mut(v) else { continue };
            shared.job_started(&vol.key, now);
            vol.running.push(i);
            if vol.running.len() == 1 {
                vol.silent = 0;
                vol.cancel_silent = 0;
            }
        }
        if let Some(vol) = self.vols.get_mut(home) {
            vol.last_start = self.seq;
        }
        self.busy += 1;
        let task = Task { index: i, run, job: state, dest: dest_key };
        let task = match (&self.tasks_tx, inline) {
            (Some(tx), false) => match tx.send(task) {
                Ok(()) => return true,
                Err(e) => e.0,
            },
            _ => task,
        };
        // inline: on the calling thread
        let (result, metrics) = execute(task.run, &task.job, task.dest);
        let _ = task.job.try_finish();
        let now = self.clock_now();
        self.inline_ns = self.inline_ns.saturating_add(metrics.wall);
        self.finish(Finished { index: i, result, metrics }, now);
        true
    }

    fn finish(&mut self, f: Finished<T>, now: u64) {
        let Some(slot) = self.slots.get_mut(f.index) else { return };
        if slot.outcome.is_some() {
            return;
        }
        let (vols, id, recovery) = ([Some(slot.vol), slot.dest], slot.id, slot.recovery);
        self.busy = self.busy.saturating_sub(1);
        let moved = f.metrics.bytes > 0;
        let ok = f.result.as_ref().is_ok_and(self.succeeded);
        // (a copy's completion is progress on both of its volumes)
        for v in vols.into_iter().flatten() {
            let Some(vol) = self.vols.get_mut(v) else { continue };
            vol.running.retain(|&r| r != f.index);
            vol.silent = 0;
            vol.cancel_silent = 0;
            let ended = self.ctx.shared.job_finished(&vol.key, now, took(&f.metrics), moved, ok, (recovery == Some(v)).then_some(id));
            if recovery == Some(v) {
                vol.recovery = None;
                // (answered without data: the next queued job goes as the next recovery job)
                vol.answered |= ended == Ended::Answered;
            }
        }
        let outcome = match f.result {
            Ok(t) => JobOutcome::Done(t),
            Err(m) => JobOutcome::Panicked(m),
        };
        self.set_outcome(f.index, outcome, Some(f.metrics));
    }

    /// Every queued job that needs volume `v` (from its queue, or a copy into it from another)
    /// fails "volume not responding", and it takes no more jobs.
    fn fail_queue(&mut self, v: usize) {
        let Some(vol) = self.vols.get_mut(v) else { return };
        vol.dead = true;
        let key = vol.key.clone();
        let mut queued: Vec<usize> = vol.queue.drain(..).collect();
        let slots = &self.slots;
        for (o, other) in self.vols.iter_mut().enumerate() {
            if o != v {
                other.queue.retain(|&i| {
                    let into = slots.get(i).is_some_and(|s| s.dest == Some(v));
                    if into {
                        queued.push(i);
                    }
                    !into
                });
            }
        }
        queued.sort_unstable();
        for i in queued {
            self.set_outcome(i, JobOutcome::VolumeNotResponding { volume: key.clone(), abandoned: false }, None);
        }
    }

    /// Leave running job `i` behind on volume `v` (the one stalled or silent after cancel; a copy
    /// leaves both of its volumes); `false` if it finished first (its result is on its way).
    fn abandon(&mut self, i: usize, v: usize, kind: AbandonKind, now: u64) -> bool {
        let Some(slot) = self.slots.get(i).filter(|s| s.outcome.is_none()) else { return false };
        let Some(state) = slot.state.clone() else { return false };
        let (vols, started) = ([Some(slot.vol), slot.dest], slot.started);
        let Some(key) = self.vols.get(v).map(|vol| vol.key.clone()) else { return false };
        let shared = &self.ctx.shared;
        shared.add_entry(&state, &key, kind, now);
        if !state.try_abandon() {
            shared.drop_entry(state.id);
            return false;
        }
        self.busy = self.busy.saturating_sub(1);
        self.live = self.live.saturating_sub(1);
        let mut dest = None;
        for w in vols.into_iter().flatten() {
            let Some(vol) = self.vols.get_mut(w) else { continue };
            shared.abandoned(&vol.key, now);
            vol.running.retain(|&r| r != i);
            if vol.key != state.volume {
                dest = Some(vol.key.clone());
            }
        }
        let moved = state.moved();
        let metrics = JobMetrics {
            volume: state.volume.clone(),
            dest,
            bytes: moved.total(),
            moved,
            wall: now.saturating_sub(started),
            gate_wait: 0,
            gate_failure: state.gate_failure(),
        };
        let outcome = match kind {
            AbandonKind::Stall => JobOutcome::VolumeNotResponding { volume: key, abandoned: true },
            AbandonKind::Cancel => JobOutcome::Cancelled,
        };
        self.set_outcome(i, outcome, Some(metrics));
        true
    }

    fn begin_cancel(&mut self) {
        if self.cancelling {
            return;
        }
        self.cancelling = true;
        let queued: Vec<usize> = self.vols.iter_mut().flat_map(|v| v.queue.drain(..)).collect();
        for i in queued {
            self.set_outcome(i, JobOutcome::NotStarted, None);
        }
        for v in &mut self.vols {
            v.cancel_silent = 0;
        }
    }

    /// The bookkeeping tick: progress, the stall rule, cancel's rules.
    fn tick(&mut self, now: u64, dt: u64) {
        if !self.cancelling && self.cancel.load(Ordering::Relaxed) {
            self.begin_cancel();
        }
        // a job's bytes since the last tick are progress on the volume they moved on (a copy's
        // reads on its source, its writes and read-back on its destination); a copy whose current
        // call is on its other volume waits there, not here: it pauses this one's clock
        let mut silent: Vec<(u64, usize)> = Vec::new();
        for v in 0..self.vols.len() {
            let Some(vol) = self.vols.get(v) else { continue };
            if vol.running.is_empty() {
                continue;
            }
            let key = vol.key.clone();
            let stall_time = self.ctx.shared.stall_time(&key);
            let mut progressed = false;
            let mut paused_all = true;
            for &i in &vol.running {
                let Some(slot) = self.slots.get_mut(i) else { continue };
                let Some(st) = slot.state.clone() else { continue };
                let side = side_on(slot, v);
                let (bytes, at, seen) = match side {
                    None => (st.bytes(), st.last_bytes(), slot.seen.get_mut(0)),
                    Some(s @ Side::Source) => (st.moved().on(s), st.last_bytes_on(s), slot.seen.get_mut(0)),
                    Some(s @ Side::Dest) => (st.moved().on(s), st.last_bytes_on(s), slot.seen.get_mut(1)),
                };
                if let Some(seen) = seen
                    && bytes > *seen
                {
                    *seen = bytes;
                    progressed = true;
                    self.ctx.shared.bytes_seen(&key, at);
                }
                let computing = st.computing_for(now).is_some_and(|s| s <= stall_time.saturating_mul(COMPUTING_FACTOR));
                let elsewhere = side.is_some_and(|s| st.on() == Some(s.other()));
                paused_all &= st.at_gate() || computing || elsewhere;
            }
            let cancelling = self.cancelling;
            let Some(vol) = self.vols.get_mut(v) else { continue };
            if progressed {
                vol.silent = 0;
                vol.cancel_silent = 0;
            } else {
                if !paused_all {
                    vol.silent = vol.silent.saturating_add(dt);
                }
                if cancelling {
                    vol.cancel_silent = vol.cancel_silent.saturating_add(dt);
                }
            }
            silent.push((vol.silent, v));
        }
        // the volume silent longest is judged first: a copy stuck on one of its volumes leaves
        // the other when that one is declared, so the other isn't declared with it unless it is
        // silent as long (a copy whose stuck call named no volume pauses neither: then the
        // source, created first, goes first)
        silent.sort_by_key(|&(s, v)| (std::cmp::Reverse(s), v));
        for (_, v) in silent {
            let Some(vol) = self.vols.get(v) else { continue };
            if vol.running.is_empty() {
                continue;
            }
            let key = vol.key.clone();
            let running = vol.running.clone();
            let stall_time = self.ctx.shared.stall_time(&key);
            let (silent, cancel_silent) = (vol.silent, vol.cancel_silent);
            if silent >= stall_time && !self.ctx.shared.wake_holds(now) {
                log::warn!("io: volume {key} moved nothing for {} s: not responding", stall_time / 1_000_000_000);
                for i in running {
                    self.abandon(i, v, AbandonKind::Stall, now);
                }
                self.fail_queue(v);
            } else if self.cancelling && cancel_silent >= CANCEL_SILENCE {
                for i in running {
                    self.abandon(i, v, AbandonKind::Cancel, now);
                }
            } else if self.cancelling {
                // a job still waiting at the gate has read nothing: it ends at once, unread; a
                // copy waiting for its DNG step is told to skip it instead (it keeps its raw copy)
                for i in running {
                    let Some(slot) = self.slots.get(i) else { continue };
                    let Some(st) = slot.state.clone() else { continue };
                    if !st.at_gate() {
                        continue;
                    }
                    if slot.skippable {
                        st.request_skip();
                    } else {
                        self.abandon(i, v, AbandonKind::Cancel, now);
                    }
                }
            }
        }
    }
}
