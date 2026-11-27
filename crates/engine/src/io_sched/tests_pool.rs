//! The process-wide pool (I/O spec §7, "Pool (PR 3)"), on fake volumes: calls on one volume share
//! its limit and take turns; the streaming form; single jobs bypass the pool; jobs on pool threads
//! run one job inline and are refused several; the supervisor's panic and spawn failure fall back;
//! a streaming caller that stops reading stops nothing else; cancel and `done` stay per call.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use super::pool::{Stream, SubmitError, run_jobs};
use super::run::{IoJob, JobOutcome};
use super::shared::{CANCEL_SILENCE, STALL_WITH_BYTES};
use super::tests::{H, Latch, R, bytes, hanging, job, ok_count, quick, vnr_count, wait_until};

/// Tokens a job takes to finish: the test hands them out one at a time.
#[derive(Default)]
struct Tokens {
    n: Mutex<usize>,
    cv: Condvar,
}

impl Tokens {
    fn new() -> Arc<Tokens> {
        Arc::new(Tokens::default())
    }

    fn take(&self) {
        let mut g = self.n.lock().unwrap();
        while *g == 0 {
            g = self.cv.wait(g).unwrap();
        }
        *g -= 1;
    }

    fn give(&self, k: usize) {
        *self.n.lock().unwrap() += k;
        self.cv.notify_all();
    }
}

/// Jobs running per volume and overall, and the most seen.
#[derive(Default)]
struct Conc {
    st: Mutex<(HashMap<String, usize>, usize, HashMap<String, usize>, usize)>,
    starts: Mutex<Vec<String>>,
    /// Running and most seen per call (the first letter of `who`).
    per_call: Mutex<HashMap<char, (usize, usize)>>,
}

impl Conc {
    fn enter(&self, vol: &str, who: &str) {
        self.starts.lock().unwrap().push(who.to_string());
        if let Some(c) = who.chars().next() {
            let mut p = self.per_call.lock().unwrap();
            let e = p.entry(c).or_default();
            e.0 += 1;
            e.1 = e.1.max(e.0);
        }
        let mut g = self.st.lock().unwrap();
        let c = {
            let c = g.0.entry(vol.into()).or_default();
            *c += 1;
            *c
        };
        g.1 += 1;
        g.3 = g.3.max(g.1);
        let m = g.2.entry(vol.into()).or_default();
        *m = (*m).max(c);
    }

    fn leave(&self, vol: &str, who: &str) {
        if let Some(c) = who.chars().next() {
            self.per_call.lock().unwrap().entry(c).or_default().0 -= 1;
        }
        let mut g = self.st.lock().unwrap();
        *g.0.entry(vol.into()).or_default() -= 1;
        g.1 -= 1;
    }

    fn running(&self) -> usize {
        self.st.lock().unwrap().1
    }

    fn on(&self, vol: &str) -> usize {
        self.st.lock().unwrap().0.get(vol).copied().unwrap_or(0)
    }

    fn max_all(&self) -> usize {
        self.st.lock().unwrap().3
    }

    fn max_on(&self, vol: &str) -> usize {
        self.st.lock().unwrap().2.get(vol).copied().unwrap_or(0)
    }

    fn max_of(&self, call: char) -> usize {
        self.per_call.lock().unwrap().get(&call).map_or(0, |e| e.1)
    }

    fn started(&self) -> usize {
        self.starts.lock().unwrap().len()
    }

    fn starts_of(&self, who: &str) -> usize {
        self.starts.lock().unwrap().iter().filter(|s| s.starts_with(who)).count()
    }
}

/// `n` jobs on `vol` that record their start as `who{i}` and wait for a token.
fn tokened(vol: &str, who: &str, n: usize, conc: &Arc<Conc>, tokens: &Arc<Tokens>) -> Vec<IoJob<R>> {
    (0..n)
        .map(|i| {
            let (conc, tokens, v, w) = (conc.clone(), tokens.clone(), vol.to_string(), format!("{who}{i}"));
            job(&format!("{vol}/{who}{i}"), move || {
                conc.enter(&v, &w);
                bytes(1);
                tokens.take();
                conc.leave(&v, &w);
                Ok(i as u32)
            })
        })
        .collect()
}

