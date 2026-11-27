//! Copies through the scheduler (I/O spec §7, "Copies (PR 2)"), on fake volumes: a copy holds a
//! slot on its source and its destination (R3), R4(a) counts only volumes whose work doesn't
//! also need this one, a copy's bytes are progress on the volume they moved on and its completion
//! on both, a copy stuck on one of its volumes is declared there and not on the other, a hung
//! destination fails its own queue only, and cancel skips a copy's DNG step instead of abandoning
//! it. The real copy path (`import::run_transfers`) is tested in `import/tests_transfers.rs`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use lcc_catalog::folders::{Mounts, VolumeKey, volume_key};

use super::clock::{FakeClock, SEC};
use super::run::{IoJob, JobOutcome};
use super::shared::{CANCEL_SILENCE, COMPUTING_FACTOR, RECOVERY_EVERY, STALL_WITH_BYTES};
use super::tests::{H, Latch, R, bytes, job, ok_count, vnr_count, wait_until, wrote};

/// A copy from `source` into `dest`.
fn copy(source: &str, dest: &str, f: impl FnOnce() -> R + Send + 'static) -> IoJob<R> {
    IoJob { source: source.to_string(), dest: Some(dest.to_string()), skippable: false, run: Box::new(f) }
}

fn key(path: &str) -> VolumeKey {
    volume_key(path, &Mounts::from_points(["/", "/a", "/b", "/c", "/d", "/srv/nas"], false))
}

/// The outcome's volume and whether the job was abandoned, for a "volume not responding" one.
fn vnr(o: Option<&JobOutcome<R>>) -> Option<(String, bool)> {
    match o {
        Some(JobOutcome::VolumeNotResponding { volume, abandoned }) => Some((volume.id().to_string(), *abandoned)),
        _ => None,
    }
}

/// Jobs running per volume (a copy on both of its own), and the most seen: overall, per volume,
/// and per volume while some unfinished job didn't need it (R4a's "cut").
#[derive(Default)]
struct Conc {
    st: Mutex<ConcState>,
}

#[derive(Default)]
struct ConcState {
    running: HashMap<String, usize>,
    all: usize,
    max_all: usize,
    max_vol: HashMap<String, usize>,
    max_cut: HashMap<String, usize>,
    /// Jobs not yet finished, and per volume those that need it.
    unfinished: usize,
    unfinished_on: HashMap<String, usize>,
}

impl Conc {
    fn plan(&self, vols: &[&str]) {
        let mut g = self.st.lock().unwrap();
        g.unfinished += 1;
        for v in vols {
            *g.unfinished_on.entry(v.to_string()).or_default() += 1;
        }
    }

    fn enter(&self, vols: &[&str]) {
        let mut g = self.st.lock().unwrap();
        g.all += 1;
        g.max_all = g.max_all.max(g.all);
        for v in vols {
            let c = {
                let c = g.running.entry(v.to_string()).or_default();
                *c += 1;
                *c
            };
            let m = g.max_vol.entry(v.to_string()).or_default();
            *m = (*m).max(c);
            let on = g.unfinished_on.get(*v).copied().unwrap_or(0);
            if g.unfinished > on {
                let m = g.max_cut.entry(v.to_string()).or_default();
                *m = (*m).max(c);
            }
        }
    }

    fn leave(&self, vols: &[&str]) {
        let mut g = self.st.lock().unwrap();
        g.all -= 1;
        g.unfinished -= 1;
        for v in vols {
            *g.running.entry(v.to_string()).or_default() -= 1;
            *g.unfinished_on.entry(v.to_string()).or_default() -= 1;
        }
    }

    fn max_vol(&self, v: &str) -> usize {
        self.st.lock().unwrap().max_vol.get(v).copied().unwrap_or(0)
    }

    fn running(&self, v: &str) -> usize {
        self.st.lock().unwrap().running.get(v).copied().unwrap_or(0)
    }
}

/// `n` jobs from `src` (into `dst`, a copy, unless it is the same volume or empty) that enter
/// `conc`, move a byte, wait for `gate` (when given) and leave.
fn tracked(src: &str, dst: &str, n: usize, conc: &Arc<Conc>, gate: Option<&Arc<Latch>>) -> Vec<IoJob<R>> {
    (0..n)
        .map(|i| {
            let vols: Vec<String> = if dst.is_empty() || dst == src { vec![src.into()] } else { vec![src.into(), dst.into()] };
            conc.plan(&vols.iter().map(String::as_str).collect::<Vec<_>>());
            let (conc, gate) = (conc.clone(), gate.cloned());
            let run = move || {
                let v: Vec<&str> = vols.iter().map(String::as_str).collect();
                conc.enter(&v);
                bytes(1);
                match &gate {
                    Some(g) => g.wait(),
                    None => std::thread::sleep(Duration::from_millis(2)),
                }
                conc.leave(&v);
                Ok(i as u32)
            };
            if dst.is_empty() { job(&format!("{src}/r{i}"), run) } else { copy(&format!("{src}/f{i}"), &format!("{dst}/out"), run) }
        })
        .collect()
}

