//! The work gate's job-aware wait ([`crate::memory::WorkGate::acquire_for_job`]; I/O spec §5.2
//! "Gate", §7 "gate rule"): what abandoned jobs strand (A), when a request can never fit, the
//! order of operations, and the gate inside the scheduler.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use lcc_catalog::folders::{Mounts, VolumeKey, volume_key};

use super::clock::{Clock, FakeClock, SEC};
use super::job::JobState;
use super::run::{IoJob, JobOutcome};
use super::shared::{AbandonKind, COMPUTING_FACTOR, STALL_WITH_BYTES, Shared};
use super::tests::{H, Latch, R, bytes, job, ok_count, quick, wait_until};
use crate::memory::{GateError, Permit, WorkGate, with_job};

fn vkey(path: &str) -> VolumeKey {
    volume_key(path, &Mounts::from_points(["/", "/a", "/b"], false))
}

/// A registry with a fake clock.
struct Reg {
    clock: Arc<FakeClock>,
    shared: Arc<Shared>,
}

impl Reg {
    fn new() -> Reg {
        let clock = Arc::new(FakeClock::default());
        clock.advance(1000 * SEC);
        let shared = Arc::new(Shared::new(clock.clone()));
        Reg { clock, shared }
    }

    fn job(&self, vol: &str) -> Arc<JobState> {
        Arc::new(JobState::new(self.shared.next_id(), vkey(vol), self.clock.clone(), Arc::downgrade(&self.shared)))
    }

    /// A job on `vol` that took `bytes` of `gate`, then was abandoned (`kind`) and never returns.
    fn stranded<'g>(&self, gate: &'g WorkGate, vol: &str, bytes: usize, kind: AbandonKind) -> (Arc<JobState>, Permit<'g>) {
        let j = self.job(vol);
        let permit = with_job(Some(j.clone()), || gate.acquire_for_job(bytes)).unwrap();
        self.shared.add_entry(&j, kind, self.clock.mono());
        assert!(j.try_abandon());
        (j, permit)
    }

    fn a(&self) -> u128 {
        self.shared.stranded(self.clock.mono()).0
    }
}

/// `bytes` from `gate` as a fresh job of `reg` on /b, on another thread: `Some(result)` if it
/// returned within `within`, `None` if it was still waiting (it is then left to finish).
fn try_acquire(reg: &Reg, gate: &'static WorkGate, bytes: usize, within: Duration) -> Option<Result<(), GateError>> {
    let j = reg.job("/b");
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let r = with_job(Some(j), || gate.acquire_for_job(bytes).map(drop));
        let _ = tx.send(r);
    });
    rx.recv_timeout(within).ok()
}

fn leak(limit: usize) -> &'static WorkGate {
    Box::leak(Box::new(WorkGate::new(limit)))
}

const AT_ONCE: Duration = Duration::from_secs(5);
const WAITS: Duration = Duration::from_millis(300);

/// A permit stranded for ever: a request with stranded + size > limit fails at once with the gate
/// reason while small permits elsewhere keep cycling; one that fits beside it runs.
#[test]
fn a_stranded_permit_fails_what_can_never_fit() {
    let reg = Reg::new();
    let gate = leak(100);
    let (_j, _stuck) = reg.stranded(gate, "/a", 60, AbandonKind::Stall);
    assert_eq!(reg.a(), 60);
    let stop = Arc::new(AtomicBool::new(false));
    let s2 = stop.clone();
    let cycler = std::thread::spawn(move || {
        while !s2.load(Ordering::SeqCst) {
            drop(gate.acquire(5));
            std::thread::yield_now();
        }
    });
    match try_acquire(&reg, gate, 50, AT_ONCE) {
        Some(Err(GateError::CannotFit { volume })) => assert_eq!(volume, "/a"),
        other => panic!("{other:?}"),
    }
    assert!(GateError::CannotFit { volume: "/a".into() }.to_string().contains("memory held by a read that is not returning"));
    assert_eq!(try_acquire(&reg, gate, 30, AT_ONCE), Some(Ok(())), "a file that fits beside it");
    stop.store(true, Ordering::SeqCst);
    cycler.join().unwrap();
}

