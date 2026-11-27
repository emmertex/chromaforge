//! One call's scheduler (PR 1, §5.2–5.3 of the I/O spec): per-volume queues, round-robin
//! admission, today's limits, the stall rule, cancel, and the abandoned-job registry's block and
//! recovery rules. The calling thread does the bookkeeping, waking at every result and at least
//! every tick; worker threads run the jobs and send results over a channel, so a job that never
//! returns can be left behind.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, PoisonError};

use lcc_catalog::folders::{VolumeKey, volume_key};

use super::IoContext;
use super::job::JobState;
use super::shared::{AbandonKind, Admission, CANCEL_SILENCE, COMPUTING_FACTOR, Ended, Shared, WAKE_JUMP};

/// One unit of file work: the file it reads (its volume decides its queue) and the work.
pub(crate) struct IoJob<T> {
    pub(crate) source: String,
    pub(crate) run: Box<dyn FnOnce() -> T + Send + 'static>,
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
    /// Its volume stopped responding: abandoned while running, failed while queued, or failed at
    /// once because the volume is blocked.
    VolumeNotResponding { volume: VolumeKey },
    /// The job panicked (the message says how).
    Panicked(String),
}

/// What one job did (R9): bytes it read, its time, its wait at the work gate.
#[derive(Clone, Debug)]
pub(crate) struct JobMetrics {
    pub(crate) volume: VolumeKey,
    pub(crate) bytes: u64,
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
}

struct Finished<T> {
    index: usize,
    result: Result<T, String>,
    metrics: JobMetrics,
}

