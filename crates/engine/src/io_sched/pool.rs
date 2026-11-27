//! One process-wide pool (§5.5 of the I/O spec, PR 3): every call's jobs go to one scheduler
//! ([`Sched`]) run by a small supervisor thread, so two calls on one volume share its limit and
//! take turns on it, and the bookkeeping (admission, the stall rule, cancel, the registry checks)
//! goes on while a streaming caller is busy elsewhere.
//!
//! Two ways in: [`run_jobs`] (a vector of jobs, their outcomes in input order) and [`Stream`] (jobs
//! submitted over time, outcomes as they finish, tagged with their index). A single-job
//! [`run_jobs`] bypasses the pool and runs on the calling thread, as today. A job running on a
//! scheduler worker never submits several jobs and waits: from such a thread one job runs inline
//! and several are refused ([`SubmitError`]).
//!
//! Fallbacks, never a panic: when the supervisor can't be started, each call runs its own
//! scheduler on its own thread (as in PR 2); when it panics, its queued jobs fail "scheduler
//! stopped", running jobs' results still reach their callers (each call owns its channel), and
//! later calls run their own schedulers.

use std::ops::Range;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Arc, Weak};
use std::time::Duration;

use lcc_catalog::folders::{Mounts, VolumeKeys};

use super::IoContext;
use super::job::JobState;
use super::run::{IoJob, JobMetrics, JobOutcome, Report, execute, log_metrics, run_single};
use super::sched::{CallId, CallSink, Deliver, Inline, InlineInfo, Msg, PoolTask, Ran, Sched, SchedOutcome, Submitted, on_worker_thread};

/// How long an idle supervisor sleeps before checking that its context still exists.
const IDLE_WAIT: Duration = Duration::from_secs(1);

/// Why jobs were refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SubmitError {
    /// Several jobs submitted from a scheduler worker: it would hold a slot while waiting for one.
    FromPoolThread,
}

impl std::fmt::Display for SubmitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SubmitError::FromPoolThread => f.write_str("several files can't be read from inside a file job (it would wait for itself)"),
        }
    }
}

/// The running pool: where to send its supervisor messages.
#[derive(Clone)]
pub(crate) struct PoolHandle {
    tx: Sender<Msg>,
    /// The supervisor ended (a panic): later calls run their own schedulers.
    stopped: Arc<AtomicBool>,
}

/// The pool's state in its context.
pub(crate) enum PoolState {
    /// Not started yet, or its supervisor couldn't be started (tried again by the next call).
    Down,
    Up(PoolHandle),
    /// Its supervisor stopped: calls run their own schedulers from now on.
    Stopped,
}

impl IoContext {
    /// The pool, its supervisor started on first use; `None` when it can't run (the caller then
    /// runs its own scheduler).
    pub(crate) fn pool(&self) -> Option<PoolHandle> {
        let mut st = self.pool.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let PoolState::Up(h) = &*st {
            if !h.stopped.load(Ordering::Acquire) {
                return Some(h.clone());
            }
            *st = PoolState::Stopped;
        }
        if matches!(*st, PoolState::Stopped) {
            return None;
        }
        let h = self.start_supervisor()?;
        *st = PoolState::Up(h.clone());
        Some(h)
    }

    /// Whether the pool's supervisor runs (tests).
    #[cfg(test)]
    pub(crate) fn pool_up(&self) -> bool {
        matches!(&*self.pool.lock().unwrap_or_else(std::sync::PoisonError::into_inner), PoolState::Up(h) if !h.stopped.load(Ordering::Acquire))
    }

    fn start_supervisor(&self) -> Option<PoolHandle> {
        if self.supervisor_budget.try_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_sub(1)).is_err() {
            log::warn!("io: the I/O pool couldn't start: each call schedules its own reads");
            return None;
        }
        let sched = Sched::new(self);
        let tx = sched.sender();
        let stopped = Arc::new(AtomicBool::new(false));
        let alive = Arc::downgrade(&self.alive);
        let flag = stopped.clone();
        #[cfg(test)]
        let inject = self.panic_supervisor.clone();
        let run = move || {
            #[cfg(test)]
            let r = supervise(sched, alive, inject);
            #[cfg(not(test))]
            let r = supervise(sched, alive);
            flag.store(true, Ordering::Release);
            if let Err(e) = r {
                log::error!("io: the I/O pool stopped ({e}): files not yet started fail; later calls schedule their own reads");
            }
        };
        match std::thread::Builder::new().name("lc-io-pool".into()).spawn(run) {
            Ok(_) => Some(PoolHandle { tx, stopped }),
            Err(e) => {
                log::warn!("io: the I/O pool couldn't start ({e}): each call schedules its own reads");
                None
            }
        }
    }
}

