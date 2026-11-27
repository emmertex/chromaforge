//! Preparing an import as a stream (I/O spec §7, "PR 4"): with real files on two fake card
//! volumes, the streaming form gives what one `prepare_files` call per batch gives — the same
//! items, names, duplicate decisions, records and undo steps (RAW+JPEG pairs, dual-slot backup
//! cards, rename sequences, name collisions, a failed copy, a file that can't be read), whatever
//! order the reads and copies end in; the per-volume read-ahead bound and the batch horizon (at,
//! just below, just above); a cancel and a refused batch.

#![cfg(not(target_arch = "wasm32"))]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use lcc_catalog::folders::Mounts;

use super::stream::Lookahead;
use super::*;
use crate::import_move::{Hold, hold_copies_into, release_copies_into};
use crate::io_sched::IoContext;
use crate::io_sched::clock::{FakeClock, SEC};

fn temp_root(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lc-import-stream-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("a")).unwrap();
    std::fs::create_dir_all(d.join("b")).unwrap();
    std::fs::create_dir_all(d.join("dest")).unwrap();
    d
}

/// A scheduler context in which `root`'s `a`, `b` and `dest` are three volumes.
fn context(root: &Path) -> Arc<IoContext> {
    let points: Vec<String> =
        ["", "a", "b", "dest"].iter().map(|p| if p.is_empty() { "/".to_string() } else { root.join(p).to_string_lossy().into_owned() }).collect();
    let clock = Arc::new(FakeClock::default());
    clock.advance(1000 * SEC);
    let reader: crate::io_sched::mounts::MountReader = Arc::new(move || Some(Mounts::from_points(points.clone(), false)));
    Arc::new(IoContext::new(clock, reader, Duration::from_millis(1)))
}

fn image(path: &Path, seed: u8, format: crate::export::ExportFormat) {
    let img = lcc_raster::Rgba8::from_fn(16, 12, |x, y| [(x * 9) as u8 ^ seed, (y * 11) as u8, seed, 255]);
    let b = crate::export::encode_image(&img, &crate::export::ExportOptions { format, ..Default::default() }).unwrap();
    std::fs::write(path, b).unwrap();
}

fn png(path: &Path, seed: u8) {
    image(path, seed, crate::export::ExportFormat::Png);
}

fn jpg(path: &Path, seed: u8) {
    image(path, seed, crate::export::ExportFormat::Jpeg);
}

/// Two cards: RAW+JPEG-like pairs (one stem, two formats), a dual-slot backup (the same files on
/// both cards), a name on both cards with different contents, a file that can't be read, and
/// enough others for several batches. Its files in card order (`a` then `b`) and interleaved.
fn two_cards(root: &Path) -> (Vec<String>, Vec<String>) {
    let (a, b) = (root.join("a"), root.join("b"));
    for i in 0..3u8 {
        png(&a.join(format!("DSC_000{i}.png")), 10 + i);
        jpg(&a.join(format!("DSC_000{i}.jpg")), 20 + i);
        // the backup slot: the same files
        png(&b.join(format!("DSC_000{i}.png")), 10 + i);
        jpg(&b.join(format!("DSC_000{i}.jpg")), 20 + i);
    }
    png(&a.join("IMG_0100.png"), 30);
    png(&b.join("IMG_0100.png"), 31);
    std::fs::write(a.join("broken.png"), b"not an image").unwrap();
    for i in 0..5u8 {
        png(&a.join(format!("F_{i:03}.png")), 40 + i);
        png(&b.join(format!("G_{i:03}.png")), 60 + i);
    }
    let list = |d: &Path| {
        let mut v: Vec<String> = std::fs::read_dir(d).unwrap().map(|e| e.unwrap().path().to_string_lossy().into_owned()).collect();
        v.sort();
        v
    };
    let (la, lb) = (list(&a), list(&b));
    let in_order: Vec<String> = la.iter().chain(&lb).cloned().collect();
    let mut mixed = Vec::new();
    for i in 0..la.len().max(lb.len()) {
        mixed.extend(la.get(i).cloned());
        mixed.extend(lb.get(i).cloned());
    }
    (in_order, mixed)
}

fn options(root: &Path, mode: ImportMode) -> ImportOptions {
    ImportOptions { mode, destination: Some(root.join("dest").to_string_lossy().into_owned()), organize: Organize::Flat, ..Default::default() }
}

