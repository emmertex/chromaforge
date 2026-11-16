//! The catalog log written by a thread of its own (OneSlips' PERF-STEP-STALL): appends never wait
//! for the drive, yet what was acknowledged is durable, order is kept, a crash loses at most the
//! unacknowledged tail, a failed write loses nothing a snapshot can't save, and shutdown flushes.

#![cfg(not(target_arch = "wasm32"))]

use std::path::{Path, PathBuf};

use crate::journal::LOG;
use crate::log_writer::FAIL_WRITES;
use crate::*;

struct Dir(PathBuf);

impl Dir {
    fn new(tag: &str) -> Dir {
        let d = std::env::temp_dir().join(format!("lc-log-thread-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Dir(d)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        FAIL_WRITES.lock().unwrap().retain(|p| !p.starts_with(&self.0));
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn open(dir: &Path) -> (Journal, Catalog, LoadReport) {
    Journal::open(Box::new(FsStore::open(dir).unwrap())).unwrap()
}

/// A copy of the library's files as they are on disk now: what a crash at this moment leaves.
fn crash_copy(from: &Path, to: &Path) {
    let _ = std::fs::remove_dir_all(to);
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        if e.file_type().unwrap().is_file() {
            std::fs::copy(e.path(), to.join(e.file_name())).unwrap();
        }
    }
}

fn add_photo(c: &mut Catalog) -> Vec<Op> {
    let id = c.alloc_photo_id();
    let op = Op::AddPhoto { photo: Box::new(Photo::new(id, Source::File { path: format!("/p/{}.jpg", id.0) }, "p.jpg", "JPEG", 4, 3, "2026-01-01")) };
    c.apply(op.clone()).unwrap();
    vec![op]
}

fn rate(c: &mut Catalog, id: PhotoId, rating: u8) -> Vec<Op> {
    let op = Op::SetRating { id, rating };
    c.apply(op.clone()).unwrap();
    vec![op]
}

fn rating(c: &Catalog, id: PhotoId) -> u8 {
    c.photo(id).unwrap().rating
}

#[test]
fn a_store_writes_the_log_on_its_own_thread_in_order() {
    let d = Dir::new("order");
    let mut store = FsStore::open(&d.0).unwrap();
    let mut want = Vec::new();
    for i in 0..200u32 {
        let line = format!("{i}\n");
        store.append(LOG, line.as_bytes()).unwrap();
        want.extend_from_slice(line.as_bytes());
        if i % 50 == 0 {
            assert_eq!(store.read(LOG).unwrap().unwrap(), want, "all of it, through the store");
        }
    }
    assert!(store.log_progress(LOG).is_some(), "a thread writes the log");
    store.flush(LOG).unwrap();
    assert_eq!(store.log_progress(LOG).unwrap().durable_len, want.len() as u64, "all of it acknowledged");
    store.truncate(LOG, 4).unwrap();
    assert_eq!(std::fs::read(d.0.join(LOG)).unwrap(), b"0\n1\n");
    // never longer than it is
    store.truncate(LOG, 400).unwrap();
    assert_eq!(std::fs::read(d.0.join(LOG)).unwrap(), b"0\n1\n");
    store.append(LOG, b"x\n").unwrap();
    store.write_atomic(LOG, b"tail\n").unwrap();
    assert_eq!(std::fs::read(d.0.join(LOG)).unwrap(), b"tail\n", "a replace comes after the appends before it");
    store.append(LOG, b"more\n").unwrap();
    drop(store);
    assert_eq!(std::fs::read(d.0.join(LOG)).unwrap(), b"tail\nmore\n", "dropping the store writes what it was handed");
}

/// A flush acknowledges every append, and a crash right after it keeps all of them.
#[test]
fn flushed_appends_are_acknowledged_and_survive_a_crash() {
    let d = Dir::new("flush");
    let copy = Dir::new("flush-copy");
    let (mut j, mut c, _) = open(&d.0);
    j.append(&add_photo(&mut c)).unwrap();
    let id = c.photos().next().unwrap().id;
    for r in 0..40u8 {
        j.append(&rate(&mut c, id, r % 6)).unwrap();
    }
    j.flush().unwrap();
    assert_eq!(j.durable_seq(), j.seq(), "every op acknowledged after a flush");
    let want = rating(&c, id);
    // the process dies here: no drop, no close
    crash_copy(&d.0, &copy.0);
    let (j2, c2, report) = open(&copy.0);
    assert!(report.damaged.is_none());
    assert_eq!(j2.seq(), j.seq());
    assert_eq!(rating(&c2, id), want);
}

/// A crash in the middle of a write (part of the last record on disk, more handed over and not
/// written) loses only what was not acknowledged: every acknowledged op comes back, nothing reads
/// as damage, and later ops are never replayed without the ones before them.
#[test]
fn a_crash_mid_write_loses_at_most_the_unacknowledged_tail() {
    let d = Dir::new("crash");
    let copy = Dir::new("crash-copy");
    let (mut j, mut c, _) = open(&d.0);
    j.append(&add_photo(&mut c)).unwrap();
    let id = c.photos().next().unwrap().id;
    j.append(&rate(&mut c, id, 2)).unwrap();
    j.flush().unwrap();
    let acked = j.durable_seq();
    for r in 0..30u8 {
        j.append(&rate(&mut c, id, 3 + r % 3)).unwrap();
    }
    crash_copy(&d.0, &copy.0);
    // the crash tore the last record in half
    let log = copy.0.join(LOG);
    let mut bytes = std::fs::read(&log).unwrap();
    bytes.extend_from_slice(b"{\"seq\":99999,\"op\":{\"SetRat");
    std::fs::write(&log, &bytes).unwrap();
    let (j2, c2, report) = open(&copy.0);
    assert!(report.damaged.is_none(), "a torn tail is not damage");
    assert!(j2.seq() >= acked, "acknowledged ops are back: {} < {acked}", j2.seq());
    assert!(j2.seq() <= j.seq());
    assert!(rating(&c2, id) >= 2);
}

/// A write on the log's thread fails (a full disk): the journal cuts the log back to its last
/// acknowledged record, refuses appends that would leave a hole, and a snapshot saves the ops the
/// write lost. Reopened, everything is there.
#[test]
fn a_failed_write_loses_nothing_once_a_snapshot_saves_it() {
    let d = Dir::new("fail");
    let (mut j, mut c, _) = open(&d.0);
    j.append(&add_photo(&mut c)).unwrap();
    let id = c.photos().next().unwrap().id;
    j.append(&rate(&mut c, id, 1)).unwrap();
    j.flush().unwrap();
    let acked = j.durable_seq();
    FAIL_WRITES.lock().unwrap().push(d.0.join(LOG));
    j.append(&rate(&mut c, id, 4)).unwrap(); // handed over: not acknowledged
    assert!(j.flush().is_err(), "the failed write is reported");
    assert_eq!(j.durable_seq(), acked);
    assert_eq!(j.lost(), 1);
    assert!(j.wants_snapshot());
    FAIL_WRITES.lock().unwrap().clear();
    // no append after a hole
    let more = rate(&mut c, id, 5);
    assert!(j.append(&more).is_err());
    // the engine's answer: a snapshot of the live catalog, counting the queued op
    j.snapshot_with_unlogged(&c, 1).unwrap();
    assert_eq!(j.lost(), 0);
    j.append(&rate(&mut c, id, 3)).unwrap();
    j.flush().unwrap();
    let seq = j.seq();
    drop(j);
    let (j2, c2, report) = open(&d.0);
    assert!(report.damaged.is_none());
    assert_eq!(j2.seq(), seq);
    assert_eq!(rating(&c2, id), 3);
}

/// Closing without a flush (the app quitting) still writes and syncs everything handed over.
#[test]
fn shutdown_flushes_the_log() {
    let d = Dir::new("shutdown");
    let (mut j, mut c, _) = open(&d.0);
    j.append(&add_photo(&mut c)).unwrap();
    let id = c.photos().next().unwrap().id;
    for r in 0..50u8 {
        j.append(&rate(&mut c, id, r % 6)).unwrap();
    }
    let (seq, want) = (j.seq(), rating(&c, id));
    drop(j);
    let (j2, c2, _) = open(&d.0);
    assert_eq!(j2.seq(), seq);
    assert_eq!(rating(&c2, id), want);
}