/// Wait until the pool holds `n` jobs with no outcome (every call's jobs submitted).
fn pending(h: &H, n: usize) {
    wait_until(|| h.ctx.pending.load(Ordering::SeqCst) == n, "the pool to hold every job");
}

/// Hand out `total` tokens one at a time, each once the last was taken and the pool has looked
/// again (a freed slot starts the next job before the next token), so the limits are met at
/// every step.
fn drain(h: &H, tokens: &Tokens, total: usize) {
    for _ in 0..total {
        tokens.give(1);
        wait_until(|| *tokens.n.lock().unwrap() == 0, "the token to be taken");
        let t = h.ctx.ticks.load(Ordering::SeqCst);
        wait_until(|| h.ctx.ticks.load(Ordering::SeqCst) >= t + 2 || h.ctx.pending.load(Ordering::SeqCst) == 0, "the pool to look again");
    }
}

/// R2 process-wide: two calls on one volume share its limit (at the limit, never above it); the
/// pool's cap is the largest `workers` of its calls, and each call keeps its own.
#[test]
fn two_calls_on_one_volume_share_its_limit() {
    for (wa, wb) in [(4usize, 4usize), (2, 4), (4, 2)] {
        let h = H::new();
        let (conc, tokens) = (Arc::new(Conc::default()), Tokens::new());
        let a = h.start(tokened("/a", "A", 12, &conc, &tokens), wa);
        wait_until(|| conc.running() == wa, "A at its cap");
        let b = h.start(tokened("/a", "B", 12, &conc, &tokens), wb);
        pending(&h, 24);
        a.settle(&h, 5);
        // A holds `wa`; B may add up to the shared limit, max(wa, wb)
        let limit = wa.max(wb);
        wait_until(|| conc.running() == limit, "the shared limit reached");
        a.settle(&h, 5);
        assert_eq!(conc.running(), limit, "({wa}, {wb}): one limit for both calls");
        drain(&h, &tokens, 24);
        assert_eq!(ok_count(&a.join()), 12);
        assert_eq!(ok_count(&b.join()), 12);
        assert_eq!(conc.max_on("/a"), limit, "({wa}, {wb}): never above the shared limit");
        assert!(conc.max_of('A') <= wa && conc.max_of('B') <= wb, "({wa}, {wb}): each call keeps its own cap");
    }
}

/// R2 process-wide: two calls on two volumes share one cap (each volume one fewer, R4a), where
/// each call alone would run its own `workers`.
#[test]
fn two_calls_on_two_volumes_share_one_cap() {
    let h = H::new();
    let (conc, tokens) = (Arc::new(Conc::default()), Tokens::new());
    let a = h.start(tokened("/a", "A", 8, &conc, &tokens), 3);
    wait_until(|| conc.running() == 3, "A alone at its cap");
    let b = h.start(tokened("/b", "B", 8, &conc, &tokens), 3);
    pending(&h, 16);
    // A's three keep running; B gets a slot as soon as one frees (/a is held to 2 now)
    tokens.give(1);
    wait_until(|| conc.starts_of("B") == 1, "B's first start");
    a.settle(&h, 5);
    assert_eq!(conc.running(), 3, "one cap for both calls");
    assert_eq!((conc.on("/a"), conc.on("/b")), (2, 1), "/a held to one fewer while /b has work");
    drain(&h, &tokens, 15);
    assert_eq!(ok_count(&a.join()) + ok_count(&b.join()), 16);
    assert_eq!(conc.max_all(), 3, "never above the cap");
}

/// R4a across calls on one CPU (`workers()` 1): two calls on two volumes run two jobs at once,
/// one per volume, so a hung volume in one call never stops the other call.
#[test]
fn one_cpu_a_hung_volume_never_stops_another_call() {
    let h = H::new();
    let (latch, started) = (Latch::new(), Arc::new(AtomicUsize::new(0)));
    let mut jobs = hanging("/a", 1, &latch, &started, true);
    jobs.extend(quick("/a", 2));
    let a = h.start(jobs, 1);
    wait_until(|| started.load(Ordering::SeqCst) == 1, "A's hung job");
    let (conc, tokens) = (Arc::new(Conc::default()), Tokens::new());
    tokens.give(5);
    let b = h.start(tokened("/b", "B", 5, &conc, &tokens), 1);
    assert_eq!(ok_count(&b.join()), 5, "B finished beside the hung /a");
    assert_eq!(conc.max_on("/b"), 1, "one job per volume");
    assert!(!a.finished());
    latch.release();
    assert_eq!(ok_count(&a.join()), 3);
}

