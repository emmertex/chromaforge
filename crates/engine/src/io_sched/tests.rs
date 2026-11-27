//! The per-volume scheduler (I/O spec §7, "Scheduler (PR 1)"), on fake volumes: an injected mount
//! list, a clock the test steps, jobs that move bytes, compute, hang or panic on command.
//!
//! Timing: the bookkeeping ticks every millisecond of real time; every rule reads the fake clock,
//! so each threshold is pinned exactly (at, just below, just above). A test waits for ticks
//! ([`H::ticks`]) instead of sleeping.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use lcc_catalog::folders::Mounts;

use super::clock::{Clock, FakeClock, SEC};
use super::run::{IoJob, JobOutcome, Report, run_jobs};
use super::shared::{CANCEL_SILENCE, COMPUTING_FACTOR, IDLE_AFTER, RECOVERY_EVERY, RECOVERY_STUCK, STALL_GRACE, STALL_WITH_BYTES, WAKE_JUMP};
use super::{IoContext, mounts::MountReader};

pub(super) type R = Result<u32, String>;

/// A test's scheduler context and its clock.
pub(super) struct H {
    pub(super) clock: Arc<FakeClock>,
    pub(super) ctx: Arc<IoContext>,
}

pub(super) fn list(points: &[&str]) -> MountReader {
    let points: Vec<String> = points.iter().map(|s| s.to_string()).collect();
    Arc::new(move || Some(Mounts::from_points(points.clone(), false)))
}

impl H {
    pub(super) fn new() -> H {
        H::with_reader(list(&["/", "/a", "/b", "/c", "/srv/nas"]))
    }

    pub(super) fn with_reader(reader: MountReader) -> H {
        let clock = Arc::new(FakeClock::default());
        clock.advance(1000 * SEC);
        let mut ctx = IoContext::new(clock.clone(), reader, Duration::from_millis(1));
        // (tests step the clock by whole stall times: no late tick is a wake unless a test says so)
        ctx.late_tick_is_wake = false;
        H { clock, ctx: Arc::new(ctx) }
    }

    pub(super) fn advance(&self, nanos: u64) {
        self.clock.advance(nanos);
    }

    pub(super) fn now(&self) -> u64 {
        self.clock.mono()
    }

    /// Run `jobs` on another thread (the test drives the clock meanwhile).
    pub(super) fn start(&self, jobs: Vec<IoJob<R>>, workers: usize) -> Call {
        let (cancel, done) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicUsize::new(0)));
        let (ctx, c, d) = (self.ctx.clone(), cancel.clone(), done.clone());
        let handle = std::thread::spawn(move || run_jobs(&ctx, jobs, workers, Result::is_ok, &c, &d));
        Call { handle, cancel, done }
    }

    /// Run `jobs` here and return the report.
    pub(super) fn run(&self, jobs: Vec<IoJob<R>>, workers: usize) -> (Report<R>, usize) {
        let (cancel, done) = (AtomicBool::new(false), AtomicUsize::new(0));
        let r = run_jobs(&self.ctx, jobs, workers, Result::is_ok, &cancel, &done);
        (r, done.load(Ordering::SeqCst))
    }
}

pub(super) struct Call {
    pub(super) handle: JoinHandle<Report<R>>,
    pub(super) cancel: Arc<AtomicBool>,
    pub(super) done: Arc<AtomicUsize>,
}

impl Call {
    /// Wait for `n` more bookkeeping ticks, or for the call to return (a stall or cancel may end
    /// it, and then no tick follows).
    pub(super) fn settle(&self, h: &H, n: u64) {
        let start = h.ctx.ticks.load(Ordering::SeqCst);
        wait_until(|| self.handle.is_finished() || h.ctx.ticks.load(Ordering::SeqCst) >= start + n, "ticks or the end of the call");
    }

    pub(super) fn finished(&self) -> bool {
        self.handle.is_finished()
    }

    pub(super) fn join(self) -> Report<R> {
        wait_until(|| self.handle.is_finished(), "the call to return");
        self.handle.join().unwrap()
    }
}

pub(super) fn wait_until(mut cond: impl FnMut() -> bool, what: &str) {
    let start = std::time::Instant::now();
    while !cond() {
        assert!(start.elapsed() < Duration::from_secs(20), "timed out waiting for {what}");
        std::thread::sleep(Duration::from_micros(200));
    }
}

/// A latch jobs wait on until the test releases them.
#[derive(Default)]
pub(super) struct Latch {
    open: Mutex<bool>,
    cv: Condvar,
}

impl Latch {
    pub(super) fn is_open(&self) -> bool {
        *self.open.lock().unwrap()
    }

    pub(super) fn new() -> Arc<Latch> {
        Arc::new(Latch::default())
    }

    pub(super) fn wait(&self) {
        let mut g = self.open.lock().unwrap();
        while !*g {
            g = self.cv.wait(g).unwrap();
        }
    }

    pub(super) fn release(&self) {
        *self.open.lock().unwrap() = true;
        self.cv.notify_all();
    }
}

pub(super) fn job(path: &str, f: impl FnOnce() -> R + Send + 'static) -> IoJob<R> {
    IoJob::read(path.to_string(), Box::new(f))
}

/// Count `n` bytes for the thread's job (as the capped counting reader does): read from its source.
pub(super) fn bytes(n: usize) {
    if let Some(j) = crate::memory::current_job() {
        j.add_bytes(n);
    }
}

/// Count `n` bytes written to the thread's job's destination (as a copy's counting writer does):
/// its current call is then on the destination.
pub(super) fn wrote(n: usize) {
    if let Some(j) = crate::memory::current_job() {
        j.add(super::job::ByteKind::Written, n);
    }
}

pub(super) fn ok_count(r: &Report<R>) -> usize {
    r.outcomes.iter().filter(|o| matches!(o, JobOutcome::Done(Ok(_)))).count()
}

pub(super) fn vnr_count(r: &Report<R>) -> usize {
    r.outcomes.iter().filter(|o| matches!(o, JobOutcome::VolumeNotResponding { .. })).count()
}

/// `n` jobs on `vol` that count a byte, signal they started, then wait for `latch`.
pub(super) fn hanging(vol: &str, n: usize, latch: &Arc<Latch>, started: &Arc<AtomicUsize>, with_bytes: bool) -> Vec<IoJob<R>> {
    (0..n)
        .map(|i| {
            let (latch, started) = (latch.clone(), started.clone());
            job(&format!("{vol}/h{i}"), move || {
                if with_bytes {
                    bytes(1);
                }
                started.fetch_add(1, Ordering::SeqCst);
                latch.wait();
                Ok(i as u32)
            })
        })
        .collect()
}

pub(super) fn quick(vol: &str, n: usize) -> Vec<IoJob<R>> {
    (0..n)
        .map(|i| {
            job(&format!("{vol}/q{i}"), move || {
                bytes(10);
                Ok(i as u32)
            })
        })
        .collect()
}

/// Concurrency seen per volume and overall.
#[derive(Default)]
struct Conc {
    now: Mutex<(HashMap<String, usize>, usize)>,
    max_vol: Mutex<HashMap<String, usize>>,
    max_all: AtomicUsize,
    /// Most running on a volume while another volume still had unfinished jobs.
    max_shared: Mutex<HashMap<String, usize>>,
    unfinished: Mutex<HashMap<String, usize>>,
}

impl Conc {
    fn enter(&self, vol: &str) {
        let mut g = self.now.lock().unwrap();
        let c = g.0.entry(vol.into()).or_default();
        *c += 1;
        let c = *c;
        g.1 += 1;
        self.max_all.fetch_max(g.1, Ordering::SeqCst);
        let mut mv = self.max_vol.lock().unwrap();
        let m = mv.entry(vol.into()).or_default();
        *m = (*m).max(c);
        drop(mv);
        let others = self.unfinished.lock().unwrap().iter().any(|(v, n)| v != vol && *n > 0);
        if others {
            let mut ms = self.max_shared.lock().unwrap();
            let e = ms.entry(vol.into()).or_default();
            *e = (*e).max(c);
        }
    }

    fn leave(&self, vol: &str) {
        let mut g = self.now.lock().unwrap();
        *g.0.entry(vol.into()).or_default() -= 1;
        g.1 -= 1;
        *self.unfinished.lock().unwrap().entry(vol.into()).or_default() -= 1;
    }
}