/// The supervisor: steps the scheduler until its context is gone, under the panic guard (the
/// scheduler, moved in, is dropped by a panic: its queued jobs then tell their calls "scheduler
/// stopped").
fn supervise(mut sched: Sched, alive: Weak<()>, #[cfg(test)] inject: Arc<AtomicBool>) -> Result<(), String> {
    crate::guard::catch("the I/O pool", move || {
        loop {
            #[cfg(test)]
            if inject.swap(false, Ordering::AcqRel) {
                std::panic::resume_unwind(Box::new("injected supervisor panic"));
            }
            let wait = if sched.idle() {
                if alive.strong_count() == 0 {
                    return;
                }
                IDLE_WAIT
            } else {
                sched.until_tick()
            };
            sched.step(wait);
        }
    })
}

/// What a call's channel carries.
pub(crate) enum CallEvent<T> {
    Outcome {
        index: usize,
        outcome: JobOutcome<T>,
        metrics: Option<JobMetrics>,
    },
    /// No worker thread exists: the call's own thread runs this job.
    RunHere(Inline),
}

fn outcome_of<T>(o: SchedOutcome) -> JobOutcome<T> {
    match o {
        SchedOutcome::NotStarted => JobOutcome::NotStarted,
        SchedOutcome::Cancelled => JobOutcome::Cancelled,
        SchedOutcome::VolumeNotResponding { volume, abandoned } => JobOutcome::VolumeNotResponding { volume, abandoned },
        SchedOutcome::SchedulerStopped => JobOutcome::SchedulerStopped,
    }
}

impl<T: Send + 'static> CallSink for Sender<CallEvent<T>> {
    fn outcome(&self, index: usize, outcome: SchedOutcome, metrics: Option<JobMetrics>) {
        let _ = self.send(CallEvent::Outcome { index, outcome: outcome_of(outcome), metrics });
    }
}

/// A finished job's result: sent to its call when delivered or dropped.
struct Delivery<T> {
    tx: Sender<CallEvent<T>>,
    index: usize,
    outcome: Option<JobOutcome<T>>,
    metrics: Option<JobMetrics>,
}

impl<T: Send + 'static> Deliver for Delivery<T> {
    fn deliver(self: Box<Self>) {}
}

impl<T> Drop for Delivery<T> {
    fn drop(&mut self) {
        if let Some(outcome) = self.outcome.take() {
            let _ = self.tx.send(CallEvent::Outcome { index: self.index, outcome, metrics: self.metrics.take() });
        }
    }
}

/// A job as the pool holds it. Dropped unrun, it tells its call "scheduler stopped".
struct Task<T> {
    index: usize,
    run: Option<Box<dyn FnOnce() -> T + Send + 'static>>,
    dest: Option<lcc_catalog::folders::VolumeKey>,
    succeeded: fn(&T) -> bool,
    tx: Sender<CallEvent<T>>,
}

impl<T> Drop for Task<T> {
    fn drop(&mut self) {
        if self.run.take().is_some() {
            let _ = self.tx.send(CallEvent::Outcome { index: self.index, outcome: JobOutcome::SchedulerStopped, metrics: None });
        }
    }
}