/// A stall is seen in one place: every call's running jobs on the volume are abandoned and
/// every call's queued jobs on it fail, while other volumes go on.
#[test]
fn a_stall_fails_every_calls_jobs_on_the_volume() {
    let h = H::new();
    let (latch, started) = (Latch::new(), Arc::new(AtomicUsize::new(0)));
    let a = h.start(hanging("/a", 2, &latch, &started, true), 4);
    wait_until(|| started.load(Ordering::SeqCst) == 2, "A's two");
    let mut jobs = hanging("/a", 2, &latch, &started, true);
    jobs.extend(quick("/a", 2));
    jobs.extend(quick("/b", 2));
    let b = h.start(jobs, 4);
    wait_until(|| started.load(Ordering::SeqCst) == 4 && b.done.load(Ordering::SeqCst) == 2, "B's two on /a and /b done");
    b.settle(&h, 3);
    h.advance(STALL_WITH_BYTES);
    let (ra, rb) = (a.join(), b.join());
    let kinds = |r: &super::run::Report<R>| -> Vec<String> {
        r.outcomes
            .iter()
            .map(|o| match o {
                JobOutcome::Done(_) => "done".to_string(),
                JobOutcome::VolumeNotResponding { abandoned, .. } => format!("vnr {abandoned}"),
                other => format!("{other:?}"),
            })
            .collect()
    };
    assert_eq!(kinds(&ra), ["vnr true", "vnr true"]);
    assert_eq!(kinds(&rb), ["vnr true", "vnr true", "vnr false", "vnr false", "done", "done"]);
    latch.release();
}

/// Calls on one volume take turns: a 5-job call starts within one round while a 256-job call is
/// running (its jobs alternate with the big call's, in input order).
#[test]
fn calls_on_one_volume_take_turns() {
    let h = H::new();
    let (conc, tokens) = (Arc::new(Conc::default()), Tokens::new());
    let a = h.start(tokened("/a", "A", 256, &conc, &tokens), 2);
    wait_until(|| conc.started() == 2, "A's first two");
    let b = h.start(tokened("/a", "B", 5, &conc, &tokens), 2);
    pending(&h, 261);
    for _ in 0..10 {
        let before = conc.started();
        tokens.give(1);
        wait_until(|| conc.started() > before, "the next start");
    }
    let next: Vec<String> = conc.starts.lock().unwrap().iter().skip(2).take(10).cloned().collect();
    assert_eq!(next, ["B0", "A2", "B1", "A3", "B2", "A4", "B3", "A5", "B4", "A6"]);
    tokens.give(1000);
    assert_eq!(ok_count(&a.join()), 256);
    assert_eq!(ok_count(&b.join()), 5);
}

/// The streaming form yields every result once, tagged with its index (in submission order
/// across submits), as jobs finish, not in input order.
#[test]
fn the_streaming_form_yields_every_result_once() {
    let h = H::new();
    let mut s: Stream<R> = Stream::open(&h.ctx, 3, Result::is_ok);
    let slow = Latch::new();
    let l = slow.clone();
    let mut first = vec![job("/a/slow", move || {
        l.wait();
        Ok(100)
    })];
    first.extend((1..4u32).map(|i| job(&format!("/b/{i}"), move || Ok(100 + i))));
    assert_eq!(s.submit(first).unwrap(), 0..4);
    let mut got: HashMap<usize, u32> = HashMap::new();
    let mut order = Vec::new();
    while got.len() < 3 {
        if let Some((i, o, _)) = s.recv_timeout(Duration::from_millis(50)) {
            let JobOutcome::Done(Ok(v)) = o else { panic!("{o:?}") };
            assert!(got.insert(i, v).is_none(), "{i} twice");
            order.push(i);
        }
    }
    assert!(!got.contains_key(&0), "the slow job is still running");
    assert_eq!(s.submit((4..7u32).map(|i| job(&format!("/c/{i}"), move || Ok(100 + i))).collect()).unwrap(), 4..7);
    s.close();
    slow.release();
    while got.len() < 7 {
        if let Some((i, o, _)) = s.recv_timeout(Duration::from_millis(50)) {
            let JobOutcome::Done(Ok(v)) = o else { panic!("{o:?}") };
            assert!(got.insert(i, v).is_none(), "{i} twice");
            order.push(i);
        }
    }
    for (i, v) in &got {
        assert_eq!(*v, 100 + *i as u32);
    }
    assert!(s.recv_timeout(Duration::from_millis(20)).is_none(), "nothing more");
    assert!(order.iter().position(|&i| i == 0) > order.iter().position(|&i| i == 1), "as they finish");
    pending(&h, 0);
}