fn counted(vol: &str, n: usize, conc: &Arc<Conc>) -> Vec<IoJob<R>> {
    *conc.unfinished.lock().unwrap().entry(vol.into()).or_default() += n;
    (0..n)
        .map(|i| {
            let (conc, v) = (conc.clone(), vol.to_string());
            job(&format!("{vol}/c{i}"), move || {
                conc.enter(&v);
                bytes(1);
                std::thread::sleep(Duration::from_millis(2));
                conc.leave(&v);
                Ok(i as u32)
            })
        })
        .collect()
}

// ---------------------------------------------------------------- limits and admission

/// R2 and R4a: neither the call's cap nor a volume's limit is ever exceeded, and with two busy
/// volumes neither runs more than `cap − 1`.
#[test]
fn limits_are_never_exceeded() {
    for workers in [1usize, 2, 4, 8] {
        let h = H::new();
        let conc = Arc::new(Conc::default());
        let mut jobs = counted("/a", 24, &conc);
        jobs.extend(counted("/b", 24, &conc));
        jobs.extend(counted("/c", 6, &conc));
        let (r, done) = h.run(jobs, workers);
        assert_eq!(ok_count(&r), 54);
        assert_eq!(done, 54);
        let cap = workers.max(2);
        assert!(conc.max_all.load(Ordering::SeqCst) <= cap, "workers {workers}: call cap");
        for (v, m) in conc.max_vol.lock().unwrap().iter() {
            assert!(*m <= cap, "workers {workers}: {v} ran {m}");
        }
        for (v, m) in conc.max_shared.lock().unwrap().iter() {
            assert!(*m <= (cap - 1).max(1), "workers {workers}: {v} ran {m} while another volume had work");
        }
    }
}

/// Round-robin admission: every volume with work starts a job within one round.
#[test]
fn every_volume_starts_within_one_round() {
    let h = H::new();
    let order = Arc::new(Mutex::new(Vec::new()));
    let latch = Latch::new();
    let mut jobs = Vec::new();
    for vol in ["/a", "/b", "/c"] {
        for i in 0..6 {
            let (order, latch, v) = (order.clone(), latch.clone(), vol.to_string());
            jobs.push(job(&format!("{vol}/{i}"), move || {
                order.lock().unwrap().push(v);
                latch.wait();
                Ok(0)
            }));
        }
    }
    // input order is all of /a, then /b, then /c: a FIFO would start /a three times
    let call = h.start(jobs, 3);
    wait_until(|| order.lock().unwrap().len() >= 3, "three starts");
    let mut first: Vec<String> = order.lock().unwrap().iter().take(3).cloned().collect();
    first.sort();
    assert_eq!(first, ["/a", "/b", "/c"]);
    latch.release();
    assert_eq!(ok_count(&call.join()), 18);
}

/// One volume: today's thread count (`workers`) and start order (input order).
#[test]
fn one_volume_runs_todays_threads_in_input_order() {
    let h = H::new();
    let threads = Arc::new(Mutex::new(Vec::new()));
    let order = Arc::new(Mutex::new(Vec::new()));
    let barrier = Arc::new(std::sync::Barrier::new(4));
    let jobs = (0..20)
        .map(|i| {
            let (threads, order, barrier) = (threads.clone(), order.clone(), barrier.clone());
            job(&format!("/a/{i}"), move || {
                order.lock().unwrap().push(i);
                threads.lock().unwrap().push(std::thread::current().id());
                if i < 4 {
                    barrier.wait();
                }
                Ok(i)
            })
        })
        .collect();
    let (r, _) = h.run(jobs, 4);
    assert_eq!(ok_count(&r), 20);
    let mut t = threads.lock().unwrap().clone();
    t.sort_by_key(|id| format!("{id:?}"));
    t.dedup();
    assert_eq!(t.len(), 4, "today's thread count");
    assert!(!t.contains(&std::thread::current().id()));
    // the first four in input order start first (they hold every slot until all four are in;
    // which worker takes which is a race, as today); exact order with one worker below
    let mut first: Vec<u32> = order.lock().unwrap().iter().take(4).copied().collect();
    first.sort();
    assert_eq!(first, [0, 1, 2, 3]);
    let (r, _) = h.run((0..10u32).map(|i| job(&format!("/a/s{i}"), move || Ok(i))).collect(), 1);
    let got: Vec<u32> = r.outcomes.into_iter().map(|o| if let JobOutcome::Done(Ok(i)) = o { i } else { 99 }).collect();
    assert_eq!(got, (0..10).collect::<Vec<_>>(), "results in input order");
}

/// A single job runs on the calling thread.
#[test]
fn a_single_job_runs_on_the_calling_thread() {
    let h = H::new();
    let me = std::thread::current().id();
    let (r, done) = h.run(vec![job("/a/1", move || Ok(u32::from(std::thread::current().id() == me)))], 8);
    assert!(matches!(r.outcomes.as_slice(), [JobOutcome::Done(Ok(1))]));
    assert_eq!(done, 1);
}

/// `workers()` 1: two volumes run two threads and a hung volume never stops the other; one
/// volume runs one thread, as today.
#[test]
fn one_cpu_two_volumes_two_threads() {
    let h = H::new();
    let (latch, started) = (Latch::new(), Arc::new(AtomicUsize::new(0)));
    let mut jobs = hanging("/a", 1, &latch, &started, true);
    jobs.extend(quick("/b", 5));
    let call = h.start(jobs, 1);
    wait_until(|| call.done.load(Ordering::SeqCst) >= 5, "/b to finish beside the hung /a");
    assert!(!call.finished());
    latch.release();
    let r = call.join();
    assert_eq!(ok_count(&r), 6);

    let threads = Arc::new(Mutex::new(Vec::new()));
    let jobs = (0..6)
        .map(|i| {
            let t = threads.clone();
            job(&format!("/a/{i}"), move || {
                t.lock().unwrap().push(std::thread::current().id());
                Ok(i)
            })
        })
        .collect();
    let (r, _) = h.run(jobs, 1);
    assert_eq!(ok_count(&r), 6);
    let mut t = threads.lock().unwrap().clone();
    t.dedup();
    assert_eq!(t.len(), 1, "one thread, as today");
}