impl<T: Send + 'static> PoolTask for Task<T> {
    fn run(mut self: Box<Self>, state: &Arc<JobState>) -> Ran {
        let (result, metrics) = match self.run.take() {
            Some(run) => execute(run, state, self.dest.clone()),
            None => (Err("the job had already run".to_string()), JobMetrics::none(state)),
        };
        let ok = result.as_ref().is_ok_and(self.succeeded);
        let deliver = state.try_finish().then(|| {
            let outcome = match result {
                Ok(t) => JobOutcome::Done(t),
                Err(m) => JobOutcome::Panicked(m),
            };
            Box::new(Delivery { tx: self.tx.clone(), index: self.index, outcome: Some(outcome), metrics: Some(metrics.clone()) }) as Box<dyn Deliver>
        });
        Ran { ok, metrics, deliver }
    }

    fn fail(mut self: Box<Self>, outcome: SchedOutcome) {
        let _ = self.run.take();
        let _ = self.tx.send(CallEvent::Outcome { index: self.index, outcome: outcome_of(outcome), metrics: None });
    }

    fn discard(mut self: Box<Self>) {
        let _ = self.run.take();
    }

    fn run_here(self: Box<Self>, info: InlineInfo) -> bool {
        let tx = self.tx.clone();
        match tx.send(CallEvent::RunHere(Inline { task: self, info })) {
            Ok(()) => true,
            Err(e) => {
                if let CallEvent::RunHere(inline) = e.0 {
                    inline.task.discard();
                }
                false
            }
        }
    }
}

/// Who runs a stream's bookkeeping.
enum Driver {
    /// The process's pool.
    Pool(PoolHandle),
    /// Its own scheduler, stepped while the caller waits for results (the pool couldn't run).
    Own(Box<Sched>),
}

/// The streaming form: submit jobs over time, receive their outcomes as they finish, each tagged
/// with its input index (in submission order across submits). Cancel affects only its own jobs.
/// Dropping it cancels the jobs not yet started and forgets the rest. A caller with a cancel flag of
/// its own links it ([`Stream::with_cancel`]): the pool then reads it directly.
pub(crate) struct Stream<T: Send + 'static> {
    ctx: Arc<IoContext>,
    driver: Driver,
    call: CallId,
    opened: bool,
    workers: usize,
    succeeded: fn(&T) -> bool,
    tx: Sender<CallEvent<T>>,
    rx: Receiver<CallEvent<T>>,
    cancel: Arc<AtomicBool>,
    /// The caller's own cancel flag, read by the pool itself ([`Stream::with_cancel`]).
    linked: Option<Arc<AtomicBool>>,
    /// The volume keys of its jobs, from the mount list it read (per folder: an import keys a
    /// folder once, not each file).
    keys: VolumeKeys,
    next: usize,
}

