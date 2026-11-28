//! Records in `catalog.redb` that a load can't decode (a deserialization change in a build, a value written as
//! `null`): the load skips them and opens the rest, and nothing that rewrites the store from what was loaded may
//! delete them (Optimise, the full rewrite after another writer changed the store, the scheduled backup's
//! optimise). They are counted, so the user can be told and the integrity test sees them.

use std::path::Path;

use redb::{ReadableDatabase, ReadableTable};

use crate::db::DB_FILE;
use crate::library::{BackupSchedule, CatalogSettings};
use crate::tests_v4::{Scratch, fill};
use crate::*;

const PHOTOS: redb::TableDefinition<u64, &[u8]> = redb::TableDefinition::new("photos");
const COLLECTIONS: redb::TableDefinition<u64, &[u8]> = redb::TableDefinition::new("collections");
const STACKS: redb::TableDefinition<u64, &[u8]> = redb::TableDefinition::new("stacks");
const PREVIEWS: redb::TableDefinition<u64, &[u8]> = redb::TableDefinition::new("previews");

/// What an unreadable record holds (cut-off JSON).
const DAMAGED: &[u8] = b"{\"damaged\":";

fn open(dir: &Path) -> (Journal, Catalog, LoadReport) {
    Journal::open(Box::new(FsStore::open(dir).unwrap())).unwrap()
}

/// A library of `n` photos (with an album, a stack and a preview entry), checkpointed and closed.
fn library(tag: &str, n: u64) -> Scratch {
    let s = Scratch::new(tag);
    let (mut j, mut c, _) = open(s.path());
    fill(&mut j, &mut c, n);
    j.snapshot(&c).unwrap();
    s
}

/// Replace the last record of `table` with bytes no build decodes; returns its key. (In a [`library`] of 6 photos
/// or more, the last photo is in no album or stack.)
fn damage(dir: &Path, table: redb::TableDefinition<u64, &[u8]>) -> u64 {
    let db = redb::Database::open(dir.join(DB_FILE)).unwrap();
    let w = db.begin_write().unwrap();
    let key = {
        let mut t = w.open_table(table).unwrap();
        let key = t.iter().unwrap().next_back().expect("the table has a record").unwrap().0.value();
        t.insert(key, DAMAGED).unwrap();
        key
    };
    w.commit().unwrap();
    key
}

/// The record `key` of `table` as it is on disk now.
fn stored(dir: &Path, table: redb::TableDefinition<u64, &[u8]>, key: u64) -> Option<Vec<u8>> {
    let db = redb::Database::open(dir.join(DB_FILE)).unwrap();
    let r = db.begin_read().unwrap();
    let t = r.open_table(table).unwrap();
    t.get(key).unwrap().map(|v| v.value().to_vec())
}

#[test]
fn an_unreadable_photo_record_is_counted_and_fails_the_integrity_test() {
    let s = library("unreadable-count", 6);
    damage(s.path(), PHOTOS);
    let (mut j, c, r) = open(s.path());
    assert_eq!(c.len(), 5, "the rest of the library opens");
    assert_eq!(r.unreadable, 1, "the skipped record is counted");
    assert_eq!(r.failed, 1, "and folded into the load's failures");
    let report = j.check_integrity(&c).unwrap();
    assert_eq!((report.stored_photos, report.photos), (6, 5));
    // (nothing else fails it: the photo counts are the only difference)
    assert!(report.store_ok && report.dangling_refs == 0, "{report:?}");
    assert!(!report.ok(), "a photo in the store that isn't in memory fails the test: {report:?}");
}

#[test]
fn every_unreadable_record_is_counted_whatever_its_table() {
    let s = library("unreadable-tables", 6);
    for t in [PHOTOS, COLLECTIONS, STACKS, PREVIEWS] {
        damage(s.path(), t);
    }
    let (_j, _c, r) = open(s.path());
    assert_eq!(r.unreadable, 4, "{r:?}");
}