/// A single-job call never enters the pool: it runs on the calling thread, outside the volume's
/// limit (R2's exception), and still checks the registry.
#[test]
fn a_single_job_call_bypasses_the_pool() {
    let h = H::new();
    let me = std::thread::current().id();
    let (r, _) = h.run(vec![job("/a/1", move || Ok(u32::from(std::thread::current().id() == me)))], 4);
    assert!(matches!(r.outcomes.as_slice(), [JobOutcome::Done(Ok(1))]));
    assert!(!h.ctx.pool_up(), "no pool for a single job");
    // a call holds every slot of /a: a single job still runs at once, here
    let (conc, tokens) = (Arc::new(Conc::default()), Tokens::new());
    let a = h.start(tokened("/a", "A", 6, &conc, &tokens), 2);
    wait_until(|| conc.running() == 2, "A at its cap");
    let (r, _) = h.run(vec![job("/a/2", move || Ok(u32::from(std::thread::current().id() == me)))], 4);
    assert!(matches!(r.outcomes.as_slice(), [JobOutcome::Done(Ok(1))]));
    assert_eq!(conc.started(), 2, "A's queue still waits");
    assert_eq!(h.ctx.pending.load(Ordering::SeqCst), 6, "the single job never entered the pool");
    tokens.give(6);
    assert_eq!(ok_count(&a.join()), 6);
    // a blocked volume: the single job runs as the recovery job (here: fails without bytes, so
    // the block stays), and the next fails at once without entering the pool
    let (latch, started) = (Latch::new(), Arc::new(AtomicUsize::new(0)));
    let stall = h.start(hanging("/b", 2, &latch, &started, true), 4);
    wait_until(|| started.load(Ordering::SeqCst) == 2, "start");
    stall.settle(&h, 3);
    h.advance(STALL_WITH_BYTES);
    assert_eq!(vnr_count(&stall.join()), 2);
    let (r, _) = h.run(vec![job("/b/gone", || Err("gone".into()))], 4);
    assert!(matches!(r.outcomes.as_slice(), [JobOutcome::Done(Err(_))]));
    let (r, _) = h.run(vec![job("/b/next", || Ok(1))], 4);
    assert!(matches!(r.outcomes.as_slice(), [JobOutcome::VolumeNotResponding { .. }]));
    assert_eq!(h.ctx.pending.load(Ordering::SeqCst), 0);
    latch.release();
}