/// An item as text, `root` written `<root>`.
fn describe(item: &PreparedItem, root: &str) -> String {
    let s = match item {
        PreparedItem::Promote(id) => format!("promote {}", id.0),
        PreparedItem::Duplicate { path, existing, reason, .. } => format!("duplicate {path} {reason} {existing:?}"),
        PreparedItem::Failed(path, e) => format!("failed {path}: {e}"),
        PreparedItem::Interrupted(why, e) => format!("interrupted {:?} {}: {e}", why.why, why.path),
        PreparedItem::Copying(path) => format!("copying {path}"),
        PreparedItem::Kept(k) => format!("kept {} {}", k.path, k.reason),
        PreparedItem::Ready(r) => format!("ready {} -> {} {} placed={}", r.path, r.stored, r.info.format, r.placed.is_some()),
    };
    s.replace(root, "<root>")
}

/// What an import gave: each batch's items and flags, then what committing them did.
#[derive(Debug, PartialEq)]
struct Outcome {
    batches: Vec<(Vec<String>, bool, usize)>,
    photos: Vec<String>,
    reports: Vec<String>,
    undo: usize,
    dest: Vec<String>,
}

fn commit(root: &Path, opts: &ImportOptions, mut s: Session, now: &str, batches: Vec<Prepared>) -> Outcome {
    let r = root.to_string_lossy().into_owned();
    let described = batches.iter().map(|b| (b.items.iter().map(|it| describe(it, &r)).collect(), b.cut_short, b.scanned)).collect();
    let mut reports = Vec::new();
    for b in batches {
        let rep = commit_prepared(&mut s, opts, now, b).unwrap();
        let dups: Vec<String> = rep.duplicates.iter().map(|d| format!("{} {} {:?}", d.path, d.reason, d.existing)).collect();
        let failed: Vec<String> = rep.failed.iter().map(|(p, e)| format!("{p}: {e}")).collect();
        reports.push(format!("imported {:?} dups {dups:?} failed {failed:?} kept {}", rep.imported, rep.kept.len()).replace(&r, "<root>"));
    }
    let mut photos: Vec<String> = s
        .catalog
        .photos()
        .map(|p| {
            let path = match &p.source {
                lcc_catalog::Source::File { path } => path.clone(),
                other => format!("{other:?}"),
            };
            format!("{} {} {} {:?}", p.id.0, p.file_name, path, p.content_hash).replace(&r, "<root>")
        })
        .collect();
    photos.sort();
    let mut dest = Vec::new();
    for e in walk(&root.join("dest")) {
        dest.push(e.to_string_lossy().replace(&r, "<root>"));
    }
    dest.sort();
    Outcome { batches: described, photos, reports, undo: s.undo.len(), dest }
}

fn walk(d: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(walk(&p));
        } else {
            out.push(p);
        }
    }
    out
}

/// A session with file access; `browse`: the destination browsed first (its files Local records).
fn session(root: &Path, browse: bool) -> Session {
    let mut s = Session::new().with_fs();
    if browse {
        s.execute("library.browse", &serde_json::json!({"path": root.join("dest").to_string_lossy()})).unwrap();
    }
    s
}

/// Today's pipeline: one `prepare_files` call per `batch` files.
fn run_batches(root: &Path, files: &[String], opts: &ImportOptions, batch: usize, browse: bool) -> Outcome {
    let mut s = session(root, browse);
    let mut job = ImportJob::new(&mut s, opts.clone()).unwrap();
    let now = job.now().to_string();
    let cancel = Arc::new(AtomicBool::new(false));
    let batches: Vec<Prepared> = files.chunks(batch).map(|c| job.prepare_files(c.to_vec(), &cancel)).collect();
    commit(root, opts, s, &now, batches)
}

/// The streaming form.
fn run_stream(root: &Path, files: &[String], opts: &ImportOptions, look: Lookahead, browse: bool) -> Outcome {
    let ctx = context(root);
    let mut s = session(root, browse);
    let mut job = ImportJob::new(&mut s, opts.clone()).unwrap();
    let now = job.now().to_string();
    let cancel = Arc::new(AtomicBool::new(false));
    let mut batches = Vec::new();
    job.prepare_streamed(&ctx, files.to_vec(), &cancel, look, &mut |p| {
        batches.push(p);
        true
    });
    commit(root, opts, s, &now, batches)
}

fn small() -> Lookahead {
    Lookahead { batch: 4, per_volume: 8, batches: 3 }
}

/// The two pipelines over fresh copies of the same cards, in both orders and with two look-aheads
/// (the streaming form three times: its reads and copies end in a different order each time).
fn parity(tag: &str, mode: ImportMode, tune: impl Fn(&mut ImportOptions), before: impl Fn(&Path)) {
    parity_with(tag, mode, tune, before, false);
}