#[test]
fn a_readable_store_has_no_unreadable_records() {
    let s = library("unreadable-none", 5);
    let (mut j, c, r) = open(s.path());
    assert_eq!((r.unreadable, r.failed), (0, 0));
    assert!(j.check_integrity(&c).unwrap().ok());
    j.optimize(&c).expect("a store the load read whole is optimised as before");
}

/// Optimise rewrites the store from the catalog in memory: a record the load skipped would be deleted for good. It
/// is refused, nothing is changed, and the library goes on saving as before.
#[test]
fn optimise_keeps_a_record_the_load_could_not_read() {
    let s = library("unreadable-optimise", 6);
    let key = damage(s.path(), PHOTOS);
    let (mut j, mut c, r) = open(s.path());
    assert_eq!(r.unreadable, 1);
    let e = j.optimize(&c).expect_err("refused while the store holds a record the load skipped");
    let msg = e.to_string();
    assert!(msg.contains("1 record") && msg.contains("could not be read") && msg.contains("nothing was changed"), "{msg}");
    assert_eq!(stored(s.path(), PHOTOS, key).as_deref(), Some(DAMAGED), "the record is still there, as it was");
    // the session goes on: an edit is checkpointed (incrementally) and the record is still kept
    let id = c.photos().next().unwrap().id;
    let op = Op::SetRating { id, rating: 4 };
    c.apply(op.clone()).unwrap();
    j.append(&[op]).unwrap();
    j.snapshot(&c).unwrap();
    drop(j);
    assert_eq!(stored(s.path(), PHOTOS, key).as_deref(), Some(DAMAGED));
    let (_, back, r) = open(s.path());
    assert_eq!(back.photo(id).map(|p| p.rating), Some(4));
    assert_eq!(r.unreadable, 1);
}

/// A checkpoint that finds the store changed by another writer since it was loaded rewrites it whole from memory. With
/// a record the load skipped, that would delete it: the checkpoint is refused instead (the changes stay in the log).
#[test]
fn a_store_changed_by_another_writer_keeps_a_record_the_load_could_not_read() {
    let s = library("unreadable-generation", 6);
    let key = damage(s.path(), PHOTOS);
    let (mut a, mut ca, _) = open(s.path());
    let (mut b, mut cb, _) = open(s.path());
    let ids: Vec<PhotoId> = cb.photos().map(|p| p.id).collect();
    let op = Op::SetRating { id: ids[1], rating: 2 };
    cb.apply(op.clone()).unwrap();
    b.append(&[op]).unwrap();
    b.snapshot(&cb).unwrap();
    drop(b);
    // a is still on the generation it loaded
    let op = Op::SetRating { id: ids[2], rating: 3 };
    ca.apply(op.clone()).unwrap();
    a.append(&[op]).unwrap();
    let e = a.snapshot(&ca).expect_err("the whole rewrite is refused");
    assert!(e.to_string().contains("could not be read"), "{e}");
    drop(a);
    assert_eq!(stored(s.path(), PHOTOS, key).as_deref(), Some(DAMAGED), "the record is still there, as it was");
}

/// The exit-time backup with "optimise" on: the backup is made, the optimise that would delete the skipped record
/// is left out (the integrity test only sees photo records, so an unreadable album gets past it).
#[test]
fn the_scheduled_backup_keeps_a_record_the_load_could_not_read() {
    let s = library("unreadable-backup", 6);
    let key = damage(s.path(), COLLECTIONS);
    let st = CatalogSettings { backup: BackupSchedule::EveryExit, optimize: true, ..CatalogSettings::load(s.path()) };
    st.save(s.path()).unwrap();
    let (mut j, mut c, r) = open(s.path());
    assert_eq!(r.unreadable, 1);
    // something to back up
    let id = c.photos().next().unwrap().id;
    let op = Op::SetRating { id, rating: 5 };
    c.apply(op.clone()).unwrap();
    j.append(&[op]).unwrap();
    let made = j.backup_if_due(&c).unwrap();
    assert!(made.is_some(), "the backup is made");
    drop(j);
    assert_eq!(stored(s.path(), COLLECTIONS, key).as_deref(), Some(DAMAGED), "the record is still there, as it was");
}
