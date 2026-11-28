//! Import copies and moves through the I/O scheduler (I/O spec §7, "Copies (PR 2)"), with real
//! files in temp folders that an injected mount list makes separate volumes and a clock the test
//! steps: today's results, order and names; an abandoned copy's "outcome unknown"; a panicking
//! copy; cancel and a stall at the Copy as DNG step.

#![cfg(not(target_arch = "wasm32"))]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use lcc_catalog::folders::Mounts;

use super::*;
use crate::import_move::{Hold, hold_copies_into, release_copies_into};
use crate::io_sched::IoContext;
use crate::io_sched::clock::{FakeClock, SEC};
use crate::io_sched::shared::STALL_WITH_BYTES;

fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lc-io-transfers-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A scheduler context whose volumes are `mounts`, with a fake clock.
fn context(mounts: &[&Path]) -> (Arc<FakeClock>, Arc<IoContext>) {
    let points: Vec<String> = std::iter::once("/".to_string()).chain(mounts.iter().map(|p| p.to_string_lossy().into_owned())).collect();
    let clock = Arc::new(FakeClock::default());
    clock.advance(1000 * SEC);
    let reader: crate::io_sched::mounts::MountReader = Arc::new(move || Some(Mounts::from_points(points.clone(), false)));
    (clock.clone(), Arc::new(IoContext::new(clock, reader, Duration::from_millis(1))))
}

fn wait_until(mut cond: impl FnMut() -> bool, what: &str) {
    let start = std::time::Instant::now();
    while !cond() {
        assert!(start.elapsed() < Duration::from_secs(20), "timed out waiting for {what}");
        std::thread::sleep(Duration::from_micros(200));
    }
}

/// Wait for `n` more bookkeeping ticks of `ctx`.
fn settle(ctx: &IoContext, n: u64) {
    let start = ctx.ticks.load(Ordering::SeqCst);
    wait_until(|| ctx.ticks.load(Ordering::SeqCst) >= start + n, "ticks");
}

fn write(path: &Path, len: usize, seed: u8) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, (0..len).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed)).collect::<Vec<u8>>()).unwrap();
}

fn transfer(src: &Path, dir: &Path, how: How, dng: bool) -> Transfer {
    let name = src.file_name().unwrap().to_string_lossy().into_owned();
    let info = ProbeInfo { format: "NEF".into(), file_size: std::fs::metadata(src).map(|m| m.len()).unwrap_or(0), ..Default::default() };
    Transfer {
        path: src.to_string_lossy().into_owned(),
        info,
        sidecar: None,
        dir: dir.to_path_buf(),
        how,
        dng,
        promote: None,
        family: (dir.to_string_lossy().to_lowercase(), name_family(&name)),
    }
}

fn copy_to(src: &Path, dir: &Path) -> Transfer {
    transfer(src, dir, How::Copy { name: None }, false)
}

/// An item as text, with `dest` written `<dest>`.
fn describe(item: Option<&PreparedItem>, dest: &Path) -> String {
    let d = dest.to_string_lossy();
    match item {
        None => "not started".into(),
        Some(PreparedItem::Ready(r)) => format!("ready {} {} placed={}", r.stored.replace(d.as_ref(), "<dest>"), r.info.format, r.placed.is_some()),
        Some(PreparedItem::Failed(p, e)) => format!("failed {p}: {}", e.replace(d.as_ref(), "<dest>")),
        Some(PreparedItem::Interrupted(why, e)) => format!("interrupted {:?} {}: {}", why.why, why.path, e.replace(d.as_ref(), "<dest>")),
        Some(_) => "other".into(),
    }
}

/// Run `transfers` on another thread through `ctx`; its cancel flag, `done` and handle.
struct Running {
    cancel: Arc<AtomicBool>,
    done: Arc<AtomicUsize>,
    handle: std::thread::JoinHandle<Vec<Option<PreparedItem>>>,
}

fn start(ctx: &Arc<IoContext>, transfers: Vec<Transfer>, file_bytes: Option<crate::merge::ByteReader>, workers: usize) -> Running {
    let (cancel, done) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicUsize::new(0)));
    let (ctx, c, d) = (ctx.clone(), cancel.clone(), done.clone());
    let handle = std::thread::spawn(move || transfers_scheduled(&ctx, transfers, file_bytes, workers, &c, &d));
    Running { cancel, done, handle }
}