/// Run `run` as `job` on this thread, under the panic guard.
fn execute<T>(run: Box<dyn FnOnce() -> T + Send + 'static>, job: &Arc<JobState>) -> (Result<T, String>, JobMetrics) {
    let (t0, w0) = (job.now(), crate::memory::gate_wait_nanos());
    let result = crate::memory::with_job(Some(job.clone()), || crate::guard::catch("reading the file", run));
    let metrics = JobMetrics {
        volume: job.volume.clone(),
        bytes: job.bytes(),
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
        let (result, metrics) = execute(task.run, &task.job);
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
    vol: usize,
    id: u64,
    run: Option<Box<dyn FnOnce() -> T + Send + 'static>>,
    state: Option<Arc<JobState>>,
    /// Bytes seen at the last tick.
    seen: u64,
    /// When it started (monotonic ns).
    started: u64,
    recovery: bool,
    outcome: Option<JobOutcome<T>>,
    metrics: Option<JobMetrics>,
}

struct Vol {
    key: VolumeKey,
    queue: VecDeque<usize>,
    running: Vec<usize>,
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
    remaining: usize,
    /// Monotonic ns spent running jobs on the calling thread since the last tick (not lateness).
    inline_ns: u64,
}

/// Run `jobs` per volume, at most `workers` at once on one volume (today's `workers()`), and when
/// two or more volumes have work at most `max(2, workers)` at once with each volume held to one
/// fewer. Results in input order; `done` counts every outcome but [`JobOutcome::NotStarted`] and
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
        let key = volume_key(&job.source, &mounts);
        let vol = match vols.iter().position(|v| v.key == key) {
            Some(v) => v,
            None => {
                vols.push(Vol {
                    key,
                    queue: VecDeque::new(),
                    running: Vec::new(),
                    last_start: 0,
                    silent: 0,
                    cancel_silent: 0,
                    dead: false,
                    recovery: None,
                    answered: false,
                });
                vols.len() - 1
            }
        };
        if let Some(v) = vols.get_mut(vol) {
            v.queue.push_back(index);
        }
        slots.push(Slot {
            vol,
            id: ctx.shared.next_id(),
            run: Some(job.run),
            state: None,
            seen: 0,
            started: 0,
            recovery: false,
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
/// the registry, through the cached mount list: on a blocked volume it runs as the recovery job if
/// one may run now, else fails at once. Start and end follow the registry's one rule each
/// ([`Shared::admission`], [`Shared::job_finished`]), as the call's scheduler does.
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
    let key = volume_key(&job.source, &ctx.mounts.cached());
    let now = shared.clock.mono();
    let id = shared.next_id();
    let recovery = match shared.admission(&key, id, now) {
        Admission::Free => false,
        Admission::Recovery => true,
        Admission::Refused => {
            done.fetch_add(1, Ordering::Relaxed);
            return Report { outcomes: vec![JobOutcome::VolumeNotResponding { volume: key }], metrics: vec![None] };
        }
    };
    let state = Arc::new(JobState::new(id, key.clone(), shared.clock.clone(), Arc::downgrade(shared)));
    shared.job_started(&key, now);
    let (result, metrics) = execute(job.run, &state);
    let _ = state.try_finish();
    let ok = result.as_ref().is_ok_and(succeeded);
    let _ = shared.job_finished(&key, shared.clock.mono(), took(&metrics), metrics.bytes > 0, ok, recovery.then_some(id));
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

/// Per-volume totals of a call (R9): debug log, and stderr under `<PREFIX>_PROFILE`.
fn log_metrics(metrics: &[Option<JobMetrics>]) {
    let mut per: Vec<(&VolumeKey, u64, u64, u64, u64, u64)> = Vec::new();
    for m in metrics.iter().flatten() {
        let row = match per.iter().position(|r| r.0 == &m.volume) {
            Some(i) => per.get_mut(i),
            None => {
                per.push((&m.volume, 0, 0, 0, 0, 0));
                per.last_mut()
            }
        };
        if let Some(r) = row {
            r.1 = r.1.saturating_add(1);
            r.2 = r.2.saturating_add(m.bytes);
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
        self.remaining = self.remaining.saturating_sub(1);
    }

    /// The call cap and per-volume limit now (§5.3).
    fn limits(&self) -> (usize, usize) {
        let active = self.vols.iter().filter(|v| !v.dead && (!v.queue.is_empty() || !v.running.is_empty())).count();
        if active >= 2 {
            let cap = self.workers.max(2);
            (cap, (cap - 1).max(1))
        } else {
            (self.workers, self.workers)
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
        for (v, b) in blocked.iter_mut().enumerate() {
            let Some(vol) = self.vols.get(v) else { continue };
            if vol.dead || vol.queue.is_empty() || vol.recovery.is_some() {
                continue;
            }
            if !self.ctx.shared.blocked(&vol.key, now) {
                continue;
            }
            *b = true;
            if !vol.running.is_empty() || self.busy >= self.limits().0 {
                continue;
            }
            let Some(&i) = vol.queue.front() else { continue };
            let id = self.slots.get(i).map_or(0, |s| s.id);
            let shared = &self.ctx.shared;
            let may = if vol.answered { shared.continue_recovery(&vol.key, id, now) } else { shared.try_start_recovery(&vol.key, id, now) };
            if may {
                self.start_recovery(v, i, now);
            } else {
                log::info!("io: volume {} is not responding: its files fail", vol.key);
                self.fail_queue(v);
            }
        }
        loop {
            let (cap, limit) = self.limits();
            if self.busy >= cap {
                break;
            }
            let pick = self
                .vols
                .iter()
                .enumerate()
                .filter(|(v, vol)| {
                    !vol.dead
                        && !vol.queue.is_empty()
                        && vol.running.len() < limit
                        && vol.recovery.is_none()
                        && !blocked.get(*v).copied().unwrap_or(true)
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
        let Some(vol) = self.vols.get_mut(v) else { return };
        let _ = vol.queue.pop_front();
        vol.recovery = Some(i);
        let key = vol.key.clone();
        if let Some(s) = self.slots.get_mut(i) {
            s.recovery = true;
        }
        log::info!("io: volume {key} was not responding: trying one file first");
        if !self.dispatch(i, now) {
            if let Some(vol) = self.vols.get_mut(v) {
                vol.queue.push_front(i);
                vol.recovery = None;
            }
            if let Some(s) = self.slots.get_mut(i) {
                s.recovery = false;
                self.ctx.shared.end_recovery(&key, s.id);
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
        let Some(vol) = self.vols.get_mut(slot.vol) else { return false };
        let shared = &self.ctx.shared;
        let state = Arc::new(JobState::new(slot.id, vol.key.clone(), shared.clock.clone(), Arc::downgrade(shared)));
        shared.job_started(&vol.key, now);
        vol.running.push(i);
        if vol.running.len() == 1 {
            vol.silent = 0;
            vol.cancel_silent = 0;
        }
        self.seq += 1;
        vol.last_start = self.seq;
        slot.state = Some(state.clone());
        slot.seen = 0;
        slot.started = now;
        self.busy += 1;
        let task = Task { index: i, run, job: state };
        let task = match (&self.tasks_tx, inline) {
            (Some(tx), false) => match tx.send(task) {
                Ok(()) => return true,
                Err(e) => e.0,
            },
            _ => task,
        };
        // inline: on the calling thread
        let (result, metrics) = execute(task.run, &task.job);
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
        let (v, id, recovery) = (slot.vol, slot.id, slot.recovery);
        self.busy = self.busy.saturating_sub(1);
        let Some(vol) = self.vols.get_mut(v) else { return };
        vol.running.retain(|&r| r != f.index);
        vol.silent = 0;
        vol.cancel_silent = 0;
        let key = vol.key.clone();
        let moved = f.metrics.bytes > 0;
        let ok = f.result.as_ref().is_ok_and(self.succeeded);
        let ended = self.ctx.shared.job_finished(&key, now, took(&f.metrics), moved, ok, recovery.then_some(id));
        if recovery && let Some(vol) = self.vols.get_mut(v) {
            vol.recovery = None;
            // (answered without data: the next queued job goes as the next recovery job)
            vol.answered |= ended == Ended::Answered;
        }
        let outcome = match f.result {
            Ok(t) => JobOutcome::Done(t),
            Err(m) => JobOutcome::Panicked(m),
        };
        self.set_outcome(f.index, outcome, Some(f.metrics));
    }

    /// Every queued job of volume `v` fails "volume not responding", and it takes no more jobs.
    fn fail_queue(&mut self, v: usize) {
        let Some(vol) = self.vols.get_mut(v) else { return };
        vol.dead = true;
        let key = vol.key.clone();
        let queued: Vec<usize> = vol.queue.drain(..).collect();
        for i in queued {
            self.set_outcome(i, JobOutcome::VolumeNotResponding { volume: key.clone() }, None);
        }
    }

    /// Leave running job `i` behind; `false` if it finished first (its result is on its way).
    fn abandon(&mut self, i: usize, kind: AbandonKind, now: u64) -> bool {
        let Some(slot) = self.slots.get(i) else { return false };
        let Some(state) = slot.state.clone() else { return false };
        let (v, started) = (slot.vol, slot.started);
        let shared = &self.ctx.shared;
        shared.add_entry(&state, kind, now);
        if !state.try_abandon() {
            shared.drop_entry(state.id);
            return false;
        }
        shared.abandoned(&state.volume, now);
        self.busy = self.busy.saturating_sub(1);
        self.live = self.live.saturating_sub(1);
        if let Some(vol) = self.vols.get_mut(v) {
            vol.running.retain(|&r| r != i);
        }
        let metrics = JobMetrics {
            volume: state.volume.clone(),
            bytes: state.bytes(),
            wall: now.saturating_sub(started),
            gate_wait: 0,
            gate_failure: state.gate_failure(),
        };
        let outcome = match kind {
            AbandonKind::Stall => JobOutcome::VolumeNotResponding { volume: state.volume.clone() },
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
        for v in 0..self.vols.len() {
            let Some(vol) = self.vols.get(v) else { continue };
            if vol.running.is_empty() {
                continue;
            }
            let key = vol.key.clone();
            let running = vol.running.clone();
            let stall_time = self.ctx.shared.stall_time(&key);
            let mut progressed = false;
            let mut paused_all = true;
            let mut at_gate = Vec::new();
            for &i in &running {
                let Some(slot) = self.slots.get_mut(i) else { continue };
                let Some(st) = &slot.state else { continue };
                let b = st.bytes();
                if b > slot.seen {
                    slot.seen = b;
                    progressed = true;
                    self.ctx.shared.bytes_seen(&key, st.last_bytes());
                }
                let gate = st.at_gate();
                if gate {
                    at_gate.push(i);
                }
                let computing = st.computing_for(now).is_some_and(|s| s <= stall_time.saturating_mul(COMPUTING_FACTOR));
                paused_all &= gate || computing;
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
            let (silent, cancel_silent) = (vol.silent, vol.cancel_silent);
            if silent >= stall_time && !self.ctx.shared.wake_holds(now) {
                log::warn!("io: volume {key} moved nothing for {} s: not responding", stall_time / 1_000_000_000);
                for i in running {
                    self.abandon(i, AbandonKind::Stall, now);
                }
                self.fail_queue(v);
            } else if cancelling && cancel_silent >= CANCEL_SILENCE {
                for i in running {
                    self.abandon(i, AbandonKind::Cancel, now);
                }
            } else if cancelling {
                // a job still waiting at the gate has read nothing: it ends at once, unread
                for i in at_gate {
                    self.abandon(i, AbandonKind::Cancel, now);
                }
            }
        }
    }
}
