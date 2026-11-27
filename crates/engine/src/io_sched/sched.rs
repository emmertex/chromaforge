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
//! Limits: the pool's cap is the largest `workers` of the calls it holds (today's `workers()`),
//! at least 2 while some volume is held to one fewer (R4a); a volume's limit is that cap, or one
//! fewer while some job with no outcome doesn't need it; each call also keeps its own cap, as in
//! PR 1. Within a volume, calls take turns (the call that started a job on it longest ago goes
//! first); within a call, jobs start in input order.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use lcc_catalog::folders::VolumeKey;

use super::IoContext;
use super::job::{JobState, Side};
use super::run::JobMetrics;
use super::shared::{AbandonKind, CANCEL_SILENCE, COMPUTING_FACTOR, Ended, Shared, WAKE_JUMP};

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
    /// Look at the calls now (a cancel was set).
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
    skippable: bool,
    id: u64,
    task: Option<Box<dyn PoolTask>>,
    state: Option<Arc<JobState>>,
    /// Running on a worker (not on its call's thread).
    on_worker: bool,
    /// Bytes seen at the last tick: every byte of a job on one volume, else a copy's bytes on its
    /// source and on its destination.
    seen: [u64; 2],
    /// When it started (monotonic ns).
    started: u64,
    /// The volume it runs as the recovery job of.
    recovery: Option<usize>,
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
    workers: usize,
    cancel: Arc<AtomicBool>,
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
    /// Its own cap (PR 1): `workers`, at least 2 while one of its volumes is held to one fewer.
    fn cap(&self) -> usize {
        let cut = self.vols.iter().any(|cv| cv.active > 0 && self.remaining > cv.active);
        if cut { self.workers.max(2) } else { self.workers }
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
    /// A step more than [`WAKE_JUMP`] later than its wait is a wake
    /// ([`super::IoContext::late_tick_is_wake`]); off for a call that drives its own scheduler,
    /// whose steps wait on the caller.
    late_tick_is_wake: bool,
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
            late_tick_is_wake: ctx.late_tick_is_wake,
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

    /// One round: wait up to `wait` for a message, take every waiting one, then (while it holds
    /// calls) the bookkeeping tick, then admission.
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
        // (a step far later than its wait, where the monotonic clock counts sleep: the computer
        // slept in between)
        let late = self.late_tick_is_wake && dm > u64::try_from(wait.as_nanos()).unwrap_or(u64::MAX).saturating_add(WAKE_JUMP);
        // (also while idle: the registry's entries get the grace too)
        if dw > dm.saturating_add(WAKE_JUMP) || late {
            log::info!("io: the computer woke from sleep: every volume gets a grace period");
            self.shared.note_wake(now);
        }
        if !self.calls.is_empty() {
            // (a late step's time was the sleep, not the volumes' silence)
            self.tick(now, if late { 0 } else { dm });
            self.ticks.fetch_add(1, Ordering::Relaxed);
        }
        self.admit(clock.mono());
        self.reap();
    }

    pub(crate) fn handle(&mut self, msg: Msg, now: u64) {
        match msg {
            Msg::Open { call, workers, cancel, sink } => {
                let state = CallState {
                    sink,
                    slots: HashMap::new(),
                    next_pos: 0,
                    vols: Vec::new(),
                    workers: workers.max(1),
                    cancel,
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
            Msg::Wake => {}
            Msg::Ended { call, pos, ran } => self.finish(call, pos, ran, now),
        }
        self.pending.store(self.calls.values().map(|c| c.remaining).sum(), Ordering::Relaxed);
    }

    /// The volume `key`'s index, added when new.
    fn vol_index(&mut self, key: VolumeKey) -> usize {
        match self.vols.iter().position(|v| v.key == key) {
            Some(v) => v,
            None => {
                self.vols.push(Vol { key, running: Vec::new(), active: 0, last_start: 0, silent: 0, recovery: None });
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
            call_vol(&mut c.vols, vol).queue.push_back(pos);
            c.remaining += 1;
            let slot = Slot {
                index: j.index,
                vol,
                dest,
                skippable: j.skippable,
                id: j.id,
                task: Some(j.task),
                state: None,
                on_worker: false,
                seen: [0; 2],
                started: 0,
                recovery: None,
            };
            c.slots.insert(pos, slot);
        }
        // (jobs of a cancelled call never start)
        match self.calls.get(&call) {
            Some(c) if c.cancelling => self.fail_unstarted(call),
            Some(c) if c.cancel.load(Ordering::Relaxed) => self.begin_cancel(call),
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

    /// The cap now (§5.3, process-wide from PR 3): the largest `workers` among calls with work,
    /// at least 2 while some volume is held to one fewer (R4a).
    fn cap(&self, total: usize) -> usize {
        let workers = self.calls.values().filter(|c| c.remaining > 0).map(|c| c.workers).max().unwrap_or(1);
        if self.vols.iter().any(|v| cut(v, total)) { workers.max(2) } else { workers }
    }

    /// Volume `v`'s limit under `cap`.
    fn limit(&self, v: usize, cap: usize, total: usize) -> usize {
        match self.vols.get(v) {
            Some(pv) if cut(pv, total) => cap.saturating_sub(1).max(1),
            _ => cap,
        }
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
            if call.cancelling || call.busy >= call.cap() {
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
        let newly: Vec<CallId> = self.calls.iter().filter(|(_, c)| !c.cancelling && c.cancel.load(Ordering::Relaxed)).map(|(id, _)| *id).collect();
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
                    if call.cancelling || skip.contains(&c) || call.busy >= call.cap() {
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
        if let Some(s) = self.calls.get_mut(&c).and_then(|call| call.slots.get_mut(&i)) {
            s.recovery = Some(v);
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
        let Some(s) = call.slots.get_mut(&i) else { return };
        s.recovery = None;
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
        let Some(slot) = call.slots.get(&i) else { return };
        let (home, dest) = (slot.vol, slot.dest);
        self.busy = self.busy.saturating_sub(1);
        call.busy = call.busy.saturating_sub(1);
        for v in std::iter::once(home).chain(dest) {
            if let Some(pv) = self.vols.get_mut(v) {
                self.shared.abandoned(&pv.key, now);
                pv.running.retain(|&r| r != (c, i));
                if pv.recovery == Some((c, i)) {
                    pv.recovery = None;
                }
            }
            let cv = call_vol(&mut call.vols, v);
            cv.running = cv.running.saturating_sub(1);
        }
        self.end_slot(c, i);
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
        let (home, dest, id, recovery, on_worker) = (slot.vol, slot.dest, slot.id, slot.recovery, slot.on_worker);
        slot.on_worker = false;
        self.busy = self.busy.saturating_sub(1);
        call.busy = call.busy.saturating_sub(1);
        if on_worker {
            self.on_workers = self.on_workers.saturating_sub(1);
        }
        let moved = ran.metrics.bytes > 0;
        let took = super::run::took(&ran.metrics);
        // (a copy's completion is progress on both of its volumes; a recovery job's end follows
        // the registry's one rule, as the single-job path's does)
        for v in std::iter::once(home).chain(dest) {
            let mut ended = Ended::Job;
            if let Some(pv) = self.vols.get_mut(v) {
                pv.running.retain(|&r| r != (c, i));
                pv.silent = 0;
                ended = self.shared.job_finished(&pv.key, now, took, moved, ran.ok, (recovery == Some(v)).then_some(id));
                if recovery == Some(v) {
                    pv.recovery = None;
                }
            }
            let cv = call_vol(&mut call.vols, v);
            cv.running = cv.running.saturating_sub(1);
            cv.cancel_silent = 0;
            // (answered without data: the call's next job on it goes as the next recovery job)
            cv.answered |= ended == Ended::Answered;
        }
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
        let Some(call) = self.calls.get_mut(&c) else { return false };
        let Some(slot) = call.slots.get(&i) else { return false };
        let Some(state) = slot.state.clone() else { return false };
        let (home, dest, started, on_worker, index) = (slot.vol, slot.dest, slot.started, slot.on_worker, slot.index);
        self.shared.add_entry(&state, &key, kind, now);
        if !state.try_abandon() {
            self.shared.drop_entry(state.id);
            return false;
        }
        self.busy = self.busy.saturating_sub(1);
        call.busy = call.busy.saturating_sub(1);
        if on_worker {
            // (its worker exits when the job returns)
            self.on_workers = self.on_workers.saturating_sub(1);
            self.live = self.live.saturating_sub(1);
        }
        let mut dest_key = None;
        for w in std::iter::once(home).chain(dest) {
            if let Some(pv) = self.vols.get_mut(w) {
                self.shared.abandoned(&pv.key, now);
                pv.running.retain(|&r| r != (c, i));
                // (an abandoned recovery job stays the registry's until it returns)
                if pv.recovery == Some((c, i)) {
                    pv.recovery = None;
                }
                if pv.key != state.volume {
                    dest_key = Some(pv.key.clone());
                }
            }
            let cv = call_vol(&mut call.vols, w);
            cv.running = cv.running.saturating_sub(1);
        }
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
        // reads on its source, its writes and read-back on its destination); a copy whose current
        // call is on its other volume waits there, not here: it pauses this one's clock
        let mut silent: Vec<(u64, usize)> = Vec::new();
        for v in 0..self.vols.len() {
            let Some(pv) = self.vols.get(v) else { continue };
            if pv.running.is_empty() {
                continue;
            }
            let key = pv.key.clone();
            let running = pv.running.clone();
            let stall_time = self.shared.stall_time(&key);
            let mut progressed = false;
            let mut paused_all = true;
            let mut callers: Vec<(CallId, bool)> = Vec::new();
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
                    progressed = true;
                    self.shared.bytes_seen(&key, at);
                }
                match callers.iter_mut().find(|(x, _)| *x == c) {
                    Some(e) => e.1 |= mine,
                    None => callers.push((c, mine)),
                }
                let computing = st.computing_for(now).is_some_and(|s| s <= stall_time.saturating_mul(COMPUTING_FACTOR));
                let elsewhere = side.is_some_and(|s| st.on() == Some(s.other()));
                paused_all &= st.at_gate() || computing || elsewhere;
            }
            // cancel's clock counts each call's own jobs here (another call's reads don't keep
            // a cancelled call waiting)
            for (c, progressed) in callers {
                if let Some(call) = self.calls.get_mut(&c)
                    && call.cancelling
                {
                    let cv = call_vol(&mut call.vols, v);
                    cv.cancel_silent = if progressed { 0 } else { cv.cancel_silent.saturating_add(dt) };
                }
            }
            let Some(pv) = self.vols.get_mut(v) else { continue };
            if progressed {
                pv.silent = 0;
            } else if !paused_all {
                pv.silent = pv.silent.saturating_add(dt);
            }
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
}

/// Whether volume `v` is held to one fewer than the cap (R4a): it has work and some job with no
/// outcome yet doesn't need it (a job on another volume, or a copy between two others).
fn cut(v: &Vol, total: usize) -> bool {
    v.active > 0 && total > v.active
}