/// Outcomes are typed; a panicking job fails only itself; `done` counts every outcome but
/// not-started and cancelled.
#[test]
fn a_panic_fails_only_its_job() {
    let h = H::new();
    let mut jobs = quick("/a", 3);
    jobs.insert(1, job("/a/boom", || panic!("boom")));
    let (r, done) = h.run(jobs, 4);
    assert_eq!(ok_count(&r), 3);
    match r.outcomes.get(1) {
        Some(JobOutcome::Panicked(m)) => assert!(m.contains("boom"), "{m}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(done, 4);
}

/// A spawn failure falls back without panicking: on an existing thread, or on the calling thread
/// when none exists.
#[test]
fn spawn_failure_falls_back() {
    let h = H::new();
    h.ctx.spawn_budget.store(0, Ordering::SeqCst);
    let me = std::thread::current().id();
    let jobs = (0..5).map(|i| job(&format!("/a/{i}"), move || Ok(u32::from(std::thread::current().id() == me)))).collect();
    let (r, done) = h.run(jobs, 4);
    assert!(r.outcomes.iter().all(|o| matches!(o, JobOutcome::Done(Ok(1)))), "all on the calling thread");
    assert_eq!(done, 5);

    let h = H::new();
    h.ctx.spawn_budget.store(1, Ordering::SeqCst);
    let threads = Arc::new(Mutex::new(Vec::new()));
    let jobs = (0..8)
        .map(|i| {
            let t = threads.clone();
            job(&if i % 2 == 0 { format!("/a/{i}") } else { format!("/b/{i}") }, move || {
                t.lock().unwrap().push(std::thread::current().id());
                Ok(i)
            })
        })
        .collect();
    let (r, _) = h.run(jobs, 4);
    assert_eq!(ok_count(&r), 8);
    let mut t = threads.lock().unwrap().clone();
    t.sort_by_key(|id| format!("{id:?}"));
    t.dedup();
    assert_eq!(t.len(), 1, "every job on the one thread that started");
    assert_ne!(t.first(), Some(&me));
}

// ---------------------------------------------------------------- stall rule

/// Start `n` hanging jobs on `/a` (counting a byte when `with_bytes`) plus nothing else, wait until
/// the bookkeeping has seen them, advance `by`, and say whether `/a` was declared not responding.
fn stalls_after(h: &H, with_bytes: bool, by: u64) -> (bool, Call, Arc<Latch>) {
    let (latch, started) = (Latch::new(), Arc::new(AtomicUsize::new(0)));
    let call = h.start(hanging("/a", 2, &latch, &started, with_bytes), 4);
    wait_until(|| started.load(Ordering::SeqCst) == 2, "the jobs to start");
    call.settle(h, 3);
    h.advance(by);
    call.settle(h, 3);
    (call.finished(), call, latch)
}

/// R4c: a volume whose jobs never return is declared not responding after the stall time while
/// the others finish, and its files fail with that reason.
#[test]
fn a_hung_volume_is_declared_not_responding() {
    let h = H::new();
    let (latch, started) = (Latch::new(), Arc::new(AtomicUsize::new(0)));
    let mut jobs = hanging("/a", 3, &latch, &started, true);
    jobs.extend(hanging("/a", 3, &latch, &started, true).into_iter().map(|mut j| {
        j.source = j.source.replace("/h", "/queued");
        j
    }));
    jobs.extend(quick("/b", 4));
    let call = h.start(jobs, 4);
    // (/a holds 3 of the 4 slots while /b has work, then takes the 4th)
    wait_until(|| started.load(Ordering::SeqCst) == 4 && call.done.load(Ordering::SeqCst) == 4, "/a hung and /b done");
    call.settle(&h, 3);
    h.advance(STALL_WITH_BYTES);
    let done = call.done.clone();
    let r = call.join();
    assert_eq!(vnr_count(&r), 6, "running and queued jobs of /a");
    assert_eq!(done.load(Ordering::SeqCst), 10, "`done` counts the files that failed too");
    assert_eq!(ok_count(&r), 4);
    for o in r.outcomes.iter().take(6) {
        match o {
            JobOutcome::VolumeNotResponding { volume, .. } => assert_eq!(volume.id(), "/a"),
            other => panic!("{other:?}"),
        }
    }
    assert_eq!(h.ctx.shared.entries(), 4, "the four hung jobs are in the registry");
    latch.release();
    wait_until(|| h.ctx.shared.entries() == 0, "late returns to leave the registry");
}

/// The 30 s rule at, just below and just above.
#[test]
fn stall_after_30_s_with_bytes() {
    for (by, stalls) in [(STALL_WITH_BYTES - 1, false), (STALL_WITH_BYTES, true), (STALL_WITH_BYTES + 1, true)] {
        let h = H::new();
        let (stalled, call, latch) = stalls_after(&h, true, by);
        assert_eq!(stalled, stalls, "{by}");
        latch.release();
        let r = call.join();
        assert_eq!(vnr_count(&r), if stalls { 2 } else { 0 });
    }
}

/// A job type that reports no bytes uses the completion rule: max(120 s, 4 × the longest finished
/// job), at, just below and just above; the completion rule wins when longer than 120 s.
#[test]
fn completion_rule_without_bytes() {
    for (longest, threshold) in [(10 * SEC, STALL_GRACE), (40 * SEC, 160 * SEC)] {
        for (by, stalls) in [(threshold - 1, false), (threshold, true), (threshold + 1, true)] {
            let h = H::new();
            // a finished job of `longest` on /a, reporting no bytes
            let latch = Latch::new();
            let l2 = latch.clone();
            let (go, started) = (Latch::new(), Arc::new(AtomicUsize::new(0)));
            let s2 = started.clone();
            let call = h.start(
                vec![
                    job("/a/long", move || {
                        s2.fetch_add(1, Ordering::SeqCst);
                        l2.wait();
                        Ok(0)
                    }),
                    job("/a/x", || Ok(1)),
                ],
                1,
            );
            wait_until(|| started.load(Ordering::SeqCst) == 1, "the long job");
            h.advance(longest);
            latch.release();
            assert_eq!(ok_count(&call.join()), 2);
            drop(go);
            let (stalled, call, latch) = stalls_after(&h, false, by);
            assert_eq!(stalled, stalls, "longest {longest}, by {by}");
            latch.release();
            call.join();
        }
    }
}

/// Moves bytes on /a and finishes (so /a has reported bytes), then leaves it idle for `idle`.
fn used_then_idle(h: &H, idle: u64) {
    let (r, _) = h.run(quick("/a", 2), 2);
    assert_eq!(ok_count(&r), 2);
    h.advance(idle);
}

/// A volume idle for more than a minute (at, just below, just above 60 s) gets at least 120 s
/// before its first stall; then 30 s once it has moved bytes.
#[test]
fn idle_volume_gets_the_wake_grace() {
    for (idle, grace) in [(IDLE_AFTER - 1, false), (IDLE_AFTER, false), (IDLE_AFTER + 1, true)] {
        let h = H::new();
        used_then_idle(&h, idle);
        let (stalled, call, latch) = stalls_after(&h, false, STALL_WITH_BYTES);
        assert_eq!(stalled, !grace, "idle {idle}: 30 s stalls only without the grace");
        if grace {
            h.advance(STALL_GRACE - STALL_WITH_BYTES - 1);
            call.settle(&h, 3);
            assert!(!call.finished(), "not before 120 s");
            h.advance(1);
            call.settle(&h, 3);
            assert!(call.finished(), "at 120 s");
        }
        latch.release();
        call.join();
    }
    // after the grace, once it moves bytes again, 30 s
    let h = H::new();
    used_then_idle(&h, IDLE_AFTER + 1);
    let (stalled, call, latch) = stalls_after(&h, true, STALL_WITH_BYTES);
    assert!(stalled, "bytes moved: back to 30 s");
    latch.release();
    call.join();
}

/// A wall clock that jumps more than 5 s past the monotonic one between ticks is a wake: every
/// volume gets 120 s from it (jump at, just below, just above 5 s).
#[test]
fn a_wake_from_sleep_gives_120_s() {
    for (jump, wake) in [(WAKE_JUMP - 1, false), (WAKE_JUMP, false), (WAKE_JUMP + 1, true)] {
        let h = H::new();
        let (latch, started) = (Latch::new(), Arc::new(AtomicUsize::new(0)));
        let call = h.start(hanging("/a", 2, &latch, &started, true), 4);
        wait_until(|| started.load(Ordering::SeqCst) == 2, "start");
        call.settle(&h, 3);
        h.advance(20 * SEC);
        call.settle(&h, 3);
        h.clock.sleep(jump);
        call.settle(&h, 3);
        h.advance(10 * SEC);
        call.settle(&h, 3);
        assert_eq!(call.finished(), !wake, "jump {jump}");
        if wake {
            h.advance(STALL_GRACE - 10 * SEC - 1);
            call.settle(&h, 3);
            assert!(!call.finished(), "within 120 s of the wake");
            h.advance(1);
            call.settle(&h, 3);
            assert!(call.finished(), "120 s after the wake");
        }
        latch.release();
        call.join();
    }
}

/// Where the monotonic clock counts sleep (Windows), a tick more than 5 s later than its period
/// is a wake: the gap isn't silence, and every volume gets 120 s from it. Gap at, just below and
/// just above the period + 5 s; then the grace's end, stepped as a running computer does.
#[test]
fn a_late_tick_is_a_wake_where_the_clock_counts_sleep() {
    let period = 1_000_000u64; // H's tick: 1 ms
    let start_hung = |h: &H| {
        let (latch, started) = (Latch::new(), Arc::new(AtomicUsize::new(0)));
        let call = h.start(hanging("/a", 2, &latch, &started, true), 4);
        wait_until(|| started.load(Ordering::SeqCst) == 2, "start");
        call.settle(h, 3);
        (call, latch)
    };
    // step the clocks together, at most 1 s a tick (no step is late)
    let run_for = |h: &H, call: &Call, nanos: u64| {
        let mut left = nanos;
        while left > 0 && !call.finished() {
            let step = left.min(SEC);
            h.advance(step);
            call.settle(h, 2);
            left -= step;
        }
    };
    for (gap, wake) in [(period + WAKE_JUMP - 1, false), (period + WAKE_JUMP, false), (period + WAKE_JUMP + 1, true), (60 * SEC, true)] {
        let clock = Arc::new(FakeClock::default());
        clock.advance(1000 * SEC);
        let mut ctx = IoContext::new(clock.clone(), list(&["/", "/a"]), Duration::from_millis(1));
        ctx.late_tick_is_wake = true;
        let h = H { clock, ctx: Arc::new(ctx) };
        let (call, latch) = start_hung(&h);
        // one late tick (both clocks move together, as on Windows)
        h.advance(gap);
        call.settle(&h, 3);
        if gap >= STALL_WITH_BYTES {
            assert!(!call.finished(), "a 60 s sleep doesn't stall the volume at once");
        }
        run_for(&h, &call, STALL_WITH_BYTES);
        assert_eq!(call.finished(), !wake, "gap {gap}: 30 s more of silence stalls only without a wake");
        if wake {
            run_for(&h, &call, STALL_GRACE - STALL_WITH_BYTES - 2 * SEC);
            assert!(!call.finished(), "within 120 s of the wake");
            run_for(&h, &call, 3 * SEC);
            assert!(call.finished(), "after 120 s of silence since the wake");
        }
        latch.release();
        call.join();
    }
}

/// Start two jobs on /a that count a byte, then compute (inside the guard) until released.
fn computing_jobs(h: &H) -> (Call, Arc<Latch>) {
    let (latch, started) = (Latch::new(), Arc::new(AtomicUsize::new(0)));
    let jobs = (0..2)
        .map(|i| {
            let (latch, started) = (latch.clone(), started.clone());
            job(&format!("/a/{i}"), move || {
                bytes(1);
                let _computing = crate::memory::computing();
                started.fetch_add(1, Ordering::SeqCst);
                latch.wait();
                Ok(i)
            })
        })
        .collect();
    let call = h.start(jobs, 4);
    wait_until(|| started.load(Ordering::SeqCst) == 2, "start");
    call.settle(h, 3);
    (call, latch)
}

/// A job inside a "computing" guard longer than the stall time doesn't stall its volume; one past
/// 10 × the stall time does (span at, just below, just above).
#[test]
fn computing_pauses_the_clock_up_to_ten_stall_times() {
    let limit = STALL_WITH_BYTES * COMPUTING_FACTOR;
    for (by, stalls) in [(STALL_WITH_BYTES + 1, false), (limit - 1, false), (limit, false), (limit + 1, true)] {
        let h = H::new();
        let (call, latch) = computing_jobs(&h);
        h.advance(by);
        call.settle(&h, 3);
        assert_eq!(call.finished(), stalls, "computing for {by}");
        latch.release();
        call.join();
    }
}

/// A probe that hangs after its computing span (as in `file_modified`) stalls its volume: the
/// guard covers only the CPU work.
#[test]
fn a_hang_after_the_computing_span_stalls() {
    let h = H::new();
    let (latch, started) = (Latch::new(), Arc::new(AtomicUsize::new(0)));
    let jobs = (0..2)
        .map(|i| {
            let (latch, started) = (latch.clone(), started.clone());
            job(&format!("/a/{i}"), move || {
                bytes(1);
                drop(crate::memory::computing());
                started.fetch_add(1, Ordering::SeqCst);
                latch.wait();
                Ok(i)
            })
        })
        .collect();
    let call = h.start(jobs, 4);
    wait_until(|| started.load(Ordering::SeqCst) == 2, "start");
    call.settle(&h, 3);
    h.advance(STALL_WITH_BYTES);
    call.settle(&h, 3);
    assert!(call.finished());
    latch.release();
    assert_eq!(vnr_count(&call.join()), 2);
}

/// A fake card: every read moves at most what 10 MB/s allows in the time it takes, stepping the
/// fake clock.
struct SlowCard {
    left: u64,
    clock: Arc<FakeClock>,
    max_read: Arc<AtomicUsize>,
}

impl std::io::Read for SlowCard {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.max_read.fetch_max(buf.len(), Ordering::SeqCst);
        let n = (buf.len() as u64).min(self.left) as usize;
        self.left -= n as u64;
        // 10 MB/s
        self.clock.advance(n as u64 * SEC / 10_000_000);
        std::thread::sleep(Duration::from_micros(50));
        Ok(n)
    }
}

/// A slow volume that keeps moving bytes (one 4 GB file at 10 MB/s, read through the 4 MiB-capped
/// counting reader) is never declared stalled, though the file takes far longer than 30 s.
#[test]
fn a_slow_volume_moving_bytes_never_stalls() {
    use std::io::Read;
    let h = H::new();
    let max_read = Arc::new(AtomicUsize::new(0));
    let size = 4u64 << 30;
    let jobs = (0..2)
        .map(|i| {
            let (clock, max_read) = (h.clock.clone(), max_read.clone());
            job(&format!("/a/clip{i}"), move || {
                let mut r = crate::bounded_read::counting(SlowCard { left: size / 2, clock, max_read });
                let mut buf = vec![0u8; 16 << 20];
                let mut total = 0u64;
                loop {
                    let n = r.read(&mut buf).map_err(|e| e.to_string())?;
                    if n == 0 {
                        break;
                    }
                    total += n as u64;
                }
                Ok((total >> 20) as u32)
            })
        })
        .collect();
    let start = h.now();
    let (r, _) = h.run(jobs, 2);
    assert_eq!(ok_count(&r), 2, "never stalled");
    assert!(h.now() - start > 100 * SEC, "the reads took {} s of the fake clock", (h.now() - start) / SEC);
    assert_eq!(max_read.load(Ordering::SeqCst), crate::bounded_read::MAX_READ_CHUNK, "each read capped at 4 MiB");
    let counted: u64 = r.metrics.iter().flatten().map(|m| m.bytes).sum();
    assert_eq!(counted, size);
}

// ---------------------------------------------------------------- cancel

/// Cancel before anything starts: every job not started, `done` 0; a single job too (the
/// calling thread's path), which then doesn't run.
#[test]
fn cancel_before_the_start() {
    let h = H::new();
    let (cancel, done) = (AtomicBool::new(true), AtomicUsize::new(0));
    let r = run_jobs(&h.ctx, quick("/a", 5), 4, Result::is_ok, &cancel, &done);
    assert!(r.outcomes.iter().all(|o| matches!(o, JobOutcome::NotStarted)));
    assert_eq!(done.load(Ordering::SeqCst), 0);
    let ran = Arc::new(AtomicBool::new(false));
    let ran2 = ran.clone();
    let one = job("/a/one", move || {
        ran2.store(true, Ordering::SeqCst);
        Ok(1)
    });
    let r = run_jobs(&h.ctx, vec![one], 4, Result::is_ok, &cancel, &done);
    assert!(matches!(r.outcomes.as_slice(), [JobOutcome::NotStarted]));
    assert!(!ran.load(Ordering::SeqCst));
    assert_eq!(done.load(Ordering::SeqCst), 0);
}

/// R4b: cancel returns promptly with a hung volume while jobs on a healthy volume, still moving
/// bytes, finish Ok; queued jobs never start.
#[test]
fn cancel_returns_with_a_hung_volume() {
    let h = H::new();
    let (hung, started) = (Latch::new(), Arc::new(AtomicUsize::new(0)));
    let mut jobs = hanging("/a", 2, &hung, &started, true);
    let stop = Arc::new(AtomicBool::new(false));
    for i in 0..2 {
        let (stop, started) = (stop.clone(), started.clone());
        jobs.push(job(&format!("/b/{i}"), move || {
            started.fetch_add(1, Ordering::SeqCst);
            while !stop.load(Ordering::SeqCst) {
                bytes(1);
                std::thread::sleep(Duration::from_micros(300));
            }
            Ok(7)
        }));
    }
    jobs.extend(quick("/b", 3));
    // (cap 4: /a and /b hold every slot, so /b's quick jobs wait in the queue)
    let call = h.start(jobs, 4);
    wait_until(|| started.load(Ordering::SeqCst) == 4, "start");
    call.cancel.store(true, Ordering::SeqCst);
    call.settle(&h, 3);
    h.advance(CANCEL_SILENCE);
    call.settle(&h, 3);
    assert!(!call.finished(), "the healthy volume's jobs still run");
    stop.store(true, Ordering::SeqCst);
    let r = call.join();
    let kinds: Vec<&str> = r
        .outcomes
        .iter()
        .map(|o| match o {
            JobOutcome::Done(_) => "done",
            JobOutcome::Cancelled => "cancelled",
            JobOutcome::NotStarted => "not started",
            _ => "other",
        })
        .collect();
    assert_eq!(kinds, ["cancelled", "cancelled", "done", "done", "not started", "not started", "not started"]);
    assert_eq!(call_done(&r), 2);
    hung.release();
}

fn call_done(r: &Report<R>) -> usize {
    r.outcomes.iter().filter(|o| !matches!(o, JobOutcome::NotStarted | JobOutcome::Cancelled)).count()
}

/// Cancel's 5 s rule exempts nothing: a computing job is abandoned after 5 s without progress
/// (at, just below, just above).
#[test]
fn cancel_abandons_computing_jobs_after_5_s() {
    for (by, abandoned) in [(CANCEL_SILENCE - 1, false), (CANCEL_SILENCE, true), (CANCEL_SILENCE + 1, true)] {
        let h = H::new();
        let (call, latch) = computing_jobs(&h);
        call.cancel.store(true, Ordering::SeqCst);
        call.settle(&h, 3);
        h.advance(by);
        call.settle(&h, 3);
        assert_eq!(call.finished(), abandoned, "{by}");
        latch.release();
        let r = call.join();
        let cancelled = r.outcomes.iter().filter(|o| matches!(o, JobOutcome::Cancelled)).count();
        assert_eq!(cancelled, if abandoned { 2 } else { 0 });
    }
}

/// A worker whose abandoned job returns exits: it never takes another job, and the call never
/// exceeds its cap.
#[test]
fn an_abandoned_worker_exits_when_its_job_returns() {
    let h = H::new();
    let (latch, started) = (Latch::new(), Arc::new(AtomicUsize::new(0)));
    let hung_threads = Arc::new(Mutex::new(Vec::new()));
    let mut jobs: Vec<IoJob<R>> = (0..2)
        .map(|i| {
            let (latch, started, t) = (latch.clone(), started.clone(), hung_threads.clone());
            job(&format!("/a/{i}"), move || {
                t.lock().unwrap().push(std::thread::current().id());
                bytes(1);
                started.fetch_add(1, Ordering::SeqCst);
                latch.wait();
                Ok(0)
            })
        })
        .collect();
    let b_threads = Arc::new(Mutex::new(Vec::new()));
    let go = Latch::new();
    let conc = Arc::new(Conc::default());
    *conc.unfinished.lock().unwrap().entry("/b".into()).or_default() += 12;
    for i in 0..12 {
        let (t, go, conc) = (b_threads.clone(), go.clone(), conc.clone());
        jobs.push(job(&format!("/b/{i}"), move || {
            go.wait();
            conc.enter("/b");
            t.lock().unwrap().push(std::thread::current().id());
            std::thread::sleep(Duration::from_millis(1));
            conc.leave("/b");
            Ok(1)
        }));
    }
    let call = h.start(jobs, 3);
    wait_until(|| started.load(Ordering::SeqCst) == 2, "start");
    call.settle(&h, 3);
    h.advance(STALL_WITH_BYTES);
    call.settle(&h, 3);
    // /a's jobs are abandoned; their threads return now and must exit
    latch.release();
    wait_until(|| h.ctx.shared.entries() == 0, "the abandoned jobs to return");
    go.release();
    let r = call.join();
    assert_eq!(ok_count(&r), 12);
    assert_eq!(vnr_count(&r), 2);
    let hung = hung_threads.lock().unwrap().clone();
    assert!(b_threads.lock().unwrap().iter().all(|t| !hung.contains(t)), "an abandoned worker took another job");
    assert!(conc.max_all.load(Ordering::SeqCst) <= 3);
}

/// A late result after abandonment is dropped: the outcome stays, the entry goes.
#[test]
fn a_late_result_is_dropped() {
    let h = H::new();
    let (stalled, call, latch) = stalls_after(&h, true, STALL_WITH_BYTES);
    assert!(stalled);
    let r = call.join();
    assert_eq!(vnr_count(&r), 2);
    assert_eq!(h.ctx.shared.entries(), 2);
    latch.release();
    wait_until(|| h.ctx.shared.entries() == 0, "late returns");
}

// ---------------------------------------------------------------- registry, block and recovery

/// Stall `/a` (two jobs hung for ever, after a byte each); returns the latch that frees them.
fn stall_volume(h: &H, vol: &str) -> Arc<Latch> {
    let (latch, started) = (Latch::new(), Arc::new(AtomicUsize::new(0)));
    let call = h.start(hanging(vol, 2, &latch, &started, true), 4);
    wait_until(|| started.load(Ordering::SeqCst) == 2, "start");
    call.settle(h, 3);
    h.advance(STALL_WITH_BYTES);
    let r = call.join();
    assert_eq!(vnr_count(&r), 2);
    latch
}

/// R4d: the next call on a blocked volume lets one recovery job through while the rest wait;
/// a recovery job that moves bytes ends the block and every job queued behind it runs.
#[test]
fn a_recovery_job_that_reads_ends_the_block() {
    let h = H::new();
    let hung = stall_volume(&h, "/a");
    assert!(h.ctx.shared.blocked(&key("/a"), h.now()));
    let order = Arc::new(Mutex::new(Vec::new()));
    let gate = Latch::new();
    let mut jobs = Vec::new();
    for i in 0..4u32 {
        let (order, gate) = (order.clone(), gate.clone());
        jobs.push(job(&format!("/a/r{i}"), move || {
            order.lock().unwrap().push(i);
            if i == 0 {
                gate.wait();
            }
            bytes(5);
            Ok(i)
        }));
    }
    jobs.extend(quick("/b", 3));
    let call = h.start(jobs, 4);
    wait_until(|| call.done.load(Ordering::SeqCst) >= 3 && !order.lock().unwrap().is_empty(), "/b to run beside the recovery job");
    assert_eq!(*order.lock().unwrap(), [0], "only the recovery job runs while it is out");
    gate.release();
    let r = call.join();
    assert_eq!(ok_count(&r), 7);
    assert!(!h.ctx.shared.blocked(&key("/a"), h.now()));
    hung.release();
}

/// Jobs on `vol` that fail at once without moving bytes (files that have since gone), recording
/// how many of `vol`'s jobs ran at once.
fn gone(vol: &str, n: usize, conc: &Arc<Conc>) -> Vec<IoJob<R>> {
    (0..n)
        .map(|i| {
            let (conc, vol) = (conc.clone(), vol.to_string());
            job(&format!("{vol}/gone{i}"), move || {
                conc.enter(&vol);
                std::thread::sleep(Duration::from_millis(1));
                conc.leave(&vol);
                Err("no such file".into())
            })
        })
        .collect()
}

/// A recovery job that fails without moving bytes (a file that has since gone) shows the volume
/// answers but not that it works: the block stays, and the jobs queued behind it go one at a time
/// as the next recovery jobs, each with its own result, never "volume not responding"; the first
/// that moves bytes ends the block and the rest run at once.
#[test]
fn a_recovery_job_without_bytes_hands_on_to_the_next_file() {
    let h = H::new();
    let hung = stall_volume(&h, "/a");
    let conc = Arc::new(Conc::default());
    *conc.unfinished.lock().unwrap().entry("/a".into()).or_default() += 2;
    let mut jobs = gone("/a", 2, &conc);
    jobs.extend(quick("/a", 3));
    jobs.extend(quick("/b", 2));
    let (r, done) = h.run(jobs, 4);
    assert!(matches!(r.outcomes.as_slice(), [JobOutcome::Done(Err(_)), JobOutcome::Done(Err(_)), ..]), "each its own result");
    assert_eq!(vnr_count(&r), 0);
    assert_eq!(ok_count(&r), 5);
    assert_eq!(done, 7);
    assert_eq!(conc.max_vol.lock().unwrap().get("/a").copied(), Some(1), "one recovery job at a time");
    assert!(!h.ctx.shared.blocked(&key("/a"), h.now()), "a read ended the block");
    hung.release();

    // every file gone: each runs alone, the block stays
    let h = H::new();
    let hung = stall_volume(&h, "/a");
    let conc = Arc::new(Conc::default());
    *conc.unfinished.lock().unwrap().entry("/a".into()).or_default() += 4;
    let (r, done) = h.run(gone("/a", 4, &conc), 4);
    assert!(r.outcomes.iter().all(|o| matches!(o, JobOutcome::Done(Err(_)))), "{:?}", r.outcomes);
    assert_eq!(done, 4);
    assert_eq!(conc.max_vol.lock().unwrap().get("/a").copied(), Some(1));
    assert!(h.ctx.shared.blocked(&key("/a"), h.now()));
    hung.release();
}

/// The once-a-minute recovery limit, at, just below and just above: within the minute after a
/// failed recovery job, the volume's jobs fail at once.
#[test]
fn recovery_at_most_once_a_minute() {
    for (after, runs) in [(RECOVERY_EVERY - 1, false), (RECOVERY_EVERY, true), (RECOVERY_EVERY + 1, true)] {
        let h = H::new();
        let hung = stall_volume(&h, "/a");
        let (r, _) = h.run(vec![job("/a/gone", || Err("gone".into())), job("/a/gone2", || Err("gone".into()))], 2);
        assert_eq!(vnr_count(&r), 0, "each ran as the next recovery job");
        assert!(h.ctx.shared.blocked(&key("/a"), h.now()));
        h.advance(after);
        let ran = Arc::new(AtomicBool::new(false));
        let ran2 = ran.clone();
        let (r, _) = h.run(
            vec![
                job("/a/y", move || {
                    ran2.store(true, Ordering::SeqCst);
                    bytes(1);
                    Ok(2)
                }),
                job("/a/z", || Ok(3)),
            ],
            2,
        );
        assert_eq!(ran.load(Ordering::SeqCst), runs, "{after}");
        assert_eq!(ok_count(&r), if runs { 2 } else { 0 });
        hung.release();
    }
}

/// A recovery job that is itself abandoned stops further recovery jobs until it returns, except
/// that a new one is let through after 10 minutes (at, just below, just above).
#[test]
fn a_stuck_recovery_job_lets_another_through_after_10_minutes() {
    for (after, runs) in [(RECOVERY_STUCK - 1, false), (RECOVERY_STUCK, true), (RECOVERY_STUCK + 1, true)] {
        let h = H::new();
        let hung = stall_volume(&h, "/a");
        // the recovery job hangs after a byte… then stops: abandoned by its own stall
        let (latch, started) = (Latch::new(), Arc::new(AtomicUsize::new(0)));
        let mut jobs = hanging("/a", 1, &latch, &started, false);
        jobs.extend(quick("/a", 2));
        let t0 = h.now();
        let call = h.start(jobs, 4);
        wait_until(|| started.load(Ordering::SeqCst) == 1, "the recovery job");
        call.settle(&h, 3);
        h.advance(STALL_WITH_BYTES);
        call.settle(&h, 3);
        if !call.finished() {
            h.advance(STALL_GRACE);
        }
        let r = call.join();
        assert_eq!(vnr_count(&r), 3);
        h.clock.set(t0 + after);
        let (r, _) = h.run(quick("/a", 2), 2);
        assert_eq!(ok_count(&r), if runs { 2 } else { 0 }, "{after}");
        latch.release();
        hung.release();
    }
    // once the stuck recovery job returns, the next one may run (a minute after it started)
    let h = H::new();
    let hung = stall_volume(&h, "/a");
    let (latch, started) = (Latch::new(), Arc::new(AtomicUsize::new(0)));
    let mut jobs = hanging("/a", 1, &latch, &started, false);
    jobs.extend(quick("/a", 1));
    let call = h.start(jobs, 4);
    wait_until(|| started.load(Ordering::SeqCst) == 1, "the recovery job");
    call.settle(&h, 3);
    h.advance(STALL_GRACE);
    call.join();
    h.advance(RECOVERY_EVERY);
    let (r, _) = h.run(quick("/a", 2), 2);
    assert_eq!(ok_count(&r), 0, "still the recovery job until it returns");
    latch.release();
    wait_until(|| h.ctx.shared.entries() == 2, "the recovery job to return");
    let (r, _) = h.run(quick("/a", 2), 2);
    assert_eq!(ok_count(&r), 2);
    hung.release();
}

fn key(path: &str) -> lcc_catalog::folders::VolumeKey {
    lcc_catalog::folders::volume_key(path, &Mounts::from_points(["/", "/a", "/b", "/c", "/srv/nas"], false))
}

/// A single-job call on a blocked volume runs as the recovery job when one may run, and otherwise
/// fails at once, keyed by the cached mount list (`/srv/nas`, which `place()` keys as `/`).
#[test]
fn a_single_job_on_a_blocked_volume() {
    let h = H::new();
    let hung = stall_volume(&h, "/srv/nas");
    assert!(h.ctx.shared.blocked(&key("/srv/nas/x"), h.now()));
    // recovery job: fails without bytes, so the block stays
    let (r, done) = h.run(vec![job("/srv/nas/gone", || Err("gone".into()))], 4);
    assert!(matches!(r.outcomes.as_slice(), [JobOutcome::Done(Err(_))]));
    assert_eq!(done, 1);
    // within the minute: fails at once, without running
    let ran = Arc::new(AtomicBool::new(false));
    let ran2 = ran.clone();
    let (r, done) = h.run(
        vec![job("/srv/nas/y", move || {
            ran2.store(true, Ordering::SeqCst);
            Ok(1)
        })],
        4,
    );
    assert!(matches!(r.outcomes.as_slice(), [JobOutcome::VolumeNotResponding { .. }]));
    assert!(!ran.load(Ordering::SeqCst));
    assert_eq!(done, 1);
    // another folder on the boot volume is not blocked
    let (r, _) = h.run(vec![job("/srv/other/z", || Ok(2))], 4);
    assert!(matches!(r.outcomes.as_slice(), [JobOutcome::Done(Ok(2))]));
    // a minute later it runs as the recovery job, and a read ends the block
    h.advance(RECOVERY_EVERY);
    let (r, _) = h.run(
        vec![job("/srv/nas/w", || {
            bytes(3);
            Ok(3)
        })],
        4,
    );
    assert!(matches!(r.outcomes.as_slice(), [JobOutcome::Done(Ok(3))]));
    assert!(!h.ctx.shared.blocked(&key("/srv/nas/x"), h.now()));
    hung.release();
}

/// Cancel-abandon two jobs on /a: they count a byte, then (when `resume`) keep moving bytes after
/// abandonment, or stay silent. Returns the latch.
fn cancel_abandoned(h: &H, resume: bool) -> Arc<Latch> {
    let latch = Latch::new();
    let started = Arc::new(AtomicUsize::new(0));
    let abandoned = Arc::new(AtomicBool::new(false));
    let jobs = (0..2)
        .map(|i| {
            let (latch, started, abandoned) = (latch.clone(), started.clone(), abandoned.clone());
            job(&format!("/a/{i}"), move || {
                bytes(1);
                started.fetch_add(1, Ordering::SeqCst);
                if resume {
                    while !abandoned.load(Ordering::SeqCst) {
                        std::thread::sleep(Duration::from_micros(200));
                    }
                    // keeps reading after it was left behind
                    while !latch.is_open() {
                        bytes(1);
                        std::thread::sleep(Duration::from_micros(200));
                    }
                }
                latch.wait();
                Ok(i)
            })
        })
        .collect();
    let call = h.start(jobs, 4);
    wait_until(|| started.load(Ordering::SeqCst) == 2, "start");
    call.settle(h, 3);
    call.cancel.store(true, Ordering::SeqCst);
    call.settle(h, 3);
    h.advance(CANCEL_SILENCE);
    let r = call.join();
    assert!(r.outcomes.iter().all(|o| matches!(o, JobOutcome::Cancelled)));
    abandoned.store(true, Ordering::SeqCst);
    latch
}

/// A job abandoned by cancel that keeps moving bytes never blocks a later call; one that stops
/// for its stall time blocks the next call on its volume (registry checked at admission).
#[test]
fn cancel_abandoned_jobs_block_only_once_stalled() {
    let h = H::new();
    let latch = cancel_abandoned(&h, true);
    h.advance(STALL_WITH_BYTES);
    // (it reads again after the step)
    std::thread::sleep(Duration::from_millis(5));
    assert!(!h.ctx.shared.blocked(&key("/a"), h.now()), "still moving bytes");
    let (r, _) = h.run(quick("/a", 3), 4);
    assert_eq!(ok_count(&r), 3);
    latch.release();

    let h = H::new();
    let latch = cancel_abandoned(&h, false);
    // (its own clock runs from its last byte, 5 s before it was abandoned)
    h.advance(STALL_WITH_BYTES - CANCEL_SILENCE - 1);
    assert!(!h.ctx.shared.blocked(&key("/a"), h.now()), "not before its own stall time");
    h.advance(1);
    assert!(h.ctx.shared.blocked(&key("/a"), h.now()));
    let ran = Arc::new(AtomicUsize::new(0));
    let jobs = (0..3)
        .map(|i| {
            let ran = ran.clone();
            job(&format!("/a/n{i}"), move || {
                ran.fetch_add(1, Ordering::SeqCst);
                Err("gone".into())
            })
        })
        .collect();
    let (r, _) = h.run(jobs, 4);
    assert_eq!(ran.load(Ordering::SeqCst), 3, "each as the next recovery job");
    assert_eq!(vnr_count(&r), 0);
    assert!(h.ctx.shared.blocked(&key("/a"), h.now()), "nothing showed the volume works");
    latch.release();
}

/// A cancel-abandoned entry passing 30 s doesn't block a volume on which another job moved bytes
/// after it was abandoned.
#[test]
fn bytes_after_an_abandonment_end_its_block() {
    let h = H::new();
    let latch = cancel_abandoned(&h, false);
    h.advance(SEC);
    let (r, _) = h.run(quick("/a", 2), 2);
    assert_eq!(ok_count(&r), 2);
    h.advance(STALL_WITH_BYTES);
    assert!(!h.ctx.shared.blocked(&key("/a"), h.now()));
    latch.release();
}

/// A volume where one read hangs while the others are served: that read is left behind when it
/// is the last one running, but the next call's files all run (the first read ends the block).
#[test]
fn one_hung_read_among_served_ones_fails_no_later_file() {
    let h = H::new();
    let (latch, started) = (Latch::new(), Arc::new(AtomicUsize::new(0)));
    let mut jobs = hanging("/a", 1, &latch, &started, true);
    jobs.extend(quick("/a", 6));
    let call = h.start(jobs, 4);
    wait_until(|| call.done.load(Ordering::SeqCst) == 6, "the served reads");
    call.settle(&h, 3);
    h.advance(STALL_WITH_BYTES);
    let r = call.join();
    assert_eq!(ok_count(&r), 6);
    assert_eq!(vnr_count(&r), 1);
    let (r, _) = h.run(quick("/a", 5), 4);
    assert_eq!(ok_count(&r), 5, "no file of the next call fails");
    assert!(!h.ctx.shared.blocked(&key("/a"), h.now()));
    latch.release();
}

// ---------------------------------------------------------------- mount list

/// A mount-list read that blocks is abandoned after about 1 s (at, just below, just above): the
/// call uses the cached list (same keys), and a second call meanwhile starts no second helper.
#[test]
fn a_blocked_mount_read_is_abandoned_after_a_second() {
    for (wait, fresh) in [(SEC - 1, true), (SEC, false), (SEC + 1, false)] {
        let reads = Arc::new(AtomicUsize::new(0));
        let hold = Arc::new(Mutex::new(None::<Arc<Latch>>));
        let (r2, h2) = (reads.clone(), hold.clone());
        let reader: MountReader = Arc::new(move || {
            r2.fetch_add(1, Ordering::SeqCst);
            if let Some(l) = h2.lock().unwrap().clone() {
                l.wait();
            }
            Some(Mounts::from_points(["/", "/srv/nas", "/srv/fresh"], false))
        });
        let h = H::with_reader(reader);
        // a first read fills the cache: /srv/nas is a volume of its own
        let (r, _) = h.run(quick("/srv/nas", 2), 2);
        assert_eq!(ok_count(&r), 2);
        assert_eq!(reads.load(Ordering::SeqCst), 1);
        // the next read blocks
        let latch = Latch::new();
        *hold.lock().unwrap() = Some(latch.clone());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let jobs = (0..2)
            .map(|i| {
                let seen = seen.clone();
                job(&format!("/srv/nas/{i}"), move || {
                    seen.lock().unwrap().push(crate::memory::current_job().map(|j| j.volume.id().to_string()));
                    Ok(i)
                })
            })
            .collect();
        let call = h.start(jobs, 2);
        wait_until(|| reads.load(Ordering::SeqCst) == 2, "the second read");
        h.clock.advance(wait);
        let r = if fresh {
            std::thread::sleep(Duration::from_millis(20));
            assert!(!call.finished(), "still waiting for the list below 1 s");
            latch.release();
            call.join()
        } else {
            let r = call.join();
            assert_eq!(ok_count(&r), 2);
            // a second call while the helper is stuck: no second helper, cached keys at once
            let (r, _) = h.run(quick("/srv/nas", 2), 2);
            assert_eq!(ok_count(&r), 2);
            assert_eq!(reads.load(Ordering::SeqCst), 2, "no second helper");
            assert!(h.ctx.mounts.outstanding());
            latch.release();
            wait_until(|| !h.ctx.mounts.outstanding(), "the helper to finish");
            r
        };
        assert_eq!(ok_count(&r), 2);
        assert!(seen.lock().unwrap().iter().all(|k| k.as_deref() == Some("/srv/nas")), "same keys: {:?}", seen.lock().unwrap());
    }
}

/// A mountinfo file longer than the cap: the list says it was cut, and the cut-off last line adds
/// no mount point (`/srv/nas` cut to `/srv` would otherwise take `/srv/other`'s files).
#[test]
fn a_cut_mountinfo_says_so_and_drops_its_partial_line() {
    let dir = std::env::temp_dir().join(format!("lcc-io-mountinfo-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("mountinfo");
    let whole = "1 0 0:1 / / rw - ext4 a rw\n2 1 0:2 / /media/card rw - vfat b rw\n";
    let text = format!("{whole}3 1 0:3 / /srv/nas rw - nfs c rw\n");
    std::fs::write(&path, &text).unwrap();
    // cut inside the third line's mount point, right after `/srv`
    let cut = whole.len() + "3 1 0:3 / /srv".len();
    let m = super::mounts::mounts_from_mountinfo(&path, cut as u64).unwrap();
    assert!(m.truncated());
    assert_eq!(m.len(), 2);
    assert_eq!(lcc_catalog::folders::volume_key("/srv/other/a.jpg", &m).id(), "/");
    // whole: not cut
    let m = super::mounts::mounts_from_mountinfo(&path, text.len() as u64).unwrap();
    assert!(!m.truncated());
    assert_eq!(m.len(), 3);
    assert!(super::mounts::mounts_from_mountinfo(&dir.join("missing"), 100).is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

/// macOS `/Volumes`: the startup disk's own entry (a link to `/`) is no volume of its own, so a
/// file read through it shares the startup disk's queue; the other entries are volumes.
#[cfg(unix)]
#[test]
fn the_startup_disk_link_in_volumes_keys_as_the_startup_disk() {
    let dir = std::env::temp_dir().join(format!("lcc-io-volumes-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("CARD")).unwrap();
    std::os::unix::fs::symlink("/", dir.join("Macintosh HD")).unwrap();
    std::os::unix::fs::symlink("/elsewhere", dir.join("other link")).unwrap();
    let m = super::mounts::mounts_from_volumes_dir(&dir).unwrap();
    let k = |p: &str| lcc_catalog::folders::volume_key(p, &m);
    assert_eq!(k("/Volumes/Macintosh HD/Users/me/a.jpg"), k("/Users/me/a.jpg"));
    assert_eq!(k("/Volumes/Macintosh HD/Users/me/a.jpg").id(), "/");
    assert_eq!(k("/Volumes/CARD/DCIM/a.jpg").id(), "/volumes/card");
    assert_eq!(k("/Volumes/other link/a.jpg").id(), "/volumes/other link", "a link elsewhere stays a volume, as listed");
    assert_eq!(m.len(), 2);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A helper that panics while parsing clears its flag: the next call reads again.
#[test]
fn a_panicking_mount_helper_clears_its_flag() {
    let reads = Arc::new(AtomicUsize::new(0));
    let r2 = reads.clone();
    let reader: MountReader = Arc::new(move || {
        if r2.fetch_add(1, Ordering::SeqCst) == 0 {
            panic!("bad mountinfo");
        }
        Some(Mounts::from_points(["/", "/srv/nas"], false))
    });
    let h = H::with_reader(reader);
    let (r, _) = h.run(quick("/srv/nas", 2), 2);
    assert_eq!(ok_count(&r), 2);
    wait_until(|| !h.ctx.mounts.outstanding(), "the flag to clear");
    let (r, _) = h.run(quick("/srv/nas", 2), 2);
    assert_eq!(ok_count(&r), 2);
    assert_eq!(reads.load(Ordering::SeqCst), 2);
    assert_eq!(h.ctx.mounts.cached().len(), 2);
}

/// An unreadable mount list keeps the cached one (or `place()` with none).
#[test]
fn an_unreadable_mount_list_keeps_the_last_one() {
    let h = H::with_reader(Arc::new(|| None));
    let (r, _) = h.run(quick("/srv/nas", 2), 2);
    assert_eq!(ok_count(&r), 2);
    assert!(!h.ctx.mounts.cached().is_read());
}

// ---------------------------------------------------------------- the probe_all wrapper

/// The wrapper gives today's strings: "not probed" for files cancel left unreached where today's
/// loop ran threads, "cancelled" where it ran serially (one CPU); typed outcomes become their
/// messages.
#[test]
fn the_probe_wrapper_keeps_todays_strings() {
    let h = H::new();
    let probe: crate::media::FileProbe = Arc::new(|p: &str| {
        if p.ends_with("boom") {
            panic!("bad file");
        }
        Ok(crate::media::ProbeInfo::default())
    });
    let paths: Vec<String> = ["/a/1", "/a/boom", "/b/2"].iter().map(|s| s.to_string()).collect();
    let (done, cancel) = (AtomicUsize::new(0), AtomicBool::new(false));
    let r = crate::import::probe_scheduled(&h.ctx, &probe, &paths, 4, &done, &cancel);
    assert!(r.first().is_some_and(|r| r.is_ok()));
    assert!(r.get(1).is_some_and(|r| r.as_ref().is_err_and(|e| e.starts_with("/a/boom: ") && e.contains("bad file"))));
    assert_eq!(done.load(Ordering::SeqCst), 3);
    let cancel = AtomicBool::new(true);
    let r = crate::import::probe_scheduled(&h.ctx, &probe, &paths, 4, &done, &cancel);
    assert!(r.iter().all(|r| r.as_ref().err().map(String::as_str) == Some("not probed")));
    let r = crate::import::probe_scheduled(&h.ctx, &probe, &paths, 1, &done, &cancel);
    assert!(r.iter().all(|r| r.as_ref().err().map(String::as_str) == Some("cancelled")));
    // one file, cancelled before it starts: not probed, as today's single-file path
    let ran = Arc::new(AtomicBool::new(false));
    let ran2 = ran.clone();
    let watched: crate::media::FileProbe = Arc::new(move |_| {
        ran2.store(true, Ordering::SeqCst);
        Ok(crate::media::ProbeInfo::default())
    });
    let r = crate::import::probe_scheduled(&h.ctx, &watched, &paths[..1], crate::import::workers(1), &done, &cancel);
    assert_eq!(r.first().and_then(|r| r.as_ref().err()).map(String::as_str), Some("cancelled"));
    assert!(!ran.load(Ordering::SeqCst));
    // a stalled volume's files, within the minute after its recovery job answered without data
    let _hung = stall_volume(&h, "/c");
    h.advance(1);
    let bytes_less: crate::media::FileProbe = Arc::new(|_| Err("gone".into()));
    let cancel = AtomicBool::new(false);
    let r = crate::import::probe_scheduled(&h.ctx, &bytes_less, &["/c/0".to_string()], 4, &done, &cancel);
    assert_eq!(r.first().and_then(|r| r.as_ref().err()).map(String::as_str), Some("gone"), "the recovery job's own result");
    let paths: Vec<String> = ["/c/1", "/c/2"].iter().map(|s| s.to_string()).collect();
    let r = crate::import::probe_scheduled(&h.ctx, &bytes_less, &paths, 4, &done, &cancel);
    assert_eq!(r.get(1).and_then(|r| r.as_ref().err()).map(String::as_str), Some("/c/2: volume not responding (/c)"));
}

/// Threads that checked a cached probe, and a latch the check waits on for `/a` (a hung share).
static CHECKED_ON: Mutex<Vec<std::thread::ThreadId>> = Mutex::new(Vec::new());
static HANG_A: std::sync::OnceLock<Arc<Latch>> = std::sync::OnceLock::new();

fn keep_if_named_kept(p: &str, _: &crate::media::ProbeInfo) -> bool {
    CHECKED_ON.lock().unwrap().push(std::thread::current().id());
    if p.starts_with("/a/") {
        HANG_A.get_or_init(Latch::new).wait();
    }
    p.contains("kept")
}

/// An import's cached probes are checked as each file's own job, on its volume, never on the
/// planning thread: a file whose probe still holds gets it back unread, one that changed is read
/// again, and a volume whose check hangs lets cancel return (pre-existing #06, exposed by PR 1's
/// "Cancel returns even while a drive hangs").
#[test]
fn cached_probes_are_checked_off_the_calling_thread() {
    use crate::media::ProbeInfo;
    let h = H::new();
    let reads = Arc::new(AtomicUsize::new(0));
    let r2 = reads.clone();
    let probe: crate::media::FileProbe = Arc::new(move |_| {
        r2.fetch_add(1, Ordering::SeqCst);
        Ok(ProbeInfo { format: "READ".into(), ..Default::default() })
    });
    let cached_info = || ProbeInfo { format: "CACHED".into(), ..Default::default() };
    let paths: Vec<String> = ["/b/kept1", "/b/changed", "/c/kept2", "/c/new"].iter().map(|s| s.to_string()).collect();
    let cached = [("/b/kept1", cached_info()), ("/b/changed", cached_info()), ("/c/kept2", cached_info())]
        .into_iter()
        .map(|(p, i)| (p.to_string(), i))
        .collect();
    let checked = crate::import::cached_probe(probe.clone(), cached, keep_if_named_kept);
    let (done, cancel) = (AtomicUsize::new(0), AtomicBool::new(false));
    let r = crate::import::probe_scheduled(&h.ctx, &checked, &paths, 4, &done, &cancel);
    let formats: Vec<&str> = r.iter().map(|r| r.as_ref().map(|i| i.format.as_str()).unwrap_or("err")).collect();
    assert_eq!(formats, ["CACHED", "READ", "CACHED", "READ"]);
    assert_eq!(reads.load(Ordering::SeqCst), 2);
    assert_eq!(done.load(Ordering::SeqCst), 4, "the cached files count too");
    let me = std::thread::current().id();
    assert!(!CHECKED_ON.lock().unwrap().is_empty() && CHECKED_ON.lock().unwrap().iter().all(|t| *t != me), "checked on the scheduler's threads");

    // a share that hangs in the check: cancel returns
    let paths: Vec<String> = ["/a/kept1", "/a/kept2"].iter().map(|s| s.to_string()).collect();
    let cached = paths.iter().map(|p| (p.clone(), cached_info())).collect();
    let checked = crate::import::cached_probe(probe, cached, keep_if_named_kept);
    let (ctx, cancel, done) = (h.ctx.clone(), Arc::new(AtomicBool::new(false)), Arc::new(AtomicUsize::new(0)));
    let (c2, d2) = (cancel.clone(), done.clone());
    let call = std::thread::spawn(move || crate::import::probe_scheduled(&ctx, &checked, &paths, 4, &d2, &c2));
    wait_until(|| CHECKED_ON.lock().unwrap().len() >= 5, "both checks to start");
    cancel.store(true, Ordering::SeqCst);
    let ticks = h.ctx.ticks.load(Ordering::SeqCst);
    wait_until(|| h.ctx.ticks.load(Ordering::SeqCst) > ticks + 3, "ticks");
    h.advance(CANCEL_SILENCE);
    wait_until(|| call.is_finished(), "cancel to return while the checks hang");
    let r = call.join().unwrap();
    assert!(r.iter().all(|r| r.as_ref().err().map(String::as_str) == Some("cancelled")), "{r:?}");
    HANG_A.get_or_init(Latch::new).release();
}

/// The real desktop probe, through the scheduler: counted bytes, a computing span, the job gate.
#[test]
fn the_desktop_probe_counts_its_bytes() {
    let dir = std::env::temp_dir().join(format!("lcc-io-sched-probe-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut paths = Vec::new();
    for i in 0..3 {
        let p = dir.join(format!("p{i}.jpg"));
        std::fs::write(&p, image_jpeg(i as u8)).unwrap();
        paths.push(p.to_string_lossy().into_owned());
    }
    let h = H::new();
    let (_, probe) = crate::files::fs_hooks();
    let jobs = paths
        .iter()
        .map(|p| {
            let (probe, p2) = (probe.clone(), p.clone());
            IoJob::read(p.clone(), Box::new(move || probe(&p2).map(|_| 1u32).map_err(|e| e.to_string())) as Box<dyn FnOnce() -> R + Send>)
        })
        .collect();
    let (r, _) = h.run(jobs, 2);
    assert_eq!(ok_count(&r), 3, "{:?}", r.outcomes);
    for (m, p) in r.metrics.iter().zip(&paths) {
        let m = m.as_ref().unwrap();
        assert_eq!(m.bytes, std::fs::metadata(p).unwrap().len(), "every byte of {p} counted");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

fn image_jpeg(seed: u8) -> Vec<u8> {
    let data: Vec<u8> = (0..16 * 12 * 3).map(|i| (i as u8).wrapping_mul(seed.wrapping_add(3))).collect();
    let img = lcc_codecs::EncodeImage::new(16, 12, 3, lcc_codecs::Samples::U8(&data));
    lcc_codecs::encode_jpeg(&img, 90, lcc_codecs::ChromaSubsampling::S444, &Default::default()).unwrap()
}