// ---------------------------------------------------------------- slots and limits

/// R3: a copy holds a slot on its source and on its destination; no volume ever runs more than
/// the I/O cap, and none more than `cap − 1` while some unfinished job doesn't need it (R4a),
/// at one, two, four and eight workers (the I/O cap set to PR 1's cap, `max(2, workers)`).
#[test]
fn a_copy_holds_a_slot_on_both_volumes() {
    for workers in [1usize, 2, 4, 8] {
        let h = H::capped(workers.max(2));
        let conc = Arc::new(Conc::default());
        let mut jobs = tracked("/a", "/b", 12, &conc, None);
        jobs.extend(tracked("/b", "", 12, &conc, None));
        jobs.extend(tracked("/c", "/a", 12, &conc, None));
        jobs.extend(tracked("/d", "/d", 6, &conc, None));
        let (r, done) = h.run(jobs, workers);
        assert_eq!(ok_count(&r), 42);
        assert_eq!(done, 42);
        let cap = workers.max(2);
        let g = conc.st.lock().unwrap();
        assert!(g.max_all <= cap, "workers {workers}: call cap, {} ran", g.max_all);
        for (v, m) in &g.max_vol {
            assert!(*m <= cap, "workers {workers}: {v} ran {m}");
        }
        for (v, m) in &g.max_cut {
            assert!(*m <= (cap - 1).max(1), "workers {workers}: {v} ran {m} while a job that doesn't need it waited");
        }
    }
}

/// A copy within one volume holds one slot: it runs where that volume's limit is 1 (one worker;
/// an I/O cap of 2 beside another volume's reads, where the limit is `max(1, 2 − 1)`).
#[test]
fn a_copy_within_one_volume_holds_one_slot_and_runs_at_limit_1() {
    let h = H::capped(2);
    let conc = Arc::new(Conc::default());
    let (r, _) = h.run(tracked("/a", "/a", 4, &conc, None), 1);
    assert_eq!(ok_count(&r), 4);
    assert_eq!(conc.max_vol("/a"), 1);

    let conc = Arc::new(Conc::default());
    let gate = Latch::new();
    let mut jobs = tracked("/a", "/a", 3, &conc, Some(&gate));
    jobs.extend(tracked("/b", "", 3, &conc, Some(&gate)));
    let call = h.start(jobs, 2);
    // (one slot on /a, one on /b: if the copy needed two on /a, nothing on /a could start)
    wait_until(|| conc.running("/a") == 1 && conc.running("/b") == 1, "a copy on /a beside a read on /b");
    gate.release();
    let r = call.join();
    assert_eq!(ok_count(&r), 6);
    assert_eq!(conc.max_vol("/a"), 1, "limit 1");

    // and one slot, not two: two workers run two copies within /a at once
    let conc = Arc::new(Conc::default());
    let gate = Latch::new();
    let call = h.start(tracked("/a", "/a", 4, &conc, Some(&gate)), 2);
    wait_until(|| conc.running("/a") == 2, "two copies within /a at once");
    gate.release();
    assert_eq!(ok_count(&call.join()), 4);
}

/// One card copying into one SSD: every job needs both, so neither is held to `cap − 1`: the
/// copies run `workers()` at once (at), never more (above); one worker stays one, as today.
#[test]
fn one_card_into_one_ssd_runs_workers_copies() {
    for workers in [1usize, 2, 4, 8] {
        let h = H::new();
        let conc = Arc::new(Conc::default());
        let gate = Latch::new();
        let call = h.start(tracked("/a", "/b", workers + 3, &conc, Some(&gate)), workers);
        wait_until(|| conc.running("/b") == workers, "workers copies at once");
        call.settle(&h, 5);
        assert_eq!(conc.running("/a"), workers, "workers {workers}: no more than workers");
        gate.release();
        let r = call.join();
        assert_eq!(ok_count(&r), workers + 3);
        assert_eq!((conc.max_vol("/a"), conc.max_vol("/b")), (workers, workers), "workers {workers}");
    }
}