fn parity_with(tag: &str, mode: ImportMode, tune: impl Fn(&mut ImportOptions), before: impl Fn(&Path), browse: bool) {
    for order in 0..2 {
        let fresh = |name: &str| {
            let root = temp_root(&format!("{tag}-{order}-{name}"));
            let (in_order, mixed) = two_cards(&root);
            before(&root);
            let mut files = if order == 0 { in_order } else { mixed };
            if browse {
                // a browsed copy of a card's file, inside the destination (it joins the library
                // where it is), listed a batch after the file it copies
                let src = root.join("a").join("F_002.png");
                let local = root.join("dest").join("LOCAL.png");
                std::fs::copy(&src, &local).unwrap();
                let at = files.iter().position(|f| Path::new(f) == src).unwrap();
                files.insert((at + 5).min(files.len()), local.to_string_lossy().into_owned());
            }
            let mut opts = options(&root, mode);
            tune(&mut opts);
            (root, files, opts)
        };
        let (root, files, opts) = fresh("old");
        let want = run_batches(&root, &files, &opts, 4, browse);
        let _ = std::fs::remove_dir_all(&root);
        assert!(want.batches.len() > 3, "{tag}: several batches");
        for (i, look) in [small(), small(), Lookahead { batch: 4, per_volume: 4, batches: 1 }].into_iter().enumerate() {
            let (root, files, opts) = fresh(&format!("new{i}"));
            let got = run_stream(&root, &files, &opts, look, browse);
            let _ = std::fs::remove_dir_all(&root);
            assert_eq!(got, want, "{tag}, order {order}, run {i}");
        }
    }
}

/// `library.import`'s one batch of every file: what one `prepare_files` call gives (copy, move,
/// rename), in both orders.
#[test]
fn one_batch_of_every_file_matches_one_call() {
    let tunes: [(&str, ImportMode, fn(&mut ImportOptions)); 3] = [
        ("whole-copy", ImportMode::Copy, |_| {}),
        ("whole-move", ImportMode::Move, |_| {}),
        ("whole-rename", ImportMode::Copy, |o| {
            o.rename = Some("shot-{seq:3}".into());
            o.organize = Organize::Template("{folder}".into());
        }),
    ];
    for (tag, mode, tune) in tunes {
        for order in 0..2 {
            let fresh = |name: &str| {
                let root = temp_root(&format!("{tag}-{order}-{name}"));
                let (in_order, mixed) = two_cards(&root);
                let mut opts = options(&root, mode);
                tune(&mut opts);
                (root, if order == 0 { in_order } else { mixed }, opts)
            };
            let (root, files, opts) = fresh("old");
            let want = run_batches(&root, &files, &opts, files.len(), false);
            let _ = std::fs::remove_dir_all(&root);
            for i in 0..2 {
                let (root, files, opts) = fresh(&format!("new{i}"));
                let ctx = context(&root);
                let mut s = session(&root, false);
                let mut job = ImportJob::new(&mut s, opts.clone()).unwrap();
                let now = job.now().to_string();
                let p = job.prepare_whole(&ctx, files.clone(), &Arc::new(AtomicBool::new(false)));
                let got = commit(&root, &opts, s, &now, vec![p]);
                let _ = std::fs::remove_dir_all(&root);
                assert_eq!(got, want, "{tag}, order {order}, run {i}");
            }
        }
    }
}

/// Copy into one folder: pairs keep their stems, the backup card's files are duplicates of the
/// first card's copies, the name on both cards becomes `IMG_0100-1.png`, the unreadable file
/// fails in its place.
#[test]
fn copies_match_batch_by_batch() {
    parity("copy", ImportMode::Copy, |_| {}, |_| {});
}

/// A rename template with a sequence number, into a folder per card.
#[test]
fn renamed_copies_match_batch_by_batch() {
    parity(
        "rename",
        ImportMode::Copy,
        |o| {
            o.rename = Some("shot-{seq:3}".into());
            o.rename_start = 7;
            o.organize = Organize::Template("{folder}".into());
        },
        |_| {},
    );
}

/// Every copy into the first card's folder fails: the backup card's copies are then no
/// duplicates (they are imported), as batch by batch.
#[test]
fn a_failed_copy_leaves_its_twin_imported_as_batch_by_batch() {
    parity(
        "fail",
        ImportMode::Copy,
        |o| o.organize = Organize::Template("{folder}".into()),
        |root| hold_copies_into(&root.join("dest").join("a"), Hold::Panic),
    );
}