impl Running {
    fn join(self) -> Vec<Option<PreparedItem>> {
        wait_until(|| self.handle.is_finished(), "the call to return");
        self.handle.join().unwrap()
    }
}

/// R1: copies and moves through the scheduler give today's results, order and names: the same as
/// one after another on one thread (today's loop with one worker), at one worker and at four,
/// across two volumes and within one; a taken name gets `-1`, a missing source fails.
#[test]
fn transfers_keep_todays_results_order_and_names() {
    let base = temp_dir("parity");
    let card = base.join("card");
    for i in 0..8u8 {
        write(&card.join(format!("p{i}.nef")), 3000 + usize::from(i) * 977, i);
    }
    let plan = |dest: &Path| -> Vec<Transfer> {
        std::fs::create_dir_all(dest).unwrap();
        std::fs::write(dest.join("p2.nef"), b"someone else's file").unwrap();
        let mut t: Vec<Transfer> = (0..6).map(|i| copy_to(&card.join(format!("p{i}.nef")), dest)).collect();
        t.push(transfer(&card.join("p6.nef"), dest, How::Copy { name: Some("renamed.nef".into()) }, false));
        t.push(transfer(&card.join("p7.nef"), &dest.join("moved"), How::Place { name: "p7.nef".into() }, false));
        t.push(copy_to(&card.join("missing.nef"), dest));
        // Copy as DNG of a file that isn't a raw: the DNG step fails and the raw copy is kept
        t.push(transfer(&card.join("p1.nef"), &dest.join("dng"), How::Copy { name: None }, true));
        t
    };
    let reference: Vec<String> = {
        let dest = base.join("today");
        plan(&dest).into_iter().map(|t| describe(Some(&run_transfer(t, None)), &dest)).collect()
    };
    assert!(reference.iter().any(|r| r.starts_with("ready <dest>/p2-1.nef")), "{reference:?}");
    for (workers, cross) in [(1, true), (4, true), (4, false)] {
        let dest = base.join(format!("w{workers}-{cross}"));
        let mounts: Vec<&Path> = if cross { vec![&card, &dest] } else { vec![] };
        let (_clock, ctx) = context(&mounts);
        let (cancel, done) = (Arc::new(AtomicBool::new(false)), AtomicUsize::new(0));
        let got = transfers_scheduled(&ctx, plan(&dest), None, workers, &cancel, &done);
        let got: Vec<String> = got.iter().map(|i| describe(i.as_ref(), &dest)).collect();
        assert_eq!(got, reference, "workers {workers}, two volumes {cross}");
        assert_eq!(done.load(Ordering::SeqCst), reference.len());
        for i in 0..6 {
            let name = if i == 2 { "p2-1.nef".to_string() } else { format!("p{i}.nef") };
            assert_eq!(std::fs::read(dest.join(&name)).unwrap(), std::fs::read(card.join(format!("p{i}.nef"))).unwrap(), "{name}");
        }
    }
    let _ = std::fs::remove_dir_all(&base);
}

