//! The scheduler (§5.2–5.5 of the I/O spec): per-volume queues, round-robin admission, today's
//! limits, the stall rule, cancel, and the abandoned-job registry's block and recovery rules, over
//! every call it holds. One runs for the whole process in the pool's supervisor thread
//! ([`super::pool`]), so every call's traffic on a volume is seen in one place (PR 3); a call that
//! can't reach the pool runs a private one on its own thread, as in PR 2.
//!
//! Jobs are type-erased ([`PoolTask`]): a job sends its own result to its call's channel, so the
//! scheduler never sees `T`; it sends the outcomes only it can give (not started, cancelled,
//! volume not responding, scheduler stopped). A finished job's result is handed back to the
//! scheduler first ([`Msg::Ended`]) and delivered after the bookkeeping, so a caller that sees a
//! result also sees what the job did to the volume's history.
//!
//! Limits (PR 5, §5.7): the pool runs at most the I/O cap ([`IoContext::io_cap`], 32) jobs at
//! once; each volume's own limit is learned by the controller ([`super::limits`]), starting from
//! its calls' `workers()` (today's limit); while some job with no outcome doesn't need a volume,
//! that volume is held to one fewer than the cap (R4a). Calls have no cap of their own any more.
//! Within a volume, calls take turns (the call that started a job on it longest ago goes first);
//! within a call, jobs start in input order.
//!
//! What the controller measures ([`measured_bytes`]): a probe's bytes read, on its source volume;
//! a copy's bytes written, on its destination only (§5.4: the rate a destination is judged on is
//! what it wrote; a copy's source read mostly comes from the page cache, and its verify read-back
//! would count the copy twice), counted as they move (at each tick) and when the job ends; a job's
//! completion likewise.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use lcc_catalog::folders::VolumeKey;

use super::IoContext;
use super::job::{JobState, Moved, Side};
use super::limits::{self, AbortReason, Controller, Counters, Demand, Event, Phase};
use super::run::JobMetrics;
use super::shared::{AbandonKind, CANCEL_SILENCE, COMPUTING_FACTOR, Ended, Learned, Shared, WAKE_JUMP};

/// A call's id within its scheduler.
pub(crate) type CallId = u64;