/// A browsed record in the destination with the content of a card's file copied a batch before:
/// it joins the library or is a duplicate exactly as batch by batch (it waits for that copy).
#[test]
fn a_browsed_record_waits_for_the_copy_of_its_content() {
    parity_with("local", ImportMode::Copy, |_| {}, |_| {}, true);
}

#[test]
fn moves_match_batch_by_batch() {
    parity("move", ImportMode::Move, |_| {}, |_| {});
}

/// Added where they are (as Synchronize Folder does): only the reads run ahead.
#[test]
fn adds_match_batch_by_batch() {
    parity("add", ImportMode::Add, |_| {}, |_| {});
}

/// A probe the test holds until it lets go, counting its calls per volume.
#[derive(Default)]
struct Gate {
    open: Mutex<bool>,
    cv: Condvar,
    calls: Mutex<Vec<String>>,
}

impl Gate {
    fn release(&self) {
        *self.open.lock().unwrap() = true;
        self.cv.notify_all();
    }

    fn calls_on(&self, card: &str) -> usize {
        self.calls.lock().unwrap().iter().filter(|p| p.contains(&format!("/{card}/"))).count()
    }
}

fn gated_probe(gate: &Arc<Gate>, real: crate::media::FileProbe) -> crate::media::FileProbe {
    let gate = gate.clone();
    Arc::new(move |path: &str| {
        gate.calls.lock().unwrap().push(path.to_string());
        let mut g = gate.open.lock().unwrap();
        while !*g {
            g = gate.cv.wait(g).unwrap();
        }
        drop(g);
        real(path)
    })
}

fn wait_until(mut cond: impl FnMut() -> bool, what: &str) {
    let start = std::time::Instant::now();
    while !cond() {
        assert!(start.elapsed() < Duration::from_secs(20), "timed out waiting for {what}");
        std::thread::sleep(Duration::from_micros(500));
    }
}

/// `n` readable files on each card, every one with its own content.
fn cards_of(root: &Path, n: usize) -> Vec<String> {
    let mut files = Vec::new();
    for (c, card) in ["a", "b"].into_iter().enumerate() {
        for i in 0..n {
            let p = root.join(card).join(format!("{card}{i:03}.png"));
            png(&p, (c * 100 + i % 100) as u8);
            files.push(p.to_string_lossy().into_owned());
        }
    }
    files
}

/// Run an import of `files` on another thread with `probe`; its batches come back on join.
fn start(
    root: &Path,
    files: Vec<String>,
    opts: ImportOptions,
    look: Lookahead,
    probe: Option<crate::media::FileProbe>,
) -> std::thread::JoinHandle<Vec<Prepared>> {
    start_in(&context(root), files, opts, look, probe)
}

fn start_in(
    ctx: &Arc<IoContext>,
    files: Vec<String>,
    opts: ImportOptions,
    look: Lookahead,
    probe: Option<crate::media::FileProbe>,
) -> std::thread::JoinHandle<Vec<Prepared>> {
    let ctx = ctx.clone();
    let mut s = Session::new().with_fs();
    s.media.file_probe = probe;
    let mut job = ImportJob::new(&mut s, opts).unwrap();
    std::thread::spawn(move || {
        let cancel = Arc::new(AtomicBool::new(false));
        let mut out = Vec::new();
        job.prepare_streamed(&ctx, files, &cancel, look, &mut |p| {
            out.push(p);
            true
        });
        out
    })
}

/// The read-ahead per volume stops at `per_volume` files (at, just below, just above), each volume
/// on its own, while planning waits for the first one: the reads submitted (the pool holds them;
/// it runs a few at a time) are `per_volume` per card.
#[test]
fn reads_ahead_at_most_per_volume_files_on_each_volume() {
    let w = 8;
    for (n, want) in [(w - 1, w - 1), (w, w), (w + 1, w)] {
        let root = temp_root(&format!("ahead-{n}"));
        let files = cards_of(&root, n);
        let gate = Arc::new(Gate::default());
        let real = Session::new().with_fs().media.file_probe.unwrap();
        let look = Lookahead { batch: 4, per_volume: w, batches: 64 };
        let ctx = context(&root);
        let h = start_in(&ctx, files.clone(), options(&root, ImportMode::Add), look, Some(gated_probe(&gate, real)));
        wait_until(|| ctx.pending.load(Ordering::SeqCst) >= 2 * want, "the read-ahead to fill");
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(ctx.pending.load(Ordering::SeqCst), 2 * want, "{n} files a card: no more than {w} read ahead on each");
        assert!(gate.calls_on("a") > 0 && gate.calls_on("b") > 0, "both cards read at once");
        gate.release();
        let batches = h.join().unwrap();
        let ready = batches.iter().flat_map(|b| &b.items).filter(|it| matches!(it, PreparedItem::Ready(_))).count();
        assert_eq!(ready, 2 * n, "every file, once");
        let _ = std::fs::remove_dir_all(&root);
    }
}