/// An abandoned copy is "outcome unknown", typed as such with the drive and the folder a stray
/// file may appear in; when its thread completes later, the file exists at the destination
/// (nothing in the import's results points at it) and the source is kept. The other copy into
/// the same volume finishes.
#[test]
fn an_abandoned_copy_is_outcome_unknown_and_may_complete_later() {
    let base = temp_dir("abandoned");
    let (card, ssd) = (base.join("card"), base.join("ssd"));
    write(&card.join("a.nef"), 300_000, 1);
    write(&card.join("b.nef"), 200_000, 2);
    let held = ssd.join("held");
    hold_copies_into(&held, Hold::Wait(Default::default()));
    let (clock, ctx) = context(&[&card, &ssd]);
    let run = start(&ctx, vec![copy_to(&card.join("a.nef"), &held), copy_to(&card.join("b.nef"), &ssd.join("free"))], None, 4);
    wait_until(|| run.done.load(Ordering::SeqCst) == 1, "the other copy");
    wait_until(|| std::fs::metadata(held.join("a.nef")).is_ok_and(|m| m.len() == 300_000), "the held copy's bytes");
    settle(&ctx, 3);
    clock.advance(STALL_WITH_BYTES);
    let done = run.done.clone();
    let got = run.join();
    match got.first() {
        Some(Some(PreparedItem::Interrupted(why, e))) => {
            assert!(why.path.ends_with("a.nef"));
            assert_eq!(why.why, Interruption::OutcomeUnknown);
            assert_eq!(why.folder.as_deref().map(Path::new), Some(held.as_path()), "where a stray file may appear");
            assert_eq!(why.volume.as_deref().map(Path::new), Some(ssd.as_path()), "the destination stopped responding");
            assert!(e.starts_with("outcome unknown: ") && e.contains("the original is kept"), "{e}");
        }
        _ => panic!("{}", describe(got.first().and_then(Option::as_ref), &ssd)),
    }
    assert!(matches!(got.get(1), Some(Some(PreparedItem::Ready(_)))));
    assert_eq!(done.load(Ordering::SeqCst), 2, "`done` counts the abandoned copy");
    // the copy's thread completes after all: the file is there, untracked; the source stays
    release_copies_into(&held);
    wait_until(|| ctx.shared.entries() == 0, "the abandoned copy to return");
    assert_eq!(std::fs::read(held.join("a.nef")).unwrap(), std::fs::read(card.join("a.nef")).unwrap());
    assert!(card.join("a.nef").is_file(), "the source is kept");
    let _ = std::fs::remove_dir_all(&base);
}

/// A real copy's flush to disk (the device writing what the page cache holds) pauses its volumes'
/// stall clocks: a flush longer than the stall time doesn't make the copy "outcome unknown".
#[test]
fn a_long_flush_is_not_a_stall() {
    let base = temp_dir("flush");
    let (card, ssd) = (base.join("card"), base.join("ssd"));
    write(&card.join("clip.mov"), 500_000, 8);
    write(&card.join("b.nef"), 1000, 9);
    let slow = ssd.join("slow");
    hold_copies_into(&slow, Hold::InFlush(Default::default()));
    let (clock, ctx) = context(&[&card, &ssd]);
    let run = start(&ctx, vec![copy_to(&card.join("clip.mov"), &slow), copy_to(&card.join("b.nef"), &ssd.join("o"))], None, 4);
    wait_until(|| run.done.load(Ordering::SeqCst) == 1, "the other copy");
    wait_until(|| std::fs::metadata(slow.join("clip.mov")).is_ok_and(|m| m.len() == 500_000), "the clip's bytes");
    settle(&ctx, 3);
    clock.advance(STALL_WITH_BYTES + SEC);
    settle(&ctx, 3);
    assert!(!run.handle.is_finished(), "still flushing, not declared");
    release_copies_into(&slow);
    let got = run.join();
    assert!(
        got.iter().all(|i| matches!(i, Some(PreparedItem::Ready(_)))),
        "{:?}",
        got.iter().map(|i| describe(i.as_ref(), &ssd)).collect::<Vec<_>>()
    );
    let _ = std::fs::remove_dir_all(&base);
}

/// A copy that panics fails only its own file, with today's message.
#[test]
fn a_panicking_copy_fails_only_its_file() {
    let base = temp_dir("panic");
    let (card, ssd) = (base.join("card"), base.join("ssd"));
    for n in ["x", "y", "z"] {
        write(&card.join(format!("{n}.nef")), 5000, 3);
    }
    let boom = ssd.join("boom");
    hold_copies_into(&boom, Hold::Panic);
    let (_clock, ctx) = context(&[&card, &ssd]);
    let transfers =
        vec![copy_to(&card.join("x.nef"), &ssd.join("ok")), copy_to(&card.join("y.nef"), &boom), copy_to(&card.join("z.nef"), &ssd.join("ok2"))];
    let (cancel, done) = (Arc::new(AtomicBool::new(false)), AtomicUsize::new(0));
    let got = transfers_scheduled(&ctx, transfers, None, 4, &cancel, &done);
    release_copies_into(&boom);
    assert!(matches!(got.first(), Some(Some(PreparedItem::Ready(_)))));
    match got.get(1) {
        Some(Some(PreparedItem::Failed(_, e))) => assert!(e.starts_with("import failed unexpectedly") && e.contains("injected"), "{e}"),
        other => panic!("{}", describe(other.and_then(Option::as_ref), &ssd)),
    }
    assert!(matches!(got.get(2), Some(Some(PreparedItem::Ready(_)))));
    assert_eq!(done.load(Ordering::SeqCst), 3);
    let _ = std::fs::remove_dir_all(&base);
}