impl<T: Send + 'static> Stream<T> {
    /// A stream whose jobs run at most `workers` at once (today's `workers()`; the pool's limits
    /// hold too). Reads the mount list (waiting up to about a second; §5.1), used for every job.
    pub(crate) fn open(ctx: &Arc<IoContext>, workers: usize, succeeded: fn(&T) -> bool) -> Stream<T> {
        let mounts = ctx.mounts.fresh(ctx.shared.clock.as_ref(), ctx.mount_wait);
        Stream::with_mounts(ctx, workers, succeeded, mounts)
    }

    fn with_mounts(ctx: &Arc<IoContext>, workers: usize, succeeded: fn(&T) -> bool, mounts: Arc<Mounts>) -> Stream<T> {
        let driver = match ctx.pool() {
            Some(h) => Driver::Pool(h),
            None => Driver::Own(Box::new(Sched::new(ctx).driven_by_caller())),
        };
        let (tx, rx) = std::sync::mpsc::channel();
        Stream {
            ctx: ctx.clone(),
            driver,
            call: ctx.next_call.fetch_add(1, Ordering::Relaxed),
            opened: false,
            workers: workers.max(1),
            succeeded,
            tx,
            rx,
            cancel: Arc::new(AtomicBool::new(false)),
            linked: None,
            keys: VolumeKeys::new(mounts),
            next: 0,
        }
    }

    /// Link the caller's own cancel flag (before the first submit): once it is set, no further job
    /// of this stream starts, whether or not the caller has looked at it since. The pool only
    /// reads it ([`Stream::cancel`] and dropping the stream never set it). Without it, a cancel
    /// the caller forwards later lets the pool start jobs in between (a finished job's slot is
    /// refilled at once).
    pub(crate) fn with_cancel(mut self, cancel: Arc<AtomicBool>) -> Stream<T> {
        self.linked = Some(cancel);
        self
    }

    /// The volume `path` is on, as this stream keys its jobs (from the mount list it read).
    pub(crate) fn volume_of(&mut self, path: &str) -> lcc_catalog::folders::VolumeKey {
        self.keys.file(path)
    }

    /// Whether this stream runs its own scheduler (the pool couldn't run).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn own_scheduler(&self) -> bool {
        matches!(self.driver, Driver::Own(_))
    }

    fn send(&mut self, msg: Msg) {
        match &mut self.driver {
            // (a stopped pool: the message drops, and its jobs tell this stream "scheduler stopped")
            Driver::Pool(h) => drop(h.tx.send(msg)),
            Driver::Own(s) => {
                let now = self.ctx.shared.clock.mono();
                s.handle(msg, now);
            }
        }
    }

    /// Submit `jobs`; their indexes. From a scheduler worker one job runs inline, at once, and
    /// several are refused.
    pub(crate) fn submit(&mut self, jobs: Vec<IoJob<T>>) -> Result<Range<usize>, SubmitError> {
        let first = self.next;
        if on_worker_thread() {
            if jobs.len() > 1 {
                return Err(SubmitError::FromPoolThread);
            }
            for job in jobs {
                let index = self.next;
                self.next += 1;
                let report = run_single(&self.ctx, job, self.succeeded, &self.cancel, &AtomicUsize::new(0));
                for (outcome, metrics) in report.outcomes.into_iter().zip(report.metrics) {
                    let _ = self.tx.send(CallEvent::Outcome { index, outcome, metrics });
                }
            }
            return Ok(first..self.next);
        }
        if !self.opened {
            self.opened = true;
            let sink: Box<dyn CallSink> = Box::new(self.tx.clone());
            let linked = self.linked.clone();
            self.send(Msg::Open { call: self.call, workers: self.workers, cancel: self.cancel.clone(), linked, sink });
        }
        let mut submitted = Vec::with_capacity(jobs.len());
        for job in jobs {
            let index = self.next;
            self.next += 1;
            let source = self.keys.file(&job.source);
            let dest = job.dest.as_deref().map(|d| self.keys.folder(d));
            let task =
                Task { index, run: Some(job.run), dest: dest.clone().filter(|d| d != &source), succeeded: self.succeeded, tx: self.tx.clone() };
            submitted.push(Submitted { index, source, dest, skippable: job.skippable, id: self.ctx.shared.next_id(), task: Box::new(task) });
        }
        self.send(Msg::Submit { call: self.call, jobs: submitted });
        Ok(first..self.next)
    }

    /// No more jobs come for what was submitted so far: the pool forgets this call once they all
    /// have outcomes. A later submit opens a new call (indexes go on).
    pub(crate) fn close(&mut self) {
        if self.opened {
            self.send(Msg::Close { call: self.call });
            self.opened = false;
            self.call = self.ctx.next_call.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Cancel (every job this stream submitted, closed or not): jobs not started never start;
    /// running ones finish, or are left behind after 5 s without progress (§5.2).
    pub(crate) fn cancel(&mut self) {
        self.cancel.store(true, Ordering::Release);
        self.send(Msg::Wake);
    }

    /// Whether the pool this stream uses has stopped (its running jobs then have no bookkeeping).
    fn pool_stopped(&self) -> bool {
        match &self.driver {
            Driver::Pool(h) => h.stopped.load(Ordering::Acquire),
            Driver::Own(_) => false,
        }
    }

    /// Whether a caller still waiting for this stream's outcomes stops waiting: the stream was
    /// cancelled ([`Stream::cancel`]) and the pool it uses has stopped (a supervisor panic), so no
    /// rule watches its jobs still running any more, and a job hung on a dead volume would keep
    /// the caller waiting for ever, Cancel or not. The caller takes the outcomes still missing as
    /// [`JobOutcome::Cancelled`] (left behind). The one escape every waiting loop uses.
    pub(crate) fn left_behind(&self) -> bool {
        self.cancel.load(Ordering::Acquire) && self.pool_stopped()
    }

    /// The next outcome, waiting up to `wait`. A job no worker could take runs here meanwhile.
    pub(crate) fn recv_timeout(&mut self, wait: Duration) -> Option<(usize, JobOutcome<T>, Option<JobMetrics>)> {
        let start = std::time::Instant::now();
        let mut stepped = false;
        loop {
            let left = wait.saturating_sub(start.elapsed());
            let event = match &mut self.driver {
                Driver::Pool(_) => match self.rx.recv_timeout(left) {
                    Ok(e) => e,
                    Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => return None,
                },
                Driver::Own(s) => match self.rx.try_recv() {
                    Ok(e) => e,
                    Err(TryRecvError::Empty | TryRecvError::Disconnected) => {
                        if left.is_zero() && stepped {
                            return None;
                        }
                        s.step(left.min(s.until_tick()));
                        stepped = true;
                        continue;
                    }
                },
            };
            match event {
                CallEvent::Outcome { index, outcome, metrics } => return Some((index, outcome, metrics)),
                CallEvent::RunHere(inline) => inline.run(),
            }
        }
    }
}

impl<T: Send + 'static> Drop for Stream<T> {
    fn drop(&mut self) {
        if self.next > 0 {
            self.cancel();
            self.close();
        }
    }
}