/// A file read ahead keeps its place until its copy has its outcome: with copies held, the reads
/// stop at `per_volume` (at, just below, just above), so a copy's read still finds the file in
/// memory.
#[test]
fn a_read_holds_its_place_until_its_copy_is_done() {
    let w = 8;
    for (n, want) in [(w - 1, w - 1), (w, w), (w + 1, w)] {
        let root = temp_root(&format!("hold-{n}"));
        let files: Vec<String> = cards_of(&root, n).into_iter().filter(|f| f.contains("/a/")).collect();
        let calls = Arc::new(AtomicUsize::new(0));
        let real = Session::new().with_fs().media.file_probe.unwrap();
        let c = calls.clone();
        let probe: crate::media::FileProbe = Arc::new(move |p: &str| {
            c.fetch_add(1, Ordering::SeqCst);
            real(p)
        });
        let dest = root.join("dest");
        let g = Arc::new((Mutex::new(false), Condvar::new()));
        hold_copies_into(&dest, Hold::Wait(g));
        let look = Lookahead { batch: 4, per_volume: w, batches: 64 };
        let h = start(&root, files, options(&root, ImportMode::Copy), look, Some(probe));
        wait_until(|| calls.load(Ordering::SeqCst) >= want, "the reads");
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(calls.load(Ordering::SeqCst), want, "{n} files: reads stop at {w} while the copies wait");
        release_copies_into(&dest);
        let batches = h.join().unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), n);
        let ready = batches.iter().flat_map(|b| &b.items).filter(|it| matches!(it, PreparedItem::Ready(_))).count();
        assert_eq!(ready, n);
        let _ = std::fs::remove_dir_all(&root);
    }
}

/// Planning stays within `batches` batches of the oldest one not handed over (at, just below,
/// just above): with copies held, `batches` batches' copies are under way and no more.
#[test]
fn planning_stays_within_the_batch_horizon() {
    let (batch, horizon) = (4usize, 3usize);
    for (n, want) in [(batch * horizon - 1, batch * horizon - 1), (batch * horizon, batch * horizon), (batch * horizon + 1, batch * horizon)] {
        let root = temp_root(&format!("horizon-{n}"));
        let files: Vec<String> = cards_of(&root, n).into_iter().filter(|f| f.contains("/a/")).collect();
        let ctx = context(&root);
        let dest = root.join("dest");
        let g = Arc::new((Mutex::new(false), Condvar::new()));
        hold_copies_into(&dest, Hold::Wait(g));
        // no file access needed to plan: nothing is read ahead, only the batch horizon bounds it
        let mut s = Session::new().with_fs();
        s.media.file_probe = None;
        let mut job = ImportJob::new(&mut s, options(&root, ImportMode::Copy)).unwrap();
        let c = ctx.clone();
        let look = Lookahead { batch, per_volume: 64, batches: horizon };
        let h = std::thread::spawn(move || {
            let cancel = Arc::new(AtomicBool::new(false));
            let mut out = Vec::new();
            job.prepare_streamed(&c, files, &cancel, look, &mut |p| {
                out.push(p);
                true
            });
            out
        });
        wait_until(|| ctx.pending.load(Ordering::SeqCst) >= want, "the copies of the horizon");
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(ctx.pending.load(Ordering::SeqCst), want, "{n} files: {horizon} batches under way, no more");
        release_copies_into(&dest);
        let batches = h.join().unwrap();
        assert_eq!(batches.len(), n.div_ceil(batch));
        let _ = std::fs::remove_dir_all(&root);
    }
}

/// Reads that end in reverse order still give the items in file order, batch by batch.
#[test]
fn results_keep_file_order_whatever_order_reads_end_in() {
    let root = temp_root("order");
    let files = cards_of(&root, 10);
    let real = Session::new().with_fs().media.file_probe.unwrap();
    let total = files.len();
    let index: std::collections::HashMap<String, usize> = files.iter().cloned().enumerate().map(|(i, f)| (f, i)).collect();
    let probe: crate::media::FileProbe = Arc::new(move |p: &str| {
        // later files answer first
        let i = index.get(p).copied().unwrap_or(0);
        std::thread::sleep(Duration::from_millis(((total - i) % 7) as u64 * 3));
        real(p)
    });
    let look = Lookahead { batch: 4, per_volume: 8, batches: 64 };
    let batches = start(&root, files.clone(), options(&root, ImportMode::Add), look, Some(probe)).join().unwrap();
    let got: Vec<Vec<String>> = batches
        .iter()
        .map(|b| {
            b.items
                .iter()
                .map(|it| match it {
                    PreparedItem::Ready(r) => r.path.clone(),
                    other => format!("{other:?}"),
                })
                .collect()
        })
        .collect();
    let want: Vec<Vec<String>> = files.chunks(4).map(<[String]>::to_vec).collect();
    assert_eq!(got, want);
    let _ = std::fs::remove_dir_all(&root);
}