/// Cancel while a copy waits at the work gate for its Copy as DNG step: it skips the step and
/// returns Ready with its raw copy within about 100 ms, and the caller collects it. (Were it not
/// told to skip, it would wait for ever: the memory it asks for can't fit beside what is held.)
#[test]
fn cancel_at_the_dng_step_keeps_the_raw_copy() {
    let base = temp_dir("dng-cancel");
    let (card, ssd) = (base.join("card"), base.join("ssd"));
    write(&card.join("raw.nef"), 40_000, 4);
    write(&card.join("other.nef"), 1000, 5);
    let (_clock, ctx) = context(&[&card, &ssd]);
    // (held, so a request that can't fit beside it waits; tiny, so other tests' requests fit)
    let held = crate::memory::work_gate().acquire_urgent(1);
    let mut dng = transfer(&card.join("raw.nef"), &ssd, How::Copy { name: None }, true);
    dng.info.file_size = 1 << 50;
    let run = start(&ctx, vec![dng, copy_to(&card.join("other.nef"), &ssd.join("o"))], None, 4);
    wait_until(|| run.done.load(Ordering::SeqCst) == 1, "the other copy");
    wait_until(|| std::fs::metadata(ssd.join("raw.nef")).is_ok_and(|m| m.len() == 40_000), "the raw copy");
    std::thread::sleep(Duration::from_millis(100));
    let t = std::time::Instant::now();
    run.cancel.store(true, Ordering::SeqCst);
    let done = run.done.clone();
    let got = run.join();
    let took = t.elapsed();
    drop(held);
    assert!(took < Duration::from_secs(1), "returned in {took:?}");
    match got.first() {
        Some(Some(PreparedItem::Ready(r))) => {
            assert_eq!(Path::new(&r.stored), ssd.join("raw.nef"), "the raw copy");
            assert_eq!(r.info.format, "NEF");
            assert!(Path::new(&r.stored).is_file());
            assert_eq!(r.dng_not_made, Some(ImportWarningCode::DngCancelled), "and the report will say the DNG wasn't made");
        }
        other => panic!("{}", describe(other.and_then(Option::as_ref), &ssd)),
    }
    assert_eq!(done.load(Ordering::SeqCst), 2, "collected and counted as today");
    let _ = std::fs::remove_dir_all(&base);
}

/// A copy abandoned by a stall at its DNG step (its read of the raw copy hangs) is "outcome
/// unknown"; when it returns, the raw copy is left at the destination, untracked.
#[test]
fn a_stall_at_the_dng_step_is_outcome_unknown() {
    let base = temp_dir("dng-stall");
    let (card, ssd) = (base.join("card"), base.join("ssd"));
    write(&card.join("raw.nef"), 40_000, 6);
    write(&card.join("other.nef"), 1000, 7);
    let (clock, ctx) = context(&[&card, &ssd]);
    let gate = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
    let reading = Arc::new(AtomicBool::new(false));
    let (g2, r2) = (gate.clone(), reading.clone());
    let reader: crate::merge::ByteReader = Arc::new(move |_| {
        r2.store(true, Ordering::SeqCst);
        let mut open = g2.0.lock().unwrap();
        while !*open {
            open = g2.1.wait(open).unwrap();
        }
        Err("the card went away".into())
    });
    let dng = transfer(&card.join("raw.nef"), &ssd, How::Copy { name: None }, true);
    let run = start(&ctx, vec![dng, copy_to(&card.join("other.nef"), &ssd.join("o"))], Some(reader), 4);
    wait_until(|| reading.load(Ordering::SeqCst) && run.done.load(Ordering::SeqCst) == 1, "the DNG step's read to hang");
    settle(&ctx, 3);
    clock.advance(STALL_WITH_BYTES);
    let got = run.join();
    match got.first() {
        Some(Some(PreparedItem::Interrupted(why, e))) => {
            assert!(e.starts_with("outcome unknown: "), "{e}");
            assert_eq!((why.why, why.volume.as_deref().map(Path::new)), (Interruption::OutcomeUnknown, Some(ssd.as_path())));
        }
        other => panic!("{}", describe(other.and_then(Option::as_ref), &ssd)),
    }
    *gate.0.lock().unwrap() = true;
    gate.1.notify_all();
    wait_until(|| ctx.shared.entries() == 0, "the abandoned copy to return");
    assert!(ssd.join("raw.nef").is_file(), "the raw copy is left, untracked");
    assert!(card.join("raw.nef").is_file(), "the source is kept");
    let _ = std::fs::remove_dir_all(&base);
}

