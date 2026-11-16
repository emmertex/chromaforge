//! The one queue every AI job goes through: a worker thread of its own takes jobs in priority order, runs each on the
//! chosen device (a job that fails on the GPU with [`JobError::Device`] runs again on the CPU, and the GPU is not used
//! again until [`Queue::set_preference`]), and keeps the results for their owner to [`Queue::drain`].
//!
//! While there is work the queue holds one batch row ([`Rows`]): the activity stack's "Analyzing photos · 37 of 200".
//! Its ✕ sets the batch's cancel flag: the running job sees it through [`Ctx::cancelled`] and everything waiting is
//! dropped as [`Outcome::Cancelled`]. The row goes when the queue is empty.
//!
//! Without threads (the browser build) the worker can't start; then [`Queue::drain`] runs one job in place each call.

use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};

use crate::device::{Device, Kind, Preference, Probe, select};

/// Which jobs go first.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Priority {
    /// Someone is waiting for it (a search typed now, the photo on screen).
    Interactive,
    /// Work done ahead of time (embeddings and scores after an import).
    #[default]
    Background,
}

impl Priority {
    fn lane(self) -> usize {
        match self {
            Priority::Interactive => 0,
            Priority::Background => 1,
        }
    }
}

/// How a job did not finish.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum JobError {
    /// It saw [`Ctx::cancelled`] and stopped.
    #[error("cancelled")]
    Cancelled,
    /// The device failed (out of memory, lost, a kernel error). On the GPU the job runs again on the CPU.
    #[error("device failed: {0}")]
    Device(String),
    /// Anything else (a model missing, a photo that can't be read).
    #[error("{0}")]
    Failed(String),
}

/// What a running job can ask the queue.
pub struct Ctx<'a> {
    device: &'a Device,
    job: &'a AtomicBool,
    batch: &'a AtomicBool,
    stop: &'a AtomicBool,
}

impl Ctx<'_> {
    /// The device to run on.
    pub fn device(&self) -> &Device {
        self.device
    }

    /// Whether the job should stop: it was cancelled, its batch was, or the queue is closing. Check it between steps.
    pub fn cancelled(&self) -> bool {
        self.job.load(Ordering::Relaxed) || self.batch.load(Ordering::Relaxed) || self.stop.load(Ordering::Relaxed)
    }

    /// `Err(Cancelled)` once [`Ctx::cancelled`], for `?` between steps.
    pub fn check(&self) -> Result<(), JobError> {
        if self.cancelled() { Err(JobError::Cancelled) } else { Ok(()) }
    }
}

/// A job's work. It may run twice (on the GPU, then on the CPU after a device failure), so it is `FnMut`.
pub type Work<T> = Box<dyn FnMut(&Ctx) -> Result<T, JobError> + Send>;

/// One piece of work.
pub struct Job<T> {
    key: String,
    name: String,
    priority: Priority,
    work: Work<T>,
}

impl<T> Job<T> {
    /// A job known by `key` (one job per key waits at a time: submitting the key again replaces it), e.g.
    /// `"embed:1234"`. The owner of the results routes them by it.
    pub fn new(key: impl Into<String>, work: impl FnMut(&Ctx) -> Result<T, JobError> + Send + 'static) -> Job<T> {
        Job { key: key.into(), name: String::new(), priority: Priority::Background, work: Box::new(work) }
    }

    /// What the activity row shows while it runs (a file name).
    pub fn name(mut self, name: impl Into<String>) -> Job<T> {
        self.name = name.into();
        self
    }

    pub fn priority(mut self, priority: Priority) -> Job<T> {
        self.priority = priority;
        self
    }

    pub fn key(&self) -> &str {
        &self.key
    }
}

/// How a job ended.
#[derive(Debug, PartialEq)]
pub enum Outcome<T> {
    Done(T),
    Failed(String),
    Cancelled,
}

/// A job that ended, waiting for [`Queue::drain`].
#[derive(Debug)]
pub struct Finished<T> {
    pub key: String,
    pub outcome: Outcome<T>,
    /// Where it ran; `None` when it was cancelled before it started.
    pub device: Option<Kind>,
    /// How long it ran.
    pub millis: u64,
}

/// Makes the batch row (the engine's activity stack). `cancel` is the batch's flag: the row's ✕ sets it.
pub trait Rows: Send + Sync {
    fn begin(&self, cancel: Arc<AtomicBool>) -> Box<dyn Row>;
}

/// The batch row while the queue has work; dropping it ends the row.
pub trait Row: Send {
    fn progress(&self, done: u64, total: u64);
    fn detail(&self, name: &str);
    fn item_done(&self);
    fn item_failed(&self, why: &str);
}