/// A file that can't be read is a failed file in its place; planning goes on.
#[test]
fn a_failed_read_is_a_failed_file_in_its_place() {
    let root = temp_root("failed");
    let files = cards_of(&root, 3);
    let real = Session::new().with_fs().media.file_probe.unwrap();
    let probe: crate::media::FileProbe =
        Arc::new(move |p: &str| if p.ends_with("a001.png") { Err(format!("{p}: volume not responding")) } else { real(p) });
    let batches = start(&root, files.clone(), options(&root, ImportMode::Add), small(), Some(probe)).join().unwrap();
    let kinds: Vec<&str> = batches
        .iter()
        .flat_map(|b| &b.items)
        .map(|it| match it {
            PreparedItem::Ready(_) => "ready",
            PreparedItem::Failed(..) => "failed",
            _ => "other",
        })
        .collect();
    assert_eq!(kinds, ["ready", "failed", "ready", "ready", "ready", "ready"]);
    let _ = std::fs::remove_dir_all(&root);
}

/// Cancel mid-import: the batches with items are handed over in order, the last cut short; every
/// file copied into the destination has its item (nothing copied is left outside the library).
#[test]
fn cancel_hands_over_what_was_done_in_order() {
    let root = temp_root("cancel");
    let files: Vec<String> = cards_of(&root, 24);
    let ctx = context(&root);
    let mut s = Session::new().with_fs();
    let mut job = ImportJob::new(&mut s, options(&root, ImportMode::Copy)).unwrap();
    let cancel = Arc::new(AtomicBool::new(false));
    let mut batches: Vec<Prepared> = Vec::new();
    job.prepare_streamed(&ctx, files.clone(), &cancel, small(), &mut |p| {
        batches.push(p);
        // cancel once the second batch is in
        if batches.len() == 2 {
            cancel.store(true, Ordering::SeqCst);
        }
        true
    });
    assert!(batches.len() >= 2, "{} batches", batches.len());
    let items: usize = batches.iter().map(|b| b.items.len()).sum();
    assert!(items < files.len(), "cancel left files undone ({items} of {})", files.len());
    let mut seen = Vec::new();
    for b in &batches {
        for it in &b.items {
            if let PreparedItem::Ready(r) = it {
                seen.push(PathBuf::from(&r.stored));
                assert!(Path::new(&r.stored).exists());
            }
        }
    }
    let mut on_disk = walk(&root.join("dest"));
    on_disk.sort();
    seen.sort();
    assert_eq!(on_disk, seen, "every copy has its item");
    // and in file order
    let order: Vec<usize> = batches
        .iter()
        .flat_map(|b| &b.items)
        .filter_map(|it| if let PreparedItem::Ready(r) = it { files.iter().position(|f| *f == r.path) } else { None })
        .collect();
    assert!(order.windows(2).all(|w| w[0] < w[1]), "{order:?}");
    let _ = std::fs::remove_dir_all(&root);
}