thread_local! {
    /// This thread is a scheduler worker (§5.5: a job never submits several jobs and waits).
    static WORKER: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Whether this thread is one of the scheduler's workers. A job running on one must not submit
/// several jobs and wait for them: it would hold a slot while waiting for one.
pub(crate) fn on_worker_thread() -> bool {
    WORKER.with(std::cell::Cell::get)
}

/// The outcomes only the scheduler gives (every one but a job's own result).
#[derive(Clone, Debug)]
pub(crate) enum SchedOutcome {
    NotStarted,
    Cancelled,
    VolumeNotResponding { volume: VolumeKey, abandoned: bool },
    SchedulerStopped,
}

/// A finished job's result on its way to its call: sent when delivered, and when dropped (so a
/// result reaches its caller even when the scheduler that should forward it has stopped).
pub(crate) trait Deliver: Send {
    fn deliver(self: Box<Self>);
}

/// What running a job gave the scheduler.
pub(crate) struct Ran {
    /// Its result shows its volumes work (a probe returned Ok), for the recovery rule.
    pub(crate) ok: bool,
    pub(crate) metrics: JobMetrics,
    /// Its result, `None` when the scheduler had abandoned it (the result is dropped).
    pub(crate) deliver: Option<Box<dyn Deliver>>,
}

/// One submitted job, its type erased. A task dropped without running or failing tells its call
/// "scheduler stopped" (the supervisor panicked with it queued).
pub(crate) trait PoolTask: Send {
    /// Run it as `state` on this thread.
    fn run(self: Box<Self>, state: &Arc<JobState>) -> Ran;
    /// It never ran: tell its call `outcome`.
    fn fail(self: Box<Self>, outcome: SchedOutcome);
    /// Drop it silently (its outcome was given another way).
    fn discard(self: Box<Self>);
    /// No worker thread exists: hand it to its call's own thread; `false` if the call is gone.
    fn run_here(self: Box<Self>, inline: InlineInfo) -> bool;
}

/// Where a job run on its call's thread reports back.
pub(crate) struct InlineInfo {
    pub(crate) state: Arc<JobState>,
    pub(crate) events: Sender<Msg>,
    pub(crate) call: CallId,
    pub(crate) pos: usize,
}

/// A job handed to its call's thread (no worker could be started).
pub(crate) struct Inline {
    pub(crate) task: Box<dyn PoolTask>,
    pub(crate) info: InlineInfo,
}

impl Inline {
    /// Run it here, unless the scheduler abandoned it meanwhile, and tell the scheduler.
    pub(crate) fn run(self) {
        let Inline { task, info } = self;
        let ran = if info.state.is_abandoned() {
            task.discard();
            Ran { ok: false, metrics: JobMetrics::none(&info.state), deliver: None }
        } else {
            task.run(&info.state)
        };
        if ran.deliver.is_none()
            && let Some(shared) = info.state.shared()
        {
            shared.job_returned(&info.state);
        }
        // (a stopped scheduler: the result is delivered as the message drops)
        let _ = info.events.send(Msg::Ended { call: info.call, pos: info.pos, ran });
    }
}

/// Where the scheduler sends a call's outcomes for jobs that were running (abandoned ones).
pub(crate) trait CallSink: Send {
    fn outcome(&self, index: usize, outcome: SchedOutcome, metrics: Option<JobMetrics>);
}

/// A job as submitted: its volumes already keyed (on the caller's thread, from its mount list).
pub(crate) struct Submitted {
    /// Its index in its call (results are tagged with it).
    pub(crate) index: usize,
    pub(crate) source: VolumeKey,
    pub(crate) dest: Option<VolumeKey>,
    pub(crate) skippable: bool,
    pub(crate) id: u64,
    pub(crate) task: Box<dyn PoolTask>,
}

/// What the scheduler is told.
pub(crate) enum Msg {
    Open {
        call: CallId,
        workers: usize,
        cancel: Arc<AtomicBool>,
        /// The caller's own cancel flag, read here directly: once it is set no job of the call
        /// starts, even before the caller next looks at it.
        linked: Option<Arc<AtomicBool>>,
        sink: Box<dyn CallSink>,
    },
    Submit {
        call: CallId,
        jobs: Vec<Submitted>,
    },
    /// No more jobs come for the call: it is forgotten once every job has an outcome.
    Close {
        call: CallId,
    },
    /// Look at the calls now (a cancel was set): the next step runs the bookkeeping tick at once.
    Wake,
    /// A job's thread is done with it (`pos`: its place in the call).
    Ended {
        call: CallId,
        pos: usize,
        ran: Ran,
    },
}

/// A job handed to a worker.
struct Work {
    call: CallId,
    pos: usize,
    task: Box<dyn PoolTask>,
    state: Arc<JobState>,
}

/// A worker: runs the jobs it is handed until the scheduler retires it; a worker whose job was
/// abandoned removes the registry entry when the job returns, and exits (its replacement
/// already runs).
fn worker(work: Arc<Mutex<Receiver<Work>>>, events: Sender<Msg>, shared: Arc<Shared>) {
    WORKER.with(|w| w.set(true));
    loop {
        let next = work.lock().unwrap_or_else(PoisonError::into_inner).recv();
        let Ok(w) = next else { return };
        let ran = w.task.run(&w.state);
        if ran.deliver.is_none() {
            shared.job_returned(&w.state);
            return;
        }
        // (a stopped scheduler: the result is delivered as the message drops)
        if events.send(Msg::Ended { call: w.call, pos: w.pos, ran }).is_err() {
            return;
        }
    }
}

struct Slot {
    /// Its index in its call.
    index: usize,
    /// Its source's volume (whose queue it waits in).
    vol: usize,
    /// A copy's destination volume, when not the source's.
    dest: Option<usize>,
    /// It is a copy (its destination may be on its source's volume): measured on what it wrote.
    copy: bool,
    skippable: bool,
    id: u64,
    task: Option<Box<dyn PoolTask>>,
    state: Option<Arc<JobState>>,
    /// Running on a worker (not on its call's thread).
    on_worker: bool,
    /// Bytes seen at the last tick: every byte of a job on one volume, else a copy's bytes on its
    /// source and on its destination.
    seen: [u64; 2],
    /// Bytes metered for the limit controller so far ([`measured_bytes`], on its measured volume).
    metered: u64,
    /// When it started (monotonic ns).
    started: u64,
}

/// A volume, every call together.
struct Vol {
    key: VolumeKey,
    /// Running jobs holding a slot on it (as source or destination).
    running: Vec<(CallId, usize)>,
    /// Jobs that need it and have no outcome yet.
    active: usize,
    /// Admission sequence of the last job started from its queues (0: none yet).
    last_start: u64,
    /// Nanoseconds without progress while not paused.
    silent: u64,
    /// Its recovery job, running.
    recovery: Option<(CallId, usize)>,
    /// Jobs with no outcome measured on it (a probe on its source, a copy on its destination).
    measured: usize,
    /// What its measured jobs did, for the controller.
    meter: Meter,
    /// It had jobs at the last tick (the profile output says when it starts and ends).
    was_busy: bool,
}

/// A volume's measured traffic, cumulative.
#[derive(Clone, Copy, Default)]
struct Meter {
    bytes: u64,
    jobs: u64,
    /// Nanoseconds its jobs ran, summed over jobs (sampled at each tick).
    busy: u64,
    /// Nanoseconds of that spent waiting at the work gate.
    gate: u64,
}

impl Meter {
    fn counters(self) -> Counters {
        Counters { bytes: self.bytes as f64, jobs: self.jobs, busy: secs(self.busy), gate: secs(self.gate) }
    }
}

/// Nanoseconds as seconds.
fn secs(ns: u64) -> f64 {
    ns as f64 / 1e9
}

/// A volume, as one call sees it.
#[derive(Default)]
struct CallVol {
    /// Its jobs whose source is on the volume, waiting, in input order.
    queue: VecDeque<usize>,
    /// Its running jobs holding a slot on the volume.
    running: usize,
    /// Its jobs that need the volume and have no outcome yet.
    active: usize,
    /// Admission sequence of its last start from this queue (turns between calls).
    last_start: u64,
    /// Nanoseconds its jobs here showed no progress since its cancel.
    cancel_silent: u64,
    /// A recovery job of this call on the volume answered without data ([`Ended::Answered`]):
    /// the call's next job on it may start at once as the next recovery job.
    answered: bool,
}

struct CallState {
    sink: Box<dyn CallSink>,
    /// Jobs with no outcome yet, by place.
    slots: HashMap<usize, Slot>,
    /// Places given so far.
    next_pos: usize,
    /// By volume index.
    vols: Vec<CallVol>,
    /// Today's thread count for it (`workers()`): where its volumes start before they are
    /// measured.
    workers: usize,
    cancel: Arc<AtomicBool>,
    /// The caller's own cancel flag (see [`Msg::Open`]).
    linked: Option<Arc<AtomicBool>>,
    cancelling: bool,
    /// Jobs with no outcome yet.
    remaining: usize,
    /// Jobs running.
    busy: usize,
    closed: bool,
    /// A job handed to the call's own thread, not yet back.
    inline: Option<usize>,
}

impl CallState {
    /// Whether the call was cancelled: by its stream, or by the caller's own flag.
    fn cancel_set(&self) -> bool {
        self.cancel.load(Ordering::Relaxed) || self.linked.as_ref().is_some_and(|l| l.load(Ordering::Relaxed))
    }
}

/// Job `slot`'s side on volume `v`: `None` when it isn't a copy between two volumes (all its I/O
/// is on `v`).
fn side_on(slot: &Slot, v: usize) -> Option<Side> {
    slot.dest.map(|_| if slot.vol == v { Side::Source } else { Side::Dest })
}

fn call_vol(vols: &mut Vec<CallVol>, v: usize) -> &mut CallVol {
    if vols.len() <= v {
        vols.resize_with(v + 1, CallVol::default);
    }
    // (just grown to hold `v`)
    #[allow(clippy::indexing_slicing)]
    &mut vols[v]
}

/// How a running job leaves its volumes ([`Sched::leave`]).
#[derive(Clone, Copy)]
enum Leave {
    /// It returned with its result: the registry's end-of-job rule (a recovery job's end with it).
    Finished { took: u64, moved: bool, ok: bool },
    /// The scheduler left it behind: it no longer runs there; a recovery job stays the registry's
    /// until its thread returns ([`Shared::job_returned`]).
    Abandoned,
    /// It was counted as started but never ran (its call is gone): it no longer runs there, and a
    /// recovery job frees the registry's slot at once (no other job waits ten minutes for it).
    NeverRan,
}

/// Why a job could not be started.
enum Dispatch {
    Started,
    /// Every worker is busy and no new one could be started: it waits.
    NoWorker,
    /// Its call's thread already runs one of its jobs: it waits.
    CallBusy,
}

/// The scheduler: every call it holds, the volumes they use, the worker threads.
pub(crate) struct Sched {
    shared: Arc<Shared>,
    spawn_budget: Arc<AtomicUsize>,
    ticks: Arc<AtomicU64>,
    pending: Arc<AtomicUsize>,
    pub(crate) tick: Duration,
    vols: Vec<Vol>,
    calls: BTreeMap<CallId, CallState>,
    seq: u64,
    /// Worker threads alive, not abandoned.
    live: usize,
    /// Jobs handed to workers, not finished nor abandoned.
    on_workers: usize,
    /// Jobs running, anywhere.
    busy: usize,
    work_tx: Sender<Work>,
    work_rx: Arc<Mutex<Receiver<Work>>>,
    events_tx: Sender<Msg>,
    events_rx: Receiver<Msg>,
    /// Both clocks at the last step (wake detection).
    prev: (u64, u64),
    /// Monotonic time the next tick counts silence from: the last tick, a late step (its gap was
    /// the sleep), or the last step while it held no call.
    since: u64,
    /// When the bookkeeping tick last ran, in real time: it runs once per `tick`, however many
    /// messages (job ends, submits) come in between. (Its cadence is a CPU cost, not a rule:
    /// every rule reads the injected clocks, through `since`.)
    last_tick: std::time::Instant,
    /// A cancel woke it ([`Msg::Wake`]): the next step ticks at once (its gate waits end now).
    tick_now: bool,
    /// A step more than [`WAKE_JUMP`] later than its wait is a wake
    /// ([`super::IoContext::late_tick_is_wake`]); off for a call that drives its own scheduler,
    /// whose steps wait on the caller.
    late_tick_is_wake: bool,
    /// Running jobs at most (§5.7's I/O cap).
    io_cap: usize,
    /// Every volume's limit (§5.7).
    ctl: Controller<VolumeKey>,
    /// Bytes every volume's measured jobs moved, together.
    measured_total: u64,
    /// Print the controller's windows and decisions to stderr (`<PREFIX>_PROFILE`).
    profile: bool,
    /// Windows completed and discarded per volume (published with the limits).
    windows: HashMap<VolumeKey, (u64, u64)>,
}

impl Sched {
    pub(crate) fn new(ctx: &IoContext) -> Sched {
        let (work_tx, work_rx) = std::sync::mpsc::channel();
        let (events_tx, events_rx) = std::sync::mpsc::channel();
        let clock = &ctx.shared.clock;
        Sched {
            shared: ctx.shared.clone(),
            spawn_budget: ctx.spawn_budget.clone(),
            ticks: ctx.ticks.clone(),
            pending: ctx.pending.clone(),
            tick: ctx.tick,
            vols: Vec::new(),
            calls: BTreeMap::new(),
            seq: 0,
            live: 0,
            on_workers: 0,
            busy: 0,
            work_tx,
            work_rx: Arc::new(Mutex::new(work_rx)),
            events_tx,
            events_rx,
            prev: (clock.mono(), clock.wall()),
            since: clock.mono(),
            last_tick: std::time::Instant::now(),
            tick_now: false,
            late_tick_is_wake: ctx.late_tick_is_wake,
            io_cap: ctx.io_cap.max(1),
            ctl: Controller::new(ctx.io_cap.max(1), crate::import::workers(usize::MAX)),
            measured_total: 0,
            profile: lcc_pipeline::profiling(),
            windows: HashMap::new(),
        }
    }

    /// A scheduler a call drives itself (no pool): its steps come when the caller asks for
    /// outcomes, so a late one is no wake.
    pub(crate) fn driven_by_caller(mut self) -> Sched {
        self.late_tick_is_wake = false;
        self
    }

    /// Where to send it messages.
    pub(crate) fn sender(&self) -> Sender<Msg> {
        self.events_tx.clone()
    }

    /// Whether it holds no call.
    pub(crate) fn idle(&self) -> bool {
        self.calls.is_empty()
    }

    /// How long until the next bookkeeping tick is due (a step waits at most this long while it
    /// holds calls).
    pub(crate) fn until_tick(&self) -> Duration {
        self.tick.saturating_sub(self.last_tick.elapsed())
    }

    /// One round: wait up to `wait` for a message, take every waiting one, then (while it holds
    /// calls, once per `tick`) the bookkeeping tick, then admission.
    pub(crate) fn step(&mut self, wait: Duration) {
        let clock = self.shared.clock.clone();
        if let Ok(m) = self.events_rx.recv_timeout(wait) {
            self.handle(m, clock.mono());
            while let Ok(m) = self.events_rx.try_recv() {
                self.handle(m, clock.mono());
            }
        }
        let (now, wall) = (clock.mono(), clock.wall());
        let (dm, dw) = (now.saturating_sub(self.prev.0), wall.saturating_sub(self.prev.1));
        self.prev = (now, wall);
        // (a step far later than it was due, where the monotonic clock counts sleep: the computer
        // slept in between; due: its wait, and a busy step's wait ends by the next tick)
        let due = wait.max(self.tick);
        let late = self.late_tick_is_wake && dm > u64::try_from(due.as_nanos()).unwrap_or(u64::MAX).saturating_add(WAKE_JUMP);
        // (also while idle: the registry's entries get the grace too)
        if dw > dm.saturating_add(WAKE_JUMP) || late {
            log::info!("io: the computer woke from sleep: every volume gets a grace period");
            self.shared.note_wake(now);
        }
        // (every step, before the tick and admission: newly submitted work starts at its volume's
        // starting limit, not the controller's default)
        if !self.calls.is_empty() {
            self.offer_starts(now);
        }
        // (a late step's time was the sleep, not the volumes' silence)
        if late || self.calls.is_empty() {
            self.since = now;
        }
        // (a call with every outcome is forgotten below: the tick sees its volumes go idle first,
        // so what the controller learned, and the probe it held, are settled and published)
        let ending = self.calls.values().any(|c| c.closed && c.remaining == 0);
        let ticking = !self.calls.is_empty() && (std::mem::take(&mut self.tick_now) || ending || self.last_tick.elapsed() >= self.tick);
        if ticking {
            self.last_tick = std::time::Instant::now();
            self.tick(now, now.saturating_sub(self.since));
            self.since = now;
        }
        self.admit(clock.mono());
        // (the controller's tick after admission: a volume below its limit with queued work is
        // then held back by something else, not waiting for a free slot of its own)
        if ticking {
            self.adapt(now);
            self.ticks.fetch_add(1, Ordering::Relaxed);
        }
        self.reap();
    }

    pub(crate) fn handle(&mut self, msg: Msg, now: u64) {
        match msg {
            Msg::Open { call, workers, cancel, linked, sink } => {
                let state = CallState {
                    sink,
                    slots: HashMap::new(),
                    next_pos: 0,
                    vols: Vec::new(),
                    workers: workers.max(1),
                    cancel,
                    linked,
                    cancelling: false,
                    remaining: 0,
                    busy: 0,
                    closed: false,
                    inline: None,
                };
                self.calls.insert(call, state);
            }
            Msg::Submit { call, jobs } => self.submit(call, jobs),
            Msg::Close { call } => {
                if let Some(c) = self.calls.get_mut(&call) {
                    c.closed = true;
                }
            }
            Msg::Wake => self.tick_now = true,
            Msg::Ended { call, pos, ran } => self.finish(call, pos, ran, now),
        }
        self.pending.store(self.calls.values().map(|c| c.remaining).sum(), Ordering::Relaxed);
    }

    /// The volume `key`'s index, added when new.
    fn vol_index(&mut self, key: VolumeKey) -> usize {
        match self.vols.iter().position(|v| v.key == key) {
            Some(v) => v,
            None => {
                self.vols.push(Vol {
                    key,
                    running: Vec::new(),
                    active: 0,
                    last_start: 0,
                    silent: 0,
                    recovery: None,
                    measured: 0,
                    meter: Meter::default(),
                    was_busy: false,
                });
                self.vols.len() - 1
            }
        }
    }

    fn submit(&mut self, call: CallId, jobs: Vec<Submitted>) {
        if !self.calls.contains_key(&call) {
            for j in jobs {
                j.task.fail(SchedOutcome::SchedulerStopped);
            }
            return;
        }
        for j in jobs {
            let vol = self.vol_index(j.source);
            let copy = j.dest.is_some();
            let dest = j.dest.map(|d| self.vol_index(d)).filter(|&d| d != vol);
            let Some(c) = self.calls.get_mut(&call) else { return };
            let pos = c.next_pos;
            c.next_pos += 1;
            for v in std::iter::once(vol).chain(dest) {
                call_vol(&mut c.vols, v).active += 1;
                if let Some(pv) = self.vols.get_mut(v) {
                    pv.active += 1;
                }
            }
            if let Some(pv) = self.vols.get_mut(dest.unwrap_or(vol)) {
                pv.measured += 1;
            }
            call_vol(&mut c.vols, vol).queue.push_back(pos);
            c.remaining += 1;
            let slot = Slot {
                index: j.index,
                vol,
                dest,
                copy,
                skippable: j.skippable,
                id: j.id,
                task: Some(j.task),
                state: None,
                on_worker: false,
                seen: [0; 2],
                metered: 0,
                started: 0,
            };
            c.slots.insert(pos, slot);
        }
        // (jobs of a cancelled call never start)
        match self.calls.get(&call) {
            Some(c) if c.cancelling => self.fail_unstarted(call),
            Some(c) if c.cancel_set() => self.begin_cancel(call),
            _ => {}
        }
    }

    /// Forget calls that are closed and have every outcome; with none left, retire the idle
    /// workers (a new call starts its own).
    fn reap(&mut self) {
        self.calls.retain(|_, c| !(c.closed && c.remaining == 0));
        self.pending.store(self.calls.values().map(|c| c.remaining).sum(), Ordering::Relaxed);
        if self.calls.is_empty() && self.on_workers == 0 && self.live > 0 {
            // (idle workers see their channel close and exit)
            let (tx, rx) = std::sync::mpsc::channel();
            self.work_tx = tx;
            self.work_rx = Arc::new(Mutex::new(rx));
            self.live = 0;
        }
    }

    /// Jobs with no outcome yet, every call together.
    fn total(&self) -> usize {
        self.calls.values().map(|c| c.remaining).sum()
    }

    /// The cap now (§5.7): the I/O cap, at least 2 while some volume is held to one fewer (R4a).
    fn cap(&self, total: usize) -> usize {
        if self.vols.iter().any(|v| limits::cut(v.active, total)) { self.io_cap.max(2) } else { self.io_cap }
    }

    /// Volume `v`'s limit under `cap`: the controller's, held to one fewer than the cap while
    /// some job with no outcome doesn't need it (R4a; [`limits::under_cap`]).
    fn limit(&self, v: usize, cap: usize, total: usize) -> usize {
        let Some(pv) = self.vols.get(v) else { return cap };
        limits::under_cap(self.ctl.limit(&pv.key), cap, limits::cut(pv.active, total))
    }

    /// Whether volume `v` can take one more running job now (not waiting on its recovery job,
    /// not blocked, under its limit).
    fn has_slot(&self, v: usize, cap: usize, total: usize, blocked: &[bool]) -> bool {
        self.vols
            .get(v)
            .is_some_and(|pv| pv.recovery.is_none() && !blocked.get(v).copied().unwrap_or(true) && pv.running.len() < self.limit(v, cap, total))
    }

    /// The first queued job needing volume `v` (its queue's front, or a copy into it), of the
    /// call whose turn it is on `v`.
    fn first_queued_needing(&self, v: usize) -> Option<(CallId, usize)> {
        let mut best: Option<((u64, CallId), usize)> = None;
        for (&c, call) in &self.calls {
            if call.cancelling {
                continue;
            }
            let own = call.vols.get(v).and_then(|cv| cv.queue.front().copied());
            let into =
                call.vols.iter().filter_map(|cv| cv.queue.iter().copied().find(|i| call.slots.get(i).is_some_and(|s| s.dest == Some(v)))).min();
            let Some(i) = own.into_iter().chain(into).min() else { continue };
            let turn = (call.vols.get(v).map_or(0, |cv| cv.last_start), c);
            if best.as_ref().is_none_or(|b| turn < b.0) {
                best = Some((turn, i));
            }
        }
        best.map(|((_, c), i)| (c, i))
    }

    /// Calls whose cancel was set and not yet acted on: their queued jobs never start.
    fn take_cancels(&mut self) {
        let newly: Vec<CallId> = self.calls.iter().filter(|(_, c)| !c.cancelling && c.cancel_set()).map(|(id, _)| *id).collect();
        for c in newly {
            self.begin_cancel(c);
        }
    }

    fn admit(&mut self, now: u64) {
        self.take_cancels();
        // blocked volumes: hold while their running jobs may show they work, else one recovery
        // job first (or fail their queued jobs at once when none may run)
        let mut blocked = vec![false; self.vols.len()];
        for v in 0..self.vols.len() {
            let Some(pv) = self.vols.get(v) else { continue };
            // (queued jobs that need it: those with no outcome that aren't running)
            if pv.active <= pv.running.len() || pv.recovery.is_some() || !self.shared.blocked(&pv.key, now) {
                continue;
            }
            let (idle, key) = (pv.running.is_empty(), pv.key.clone());
            if let Some(b) = blocked.get_mut(v) {
                *b = true;
            }
            let total = self.total();
            let cap = self.cap(total);
            if !idle || self.busy >= cap {
                continue;
            }
            let Some((c, i)) = self.first_queued_needing(v) else { continue };
            let Some(slot) = self.calls.get(&c).and_then(|call| call.slots.get(&i)) else { continue };
            // (a copy's other volume needs a free slot too: else it waits)
            let other = if slot.vol == v { slot.dest } else { Some(slot.vol) };
            if let Some(o) = other
                && !self.vols.get(o).is_some_and(|ov| ov.running.len() < self.limit(o, cap, total))
            {
                continue;
            }
            let answered = self.calls.get(&c).and_then(|call| call.vols.get(v)).is_some_and(|cv| cv.answered);
            let may = if answered { self.shared.continue_recovery(&key, slot.id, now) } else { self.shared.try_start_recovery(&key, slot.id, now) };
            if may {
                self.start_recovery(c, v, i, now);
            } else {
                log::info!("io: volume {key} is not responding: its files fail");
                self.fail_queue(v);
            }
        }
        // calls that can't start a job now (their own thread is busy with one)
        let mut skip: HashSet<CallId> = HashSet::new();
        loop {
            let total = self.total();
            let cap = self.cap(total);
            if self.busy >= cap {
                break;
            }
            // round robin over the volumes; within a volume, the call whose turn it is; within a
            // call, input order (a queue's front starts when its source and its destination both
            // have a slot)
            let mut pick: Option<((u64, usize, u64, CallId), usize)> = None;
            for (v, pv) in self.vols.iter().enumerate() {
                if !self.has_slot(v, cap, total, &blocked) {
                    continue;
                }
                for (&c, call) in &self.calls {
                    if call.cancelling || skip.contains(&c) {
                        continue;
                    }
                    let Some(cv) = call.vols.get(v) else { continue };
                    let Some(&i) = cv.queue.front() else { continue };
                    if !call.slots.get(&i).and_then(|s| s.dest).is_none_or(|d| self.has_slot(d, cap, total, &blocked)) {
                        continue;
                    }
                    let k = (pv.last_start, v, cv.last_start, c);
                    if pick.as_ref().is_none_or(|p| k < p.0) {
                        pick = Some((k, i));
                    }
                }
            }
            let Some(((_, v, _, c), i)) = pick else { break };
            if let Some(cv) = self.calls.get_mut(&c).and_then(|call| call.vols.get_mut(v)) {
                let _ = cv.queue.pop_front();
            }
            match self.dispatch(c, i, now) {
                Dispatch::Started => {}
                failed => {
                    if let Some(cv) = self.calls.get_mut(&c).and_then(|call| call.vols.get_mut(v)) {
                        cv.queue.push_front(i);
                    }
                    if matches!(failed, Dispatch::NoWorker) {
                        break;
                    }
                    skip.insert(c);
                }
            }
        }
    }

    fn start_recovery(&mut self, c: CallId, v: usize, i: usize, now: u64) {
        // out of its queue (its source's: a copy into `v` waits there)
        let home = self.calls.get(&c).and_then(|call| call.slots.get(&i)).map(|s| s.vol);
        if let Some(q) = home.and_then(|h| self.calls.get_mut(&c).and_then(|call| call.vols.get_mut(h))).map(|cv| &mut cv.queue)
            && let Some(at) = q.iter().position(|&x| x == i)
        {
            let _ = q.remove(at);
        }
        let Some(key) = self.vols.get(v).map(|pv| pv.key.clone()) else { return };
        if let Some(pv) = self.vols.get_mut(v) {
            pv.recovery = Some((c, i));
        }
        log::info!("io: volume {key} was not responding: trying one file first");
        if matches!(self.dispatch(c, i, now), Dispatch::Started) {
            return;
        }
        if let Some(pv) = self.vols.get_mut(v) {
            pv.recovery = None;
        }
        // (back to its place in its own queue: input order)
        let Some(call) = self.calls.get_mut(&c) else { return };
        let Some(s) = call.slots.get(&i) else { return };
        self.shared.end_recovery(&key, s.id);
        let home = s.vol;
        let q = &mut call_vol(&mut call.vols, home).queue;
        let at = q.iter().position(|&x| x > i).unwrap_or(q.len());
        q.insert(at, i);
    }

    /// Start a worker thread (the injected budget allowing).
    fn spawn_worker(&mut self) -> bool {
        if self.spawn_budget.try_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_sub(1)).is_err() {
            return false;
        }
        let (rx, tx, shared) = (self.work_rx.clone(), self.events_tx.clone(), self.shared.clone());
        std::thread::Builder::new().name(format!("lc-io-{}", self.live)).spawn(move || worker(rx, tx, shared)).is_ok()
    }

    /// Start job `i` of call `c` on a free worker, spawning one if none is idle. With no worker at
    /// all, its call's own thread runs it (one at a time per call).
    fn dispatch(&mut self, c: CallId, i: usize, now: u64) -> Dispatch {
        let mut inline = false;
        if self.live <= self.on_workers {
            if self.spawn_worker() {
                self.live += 1;
            } else if self.live == 0 {
                if self.calls.get(&c).is_none_or(|call| call.inline.is_some()) {
                    return Dispatch::CallBusy;
                }
                log::warn!("io: no thread could be started: a file is read on the calling thread");
                inline = true;
            } else {
                return Dispatch::NoWorker;
            }
        }
        let shared = self.shared.clone();
        let Some(call) = self.calls.get_mut(&c) else { return Dispatch::NoWorker };
        let Some(slot) = call.slots.get_mut(&i) else { return Dispatch::NoWorker };
        let Some(key) = self.vols.get(slot.vol).map(|v| v.key.clone()) else { return Dispatch::NoWorker };
        let Some(task) = slot.task.take() else { return Dispatch::NoWorker };
        let (home, dest) = (slot.vol, slot.dest);
        let state = Arc::new(JobState::new(slot.id, key, shared.clock.clone(), Arc::downgrade(&shared)));
        slot.state = Some(state.clone());
        slot.seen = [0; 2];
        slot.metered = 0;
        slot.started = now;
        slot.on_worker = !inline;
        self.seq += 1;
        for v in std::iter::once(home).chain(dest) {
            if let Some(pv) = self.vols.get_mut(v) {
                shared.job_started(&pv.key, now);
                pv.running.push((c, i));
                if pv.running.len() == 1 {
                    pv.silent = 0;
                }
            }
            let cv = call_vol(&mut call.vols, v);
            cv.running += 1;
            if cv.running == 1 {
                cv.cancel_silent = 0;
            }
        }
        if let Some(pv) = self.vols.get_mut(home) {
            pv.last_start = self.seq;
        }
        call_vol(&mut call.vols, home).last_start = self.seq;
        self.busy += 1;
        call.busy += 1;
        if inline {
            call.inline = Some(i);
            let info = InlineInfo { state, events: self.events_tx.clone(), call: c, pos: i };
            if !task.run_here(info) {
                // (nobody is left to run it or to read its outcome)
                self.unstart(c, i, now);
            }
            return Dispatch::Started;
        }
        match self.work_tx.send(Work { call: c, pos: i, task, state }) {
            Ok(()) => self.on_workers += 1,
            Err(e) => {
                // (can't happen: this scheduler holds the receiver)
                e.0.task.discard();
                self.unstart(c, i, now);
            }
        }
        Dispatch::Started
    }

    /// Job `i` was counted as started but never ran (its call is gone): undo, and end it.
    fn unstart(&mut self, c: CallId, i: usize, now: u64) {
        let Some(call) = self.calls.get_mut(&c) else { return };
        call.inline = call.inline.filter(|&x| x != i);
        self.leave(c, i, Leave::NeverRan, now);
        self.end_slot(c, i);
    }

    /// Running job `i` of call `c` leaves its volumes (a copy both): the one release every path
    /// takes (§5.2). It stops holding a slot and running there and stops being a volume's
    /// recovery job here, and the registry hears how it left ([`Leave`]); a finished job also
    /// restarts its volumes' clocks and its call's cancel clock there, and may hand recovery on.
    fn leave(&mut self, c: CallId, i: usize, how: Leave, now: u64) {
        let Some(call) = self.calls.get_mut(&c) else { return };
        let Some(slot) = call.slots.get(&i) else { return };
        let (home, dest, id) = (slot.vol, slot.dest, slot.id);
        self.busy = self.busy.saturating_sub(1);
        call.busy = call.busy.saturating_sub(1);
        for v in std::iter::once(home).chain(dest) {
            let mut ended = Ended::Job;
            if let Some(pv) = self.vols.get_mut(v) {
                pv.running.retain(|&r| r != (c, i));
                let recovery = pv.recovery == Some((c, i));
                if recovery {
                    pv.recovery = None;
                }
                match how {
                    Leave::Finished { took, moved, ok } => {
                        pv.silent = 0;
                        ended = self.shared.job_finished(&pv.key, now, took, moved, ok, recovery.then_some(id));
                    }
                    Leave::Abandoned => self.shared.abandoned(&pv.key, now),
                    Leave::NeverRan => {
                        self.shared.abandoned(&pv.key, now);
                        if recovery {
                            self.shared.end_recovery(&pv.key, id);
                        }
                    }
                }
            }
            let cv = call_vol(&mut call.vols, v);
            cv.running = cv.running.saturating_sub(1);
            if matches!(how, Leave::Finished { .. }) {
                cv.cancel_silent = 0;
                // (answered without data: the call's next job on it goes as the next recovery job)
                cv.answered |= ended == Ended::Answered;
            }
        }
    }

    /// Job `i` of call `c` has its outcome: it no longer counts anywhere.
    fn end_slot(&mut self, c: CallId, i: usize) {
        let Some(call) = self.calls.get_mut(&c) else { return };
        let Some(slot) = call.slots.remove(&i) else { return };
        if let Some(task) = slot.task {
            task.discard();
        }
        for v in std::iter::once(slot.vol).chain(slot.dest) {
            if let Some(pv) = self.vols.get_mut(v) {
                pv.active = pv.active.saturating_sub(1);
            }
            let cv = call_vol(&mut call.vols, v);
            cv.active = cv.active.saturating_sub(1);
        }
        if let Some(pv) = self.vols.get_mut(slot.dest.unwrap_or(slot.vol)) {
            pv.measured = pv.measured.saturating_sub(1);
        }
        call.remaining = call.remaining.saturating_sub(1);
    }

    fn finish(&mut self, c: CallId, i: usize, ran: Ran, now: u64) {
        // (a call already forgotten: the result is delivered as `ran` drops)
        let Some(call) = self.calls.get_mut(&c) else { return };
        if call.inline == Some(i) {
            call.inline = None;
        }
        let Some(deliver) = ran.deliver else { return };
        let Some(slot) = call.slots.get_mut(&i) else {
            deliver.deliver();
            return;
        };
        // (what it moved since the last tick, and its completion, count for its measured volume)
        let (home, dest) = (slot.vol, slot.dest);
        let measured = measured_bytes(slot.copy, ran.metrics.moved);
        let unseen = measured.saturating_sub(slot.metered);
        slot.metered = slot.metered.max(measured);
        self.measured_total = self.measured_total.saturating_add(unseen);
        if let Some(pv) = self.vols.get_mut(dest.unwrap_or(home)) {
            pv.meter.bytes = pv.meter.bytes.saturating_add(unseen);
            pv.meter.jobs = pv.meter.jobs.saturating_add(1);
        }
        if std::mem::take(&mut slot.on_worker) {
            self.on_workers = self.on_workers.saturating_sub(1);
        }
        // (a copy's completion is progress on both of its volumes; a recovery job's end follows
        // the registry's one rule, as the single-job path's does)
        let how = Leave::Finished { took: super::run::took(&ran.metrics), moved: ran.metrics.bytes > 0, ok: ran.ok };
        self.leave(c, i, how, now);
        self.end_slot(c, i);
        deliver.deliver();
    }

    /// Every queued job that needs volume `v`, in every call (from its queue, or a copy into it
    /// from another), fails "volume not responding". Jobs submitted later meet the registry's
    /// block (§5.2).
    fn fail_queue(&mut self, v: usize) {
        let Some(key) = self.vols.get(v).map(|pv| pv.key.clone()) else { return };
        let ids: Vec<CallId> = self.calls.keys().copied().collect();
        for c in ids {
            let Some(call) = self.calls.get_mut(&c) else { continue };
            let mut queued: Vec<usize> = call.vols.get_mut(v).map(|cv| cv.queue.drain(..).collect()).unwrap_or_default();
            let slots = &call.slots;
            for (o, other) in call.vols.iter_mut().enumerate() {
                if o != v {
                    other.queue.retain(|i| {
                        let into = slots.get(i).is_some_and(|s| s.dest == Some(v));
                        if into {
                            queued.push(*i);
                        }
                        !into
                    });
                }
            }
            queued.sort_unstable();
            for i in &queued {
                if let Some(task) = call.slots.get_mut(i).and_then(|s| s.task.take()) {
                    task.fail(SchedOutcome::VolumeNotResponding { volume: key.clone(), abandoned: false });
                }
            }
            for i in queued {
                self.end_slot(c, i);
            }
        }
    }

    /// Leave running job `i` of call `c` behind on volume `v` (the one stalled or silent after
    /// cancel; a copy leaves both of its volumes); `false` if it finished first (its result is on
    /// its way).
    fn abandon(&mut self, c: CallId, i: usize, v: usize, kind: AbandonKind, now: u64) -> bool {
        let Some(key) = self.vols.get(v).map(|pv| pv.key.clone()) else { return false };
        let Some(slot) = self.calls.get(&c).and_then(|call| call.slots.get(&i)) else { return false };
        let Some(state) = slot.state.clone() else { return false };
        let (dest, started, on_worker, index) = (slot.dest, slot.started, slot.on_worker, slot.index);
        self.shared.add_entry(&state, &key, kind, now);
        if !state.try_abandon() {
            self.shared.drop_entry(state.id);
            return false;
        }
        if on_worker {
            // (its worker exits when the job returns)
            self.on_workers = self.on_workers.saturating_sub(1);
            self.live = self.live.saturating_sub(1);
        }
        // (an abandoned recovery job stays the registry's until it returns)
        self.leave(c, i, Leave::Abandoned, now);
        let dest_key = dest.and_then(|d| self.vols.get(d)).map(|pv| pv.key.clone());
        let Some(call) = self.calls.get(&c) else { return true };
        let moved = state.moved();
        let metrics = JobMetrics {
            volume: state.volume.clone(),
            dest: dest_key,
            bytes: moved.total(),
            moved,
            wall: now.saturating_sub(started),
            gate_wait: 0,
            gate_failure: state.gate_failure(),
        };
        let outcome = match kind {
            AbandonKind::Stall => SchedOutcome::VolumeNotResponding { volume: key, abandoned: true },
            AbandonKind::Cancel => SchedOutcome::Cancelled,
        };
        call.sink.outcome(index, outcome, Some(metrics));
        self.end_slot(c, i);
        true
    }

    fn begin_cancel(&mut self, c: CallId) {
        let Some(call) = self.calls.get_mut(&c) else { return };
        if call.cancelling {
            return;
        }
        call.cancelling = true;
        for cv in &mut call.vols {
            cv.cancel_silent = 0;
        }
        self.fail_unstarted(c);
    }

    /// A cancelled call's queued jobs never start.
    fn fail_unstarted(&mut self, c: CallId) {
        let Some(call) = self.calls.get_mut(&c) else { return };
        let mut queued: Vec<usize> = call.vols.iter_mut().flat_map(|cv| cv.queue.drain(..)).collect();
        queued.sort_unstable();
        for i in &queued {
            if let Some(task) = call.slots.get_mut(i).and_then(|s| s.task.take()) {
                task.fail(SchedOutcome::NotStarted);
            }
        }
        for i in queued {
            self.end_slot(c, i);
        }
    }

    /// The bookkeeping tick: progress, the stall rule (every call's jobs on a volume together),
    /// cancel's rules (each call's own jobs).
    fn tick(&mut self, now: u64, dt: u64) {
        self.take_cancels();
        // a job's bytes since the last tick are progress on the volume they moved on (a copy's
        // reads on its source, its writes and read-back on its destination); a job that isn't
        // silent there without them ([`held`]) pauses the clocks: the stall clock (every call's
        // jobs on the volume) and cancel's (each call's own)
        // what the controller measures, from the running jobs only (a queued job has moved
        // nothing): each on its measured volume, the bytes it moved since the last tick
        // ([`measured_bytes`]), its time, and its time at the work gate
        for v in 0..self.vols.len() {
            let Some(pv) = self.vols.get(v) else { continue };
            let mut m = pv.meter;
            for &(c, i) in &pv.running {
                let Some(slot) = self.calls.get_mut(&c).and_then(|call| call.slots.get_mut(&i)) else { continue };
                // (a copy is listed on both of its volumes: measured on its destination)
                if slot.dest.unwrap_or(slot.vol) != v {
                    continue;
                }
                let Some(st) = &slot.state else { continue };
                let measured = measured_bytes(slot.copy, st.moved());
                let new = measured.saturating_sub(slot.metered);
                slot.metered = slot.metered.max(measured);
                self.measured_total = self.measured_total.saturating_add(new);
                m.bytes = m.bytes.saturating_add(new);
                m.busy = m.busy.saturating_add(dt);
                if st.at_gate() {
                    m.gate = m.gate.saturating_add(dt);
                }
            }
            if let Some(pv) = self.vols.get_mut(v) {
                pv.meter = m;
            }
        }
        let mut silent: Vec<(u64, usize)> = Vec::new();
        for v in 0..self.vols.len() {
            let Some(pv) = self.vols.get(v) else { continue };
            if pv.running.is_empty() {
                continue;
            }
            let key = pv.key.clone();
            let running = pv.running.clone();
            let stall_time = self.shared.stall_time(&key);
            let mut volume = Seen::default();
            let mut callers: Vec<(CallId, Seen)> = Vec::new();
            for (c, i) in running {
                let Some(slot) = self.calls.get_mut(&c).and_then(|call| call.slots.get_mut(&i)) else { continue };
                let Some(st) = slot.state.clone() else { continue };
                let side = side_on(slot, v);
                let (bytes, at, seen) = match side {
                    None => (st.bytes(), st.last_bytes(), slot.seen.get_mut(0)),
                    Some(s @ Side::Source) => (st.moved().on(s), st.last_bytes_on(s), slot.seen.get_mut(0)),
                    Some(s @ Side::Dest) => (st.moved().on(s), st.last_bytes_on(s), slot.seen.get_mut(1)),
                };
                let mine = seen.is_some_and(|seen| {
                    let more = bytes > *seen;
                    *seen = (*seen).max(bytes);
                    more
                });
                if mine {
                    self.shared.bytes_seen(&key, at);
                }
                let busy = held(&st, side, now, stall_time);
                volume.add(mine, st.at_gate() || busy);
                match callers.iter_mut().find(|(x, _)| *x == c) {
                    Some(e) => e.1.add(mine, busy),
                    None => callers.push((c, Seen::of(mine, busy))),
                }
            }
            // cancel's clock counts each call's own jobs here (another call's reads don't keep
            // a cancelled call waiting)
            for (c, seen) in callers {
                if let Some(call) = self.calls.get_mut(&c)
                    && call.cancelling
                {
                    let cv = call_vol(&mut call.vols, v);
                    cv.cancel_silent = seen.after(cv.cancel_silent, dt);
                }
            }
            let Some(pv) = self.vols.get_mut(v) else { continue };
            pv.silent = volume.after(pv.silent, dt);
            silent.push((pv.silent, v));
        }
        // the volume silent longest is judged first: a copy stuck on one of its volumes leaves
        // the other when that one is declared, so the other isn't declared with it unless it is
        // silent as long (a copy whose stuck call named no volume pauses neither: then the one
        // met first goes first)
        silent.sort_by_key(|&(s, v)| (std::cmp::Reverse(s), v));
        for (_, v) in silent {
            let Some(pv) = self.vols.get(v) else { continue };
            if pv.running.is_empty() {
                continue;
            }
            let key = pv.key.clone();
            let running = pv.running.clone();
            let stall_time = self.shared.stall_time(&key);
            if pv.silent >= stall_time && !self.shared.wake_holds(now) {
                log::warn!("io: volume {key} moved nothing for {} s: not responding", stall_time / 1_000_000_000);
                for &(c, i) in &running {
                    self.abandon(c, i, v, AbandonKind::Stall, now);
                }
                self.fail_queue(v);
                continue;
            }
            let mut callers: Vec<CallId> = running.iter().map(|&(c, _)| c).collect();
            callers.sort_unstable();
            callers.dedup();
            for c in callers {
                let Some(call) = self.calls.get(&c).filter(|call| call.cancelling) else { continue };
                let quiet = call.vols.get(v).is_some_and(|cv| cv.cancel_silent >= CANCEL_SILENCE);
                let mine: Vec<(usize, Option<Arc<JobState>>, bool)> = running
                    .iter()
                    .filter(|r| r.0 == c)
                    .map(|&(_, i)| {
                        let s = call.slots.get(&i);
                        (i, s.and_then(|s| s.state.clone()), s.is_some_and(|s| s.skippable))
                    })
                    .collect();
                for (i, st, skippable) in mine {
                    if quiet {
                        self.abandon(c, i, v, AbandonKind::Cancel, now);
                        continue;
                    }
                    // a job still waiting at the gate has read nothing: it ends at once, unread;
                    // a copy waiting for its DNG step is told to skip it instead (it keeps its
                    // raw copy)
                    let Some(st) = st.filter(|s| s.at_gate()) else { continue };
                    if skippable {
                        st.request_skip();
                    } else {
                        self.abandon(c, i, v, AbandonKind::Cancel, now);
                    }
                }
            }
        }
    }

    /// The controller's tick (§5.7): every volume's demand and counters; what it learned is
    /// published for the import's read-ahead and printed under `<PREFIX>_PROFILE`.
    /// Each volume's starting limit, before admission, on every step (the bookkeeping tick, which
    /// measures and changes limits, runs only once per tick period): a volume with jobs that isn't
    /// measured yet takes its calls' `workers()`, so newly submitted work never starts under the
    /// controller's default; one idle for [`super::limits::FORGET`] is forgotten first.
    fn offer_starts(&mut self, now: u64) {
        let t = secs(now);
        let total = self.total();
        // (PR 1's cap is at least 2 while some volume is held to one fewer, R4a)
        let some_cut = self.vols.iter().any(|pv| limits::cut(pv.active, total));
        for v in 0..self.vols.len() {
            let Some(pv) = self.vols.get(v) else { continue };
            let key = pv.key.clone();
            let busy = pv.active > 0;
            self.ctl.note_busy(t, &key, busy);
            if busy {
                // (at least 2 for a volume not held to one fewer while another is, as PR 1's cap,
                // so a hung card's copy never holds the only slot of the SSD the other card copies
                // into: a floor for what it learns too; a volume not measured yet runs at today's
                // limit, its calls' `workers()`)
                let floor = if some_cut && !limits::cut(pv.active, total) { 2 } else { 1 };
                self.ctl.set_floor(&key, floor);
                let workers = self.calls.values().filter(|c| c.vols.get(v).is_some_and(|cv| cv.active > 0)).map(|c| c.workers).max().unwrap_or(1);
                self.ctl.offer_start(t, &key, workers);
            }
        }
    }

    fn adapt(&mut self, now: u64) {
        let t = secs(now);
        let total = self.total();
        let cap = self.cap(total);
        let measured_total = self.measured_total as f64;
        let demands = self.demands(cap, total, now);
        for v in 0..self.vols.len() {
            let Some(pv) = self.vols.get(v) else { continue };
            let key = pv.key.clone();
            let busy = pv.active > 0;
            if self.ctl.phase(&key).is_none() {
                continue;
            }
            let demand = demands.get(v).copied().unwrap_or(Demand::Limited);
            let counters = pv.meter.counters();
            let was_busy = pv.was_busy;
            self.ctl.tick(t, &key, demand, counters, measured_total);
            if busy != was_busy {
                let what = if busy { "starts" } else { "idle" };
                self.report(&format!("io-limit {key}: {what} at limit {}", self.ctl.limit(&key)));
            }
            if let Some(pv) = self.vols.get_mut(v) {
                pv.was_busy = busy;
            }
        }
        for e in self.ctl.take_events() {
            match &e {
                Event::Window { vol, .. } => self.windows.entry(vol.clone()).or_default().0 += 1,
                Event::Discarded { vol, .. } | Event::NoBytes { vol, .. } => self.windows.entry(vol.clone()).or_default().1 += 1,
                _ => {}
            }
            let line = describe(&e);
            self.report(&line);
        }
        let learned = self
            .ctl
            .limits()
            .map(|(k, limit)| {
                let (windows, discarded) = self.windows.get(k).copied().unwrap_or_default();
                let probing = self.ctl.token() == Some(k);
                (k.clone(), Learned { limit, raised: self.ctl.raised(k), probing, windows, discarded })
            })
            .collect();
        self.shared.set_learned(learned);
    }

    /// Where every volume stands against its limit, after admission (§5.7 "Window";
    /// [`limits::demand`]): demand-limited while blocked or recovering, or with nothing it is
    /// measured on. Below its limit a volume is at it only while a queued job that needs it waits
    /// for the I/O cap alone: a queue's front (what admission starts next) that isn't a copy, or a
    /// copy whose other volume has a free slot. A copy waiting for a slot on its other volume (a
    /// destination fed by a card that is full) leaves this one demand-limited, not at its limit.
    fn demands(&self, cap: usize, total: usize, now: u64) -> Vec<Demand> {
        let limits: Vec<usize> = (0..self.vols.len()).map(|v| self.limit(v, cap, total)).collect();
        let stopped: Vec<bool> = self.vols.iter().map(|pv| pv.recovery.is_some() || self.shared.blocked(&pv.key, now)).collect();
        let room = |v: usize| {
            !stopped.get(v).copied().unwrap_or(true) && self.vols.get(v).zip(limits.get(v)).is_some_and(|(pv, &limit)| pv.running.len() < limit)
        };
        let mut waiting = vec![false; self.vols.len()];
        let mut set = |v: usize| {
            if let Some(w) = waiting.get_mut(v) {
                *w = true;
            }
        };
        for call in self.calls.values().filter(|c| !c.cancelling) {
            for (u, cv) in call.vols.iter().enumerate() {
                let Some(i) = cv.queue.front() else { continue };
                match call.slots.get(i).and_then(|s| s.dest) {
                    None => set(u),
                    Some(d) => {
                        if room(d) {
                            set(u);
                        }
                        if room(u) {
                            set(d);
                        }
                    }
                }
            }
        }
        self.vols
            .iter()
            .enumerate()
            .map(|(v, pv)| {
                let (Some(&limit), Some(&waits), Some(&stop)) = (limits.get(v), waiting.get(v), stopped.get(v)) else {
                    return Demand::Limited;
                };
                if pv.measured == 0 || stop { Demand::Limited } else { limits::demand(pv.running.len(), limit, waits) }
            })
            .collect()
    }

    /// A controller line: to stderr under `<PREFIX>_PROFILE`, else the debug log.
    fn report(&self, line: &str) {
        if self.profile {
            eprintln!("{line}");
        } else {
            log::debug!("{line}");
        }
    }
}