/// A + B at, just below and just above the limit, with a legitimate holder making the request
/// wait: only above does it fail; at and below it waits, and runs once the holder lets go.
#[test]
fn cannot_fit_threshold() {
    for (b, fails) in [(39usize, false), (40, false), (41, true)] {
        let reg = Reg::new();
        let gate = leak(100);
        let (_j, _stuck) = reg.stranded(gate, "/a", 60, AbandonKind::Stall);
        let holder = gate.acquire(30);
        let got = try_acquire(&reg, gate, b, if fails { AT_ONCE } else { WAITS });
        if fails {
            assert!(matches!(got, Some(Err(GateError::CannotFit { .. }))), "B {b}: {got:?}");
        } else {
            assert_eq!(got, None, "B {b} waits");
            drop(holder);
        }
    }
}

/// B > limit with a 1-byte A fails at once; B > limit with A = 0 runs once the gate is empty.
#[test]
fn an_oversized_request() {
    let reg = Reg::new();
    let gate = leak(100);
    let (_j, _stuck) = reg.stranded(gate, "/a", 1, AbandonKind::Stall);
    assert!(matches!(try_acquire(&reg, gate, 150, AT_ONCE), Some(Err(GateError::CannotFit { .. }))));

    let reg = Reg::new();
    let gate = leak(100);
    let holder = gate.acquire(10);
    let j = reg.job("/b");
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(with_job(Some(j), || gate.acquire_for_job(150).map(drop)));
    });
    assert!(rx.recv_timeout(WAITS).is_err(), "waits for the gate to empty, as today");
    drop(holder);
    assert_eq!(rx.recv_timeout(AT_ONCE).ok(), Some(Ok(())));
}

/// A > limit fails every request of more than 0 bytes; a 0-byte request is admitted at once (a
/// probe whose `metadata` failed then reports its own error).
#[test]
fn stranded_above_the_limit() {
    let reg = Reg::new();
    let gate = leak(100);
    let (_j, _stuck) = reg.stranded(gate, "/a", 150, AbandonKind::Stall);
    assert!(matches!(try_acquire(&reg, gate, 1, AT_ONCE), Some(Err(GateError::CannotFit { .. }))));
    assert_eq!(try_acquire(&reg, gate, 0, AT_ONCE), Some(Ok(())));
}

/// `set_budget` (the gate's limit) raised during a wait lets the waiter through.
#[test]
fn a_raised_limit_lets_the_waiter_through() {
    let reg = Reg::new();
    let gate = leak(100);
    let _holder = gate.acquire(90);
    let j = reg.job("/b");
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(with_job(Some(j), || gate.acquire_for_job(20).map(drop)));
    });
    assert!(rx.recv_timeout(WAITS).is_err());
    gate.set_limit(200);
    assert_eq!(rx.recv_timeout(AT_ONCE).ok(), Some(Ok(())));
}

/// Waits through `acquire_for_job` count in `waits`/`waited_ms` and in the thread's gate wait.
#[test]
fn job_waits_are_counted() {
    let reg = Reg::new();
    let gate = leak(100);
    let holder = gate.acquire(90);
    let j = reg.job("/b");
    let before = gate.waits().0;
    let t = std::thread::spawn(move || {
        let w0 = crate::memory::gate_wait_nanos();
        with_job(Some(j.clone()), || gate.acquire_for_job(20).map(drop)).unwrap();
        (crate::memory::gate_wait_nanos() - w0, j.at_gate())
    });
    std::thread::sleep(Duration::from_millis(50));
    drop(holder);
    let (waited, still_at_gate) = t.join().unwrap();
    assert_eq!(gate.waits().0, before + 1);
    assert!(waited >= 40_000_000, "{waited} ns");
    assert!(!still_at_gate);
    // no wait, no count
    let j = reg.job("/b");
    with_job(Some(j), || gate.acquire_for_job(10).map(drop)).unwrap();
    assert_eq!(gate.waits().0, before + 1);
}

/// A permit dropped by a job that then hangs no longer counts; the gate admits again.
#[test]
fn a_dropped_permit_no_longer_counts() {
    let reg = Reg::new();
    let gate = leak(100);
    let (j, stuck) = reg.stranded(gate, "/a", 60, AbandonKind::Stall);
    assert_eq!(reg.a(), 60);
    drop(stuck);
    assert_eq!(j.permit_bytes(), 0);
    assert_eq!(reg.a(), 0);
    assert_eq!(try_acquire(&reg, gate, 90, AT_ONCE), Some(Ok(())));
}