/// Nobody takes the batches any more (the import was dropped): what is not handed over is taken
/// back — a Move's placed files are removed, its sources untouched. The first file's move is held
/// (each file into its own folder, moves copying as across volumes) until the two batches after it
/// are placed: then the first batch is taken, the second refused, and the third, placed already,
/// must be taken back.
#[test]
fn a_refused_batch_takes_back_the_rest() {
    let root = temp_root("refused");
    let files: Vec<String> = cards_of(&root, 12).into_iter().filter(|f| f.contains("/a/")).collect();
    let ctx = context(&root);
    let mut s = Session::new().with_fs();
    let mut opts = options(&root, ImportMode::Move);
    opts.organize = Organize::Template("{name}".into());
    let mut job = ImportJob::new(&mut s, opts).unwrap();
    let dest = root.join("dest");
    let first = dest.join("a000");
    hold_copies_into(&first, Hold::Wait(Arc::new((Mutex::new(false), Condvar::new()))));
    let (d, f) = (dest.clone(), first.clone());
    let releaser = std::thread::spawn(move || {
        wait_until(|| walk(&d).iter().filter(|p| !p.starts_with(&f)).count() >= 8, "batches 1 and 2 placed");
        release_copies_into(&f);
    });
    crate::import_move::inject(crate::import_move::Fault::NoLink);
    let cancel = Arc::new(AtomicBool::new(false));
    let mut taken: Vec<Prepared> = Vec::new();
    job.prepare_streamed(&ctx, files.clone(), &cancel, small(), &mut |p| {
        if taken.is_empty() {
            taken.push(p);
            true
        } else {
            // the caller takes back the one it refused
            p.rollback();
            false
        }
    });
    crate::import_move::inject(crate::import_move::Fault::None);
    releaser.join().unwrap();
    let kept: Vec<PathBuf> = taken
        .iter()
        .flat_map(|b| &b.items)
        .filter_map(|it| if let PreparedItem::Ready(r) = it { Some(PathBuf::from(&r.stored)) } else { None })
        .collect();
    assert_eq!(kept.len(), 4, "the first batch, taken");
    let mut on_disk = walk(&dest);
    on_disk.sort();
    assert_eq!(on_disk, kept, "only the batch taken is placed");
    assert!(files.iter().all(|f| Path::new(f).exists()), "sources untouched");
    let _ = std::fs::remove_dir_all(&root);
}

/// The look-ahead is never below a batch (a batch could wait for its own files' reads).
#[test]
fn the_read_ahead_is_never_below_a_batch() {
    let root = temp_root("floor");
    let files = cards_of(&root, 6);
    let look = Lookahead { batch: 6, per_volume: 1, batches: 0 };
    let h = start(&root, files, options(&root, ImportMode::Add), look, Session::new().with_fs().media.file_probe);
    // (below a batch, a batch would wait for its own reads for ever)
    wait_until(|| h.is_finished(), "the import to finish");
    let batches = h.join().unwrap();
    assert_eq!(batches.iter().map(|b| b.items.len()).sum::<usize>(), 12);
    let _ = std::fs::remove_dir_all(&root);
}

/// The caller's cancel stops the reads ahead at once, even while the run is busy elsewhere (here:
/// handing a batch over): both streams are linked to it, so the pool starts no further read
/// itself instead of waiting for the run to look. The reads after the first batch wait at a gate
/// until the cancel is set, so most of the twelve batches' reads fed ahead are still queued then:
/// none of those starts (only ones already handed to a reader, at most the stream's `workers`).
/// Without the link, 36 of them started after the cancel.
#[test]
fn a_cancel_stops_reading_ahead_while_the_run_is_busy() {
    use std::sync::atomic::Ordering::SeqCst;
    let root = temp_root("cancel-busy");
    let ctx = context(&root);
    let mut s = Session::new();
    let mut job = ImportJob::new(&mut s, ImportOptions { mode: ImportMode::Add, ..ImportOptions::default() }).unwrap();
    let (cancel, gate) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
    let (early, late) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let (c, g, e, l) = (cancel.clone(), gate.clone(), early.clone(), late.clone());
    job.probe = Some(Arc::new(move |path: &str| {
        let first_batch = ["0.jpg", "1.jpg", "2.jpg", "3.jpg"].iter().any(|n| path.ends_with(&format!("/{n}")));
        if c.load(SeqCst) { &l } else { &e }.fetch_add(1, SeqCst);
        let start = std::time::Instant::now();
        while !first_batch && !g.load(SeqCst) && start.elapsed() < Duration::from_secs(20) {
            std::thread::sleep(Duration::from_micros(200));
        }
        Ok(ProbeInfo { format: "JPEG".into(), ..ProbeInfo::default() })
    }));
    let files: Vec<String> = (0..64).map(|i| root.join("a").join(format!("{i}.jpg")).to_string_lossy().into_owned()).collect();
    let look = Lookahead { batch: 4, per_volume: 48, batches: 12 };
    let mut batches = 0;
    job.prepare_streamed(&ctx, files, &cancel, look, &mut |_| {
        batches += 1;
        if batches == 1 {
            // cancelled, and this thread away until the pool has given every read an outcome
            cancel.store(true, SeqCst);
            gate.store(true, SeqCst);
            let start = std::time::Instant::now();
            while ctx.pending.load(SeqCst) > 0 {
                assert!(start.elapsed() < Duration::from_secs(20), "the pool never settled");
                std::thread::sleep(Duration::from_micros(200));
            }
        }
        true
    });
    let (w, early, late) = (workers(usize::MAX), early.load(SeqCst), late.load(SeqCst));
    assert!(early >= 4, "the first batch was read ({early})");
    assert!(late <= w, "{late} reads started after cancel (at most {w} under way)");
    assert!(early + late < 48, "reads fed ahead never started ({early} + {late})");
    let _ = std::fs::remove_dir_all(&root);
}