/// Limits.
#[derive(Clone, Copy, Debug)]
pub struct Config {
    pub preference: Preference,
    /// Jobs that may wait at once; [`Queue::submit`] refuses more ([`SubmitError::Full`]).
    pub max_pending: usize,
    /// Results kept for [`Queue::drain`]; past it the oldest go (and a warning is logged).
    pub max_finished: usize,
}

impl Default for Config {
    fn default() -> Config {
        Config { preference: Preference::Auto, max_pending: 100_000, max_finished: 10_000 }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SubmitError {
    #[error("the AI queue is full ({0} jobs waiting)")]
    Full(usize),
    #[error("the AI queue is closed")]
    Closed,
}

/// What the queue is doing (for `ml.status` and the tests).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    /// `None` until the first job picked it.
    pub device: Option<Device>,
    pub preference: Preference,
    pub pending: usize,
    /// The running job's key.
    pub running: Option<String>,
    pub done: u64,
    pub failed: u64,
    pub cancelled: u64,
    /// Whether jobs run on a worker thread (else in [`Queue::drain`]).
    pub threaded: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Worker {
    NotStarted,
    Thread,
    Inline,
}

struct State<T> {
    lanes: [VecDeque<Job<T>>; 2],
    running: Option<(String, Arc<AtomicBool>)>,
    finished: VecDeque<Finished<T>>,
    device: Option<Device>,
    preference: Preference,
    /// The current batch's flag (the row's ✕); a new one for every batch.
    batch: Arc<AtomicBool>,
    /// Jobs ended in the current batch.
    batch_done: u64,
    done: u64,
    failed: u64,
    cancelled: u64,
    worker: Worker,
}

impl<T> State<T> {
    fn pending(&self) -> usize {
        self.lanes.iter().map(VecDeque::len).sum()
    }

    fn push_finished(&mut self, f: Finished<T>, max: usize) {
        let mut dropped = Dropped::default();
        self.push_finished_quietly(f, max, &mut dropped);
        dropped.log();
    }

    /// [`State::push_finished`] that counts the results it drops into `dropped` instead of logging each one: a ✕ on
    /// tens of thousands of waiting jobs would otherwise write a line per job while holding the lock the UI drains
    /// every frame (issue #979: 48,946 lines, a 532 ms frame).
    fn push_finished_quietly(&mut self, f: Finished<T>, max: usize, dropped: &mut Dropped) {
        match &f.outcome {
            Outcome::Done(_) => self.done = self.done.saturating_add(1),
            Outcome::Failed(_) => self.failed = self.failed.saturating_add(1),
            Outcome::Cancelled => self.cancelled = self.cancelled.saturating_add(1),
        }
        self.finished.push_back(f);
        while self.finished.len() > max.max(1) {
            if let Some(old) = self.finished.pop_front() {
                dropped.add(old);
            }
        }
    }

    /// Everything waiting ends as cancelled.
    fn cancel_pending(&mut self, max: usize) -> usize {
        let jobs: Vec<Job<T>> = self.lanes.iter_mut().flat_map(|l| l.drain(..)).collect();
        let n = jobs.len();
        let mut dropped = Dropped::default();
        for j in jobs {
            self.push_finished_quietly(Finished { key: j.key, outcome: Outcome::Cancelled, device: None, millis: 0 }, max, &mut dropped);
        }
        dropped.log();
        n
    }
}

/// Results dropped because nobody took them, logged as one line.
#[derive(Default)]
struct Dropped {
    count: usize,
    cancelled: usize,
    first: Option<String>,
    last: Option<String>,
}

impl Dropped {
    fn add<T>(&mut self, f: Finished<T>) {
        self.count = self.count.saturating_add(1);
        if matches!(f.outcome, Outcome::Cancelled) {
            self.cancelled = self.cancelled.saturating_add(1);
        }
        if self.first.is_none() {
            self.first = Some(f.key);
        } else {
            self.last = Some(f.key);
        }
    }

    fn log(self) {
        match (self.count, self.first, self.last) {
            (0, ..) => {}
            (1, Some(key), _) => log::warn!("ml queue: result of {key} dropped (nobody took it)"),
            (n, first, last) => log::warn!(
                "ml queue: {n} results dropped ({} of them cancelled), {} … {} (nobody took them)",
                self.cancelled,
                first.unwrap_or_default(),
                last.unwrap_or_default()
            ),
        }
    }
}

struct Shared<T> {
    state: Mutex<State<T>>,
    wake: Condvar,
    stop: AtomicBool,
    probe: Probe,
    rows: Option<Arc<dyn Rows>>,
    /// Only the thread running jobs touches it.
    row: Mutex<Option<Box<dyn Row>>>,
    config: Config,
}

impl<T> Shared<T> {
    fn lock(&self) -> MutexGuard<'_, State<T>> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn row(&self) -> MutexGuard<'_, Option<Box<dyn Row>>> {
        self.row.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The shared AI job queue. Dropping it stops the worker after the job it is running (which sees
/// [`Ctx::cancelled`]); the thread is not waited for.
pub struct Queue<T: Send + 'static> {
    shared: Arc<Shared<T>>,
}

enum Step {
    Ran,
    Idle,
    Stopped,
}

impl<T: Send + 'static> Queue<T> {
    /// A queue that finds its GPU with `probe` (called on the worker, before the first job) and shows its batches
    /// through `rows`. Nothing starts until the first [`Queue::submit`].
    pub fn new(config: Config, probe: Probe, rows: Option<Arc<dyn Rows>>) -> Queue<T> {
        let state = State {
            lanes: [VecDeque::new(), VecDeque::new()],
            running: None,
            finished: VecDeque::new(),
            device: None,
            preference: config.preference,
            batch: Arc::new(AtomicBool::new(false)),
            batch_done: 0,
            done: 0,
            failed: 0,
            cancelled: 0,
            worker: Worker::NotStarted,
        };
        Queue {
            shared: Arc::new(Shared {
                state: Mutex::new(state),
                wake: Condvar::new(),
                stop: AtomicBool::new(false),
                probe,
                rows,
                row: Mutex::new(None),
                config,
            }),
        }
    }

    /// Queue `job`. A job with the same key that is still waiting is replaced (it keeps its place unless the priority
    /// changed).
    pub fn submit(&self, job: Job<T>) -> Result<(), SubmitError> {
        if self.shared.stop.load(Ordering::Relaxed) {
            return Err(SubmitError::Closed);
        }
        let mut st = self.shared.lock();
        let lane = job.priority.lane();
        let mut job = Some(job);
        if let (Some(j), Some(l)) = (job.as_ref(), st.lanes.get_mut(lane))
            && let Some(slot) = l.iter_mut().find(|w| w.key == j.key)
            && let Some(j) = job.take()
        {
            *slot = j;
        }
        if let Some(j) = job {
            // waiting in the other lane: it moves
            for l in st.lanes.iter_mut() {
                l.retain(|w| w.key != j.key);
            }
            let pending = st.pending();
            if pending >= self.shared.config.max_pending {
                return Err(SubmitError::Full(pending));
            }
            if let Some(l) = st.lanes.get_mut(lane) {
                l.push_back(j);
            }
        }
        if st.worker == Worker::NotStarted {
            st.worker = self.start_worker();
        }
        drop(st);
        self.shared.wake.notify_all();
        Ok(())
    }

    fn start_worker(&self) -> Worker {
        let shared = self.shared.clone();
        match std::thread::Builder::new().name("ml-queue".into()).spawn(move || run_worker(&shared)) {
            Ok(_) => Worker::Thread,
            Err(e) => {
                log::info!("ml queue: no worker thread ({e}); jobs run when results are collected");
                Worker::Inline
            }
        }
    }

    /// Cancel the job known by `key`: dropped if waiting, told to stop if running. Whether there was one.
    pub fn cancel(&self, key: &str) -> bool {
        let max = self.shared.config.max_finished;
        let mut st = self.shared.lock();
        let mut found = false;
        for lane in 0..2 {
            let Some(l) = st.lanes.get_mut(lane) else { continue };
            let Some(i) = l.iter().position(|j| j.key == key) else { continue };
            if let Some(j) = l.remove(i) {
                st.push_finished(Finished { key: j.key, outcome: Outcome::Cancelled, device: None, millis: 0 }, max);
                found = true;
            }
        }
        if let Some((k, flag)) = &st.running
            && k == key
        {
            flag.store(true, Ordering::Relaxed);
            found = true;
        }
        found
    }

    /// Cancel everything (what the row's ✕ does): the waiting jobs end as cancelled and the running one is told to
    /// stop. How many jobs were waiting or running.
    pub fn cancel_all(&self) -> usize {
        let max = self.shared.config.max_finished;
        let mut st = self.shared.lock();
        let mut n = st.cancel_pending(max);
        if let Some((_, flag)) = &st.running {
            flag.store(true, Ordering::Relaxed);
            n = n.saturating_add(1);
        }
        n
    }

    /// The results of the jobs that ended since the last call, oldest first. Without a worker thread this first runs
    /// the next job in place.
    pub fn drain(&self) -> Vec<Finished<T>> {
        let inline = self.shared.lock().worker == Worker::Inline;
        if inline {
            let _ = step(&self.shared);
        }
        self.shared.lock().finished.drain(..).collect()
    }

    /// Use `preference` from the next job on. Also asks for the GPU again after it failed.
    pub fn set_preference(&self, preference: Preference) {
        let mut st = self.shared.lock();
        st.preference = preference;
        st.device = None;
    }

    pub fn status(&self) -> Status {
        let st = self.shared.lock();
        Status {
            device: st.device.clone(),
            preference: st.preference,
            pending: st.pending(),
            running: st.running.as_ref().map(|(k, _)| k.clone()),
            done: st.done,
            failed: st.failed,
            cancelled: st.cancelled,
            threaded: st.worker == Worker::Thread,
        }
    }

    /// Whether a job is waiting or running.
    pub fn busy(&self) -> bool {
        let st = self.shared.lock();
        st.running.is_some() || st.pending() > 0
    }
}

impl<T: Send + 'static> Drop for Queue<T> {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        let mut st = self.shared.lock();
        if let Some((_, flag)) = &st.running {
            flag.store(true, Ordering::Relaxed);
        }
        let max = self.shared.config.max_finished;
        st.cancel_pending(max);
        drop(st);
        self.shared.wake.notify_all();
    }
}

fn run_worker<T: Send + 'static>(shared: &Shared<T>) {
    loop {
        match step(shared) {
            Step::Ran => {}
            Step::Stopped => break,
            Step::Idle => {
                let st = shared.lock();
                let _st =
                    shared.wake.wait_while(st, |s| s.pending() == 0 && !shared.stop.load(Ordering::Relaxed)).unwrap_or_else(PoisonError::into_inner);
            }
        }
    }
    // the row (if any) goes with the worker
    shared.row().take();
}