/// From a pool thread: one job, through either form, runs inline on that thread (restoring the
/// outer job's state afterwards); several get an error at once, never a deadlock.
#[test]
fn a_pool_thread_runs_one_job_inline_and_refuses_several() {
    let h = H::new();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let jobs = (0..2)
        .map(|n| {
            let (ctx, seen) = (h.ctx.clone(), seen.clone());
            job(&format!("/a/outer{n}"), move || {
                let outer = crate::memory::current_job().map(|j| j.id);
                let me = std::thread::current().id();
                let (cancel, done) = (AtomicBool::new(false), AtomicUsize::new(0));
                // one job, run_jobs form: inline, with its own state
                let inner: Vec<IoJob<R>> = vec![job("/b/inner", move || {
                    let id = crate::memory::current_job().map(|j| j.id).unwrap_or(0);
                    Ok(u32::from(std::thread::current().id() == me) * 1000 + u32::from(Some(id) != outer && id != 0))
                })];
                let r = run_jobs(&ctx, inner, 4, Result::is_ok, &cancel, &done).unwrap();
                let one = matches!(r.outcomes.as_slice(), [JobOutcome::Done(Ok(1001))]);
                let restored = crate::memory::current_job().map(|j| j.id) == outer && outer.is_some();
                // several, run_jobs form: refused
                let refused = matches!(run_jobs(&ctx, quick("/b", 2), 4, Result::is_ok, &cancel, &done), Err(SubmitError::FromPoolThread));
                // the streaming form: one inline, several refused
                let mut s: Stream<R> = Stream::open(&ctx, 4, Result::is_ok);
                let me2 = std::thread::current().id();
                let range = s.submit(vec![job("/c/one", move || Ok(u32::from(std::thread::current().id() == me2)))]);
                let streamed = matches!(s.recv_timeout(Duration::from_secs(5)), Some((0, JobOutcome::Done(Ok(1)), _)));
                let stream_refused = matches!(s.submit(quick("/c", 2)), Err(SubmitError::FromPoolThread));
                seen.lock().unwrap().push((one, restored, refused, range == Ok(0..1) && streamed, stream_refused));
                Ok(n)
            })
        })
        .collect();
    let (r, _) = h.run(jobs, 2);
    assert_eq!(ok_count(&r), 2);
    for s in seen.lock().unwrap().iter() {
        assert_eq!(*s, (true, true, true, true, true));
    }
}

/// A supervisor panic (injected) fails the jobs not yet started with "scheduler stopped",
/// delivers the running jobs' results, and later calls fall back to per-call schedulers.
#[test]
fn a_supervisor_panic_stops_only_unstarted_jobs() {
    let h = H::new();
    let (latch, started) = (Latch::new(), Arc::new(AtomicUsize::new(0)));
    let mut jobs = hanging("/a", 2, &latch, &started, true);
    jobs.extend(quick("/a", 3));
    let call = h.start(jobs, 2);
    wait_until(|| started.load(Ordering::SeqCst) == 2, "the first two to run");
    pending(&h, 5);
    h.ctx.panic_supervisor.store(true, Ordering::SeqCst);
    wait_until(|| !h.ctx.pool_up(), "the supervisor to stop");
    wait_until(|| call.done.load(Ordering::SeqCst) == 3, "the queued jobs to fail");
    assert!(!call.finished(), "the running jobs' results still come");
    latch.release();
    let done = call.done.clone();
    let r = call.join();
    assert!(matches!(r.outcomes.get(..2), Some([JobOutcome::Done(Ok(0)), JobOutcome::Done(Ok(1))])), "{:?}", r.outcomes);
    assert!(r.outcomes.iter().skip(2).all(|o| matches!(o, JobOutcome::SchedulerStopped)), "{:?}", r.outcomes);
    assert_eq!(done.load(Ordering::SeqCst), 5);
    // later calls: their own schedulers, on worker threads
    let me = std::thread::current().id();
    let jobs = (0..6).map(|i| job(&format!("/a/l{i}"), move || Ok(u32::from(std::thread::current().id() != me)))).collect();
    let (r, done) = h.run(jobs, 3);
    assert!(r.outcomes.iter().all(|o| matches!(o, JobOutcome::Done(Ok(1)))));
    assert_eq!(done, 6);
    assert!(Stream::<R>::open(&h.ctx, 2, Result::is_ok).own_scheduler());
    assert!(!h.ctx.pool_up());
}

/// The supervisor can't be spawned (injected): each call runs its own scheduler (limits, stall
/// rule and all), and a later call tries the pool again.
#[test]
fn a_supervisor_spawn_failure_falls_back() {
    let h = H::new();
    h.ctx.supervisor_budget.store(0, Ordering::SeqCst);
    let (latch, started) = (Latch::new(), Arc::new(AtomicUsize::new(0)));
    let mut jobs = hanging("/a", 2, &latch, &started, true);
    jobs.extend(quick("/b", 4));
    let call = h.start(jobs, 4);
    wait_until(|| started.load(Ordering::SeqCst) == 2 && call.done.load(Ordering::SeqCst) == 4, "/a hung and /b done");
    call.settle(&h, 3);
    h.advance(STALL_WITH_BYTES);
    let r = call.join();
    assert_eq!((vnr_count(&r), ok_count(&r)), (2, 4), "the stall rule runs in the call's own scheduler");
    assert!(!h.ctx.pool_up());
    h.ctx.supervisor_budget.store(1, Ordering::SeqCst);
    let (r, _) = h.run(quick("/c", 4), 2);
    assert_eq!(ok_count(&r), 4);
    assert!(h.ctx.pool_up(), "the next call starts the pool");
    latch.release();
}