/// The wrappers' words for the pool's new outcomes (I/O spec PR 3): a job the stopped pool never
/// started (typed as such, not the file's own failure: the import doesn't call it unreadable),
/// and several files submitted from inside a file job (refused, never a deadlock).
#[test]
fn the_wrappers_name_the_pools_outcomes() {
    use crate::io_sched::JobOutcome;
    let dir = Path::new("/ssd/in");
    for (moving, word) in [(false, "copied"), (true, "moved")] {
        match transfer_outcome(JobOutcome::SchedulerStopped, "/card/a.nef".into(), dir, moving) {
            Some(PreparedItem::Interrupted(why, e)) => {
                assert_eq!(*why, Interrupted { path: "/card/a.nef".into(), why: Interruption::SchedulerStopped, volume: None, folder: None });
                assert_eq!(e, format!("not {word}: the I/O scheduler stopped; the original is kept"));
            }
            other => panic!("{}", describe(other.as_ref(), dir)),
        }
    }
    assert!(transfer_outcome(JobOutcome::NotStarted, "/card/a.nef".into(), dir, false).is_none(), "not started, as today");
    let stopped = probe_outcome(JobOutcome::SchedulerStopped, "/card/a.nef", "not probed").unwrap_err();
    assert_eq!(stopped.message, "/card/a.nef: not read: the I/O scheduler stopped");
    match stopped.into_item("/card/a.nef".into()) {
        PreparedItem::Interrupted(why, _) => assert_eq!((why.why, why.volume), (Interruption::SchedulerStopped, None)),
        other => panic!("{}", describe(Some(&other), dir)),
    }
    assert_eq!(probe_outcome(JobOutcome::NotStarted, "/card/a.nef", "not probed").unwrap_err().message, "not probed");

    // several files probed from inside a pool job: every file fails with the reason, at once
    let (_clock, ctx) = context(&[Path::new("/a")]);
    let probe: crate::media::FileProbe = Arc::new(|_: &str| Ok(ProbeInfo::default()));
    let got = Arc::new(std::sync::Mutex::new(Vec::new()));
    let jobs = (0..2)
        .map(|i| {
            let (ctx, probe, got) = (ctx.clone(), probe.clone(), got.clone());
            crate::io_sched::IoJob::read(
                format!("/a/{i}"),
                Box::new(move || {
                    let (done, cancel) = (AtomicUsize::new(0), Arc::new(AtomicBool::new(false)));
                    let paths = vec!["/a/x".to_string(), "/a/y".to_string()];
                    got.lock().unwrap().push(probe_scheduled(&ctx, &probe, &paths, 4, &done, &cancel));
                    Ok(ProbeInfo::default())
                }) as Box<dyn FnOnce() -> Result<ProbeInfo, String> + Send>,
            )
        })
        .collect();
    let (done, cancel) = (AtomicUsize::new(0), Arc::new(AtomicBool::new(false)));
    let r = crate::io_sched::run_jobs(&ctx, jobs, 2, Result::is_ok, &cancel, &done).unwrap();
    assert!(r.outcomes.iter().all(|o| matches!(o, JobOutcome::Done(Ok(_)))));
    for res in got.lock().unwrap().iter() {
        assert_eq!(res.len(), 2);
        for (r, p) in res.iter().zip(["/a/x", "/a/y"]) {
            assert_eq!(
                r.as_ref().unwrap_err().message,
                format!("{p}: several files can't be read from inside a file job (it would wait for itself)")
            );
        }
    }
}