/// Run the next job, if there is one.
fn step<T: Send + 'static>(shared: &Shared<T>) -> Step {
    let max = shared.config.max_finished;
    if shared.stop.load(Ordering::Relaxed) {
        return Step::Stopped;
    }
    let mut st = shared.lock();
    if st.batch.load(Ordering::Relaxed) {
        // the row's ✕: drop what waits, then the batch ends below
        st.cancel_pending(max);
    }
    let next = st.lanes.iter_mut().find_map(VecDeque::pop_front);
    let Some(mut job) = next else {
        // idle: the batch (and its row) ends
        if st.batch.load(Ordering::Relaxed) {
            st.batch = Arc::new(AtomicBool::new(false));
        }
        st.batch_done = 0;
        drop(st);
        shared.row().take();
        return Step::Idle;
    };
    let device = match st.device.clone() {
        Some(d) => d,
        None => {
            let pref = st.preference;
            // the probe may take a while (device creation): not under the lock
            drop(st);
            let d = select(pref, shared.probe.as_ref());
            log::info!("ml queue: running on {}{}", d.name, d.why_cpu.as_deref().map(|w| format!(" ({w})")).unwrap_or_default());
            st = shared.lock();
            st.device.get_or_insert(d).clone()
        }
    };
    let flag = Arc::new(AtomicBool::new(false));
    st.running = Some((job.key.clone(), flag.clone()));
    let batch = st.batch.clone();
    let total = st.batch_done.saturating_add(st.pending() as u64).saturating_add(1);
    let done = st.batch_done;
    drop(st);

    {
        let mut row = shared.row();
        if row.is_none()
            && let Some(rows) = &shared.rows
        {
            *row = Some(rows.begin(batch.clone()));
        }
        if let Some(r) = row.as_ref() {
            r.progress(done, total);
            r.detail(&job.name);
        }
    }

    let started = web_time::Instant::now();
    let mut device = device;
    let result = loop {
        let ctx = Ctx { device: &device, job: &flag, batch: &batch, stop: &shared.stop };
        let r = catch_unwind(AssertUnwindSafe(|| (job.work)(&ctx)));
        match r {
            Ok(Err(JobError::Device(why))) if device.is_gpu() => {
                log::warn!("ml queue: {} failed on {} ({why}); the CPU takes over", job.key, device.name);
                let cpu = Device::cpu(Some(format!("the GPU failed: {why}")));
                shared.lock().device = Some(cpu.clone());
                device = cpu;
            }
            other => break other,
        }
    };
    let outcome = match result {
        Ok(Ok(v)) => Outcome::Done(v),
        Ok(Err(JobError::Cancelled)) => Outcome::Cancelled,
        Ok(Err(e)) => Outcome::Failed(e.to_string()),
        Err(_) => {
            log::error!("ml queue: {} panicked", job.key);
            Outcome::Failed("the job crashed".into())
        }
    };
    let millis = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);

    let mut st = shared.lock();
    st.running = None;
    st.batch_done = st.batch_done.saturating_add(1);
    let done = st.batch_done;
    let total = done.saturating_add(st.pending() as u64);
    if let Some(r) = shared.row().as_ref() {
        match &outcome {
            Outcome::Failed(why) => r.item_failed(&format!("{}: {why}", if job.name.is_empty() { &job.key } else { &job.name })),
            _ => r.item_done(),
        }
        r.progress(done, total);
    }
    st.push_finished(Finished { key: job.key, outcome, device: Some(device.kind), millis }, max);
    Step::Ran
}