/// A streaming caller that stops reading results for a minute stops neither admission nor the
/// stall rule for other calls (the supervisor runs them), and its results wait for it.
#[test]
fn a_stream_not_read_stops_nothing_else() {
    let h = H::new();
    let mut s: Stream<R> = Stream::open(&h.ctx, 2, Result::is_ok);
    s.submit(quick("/c", 4)).unwrap();
    // another call: a hung volume and a healthy one
    let (latch, started) = (Latch::new(), Arc::new(AtomicUsize::new(0)));
    let mut jobs = hanging("/a", 2, &latch, &started, true);
    jobs.extend(quick("/b", 6));
    let call = h.start(jobs, 4);
    wait_until(|| started.load(Ordering::SeqCst) == 2 && call.done.load(Ordering::SeqCst) == 6, "/b done beside the hung /a");
    call.settle(&h, 3);
    h.advance(STALL_WITH_BYTES);
    let r = call.join();
    assert_eq!((vnr_count(&r), ok_count(&r)), (2, 6));
    h.advance(STALL_WITH_BYTES);
    let (r, _) = h.run(quick("/b", 3), 2);
    assert_eq!(ok_count(&r), 3);
    // a minute later, the stream's results are all there
    s.close();
    let mut got = Vec::new();
    while got.len() < 4 {
        if let Some((i, o, _)) = s.recv_timeout(Duration::from_millis(50)) {
            assert!(matches!(o, JobOutcome::Done(Ok(_))));
            got.push(i);
        }
    }
    got.sort_unstable();
    assert_eq!(got, [0, 1, 2, 3]);
    latch.release();
}

/// Cancel and `done` stay per call: one call's cancel leaves another call's jobs on the same
/// volume running, and each call counts only its own.
#[test]
fn cancel_and_done_stay_per_call() {
    let h = H::new();
    let (conc, tokens) = (Arc::new(Conc::default()), Tokens::new());
    let a = h.start(tokened("/a", "A", 6, &conc, &tokens), 2);
    wait_until(|| conc.running() == 2, "A at its cap");
    let b = h.start(tokened("/a", "B", 6, &conc, &tokens), 2);
    pending(&h, 12);
    a.cancel.store(true, Ordering::SeqCst);
    wait_until(|| h.ctx.pending.load(Ordering::SeqCst) == 8, "A's four queued jobs to end");
    tokens.give(100);
    let (ad, bd) = (a.done.clone(), b.done.clone());
    let (ra, rb) = (a.join(), b.join());
    assert_eq!(ok_count(&ra), 2, "A's running jobs finish");
    assert_eq!(ra.outcomes.iter().filter(|o| matches!(o, JobOutcome::NotStarted)).count(), 4);
    assert_eq!(ok_count(&rb), 6, "B is untouched");
    assert_eq!((ad.load(Ordering::SeqCst), bd.load(Ordering::SeqCst)), (2, 6));
}