/// Gate order: a stranded permit dropping while a waiter that fits beside the rest is evaluated
/// admits the waiter (the job's record is cleared before the gate subtracts).
#[test]
fn a_waiter_is_admitted_when_a_stranded_permit_drops() {
    let reg = Reg::new();
    let gate = leak(100);
    let (_j, stuck) = reg.stranded(gate, "/a", 60, AbandonKind::Stall);
    let _holder = gate.acquire(30);
    let j = reg.job("/b");
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(with_job(Some(j), || gate.acquire_for_job(20).map(drop)));
    });
    assert!(rx.recv_timeout(WAITS).is_err(), "60 + 30 held: 20 more waits");
    drop(stuck);
    assert_eq!(rx.recv_timeout(AT_ONCE).ok(), Some(Ok(())));
    // and many times over, with the drop racing the waiter's look: never a refusal, since A + B
    // never exceeds the limit
    for _ in 0..200 {
        let (_j, stuck) = reg.stranded(gate, "/a", 60, AbandonKind::Stall);
        let j = reg.job("/b");
        let t = std::thread::spawn(move || with_job(Some(j), || gate.acquire_for_job(40).map(drop)));
        drop(stuck);
        assert_eq!(t.join().unwrap(), Ok(()));
    }
}

/// A cancel-abandoned job still reading, and a computing job abandoned by its volume's stall,
/// don't count in A; the computing one does once its span passes 10 × its stall time (at, just
/// below, just above).
#[test]
fn what_counts_as_stranded() {
    let reg = Reg::new();
    let gate = leak(1000);
    reg.shared.bytes_seen(&vkey("/a/x"), reg.clock.mono());
    // still reading after cancel: its clock is fresh
    let (reading, _p1) = reg.stranded(gate, "/a", 100, AbandonKind::Cancel);
    // computing when its volume stalled
    let computing = reg.job("/a");
    let _p2 = with_job(Some(computing.clone()), || gate.acquire_for_job(200)).unwrap();
    assert!(computing.begin_computing());
    reg.shared.add_entry(&computing, AbandonKind::Stall, reg.clock.mono());
    assert!(computing.try_abandon());
    assert_eq!(reg.a(), 0);
    reg.clock.advance(10 * SEC);
    reading.add_bytes(1);
    assert_eq!(reg.a(), 0, "reading: fresh; computing: within 10 stall times");
    let span = STALL_WITH_BYTES * COMPUTING_FACTOR;
    reg.clock.advance(span - 10 * SEC - 1);
    reading.add_bytes(1);
    assert_eq!(reg.a(), 0, "just below");
    reg.clock.advance(1);
    reading.add_bytes(1);
    assert_eq!(reg.a(), 0, "at 10 stall times: still paused");
    reg.clock.advance(1);
    reading.add_bytes(1);
    assert_eq!(reg.a(), 200, "past 10 stall times");
    // the reading job stops: 30 s later it counts too
    reg.clock.advance(STALL_WITH_BYTES);
    assert_eq!(reg.a(), 300);
}

/// An entry abandoned while computing doesn't enter A until its clock, restarted when the guard
/// ends, passes its stall time (at, just below, just above).
#[test]
fn a_computing_entry_enters_a_after_its_restarted_clock() {
    let reg = Reg::new();
    let gate = leak(1000);
    reg.shared.bytes_seen(&vkey("/a/x"), reg.clock.mono());
    let j = reg.job("/a");
    let _p = with_job(Some(j.clone()), || gate.acquire_for_job(100)).unwrap();
    assert!(j.begin_computing());
    reg.shared.add_entry(&j, AbandonKind::Stall, reg.clock.mono());
    assert!(j.try_abandon());
    reg.clock.advance(5 * SEC);
    j.end_computing();
    assert_eq!(reg.a(), 0, "the guard just ended");
    reg.clock.advance(STALL_WITH_BYTES - 1);
    assert_eq!(reg.a(), 0, "just below");
    reg.clock.advance(1);
    assert_eq!(reg.a(), 100, "at its stall time");
    reg.clock.advance(1);
    assert_eq!(reg.a(), 100, "above");
}

/// A wake gives every registry entry's own clock 120 s: nothing is stranded or blocked meanwhile.
#[test]
fn a_wake_holds_entries_too() {
    let reg = Reg::new();
    let gate = leak(100);
    let (_j, _p) = reg.stranded(gate, "/a", 60, AbandonKind::Stall);
    assert_eq!(reg.a(), 60);
    reg.shared.note_wake(reg.clock.mono());
    assert_eq!(reg.a(), 0);
    assert!(!reg.shared.blocked(&vkey("/a/x"), reg.clock.mono()));
    reg.clock.advance(super::shared::STALL_GRACE);
    assert_eq!(reg.a(), 60);
}