/// Run `jobs` per volume through the pool, at most `workers` at once for this call (today's
/// `workers()`) and the pool's limits besides: two calls on one volume share its limit (R2), and
/// while some job doesn't need a volume that has work, that volume is held to one fewer (R4a).
/// Results in input order; `done` counts every outcome but [`JobOutcome::NotStarted`] and
/// [`JobOutcome::Cancelled`]; `succeeded` says whether a job's result shows its volume works (a
/// probe that returned Ok), for the recovery rules. A single job runs on the calling thread
/// outside the limits (R2's exception); from a scheduler worker several jobs are refused.
///
/// `cancel` is shared with the pool, which reads it itself: once it is set no further job starts
/// (only the ones already running still read), however long this thread takes to notice.
pub(crate) fn run_jobs<T: Send + 'static>(
    ctx: &Arc<IoContext>,
    jobs: Vec<IoJob<T>>,
    workers: usize,
    succeeded: fn(&T) -> bool,
    cancel: &Arc<AtomicBool>,
    done: &AtomicUsize,
) -> Result<Report<T>, SubmitError> {
    // (nothing to do, e.g. an Add import's batch with no copies: no mount read, no threads)
    if jobs.is_empty() {
        return Ok(Report { outcomes: Vec::new(), metrics: Vec::new() });
    }
    if jobs.len() == 1 {
        let mut jobs = jobs;
        return Ok(match jobs.pop() {
            Some(job) => run_single(ctx, job, succeeded, cancel, done),
            None => Report { outcomes: Vec::new(), metrics: Vec::new() },
        });
    }
    if on_worker_thread() {
        return Err(SubmitError::FromPoolThread);
    }
    let n = jobs.len();
    // (cancelled already: the pool sees it with the jobs, and starts none)
    let mut stream = Stream::open(ctx, workers, succeeded).with_cancel(cancel.clone());
    stream.submit(jobs)?;
    stream.close();
    let mut outcomes: Vec<Option<JobOutcome<T>>> = (0..n).map(|_| None).collect();
    let mut metrics: Vec<Option<JobMetrics>> = vec![None; n];
    let (mut got, mut forwarded) = (0, false);
    while got < n {
        // (the pool already starts no job: this wakes it to fail the queued ones now)
        if !forwarded && cancel.load(Ordering::Relaxed) {
            stream.cancel();
            forwarded = true;
        }
        let next = stream.recv_timeout(ctx.tick);
        if let Some((i, o, m)) = next {
            if let Some(slot) = outcomes.get_mut(i).filter(|s| s.is_none()) {
                if o.counts_as_done() {
                    done.fetch_add(1, Ordering::Relaxed);
                }
                *slot = Some(o);
                if let Some(mm) = metrics.get_mut(i) {
                    *mm = m;
                }
                got += 1;
            }
            continue;
        }
        // the pool stopped with jobs of this call running: nothing watches them now, so cancel
        // leaves them behind at once
        if stream.left_behind() {
            for o in outcomes.iter_mut().filter(|o| o.is_none()) {
                *o = Some(JobOutcome::Cancelled);
            }
            break;
        }
    }
    let outcomes: Vec<JobOutcome<T>> = outcomes.into_iter().map(|o| o.unwrap_or(JobOutcome::NotStarted)).collect();
    log_metrics(&metrics);
    Ok(Report { outcomes, metrics })
}