/// Two cards into one SSD: each card is held to `cap − 1` because of the other card, while the
/// SSD, which every job needs, runs the whole cap.
#[test]
fn two_cards_into_one_ssd_hold_each_card_not_the_ssd() {
    for workers in [1usize, 2, 4, 8] {
        let cap = workers.max(2);
        let h = H::capped(cap);
        let conc = Arc::new(Conc::default());
        let gate = Latch::new();
        let mut jobs = tracked("/a", "/c", 2 * cap, &conc, Some(&gate));
        jobs.extend(tracked("/b", "/c", 2, &conc, Some(&gate)));
        let call = h.start(jobs, workers);
        wait_until(|| conc.running("/c") == cap, "the SSD at the whole cap");
        call.settle(&h, 5);
        assert!(conc.running("/a") <= (cap - 1).max(1), "workers {workers}: card /a ran {}", conc.running("/a"));
        assert!(conc.running("/b") >= 1, "workers {workers}: the other card has a slot");
        gate.release();
        let r = call.join();
        assert_eq!(ok_count(&r), 2 * cap + 2);
        let g = conc.st.lock().unwrap();
        for card in ["/a", "/b"] {
            assert!(g.max_cut.get(card).copied().unwrap_or(0) <= (cap - 1).max(1), "workers {workers}: {card}");
        }
        assert_eq!(g.max_vol.get("/c").copied(), Some(cap), "workers {workers}: the SSD isn't held to cap − 1");
    }
}

/// R4a with copies: a card whose copies hang never takes the other card's last slot (it is held
/// to `cap − 1` while the other card has work), so the other card's copies into the same SSD all
/// finish.
#[test]
fn a_hung_card_never_takes_the_other_cards_slot() {
    for workers in [2usize, 4, 8] {
        let cap = workers.max(2);
        let h = H::capped(cap);
        let conc = Arc::new(Conc::default());
        let gate = Latch::new();
        let mut jobs = tracked("/a", "/c", cap + 2, &conc, Some(&gate));
        jobs.extend(tracked("/b", "/c", 6, &conc, None));
        let call = h.start(jobs, workers);
        wait_until(|| call.done.load(Ordering::SeqCst) == 6, "the other card's copies");
        // (were the hung card held to the cap instead, it would take the last slot and the wait
        // above would time out; how close it gets to cap − 1 depends on how fast /b's copies end)
        let ran = conc.st.lock().unwrap().max_cut.get("/a").copied().unwrap_or(0);
        assert!((1..cap).contains(&ran), "workers {workers}: the hung card ran {ran} while the other had work");
        gate.release();
        assert_eq!(ok_count(&call.join()), cap + 8);
    }
}

// ---------------------------------------------------------------- progress and the stall rule

/// A job that counts bytes until `stop` (a volume kept busy).
fn spinner(path: &str, dest: Option<&str>, stop: &Arc<AtomicBool>) -> IoJob<R> {
    let stop = stop.clone();
    let run = move || {
        while !stop.load(Ordering::SeqCst) {
            bytes(1);
            std::thread::yield_now();
        }
        Ok(9)
    };
    match dest {
        Some(d) => copy(path, d, run),
        None => job(path, run),
    }
}

/// `n` copies from `src` into `dst` that read a byte and write one, say they started, then hang
/// in their destination until `latch`.
fn hung_copies(src: &str, dst: &str, n: usize, tag: &str, latch: &Arc<Latch>, started: &Arc<AtomicUsize>) -> Vec<IoJob<R>> {
    (0..n)
        .map(|i| {
            let (latch, started) = (latch.clone(), started.clone());
            copy(&format!("{src}/{tag}{i}"), &format!("{dst}/out"), move || {
                bytes(1);
                wrote(1);
                started.fetch_add(1, Ordering::SeqCst);
                latch.wait();
                Ok(i as u32)
            })
        })
        .collect()
}