/// An abandoned job at the gate ends without reading.
#[test]
fn an_abandoned_waiter_ends_unread() {
    let reg = Reg::new();
    let gate = leak(100);
    let _holder = gate.acquire(100);
    let j = reg.job("/b");
    let j2 = j.clone();
    let t = std::thread::spawn(move || with_job(Some(j2), || gate.acquire_for_job(10).map(drop)));
    wait_until(|| j.at_gate(), "the waiter at the gate");
    assert!(j.try_abandon());
    let start = std::time::Instant::now();
    assert_eq!(t.join().unwrap(), Err(GateError::Abandoned));
    assert!(start.elapsed() < Duration::from_secs(2));
    // without a job, acquire_for_job is acquire
    let free = leak(10);
    assert!(free.acquire_for_job(5).is_ok());
}

/// One admission rule for both acquires: a 0-byte request holds nothing and is admitted at once,
/// even while the gate holds more than its limit (one oversized holder), with or without a job.
#[test]
fn a_zero_byte_request_never_waits() {
    let reg = Reg::new();
    let gate = leak(100);
    let _big = gate.acquire(150);
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        drop(gate.acquire(0));
        let _ = tx.send(());
    });
    assert!(rx.recv_timeout(AT_ONCE).is_ok(), "acquire(0) admitted at once");
    assert_eq!(try_acquire(&reg, gate, 0, AT_ONCE), Some(Ok(())), "acquire_for_job(0) admitted at once");
    // (1 byte still waits: it doesn't fit)
    assert_eq!(try_acquire(&reg, gate, 1, WAITS), None);
}

/// The scheduler abandoning a waiter wakes the gate: the wait ends at once, not at its next look
/// (which, with no abandoned job outstanding, is a second away).
#[test]
fn an_abandon_wakes_its_waiter_at_once() {
    let reg = Reg::new();
    let gate = leak(100);
    let _holder = gate.acquire(100);
    let j = reg.job("/b");
    let j2 = j.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(with_job(Some(j2), || gate.acquire_for_job(10).map(drop)));
    });
    wait_until(|| j.at_gate(), "the waiter at the gate");
    // (it has looked, found nothing outstanding and gone to sleep)
    std::thread::sleep(Duration::from_millis(50));
    assert!(j.try_abandon());
    assert_eq!(rx.recv_timeout(WAITS).ok(), Some(Err(GateError::Abandoned)));
}

/// A holder abandoned while another job waits (no entry outstanding when it began to wait) wakes
/// the gate too: the waiter looks again at once and fails "cannot fit".
#[test]
fn an_abandoned_holder_wakes_the_waiters() {
    let reg = Reg::new();
    let gate = leak(100);
    let x = reg.job("/a");
    let held = with_job(Some(x.clone()), || gate.acquire_for_job(60)).unwrap();
    let _other = gate.acquire(30);
    let w = reg.job("/b");
    let w2 = w.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(with_job(Some(w2), || gate.acquire_for_job(50).map(drop)));
    });
    wait_until(|| w.at_gate(), "the waiter at the gate");
    std::thread::sleep(Duration::from_millis(50));
    reg.shared.add_entry(&x, AbandonKind::Stall, reg.clock.mono());
    assert!(x.try_abandon());
    assert!(matches!(rx.recv_timeout(WAITS).ok(), Some(Err(GateError::CannotFit { .. }))));
    drop(held);
}

// ---------------------------------------------------------------- the gate inside the scheduler

/// Jobs on `vol` that take `bytes` of `gate` (through `acquire_for_job`), count a byte, and finish.
fn gated(vol: &str, n: usize, gate: &'static WorkGate, size: usize) -> Vec<IoJob<R>> {
    (0..n)
        .map(|i| {
            job(&format!("{vol}/g{i}"), move || {
                let _p = gate.acquire_for_job(size).map_err(|e| e.to_string())?;
                bytes(1);
                Ok(i as u32)
            })
        })
        .collect()
}

/// A legitimate holder (not a job) keeping most of the gate for 40 s, releasing nothing, makes
/// probes wait; none fails and no volume is declared stalled.
#[test]
fn a_legitimate_holder_makes_jobs_wait_without_a_stall() {
    let h = H::new();
    let gate = leak(100);
    // /a has reported bytes: its stall time is 30 s
    let (r, _) = h.run(quick("/a", 2), 2);
    assert_eq!(ok_count(&r), 2);
    let holder = gate.acquire(90);
    let call = h.start(gated("/a", 3, gate, 50), 4);
    std::thread::sleep(Duration::from_millis(50));
    call.settle(&h, 3);
    h.advance(40 * SEC);
    call.settle(&h, 3);
    assert!(!call.finished(), "waiting, not stalled");
    drop(holder);
    let r = call.join();
    assert_eq!(ok_count(&r), 3);
}