/// After the pool's supervisor stopped (a panic) no rule watches a copy hung on its destination:
/// a Cancel still ends the import, the hung copies left behind (outcome unknown, in their
/// places), instead of waiting for them for ever.
#[test]
fn cancel_ends_an_import_whose_copies_hang_after_the_pool_stopped() {
    let root = temp_root("pool-stopped");
    let files: Vec<String> = cards_of(&root, 3).into_iter().filter(|f| f.contains("/a/")).collect();
    let ctx = context(&root);
    let dest = root.join("dest");
    hold_copies_into(&dest, Hold::Wait(Arc::new((Mutex::new(false), Condvar::new()))));
    // (no reads: every job the pool runs is a copy)
    let mut s = Session::new().with_fs();
    s.media.file_probe = None;
    let mut job = ImportJob::new(&mut s, options(&root, ImportMode::Copy)).unwrap();
    let cancel = Arc::new(AtomicBool::new(false));
    let (c, x) = (ctx.clone(), cancel.clone());
    let h = std::thread::spawn(move || {
        let mut out = Vec::new();
        job.prepare_streamed(&c, files, &x, small(), &mut |p| {
            out.push(p);
            true
        });
        out
    });
    // the three copies under way, held
    wait_until(|| ctx.pending.load(Ordering::SeqCst) == 3, "the copies to start");
    std::thread::sleep(Duration::from_millis(100));
    ctx.panic_supervisor.store(true, Ordering::SeqCst);
    wait_until(|| !ctx.pool_up(), "the supervisor to stop");
    cancel.store(true, Ordering::SeqCst);
    let start = std::time::Instant::now();
    while !h.is_finished() && start.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(5));
    }
    let finished = h.is_finished();
    release_copies_into(&dest);
    assert!(finished, "the import never returned after Cancel");
    let batches = h.join().unwrap();
    let r = root.to_string_lossy().into_owned();
    let items: Vec<String> = batches.iter().flat_map(|b| &b.items).map(|it| describe(it, &r)).collect();
    assert_eq!(items.len(), 3, "{items:?}");
    // (a copy still queued when the supervisor stopped is "scheduler stopped")
    assert!(items.iter().all(|i| i.starts_with("interrupted OutcomeUnknown") || i.starts_with("interrupted SchedulerStopped")), "{items:?}");
    assert!(items.iter().any(|i| i.starts_with("interrupted OutcomeUnknown")), "{items:?}");
    let _ = std::fs::remove_dir_all(&root);
}

/// A panic while the run holds planned batches (here: in the caller's hand-over) takes back what
/// they placed, as a refused batch does: a Move leaves nothing in the destination outside the
/// library. The first file's move is held until the two batches after it are placed; handing the
/// first batch over then panics (the caller took that one back itself).
#[test]
fn a_panic_takes_back_the_batches_planned_and_not_handed_over() {
    let root = temp_root("panic");
    let files: Vec<String> = cards_of(&root, 12).into_iter().filter(|f| f.contains("/a/")).collect();
    let ctx = context(&root);
    let mut s = Session::new().with_fs();
    let mut opts = options(&root, ImportMode::Move);
    opts.organize = Organize::Template("{name}".into());
    let mut job = ImportJob::new(&mut s, opts).unwrap();
    let dest = root.join("dest");
    let first = dest.join("a000");
    hold_copies_into(&first, Hold::Wait(Arc::new((Mutex::new(false), Condvar::new()))));
    let (d, f) = (dest.clone(), first.clone());
    let releaser = std::thread::spawn(move || {
        // the first batch's three other files and the two batches after it
        wait_until(|| walk(&d).iter().filter(|p| !p.starts_with(&f)).count() >= 11, "batches 2 and 3 placed");
        release_copies_into(&f);
    });
    crate::import_move::inject(crate::import_move::Fault::NoLink);
    let cancel = Arc::new(AtomicBool::new(false));
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        job.prepare_streamed(&ctx, files.clone(), &cancel, small(), &mut |p| {
            p.rollback();
            panic!("a fault while handing a batch over");
        });
    }));
    crate::import_move::inject(crate::import_move::Fault::None);
    releaser.join().unwrap();
    assert!(panicked.is_err());
    assert_eq!(walk(&dest), Vec::<PathBuf>::new(), "nothing placed is left in the destination");
    assert!(files.iter().all(|f| Path::new(f).exists()), "sources untouched");
    let _ = std::fs::remove_dir_all(&root);
}