/// The bytes of a job the controller measures (§5.4, §5.7), decided here only: a copy's bytes
/// written (its source read and its verify read-back don't count: the destination is judged on
/// what it wrote), any other job's every byte (a probe reads).
fn measured_bytes(copy: bool, moved: Moved) -> u64 {
    if copy { moved.written } else { moved.total() }
}

/// MB/s from bytes/s.
fn mbs(rate: f64) -> f64 {
    rate / 1e6
}

/// One controller event as a line of the profile output (`io-limit <volume>: …`).
pub(crate) fn describe(e: &Event<VolumeKey>) -> String {
    let phase = |p: Phase| match p {
        Phase::Base1 => "baseline",
        Phase::Probe => "probe",
        Phase::Base2 => "second baseline",
        Phase::Hold => "hold",
    };
    match e {
        Event::Window { vol, phase: p, limit, rate, total, span, jobs } => format!(
            "io-limit {vol}: window ({}) at limit {limit}: {:.1} MB/s, process {:.1} MB/s, {span:.1} s, {jobs} jobs",
            phase(*p),
            mbs(*rate),
            mbs(*total)
        ),
        Event::Discarded { vol, limit, gate } => {
            format!("io-limit {vol}: window at limit {limit} discarded: {:.0} % of its jobs' time at the work gate", gate * 100.0)
        }
        Event::NoBytes { vol, limit, jobs } => format!("io-limit {vol}: window at limit {limit} discarded: {jobs} jobs moved no bytes"),
        Event::Probe { vol, from, to } => format!("io-limit {vol}: probe {from} -> {to}"),
        Event::Decided { vol, up, from, to, kept, base, probe, base_total, probe_total } => format!(
            "io-limit {vol}: {} {from} -> {to} {}: {:.1} MB/s against baselines {:.1} MB/s; process {:.1} against {:.1} MB/s; limit now {}",
            if *up { "up" } else { "down" },
            if *kept { "kept" } else { "not kept" },
            mbs(*probe),
            mbs(*base),
            mbs(*probe_total),
            mbs(*base_total),
            if *kept { to } else { from }
        ),
        Event::Aborted { vol, reason, limit } => {
            let why = match reason {
                AbortReason::Demand => "ran out of work",
                AbortReason::Deadline => "window could not complete",
                AbortReason::Discarded => "window discarded",
                AbortReason::NoBytes => "window moved no bytes",
                AbortReason::Floor => "its floor rose",
            };
            format!("io-limit {vol}: probe ended ({why}): limit {limit}")
        }
        Event::Hold { vol, limit, windows } => format!("io-limit {vol}: holds at limit {limit} for {windows} windows"),
        Event::Floor { vol, limit } => {
            format!("io-limit {vol}: raised to {limit}: a destination of copies from several volumes keeps at least {limit}")
        }
        Event::Forgotten { vol, limit } => format!("io-limit {vol}: forgotten after 10 minutes with no jobs (was {limit})"),
    }
}