/// Cancel during a worker job's gate wait returns within about 100 ms: the waiting jobs end
/// unread ("cancelled").
#[test]
fn cancel_ends_a_gate_wait_at_once() {
    let h = H::new();
    let gate = leak(100);
    let _holder = gate.acquire(100);
    let call = h.start(gated("/a", 3, gate, 10), 4);
    std::thread::sleep(Duration::from_millis(50));
    call.settle(&h, 3);
    let start = std::time::Instant::now();
    call.cancel.store(true, Ordering::SeqCst);
    let r = call.join();
    assert!(start.elapsed() < Duration::from_secs(1), "{:?}", start.elapsed());
    assert!(r.outcomes.iter().all(|o| matches!(o, JobOutcome::Cancelled)));
}

/// End to end: a job on /a takes memory and hangs; /a stalls; on the healthy /b a file that can't
/// fit beside the stranded memory fails at once with the gate reason (its volume stays usable),
/// one that fits runs, small permits keep cycling meanwhile; after a recovery job clears /a's
/// block, the stranded memory still fails a file that can't fit beside it.
#[test]
fn stranded_memory_end_to_end() {
    let h = H::new();
    let gate = leak(100);
    let hung = Latch::new();
    let started = Arc::new(AtomicUsize::new(0));
    let jobs = (0..2)
        .map(|i| {
            let (hung, started) = (hung.clone(), started.clone());
            job(&format!("/a/{i}"), move || {
                let _p = gate.acquire_for_job(if i == 0 { 60 } else { 0 }).map_err(|e| e.to_string())?;
                bytes(1);
                started.fetch_add(1, Ordering::SeqCst);
                hung.wait();
                Ok(0)
            })
        })
        .collect();
    let call = h.start(jobs, 4);
    wait_until(|| started.load(Ordering::SeqCst) == 2, "start");
    call.settle(&h, 3);
    h.advance(STALL_WITH_BYTES);
    assert_eq!(super::tests::vnr_count(&call.join()), 2);
    assert_eq!(h.ctx.shared.stranded(h.now()).0, 60);

    let stop = Arc::new(AtomicBool::new(false));
    let s2 = stop.clone();
    let cycler = std::thread::spawn(move || {
        while !s2.load(Ordering::SeqCst) {
            drop(gate.acquire(5));
            std::thread::yield_now();
        }
    });
    let failures = Arc::new(Mutex::new(Vec::new()));
    let mut jobs = Vec::new();
    for (i, size) in [50usize, 30, 30].into_iter().enumerate() {
        let f = failures.clone();
        jobs.push(job(&format!("/b/{i}"), move || match gate.acquire_for_job(size) {
            Ok(_p) => {
                bytes(1);
                Ok(i as u32)
            }
            Err(e) => {
                f.lock().unwrap().push(e.clone());
                Err(e.to_string())
            }
        }));
    }
    let (r, _) = h.run(jobs, 1);
    assert!(matches!(r.outcomes.first(), Some(JobOutcome::Done(Err(e))) if e.contains("memory held by a read that is not returning (on /a)")));
    assert_eq!(ok_count(&r), 2, "the healthy volume stays usable");
    assert_eq!(r.metrics.first().and_then(|m| m.as_ref()).and_then(|m| m.gate_failure.clone()).map(|v| v.id().to_string()).as_deref(), Some("/a"));
    assert_eq!(failures.lock().unwrap().len(), 1);

    // a recovery job reads /a again: the block ends, the stranded memory stays
    let (r, _) = h.run(
        vec![job("/a/back", || {
            bytes(1);
            Ok(9)
        })],
        4,
    );
    assert_eq!(ok_count(&r), 1);
    assert!(!h.ctx.shared.blocked(&vkey("/a/x"), h.now()));
    let (r, _) = h.run(gated("/b", 1, gate, 50), 4);
    assert!(matches!(r.outcomes.first(), Some(JobOutcome::Done(Err(e))) if e.contains("not returning")));
    stop.store(true, Ordering::SeqCst);
    cycler.join().unwrap();
    hung.release();
}