/// A hung destination stalls its copies and fails the rest of its queue while copies from the
/// same card to another destination finish; the card itself isn't declared (the copies to the
/// other destination were its progress, and the hung copies leave it with their destination).
#[test]
fn a_hung_destination_fails_its_queue_while_others_finish() {
    // (an I/O cap of 4: /a holds three slots and its fourth is the spinners' turn)
    let h = H::capped(4);
    let (hung, started) = (Latch::new(), Arc::new(AtomicUsize::new(0)));
    let stop = Arc::new(AtomicBool::new(false));
    let mut jobs = hung_copies("/a", "/b", 3, "h", &hung, &started);
    jobs.push(spinner("/a/s0", Some("/c/out"), &stop));
    jobs.push(spinner("/a/s1", Some("/c/out"), &stop));
    jobs.extend(hung_copies("/a", "/b", 2, "q", &hung, &started));
    jobs.extend((0..2).map(|i| {
        copy(&format!("/a/c{i}"), "/c/out", move || {
            bytes(10);
            Ok(100 + i)
        })
    }));
    let call = h.start(jobs, 4);
    wait_until(|| started.load(Ordering::SeqCst) == 3, "three copies hung on /b");
    call.settle(&h, 3);
    // /b falls silent while the copies to /c keep the card moving
    h.advance(10 * SEC);
    call.settle(&h, 3);
    stop.store(true, Ordering::SeqCst);
    wait_until(|| call.done.load(Ordering::SeqCst) == 2, "the copies to /c");
    call.settle(&h, 3);
    // /b: 40 s without progress, the card 30 s (since its last copy to /c): both past the stall
    // time at once; /b, silent longest, is judged first, and its hung copies leave the card
    h.advance(STALL_WITH_BYTES);
    let r = call.join();
    let got: Vec<Option<(String, bool)>> = r.outcomes.iter().map(|o| vnr(Some(o))).collect();
    let b = |abandoned| Some(("/b".to_string(), abandoned));
    assert_eq!(got, [b(true), b(true), b(true), None, None, b(false), b(false), None, None], "{:?}", r.outcomes);
    assert_eq!(ok_count(&r), 4, "the copies to /c finish");
    assert!(h.ctx.shared.blocked(&key("/b/x"), h.now()), "the destination is blocked for later calls");
    assert!(!h.ctx.shared.blocked(&key("/a/x"), h.now()), "the card is not");
    hung.release();
    wait_until(|| h.ctx.shared.entries() == 0, "late returns to leave the registry");
}

/// The card's only running jobs are copies into a destination that hangs (a library import reads
/// everything before it copies; every app import ends that way): the destination is declared and
/// blocked, not the card (both fell silent at once; the copies wait in the destination), and a
/// read of the card runs afterwards. And the other way round: copies that hang
/// reading their card declare the card, not the destination, even where the destination is the
/// volume seen first.
#[test]
fn a_copy_stuck_on_one_volume_is_declared_there_not_on_the_other() {
    let h = H::new();
    let (hung, started) = (Latch::new(), Arc::new(AtomicUsize::new(0)));
    let mut jobs = hung_copies("/a", "/b", 2, "h", &hung, &started);
    jobs.extend(hung_copies("/a", "/b", 2, "q", &hung, &started));
    let call = h.start(jobs, 2);
    wait_until(|| started.load(Ordering::SeqCst) == 2, "two copies hung in /b");
    call.settle(&h, 3);
    h.advance(STALL_WITH_BYTES - 1);
    call.settle(&h, 3);
    assert!(!call.finished(), "just under the stall time");
    h.advance(1);
    let r = call.join();
    let got: Vec<Option<(String, bool)>> = r.outcomes.iter().map(|o| vnr(Some(o))).collect();
    let b = |abandoned| Some(("/b".to_string(), abandoned));
    assert_eq!(got, [b(true), b(true), b(false), b(false)], "{:?}", r.outcomes);
    assert!(h.ctx.shared.blocked(&key("/b/x"), h.now()), "the hung destination is blocked");
    assert!(!h.ctx.shared.blocked(&key("/a/x"), h.now()), "the card is not");
    let (r, _) = h.run(
        vec![job("/a/read", || {
            bytes(1);
            Ok(7)
        })],
        2,
    );
    assert!(matches!(r.outcomes.first(), Some(JobOutcome::Done(Ok(7)))), "the card is read: {:?}", r.outcomes);
    hung.release();
    wait_until(|| h.ctx.shared.entries() == 0, "late returns to leave the registry");

    // a hung card: its copies read a byte, then hang reading it (the destination, /b, is the
    // first volume of the call: a read on it goes first)
    let h = H::new();
    let (hung, started) = (Latch::new(), Arc::new(AtomicUsize::new(0)));
    let mut jobs = vec![job("/b/w", || {
        bytes(1);
        Ok(0)
    })];
    jobs.extend((0..2).map(|i| {
        let (hung, started) = (hung.clone(), started.clone());
        copy(&format!("/a/{i}"), "/b/out", move || {
            bytes(1);
            started.fetch_add(1, Ordering::SeqCst);
            hung.wait();
            Ok(i)
        })
    }));
    let call = h.start(jobs, 2);
    wait_until(|| started.load(Ordering::SeqCst) == 2 && call.done.load(Ordering::SeqCst) == 1, "two copies hung on /a");
    call.settle(&h, 3);
    h.advance(STALL_WITH_BYTES);
    let r = call.join();
    let got: Vec<Option<(String, bool)>> = r.outcomes.iter().map(|o| vnr(Some(o))).collect();
    let a = Some(("/a".to_string(), true));
    assert_eq!(got, [None, a.clone(), a], "{:?}", r.outcomes);
    assert!(h.ctx.shared.blocked(&key("/a/x"), h.now()), "the hung card is blocked");
    assert!(!h.ctx.shared.blocked(&key("/b/x"), h.now()), "the destination is not");
    hung.release();
    wait_until(|| h.ctx.shared.entries() == 0, "late returns to leave the registry");
}