/// Whether a running job (state `st`; a copy's `side` of the volume judged) that moved no byte on
/// that volume since the last tick is still not silent there: inside a computing or flushing span
/// no longer than [`COMPUTING_FACTOR`] stall times, or a copy whose current call is on its other
/// volume (it waits there, not here). The one rule both clocks use (§5.2): the stall clock also
/// holds while a job waits at the work gate; cancel's doesn't, as cancel ends such a wait itself
/// (the job is abandoned, or told to skip its DNG step, at once).
fn held(st: &JobState, side: Option<Side>, now: u64, stall_time: u64) -> bool {
    let computing = st.computing_for(now).is_some_and(|s| s <= stall_time.saturating_mul(COMPUTING_FACTOR));
    let elsewhere = side.is_some_and(|s| st.on() == Some(s.other()));
    computing || elsewhere
}

/// What a tick saw of some running jobs on a volume: one moved bytes there, or every one is
/// [`held`] (no job yet: held).
#[derive(Clone, Copy)]
struct Seen {
    progressed: bool,
    held: bool,
}

impl Default for Seen {
    fn default() -> Seen {
        Seen { progressed: false, held: true }
    }
}

impl Seen {
    fn of(progressed: bool, held: bool) -> Seen {
        Seen { progressed, held }
    }

    fn add(&mut self, progressed: bool, held: bool) {
        self.progressed |= progressed;
        self.held &= held;
    }

    /// A silence clock at `silent` after `dt` more: back to 0 on progress, unchanged while every
    /// job is held, else counting.
    fn after(self, silent: u64, dt: u64) -> u64 {
        if self.progressed {
            0
        } else if self.held {
            silent
        } else {
            silent.saturating_add(dt)
        }
    }
}