/// Cancel's 5 s rule counts a call's own jobs: a cancelled call's silent job on a volume that
/// another call keeps reading is left behind at 5 s (not just below).
#[test]
fn cancel_silence_counts_only_the_calls_own_jobs() {
    for (by, abandoned) in [(CANCEL_SILENCE - 1, false), (CANCEL_SILENCE, true), (CANCEL_SILENCE + 1, true)] {
        let h = H::new();
        let (latch, started) = (Latch::new(), Arc::new(AtomicUsize::new(0)));
        // A: one job on /a that goes silent after its first byte, one quick job elsewhere
        let mut jobs = hanging("/a", 1, &latch, &started, true);
        jobs.extend(quick("/b", 1));
        let a = h.start(jobs, 2);
        wait_until(|| started.load(Ordering::SeqCst) == 1 && a.done.load(Ordering::SeqCst) == 1, "A's jobs");
        // B: two jobs on /a that keep moving bytes
        let stop = Arc::new(AtomicBool::new(false));
        let b_jobs = (0..2)
            .map(|i| {
                let (stop, started) = (stop.clone(), started.clone());
                job(&format!("/a/b{i}"), move || {
                    started.fetch_add(1, Ordering::SeqCst);
                    while !stop.load(Ordering::SeqCst) {
                        bytes(1);
                        std::thread::sleep(Duration::from_micros(300));
                    }
                    Ok(9)
                })
            })
            .collect();
        // (workers 4: the pool's cap lets both of B's jobs in beside A's)
        let b = h.start(b_jobs, 4);
        wait_until(|| started.load(Ordering::SeqCst) == 3, "B reading /a");
        a.cancel.store(true, Ordering::SeqCst);
        a.settle(&h, 3);
        h.advance(by);
        a.settle(&h, 5);
        if !abandoned {
            assert!(!a.finished(), "{by}");
        }
        let r = if abandoned { Some(a.join()) } else { None };
        if let Some(r) = &r {
            assert!(matches!(r.outcomes.first(), Some(JobOutcome::Cancelled)), "{by}: {:?}", r.outcomes);
        }
        stop.store(true, Ordering::SeqCst);
        assert_eq!(ok_count(&b.join()), 2);
        latch.release();
    }
}

/// Dropping a stream cancels its jobs not yet started; its running job finishes and the pool
/// forgets the call.
#[test]
fn dropping_a_stream_cancels_its_unstarted_jobs() {
    let h = H::new();
    let (latch, started) = (Latch::new(), Arc::new(AtomicUsize::new(0)));
    let mut s: Stream<R> = Stream::open(&h.ctx, 1, Result::is_ok);
    s.submit(hanging("/a", 1, &latch, &started, true)).unwrap();
    s.submit(quick("/a", 3)).unwrap();
    wait_until(|| started.load(Ordering::SeqCst) == 1, "the first to run");
    pending(&h, 4);
    drop(s);
    pending(&h, 1);
    latch.release();
    pending(&h, 0);
    // the pool goes on for others
    let (r, _) = h.run(quick("/a", 3), 2);
    assert_eq!(ok_count(&r), 3);

    // a stream submits again after closing: a new call, indexes going on; dropping it cancels
    // the closed call's unstarted jobs too
    let (latch, started) = (Latch::new(), Arc::new(AtomicUsize::new(0)));
    let mut s: Stream<R> = Stream::open(&h.ctx, 1, Result::is_ok);
    assert_eq!(s.submit(quick("/b", 2)).unwrap(), 0..2);
    s.close();
    let mut got = Vec::new();
    while got.len() < 2 {
        if let Some((i, o, _)) = s.recv_timeout(Duration::from_millis(50)) {
            assert!(matches!(o, JobOutcome::Done(Ok(_))));
            got.push(i);
        }
    }
    pending(&h, 0);
    assert_eq!(s.submit(hanging("/b", 1, &latch, &started, true)).unwrap(), 2..3);
    s.close();
    assert_eq!(s.submit(quick("/b", 2)).unwrap(), 3..5);
    wait_until(|| started.load(Ordering::SeqCst) == 1, "the hung job to run");
    pending(&h, 3);
    drop(s);
    pending(&h, 1);
    latch.release();
    pending(&h, 0);
}

/// A wake from sleep while the pool holds no call is still seen (the idle supervisor looks every
/// second): the registry's entries get the 120 s grace, so a volume blocked before the sleep
/// isn't blocked for the first call after it.
#[test]
fn a_wake_while_idle_gives_the_registry_its_grace() {
    let h = H::new();
    let (latch, started) = (Latch::new(), Arc::new(AtomicUsize::new(0)));
    let call = h.start(hanging("/a", 2, &latch, &started, true), 4);
    wait_until(|| started.load(Ordering::SeqCst) == 2, "start");
    call.settle(&h, 3);
    h.advance(STALL_WITH_BYTES);
    assert_eq!(vnr_count(&call.join()), 2);
    let a = lcc_catalog::folders::volume_key("/a/x", &lcc_catalog::folders::Mounts::from_points(["/", "/a"], false));
    assert!(h.ctx.shared.blocked(&a, h.now()));
    h.clock.sleep(super::shared::WAKE_JUMP + 1);
    wait_until(|| !h.ctx.shared.blocked(&a, h.now()), "the idle pool to notice the wake");
    latch.release();
}