/// A copy's source reads are progress on its card only: a copy stuck in a destination isn't kept
/// alive there by another copy that keeps reading its own card into the same destination. The
/// destination stalls 30 s after the stuck copy's write (at, not just below); the reading card is
/// not declared.
#[test]
fn a_copys_reads_are_not_progress_on_its_destination() {
    for (by, stalled) in [(STALL_WITH_BYTES - 1, false), (STALL_WITH_BYTES, true)] {
        let h = H::new();
        let (hung, started) = (Latch::new(), Arc::new(AtomicUsize::new(0)));
        let stop = Arc::new(AtomicBool::new(false));
        let mut jobs = hung_copies("/a", "/b", 1, "h", &hung, &started);
        jobs.push(spinner("/c/clip.mov", Some("/b/out"), &stop));
        let call = h.start(jobs, 4);
        wait_until(|| started.load(Ordering::SeqCst) == 1, "the copy hung in /b");
        call.settle(&h, 3);
        h.advance(by);
        call.settle(&h, 3);
        assert_eq!(call.finished(), stalled, "{by} ns");
        stop.store(true, Ordering::SeqCst);
        hung.release();
        let r = call.join();
        if stalled {
            assert_eq!(vnr(r.outcomes.first()), Some(("/b".into(), true)), "{:?}", r.outcomes);
            assert!(!h.ctx.shared.blocked(&key("/c/x"), h.now()), "the reading card is not declared");
        } else {
            assert_eq!(ok_count(&r), 2, "{:?}", r.outcomes);
        }
        wait_until(|| h.ctx.shared.entries() == 0, "late returns to leave the registry");
    }
}

/// A copy's bytes count on its destination: once they did, the destination's stall time is 30 s
/// (not the 120 s of a volume that never moved bytes), at 30 s, not just below (the card kept
/// busy by a copy elsewhere).
#[test]
fn a_copys_bytes_set_its_destinations_stall_time() {
    for (by, stalled) in [(STALL_WITH_BYTES - 1, false), (STALL_WITH_BYTES, true), (STALL_WITH_BYTES + 1, true)] {
        let h = H::new();
        let (hung, started) = (Latch::new(), Arc::new(AtomicUsize::new(0)));
        let stop = Arc::new(AtomicBool::new(false));
        let mut jobs = hung_copies("/a", "/b", 2, "h", &hung, &started);
        jobs.push(spinner("/a/s", Some("/c/out"), &stop));
        let call = h.start(jobs, 4);
        wait_until(|| started.load(Ordering::SeqCst) == 2, "the copies to /b");
        call.settle(&h, 3);
        h.advance(by);
        call.settle(&h, 3);
        assert_eq!(call.done.load(Ordering::SeqCst) == 2, stalled, "{by} ns");
        stop.store(true, Ordering::SeqCst);
        hung.release();
        let r = call.join();
        if stalled {
            assert_eq!(vnr(r.outcomes.first()), Some(("/b".into(), true)));
            assert!(matches!(r.outcomes.get(2), Some(JobOutcome::Done(Ok(9)))), "the card's other copy finishes");
        } else {
            assert_eq!(ok_count(&r), 3);
        }
    }
}

/// A copy's completion is progress on its destination too: with one copy hung there (after a
/// byte, so the stall time is 30 s), copies that move no bytes finishing every 20 s keep the
/// destination alive; once they stop, it stalls 30 s after the last one (not just below).
#[test]
fn a_copys_completion_is_progress_on_its_destination() {
    let h = H::new();
    let (hung, started) = (Latch::new(), Arc::new(AtomicUsize::new(0)));
    let mut jobs = hung_copies("/a", "/b", 1, "h", &hung, &started);
    let latches: Vec<Arc<Latch>> = (0..3).map(|_| Latch::new()).collect();
    for (i, l) in latches.iter().enumerate() {
        let (l, started) = (l.clone(), started.clone());
        jobs.push(copy(&format!("/a/{i}"), "/b/out", move || {
            started.fetch_add(1, Ordering::SeqCst);
            l.wait();
            Ok(1)
        }));
    }
    let call = h.start(jobs, 4);
    wait_until(|| started.load(Ordering::SeqCst) == 4, "all four copies");
    call.settle(&h, 3);
    for (i, l) in latches.iter().enumerate() {
        h.advance(20 * SEC);
        call.settle(&h, 3);
        l.release();
        wait_until(|| call.done.load(Ordering::SeqCst) == i + 1, "a byte-less copy to finish");
        call.settle(&h, 3);
    }
    assert!(!call.finished(), "60 s, kept alive by completions on both volumes");
    h.advance(STALL_WITH_BYTES - 1);
    call.settle(&h, 3);
    assert!(!call.finished(), "just under 30 s since the last completion");
    h.advance(1);
    let r = call.join();
    assert!(vnr(r.outcomes.first()).is_some_and(|(_, abandoned)| abandoned), "{:?}", r.outcomes);
    hung.release();
}

/// A fake card at 50 MB/s: each read steps the fake clock by the time it takes.
struct Card {
    left: u64,
    clock: Arc<FakeClock>,
}

impl std::io::Read for Card {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = (buf.len() as u64).min(self.left) as usize;
        self.left -= n as u64;
        self.clock.advance(n as u64 * SEC / 50_000_000);
        std::thread::sleep(Duration::from_micros(20));
        Ok(n)
    }
}

/// One slow 2 GB copy from a card (50 MB/s through the real copy loop, 43 s), with no other job
/// finishing meanwhile, is never declared stalled on the card or the SSD.
#[test]
fn one_slow_2_gb_copy_is_never_declared_stalled() {
    let h = H::new();
    // both volumes have moved bytes before (no idle grace: their stall time is 30 s)
    let warm = |p: &str| {
        job(p, || {
            bytes(1);
            Ok(0)
        })
    };
    let (r, _) = h.run(vec![warm("/a/w"), warm("/b/w")], 2);
    assert_eq!(ok_count(&r), 2);
    let size = 2_000_000_000u64;
    let stop = Arc::new(AtomicBool::new(false));
    let clock = h.clock.clone();
    let s2 = stop.clone();
    let jobs = vec![
        copy("/a/clip.mov", "/b/out", move || {
            let n = crate::import_move::copy_in_chunks(&mut Card { left: size, clock }, &mut std::io::sink()).map_err(|e| e.to_string());
            s2.store(true, Ordering::SeqCst);
            n.map(|n| (n >> 20) as u32)
        }),
        spinner("/c/x", None, &stop),
    ];
    let start = h.now();
    let (r, _) = h.run(jobs, 2);
    assert!(matches!(r.outcomes.first(), Some(JobOutcome::Done(Ok(n))) if u64::from(*n) == size >> 20), "{:?}", r.outcomes);
    assert_eq!(vnr_count(&r), 0);
    assert!(h.now() - start > STALL_WITH_BYTES, "the copy took {} s of the fake clock", (h.now() - start) / SEC);
    let m = r.metrics.first().and_then(Option::as_ref).unwrap();
    assert_eq!((m.moved.read, m.moved.written, m.moved.read_back), (size, size, 0), "every byte counted, read and written");
    assert_eq!(m.dest.as_ref().map(VolumeKey::id), Some("/b"));
}

/// A copy flushing its file to disk pauses its volumes' clocks like computing: longer than the
/// stall time is fine, up to ten stall times; past that the clock runs and a stall time later
/// both are declared.
#[test]
fn a_flush_pauses_the_clock_up_to_ten_stall_times() {
    let h = H::new();
    let (hold, flushing) = (Latch::new(), Arc::new(AtomicBool::new(false)));
    let (l2, f2) = (hold.clone(), flushing.clone());
    let jobs = vec![
        copy("/a/big.mov", "/b/out", move || {
            bytes(1 << 20);
            let _flush = crate::memory::flushing();
            f2.store(true, Ordering::SeqCst);
            l2.wait();
            Ok(1)
        }),
        job("/c/x", || Ok(2)),
    ];
    let call = h.start(jobs, 2);
    wait_until(|| flushing.load(Ordering::SeqCst), "the flush");
    call.settle(&h, 3);
    let ten = STALL_WITH_BYTES * COMPUTING_FACTOR;
    let mut t = 0;
    while t < ten {
        let step = (STALL_WITH_BYTES - SEC).min(ten - t);
        h.advance(step);
        t += step;
        call.settle(&h, 3);
        assert!(!call.finished(), "paused at {} s", t / SEC);
    }
    // past ten stall times the clock runs: just under one stall time more, then over it
    h.advance(STALL_WITH_BYTES - SEC);
    call.settle(&h, 3);
    assert!(!call.finished());
    h.advance(2 * SEC);
    let r = call.join();
    assert!(vnr(r.outcomes.first()).is_some_and(|(_, abandoned)| abandoned), "{:?}", r.outcomes);
    hold.release();
}

// ---------------------------------------------------------------- blocked destinations

/// Block `vol` for later calls: two reads hang on it until it stalls.
fn block(h: &H, vol: &str) -> Arc<Latch> {
    let (latch, started) = (Latch::new(), Arc::new(AtomicUsize::new(0)));
    let jobs = (0..2)
        .map(|i| {
            let (latch, started) = (latch.clone(), started.clone());
            job(&format!("{vol}/hung{i}"), move || {
                bytes(1);
                started.fetch_add(1, Ordering::SeqCst);
                latch.wait();
                Ok(0)
            })
        })
        .collect();
    let call = h.start(jobs, 4);
    wait_until(|| started.load(Ordering::SeqCst) == 2, "the reads to hang");
    call.settle(h, 3);
    h.advance(STALL_WITH_BYTES);
    let _ = call.join();
    assert!(h.ctx.shared.blocked(&key(&format!("{vol}/x")), h.now()));
    latch
}

/// R4d for copies: copies into a blocked destination wait for one recovery copy; one that moves
/// bytes ends the block and the rest run. Later, one that fails without bytes leaves the block
/// and hands on: the next copy goes as the next recovery job (each its own result), and one that
/// succeeds ends the block; when every copy fails without bytes the block stays, and a single
/// copy into it within the minute fails at once.
#[test]
fn copies_into_a_blocked_destination_wait_for_its_recovery_job() {
    let h = H::new();
    let _hung = block(&h, "/b");
    let order = Arc::new(Mutex::new(Vec::new()));
    let jobs = (0..3u32)
        .map(|i| {
            let order = order.clone();
            copy(&format!("/a/{i}"), "/b/out", move || {
                order.lock().unwrap().push(i);
                bytes(5);
                Ok(i)
            })
        })
        .collect();
    let (r, _) = h.run(jobs, 4);
    assert_eq!(ok_count(&r), 3, "{:?}", r.outcomes);
    assert_eq!(order.lock().unwrap().first(), Some(&0), "the first copy ran alone, as the recovery job");
    assert!(!h.ctx.shared.blocked(&key("/b/x"), h.now()));

    let h = H::new();
    let _hung = block(&h, "/b");
    let mut jobs = vec![copy("/a/0", "/b/out", || Err("gone".into()))];
    jobs.extend((1..3u32).map(|i| copy(&format!("/a/{i}"), "/b/out", move || Ok(i))));
    jobs.push(job("/a/read", || Ok(7)));
    let (r, _) = h.run(jobs, 4);
    assert!(
        matches!(r.outcomes.as_slice(), [JobOutcome::Done(Err(_)), JobOutcome::Done(Ok(1)), JobOutcome::Done(Ok(2)), JobOutcome::Done(Ok(7))]),
        "{:?}",
        r.outcomes
    );
    assert!(!h.ctx.shared.blocked(&key("/b/x"), h.now()), "the second copy's success ended the block");

    let h = H::new();
    let _hung = block(&h, "/b");
    let mut jobs: Vec<IoJob<R>> = (0..2).map(|i| copy(&format!("/a/{i}"), "/b/out", || Err("gone".into()))).collect();
    jobs.push(job("/a/read", || Ok(7)));
    let (r, _) = h.run(jobs, 4);
    let got: Vec<Option<(String, bool)>> = r.outcomes.iter().map(|o| vnr(Some(o))).collect();
    assert_eq!(got, [None, None, None], "{:?}", r.outcomes);
    assert!(matches!(r.outcomes.last(), Some(JobOutcome::Done(Ok(7)))), "the card's own read runs");
    assert!(h.ctx.shared.blocked(&key("/b/x"), h.now()));
    // a single copy into it: no recovery job may run within the minute, so it fails at once
    h.advance(RECOVERY_EVERY - 1);
    let (r, _) = h.run(vec![copy("/a/9", "/b/out", || Ok(9))], 4);
    assert_eq!(vnr(r.outcomes.first()), Some(("/b".into(), false)));
    h.advance(1);
    let (r, _) = h.run(vec![copy("/a/9", "/b/out", || Ok(9))], 4);
    assert!(matches!(r.outcomes.first(), Some(JobOutcome::Done(Ok(9)))), "after a minute it runs as the recovery job");
}

// ---------------------------------------------------------------- cancel

/// Cancel while a copy waits at the work gate for its DNG step: it is told to skip the step (not
/// abandoned), returns within about 100 ms, and the caller collects its result; a read waiting at
/// the gate beside it is abandoned unread.
#[test]
fn cancel_skips_a_copys_dng_step_and_collects_it() {
    let h = H::new();
    let gate: &'static crate::memory::WorkGate = Box::leak(Box::new(crate::memory::WorkGate::new(100)));
    let _held = gate.acquire(60);
    let waiting = Arc::new(AtomicUsize::new(0));
    let (w1, w2) = (waiting.clone(), waiting.clone());
    let jobs = vec![
        IoJob {
            source: "/a/raw.nef".into(),
            dest: Some("/b/out".into()),
            skippable: true,
            run: Box::new(move || {
                bytes(100);
                w1.fetch_add(1, Ordering::SeqCst);
                match gate.acquire_for_job(80) {
                    Err(crate::memory::GateError::Skipped) => Ok(1),
                    Err(e) => Err(e.to_string()),
                    Ok(_) => Err("admitted".into()),
                }
            }),
        },
        job("/c/x.jpg", move || {
            w2.fetch_add(1, Ordering::SeqCst);
            gate.acquire_for_job(80).map(|_| 2).map_err(|e| e.to_string())
        }),
    ];
    let call = h.start(jobs, 4);
    wait_until(|| waiting.load(Ordering::SeqCst) == 2, "both at the gate");
    call.settle(&h, 3);
    let t = std::time::Instant::now();
    call.cancel.store(true, Ordering::SeqCst);
    let r = call.join();
    assert!(t.elapsed() < Duration::from_secs(2), "returned in {:?}", t.elapsed());
    assert!(matches!(r.outcomes.first(), Some(JobOutcome::Done(Ok(1)))), "skipped and collected: {:?}", r.outcomes);
    assert!(matches!(r.outcomes.get(1), Some(JobOutcome::Cancelled)), "{:?}", r.outcomes);
}

/// After cancel a copy isn't left behind as "outcome unknown" while it does legitimate work its
/// bytes don't show on a volume (the clocks share one silence rule): flushing its file (well past
/// cancel's 5 s), then reading its copy back on the destination, which the card doesn't see.
#[test]
fn cancel_waits_for_a_copys_flush_and_its_work_on_the_other_volume() {
    let h = H::new();
    let (hold, flushing, stop) = (Latch::new(), Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
    let read_back = Arc::new(AtomicUsize::new(0));
    let (l2, f2, s2, rb) = (hold.clone(), flushing.clone(), stop.clone(), read_back.clone());
    let jobs = vec![
        copy("/a/big.mov", "/b/out", move || {
            bytes(1 << 20);
            wrote(1 << 20);
            {
                let _flush = crate::memory::flushing();
                f2.store(true, Ordering::SeqCst);
                l2.wait();
            }
            while !s2.load(Ordering::SeqCst) {
                if let Some(j) = crate::memory::current_job() {
                    j.add(super::job::ByteKind::ReadBack, 1);
                }
                rb.fetch_add(1, Ordering::SeqCst);
                std::thread::sleep(Duration::from_micros(300));
            }
            Ok(1)
        }),
        job("/c/x", || Ok(2)),
    ];
    let call = h.start(jobs, 2);
    wait_until(|| flushing.load(Ordering::SeqCst), "the flush");
    call.cancel.store(true, Ordering::SeqCst);
    call.settle(&h, 3);
    h.advance(STALL_WITH_BYTES - SEC);
    call.settle(&h, 3);
    assert!(!call.finished(), "a flush isn't silence: cancel waits for it");
    hold.release();
    let n = read_back.load(Ordering::SeqCst);
    wait_until(|| read_back.load(Ordering::SeqCst) > n + 3, "the read-back");
    call.settle(&h, 3);
    h.advance(CANCEL_SILENCE + SEC);
    call.settle(&h, 3);
    assert!(!call.finished(), "reading back on the destination isn't silence on the card");
    stop.store(true, Ordering::SeqCst);
    let r = call.join();
    assert!(matches!(r.outcomes.first(), Some(JobOutcome::Done(Ok(1)))), "the copy finished: {:?}", r.outcomes);
}

/// A call with no jobs (an Add import's batch has no copies) returns at once: no mount read, no
/// thread.
#[test]
fn an_empty_call_reads_no_mount_list() {
    let reads = Arc::new(AtomicUsize::new(0));
    let r2 = reads.clone();
    let h = H::with_reader(Arc::new(move || {
        r2.fetch_add(1, Ordering::SeqCst);
        Some(Mounts::from_points(["/"], false))
    }));
    let (r, done) = h.run(Vec::new(), 4);
    assert!(r.outcomes.is_empty());
    assert_eq!((done, reads.load(Ordering::SeqCst)), (0, 0));
}
