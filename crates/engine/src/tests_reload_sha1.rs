//! Read Originals, Reload, the preview-only recheck and relinking keep the SHA-1 their file probe
//! computed (adaptive-I/O spec, PR 0), instead of leaving it to the background back-fill to read every
//! file again: for the photos the back-fill would hash (`convert::takes_sha1`), only a well-formed
//! value, queued for the Immich checksum lookup once committed. Device-sync re-keying:
//! `device_sync::tests::read_originals_and_reload_key_photos_by_the_sha1_they_compute`.

use std::hash::{Hash, Hasher};
use std::sync::Arc;

use lcc_catalog::{Op, Photo, PhotoId, Source};
use serde_json::json;

use crate::Session;
use crate::media::{FileProbe, ProbeInfo};

/// The SHA-1 the fake probe reports for `path`: 40 hex digits, one per path.
fn sha1_for(path: &str) -> String {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut h);
    format!("{:040x}", h.finish())
}

/// What every fake file is: 30×20, 10 bytes, a content hash of its own path. A path containing `bad`
/// reports a malformed SHA-1, one containing `upper` the right one in capitals.
fn info_for(path: &str) -> ProbeInfo {
    let sha1 = if path.contains("bad") {
        "not-a-sha1".to_string()
    } else if path.contains("upper") {
        sha1_for(path).to_ascii_uppercase()
    } else {
        sha1_for(path)
    };
    ProbeInfo {
        width: 30,
        height: 20,
        file_size: 10,
        format: "JPEG".into(),
        content_hash: Some(format!("hash-{path}")),
        sha1: Some(sha1),
        ..Default::default()
    }
}

fn probe() -> FileProbe {
    Arc::new(|path: &str| Ok(info_for(path)))
}

/// A photo whose record already matches its file ([`info_for`]) in everything but the SHA-1.
fn photo(id: u64, path: &str) -> Photo {
    let mut p = Photo::new(PhotoId(id), Source::File { path: path.into() }, "a.jpg", "JPEG", 30, 20, "2026-01-01T00:00:00");
    p.file_size = 10;
    p.content_hash = Some(format!("hash-{path}"));
    p
}

/// A photo never read from its file (a Lightroom catalog import's record).
fn never_read(id: u64, path: &str) -> Photo {
    Photo::new(PhotoId(id), Source::File { path: path.into() }, "a.jpg", "JPEG", 0, 0, "2026-01-01T00:00:00")
}

fn copy_of(id: u64, master: &Photo) -> Photo {
    let mut c = master.clone();
    c.id = PhotoId(id);
    c.copy_of = Some(master.id);
    c
}

fn session(photos: Vec<Photo>) -> Session {
    let mut s = Session::new();
    for p in photos {
        s.catalog.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
    }
    s.media.file_probe = Some(probe());
    s
}

fn sha1(s: &Session, id: u64) -> Option<String> {
    s.catalog.photo(PhotoId(id)).unwrap().sha1.clone()
}

/// `photo.reload {ids}` (the command is enabled by a selection, as in the app).
fn reload(s: &mut Session, ids: &[u64]) -> serde_json::Value {
    s.selection = crate::Selection::single(PhotoId(ids[0]));
    s.execute("photo.reload", &json!({ "ids": ids })).unwrap()
}

fn queued(s: &Session) -> Vec<u64> {
    s.remote.link_queue.iter().map(|i| i.0).collect()
}

#[test]
fn reload_records_the_sha1_alone_without_reporting_the_photo_reloaded() {
    let mut s = session(vec![photo(1, "/x/a.jpg"), photo(2, "/x/bad.jpg"), photo(3, "/x/upper.jpg")]);
    let undo = s.undo.len();
    let r = reload(&mut s, &[1, 2, 3]);
    assert_eq!(r["reloaded"], json!([]), "a SHA-1 alone is not a reload: {r}");
    assert_eq!(sha1(&s, 1), Some(sha1_for("/x/a.jpg")));
    assert_eq!(sha1(&s, 2), None, "a malformed SHA-1 is skipped, not failing the batch");
    assert_eq!(sha1(&s, 3), Some(sha1_for("/x/upper.jpg")), "upper case is a SHA-1 (stored lower case)");
    assert_eq!(queued(&s), [1, 3], "queued for the Immich checksum lookup");
    assert_eq!(s.undo.len(), undo + 1, "part of Reload's undo step");
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!((sha1(&s, 1), sha1(&s, 3)), (None, None), "undoing the Reload clears it");
}

#[test]
fn read_originals_records_the_sha1_with_the_file_facts() {
    let mut s = session(vec![never_read(1, "/x/a.jpg"), never_read(2, "/x/bad.jpg")]);
    let r = s.execute("library.readOriginals", &json!({})).unwrap();
    assert_eq!(r["updated"], json!([1, 2]), "{r}");
    assert_eq!(sha1(&s, 1), Some(sha1_for("/x/a.jpg")));
    assert_eq!(sha1(&s, 2), None, "a malformed SHA-1 is skipped");
    assert_eq!(s.catalog.photo(PhotoId(2)).unwrap().content_hash.as_deref(), Some("hash-/x/bad.jpg"), "the rest of its batch applies");
    assert_eq!(queued(&s), [1]);
    assert!(s.undo.is_empty(), "still not an undo step");
}

#[test]
fn the_preview_only_recheck_records_the_sha1() {
    let flagged = |id, path| {
        let mut p = photo(id, path);
        p.preview_only = Some("embedded preview".into());
        p.preview_checked = lcc_catalog::preview_stamp(&p.preview_only);
        p
    };
    let mut s = session(vec![flagged(1, "/x/a.cr3"), flagged(2, "/x/bad.cr3")]);
    s.media.file_probe = Some(Arc::new(|path: &str| Ok(ProbeInfo { preview_only: Some("embedded preview".into()), ..info_for(path) })));
    let r = s.execute("library.recheckPreviewOnly", &json!({"all": true})).unwrap();
    assert_eq!(r["checked"], 2, "{r}");
    assert_eq!(r["updated"], json!([]), "a SHA-1 alone is not an update: {r}");
    assert_eq!(sha1(&s, 1), Some(sha1_for("/x/a.cr3")));
    assert_eq!(sha1(&s, 2), None);
    assert_eq!(queued(&s), [1]);
}

#[test]
fn photo_relink_records_the_new_files_sha1() {
    let dir = lcc_testkit::TempDir::new("lcc-engine-relink-sha1");
    let (good, bad) = (dir.join("new.jpg"), dir.join("bad.jpg"));
    for f in [&good, &bad] {
        std::fs::write(f, b"x").unwrap();
    }
    let mut s = session(vec![never_read(1, "/gone/a.jpg"), never_read(2, "/gone/b.jpg")]);
    s.execute("photo.relink", &json!({"id": 1, "path": good.to_string_lossy()})).unwrap();
    s.execute("photo.relink", &json!({"id": 2, "path": bad.to_string_lossy()})).unwrap();
    assert_eq!(sha1(&s, 1), Some(sha1_for(&good.to_string_lossy())));
    assert_eq!(sha1(&s, 2), None, "a malformed SHA-1 is skipped and the relink still applies");
    assert!(matches!(&s.catalog.photo(PhotoId(2)).unwrap().source, Source::File { path } if *path == bad.to_string_lossy()));
    assert_eq!(queued(&s), [1]);
    s.execute("edit.undo", &json!({})).unwrap();
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(sha1(&s, 1), None, "undoing the relink clears it");
}

#[test]
fn find_missing_records_the_sha1_of_never_read_photos_it_relinks() {
    let mut s = session(vec![never_read(1, "/gone/a.jpg"), never_read(2, "/gone/b.jpg")]);
    let found = json!({"found": [
        {"id": 1, "from": "/gone/a.jpg", "to": "/found/a.jpg", "by": "name"},
        {"id": 2, "from": "/gone/b.jpg", "to": "/found/bad.jpg", "by": "name"},
    ], "missing": 0, "ambiguous": []});
    s.execute("library.findMissing", &found).unwrap();
    assert_eq!(sha1(&s, 1), Some(sha1_for("/found/a.jpg")));
    assert_eq!(sha1(&s, 2), None);
    assert_eq!(queued(&s), [1]);
}

#[test]
fn link_only_local_and_virtual_copies_get_no_sha1() {
    let mut link_only = photo(1, "/cache/preview.jpg");
    link_only.preview_only = Some(crate::remote::LINK_ONLY.into());
    let mut local = photo(2, "/x/local.jpg");
    local.local = true;
    let master = photo(3, "/x/master.jpg");
    // a virtual copy relinked to a file of its own: no family member shares its file
    let mut own_file = copy_of(4, &master);
    own_file.source = Source::File { path: "/x/own.jpg".into() };
    own_file.content_hash = Some("hash-/x/own.jpg".into());
    let mut s = session(vec![link_only, local, master, own_file]);
    // the back-fill would hash exactly the photo a reload may give a SHA-1 to
    let pending: Vec<u64> = crate::remote::sha1_pending(&s, 100).iter().map(|(id, _)| id.0).collect();
    assert_eq!(pending, [3]);
    reload(&mut s, &[1, 2, 4]);
    for id in [1, 2, 3, 4] {
        assert_eq!(sha1(&s, id), None, "{id}");
    }
    assert!(queued(&s).is_empty());
}

#[test]
fn reloading_a_virtual_copy_gives_its_master_the_sha1() {
    let master = photo(1, "/x/a.jpg");
    let copy = copy_of(2, &master);
    let mut s = session(vec![master, copy]);
    let r = reload(&mut s, &[2]);
    assert_eq!(r["reloaded"], json!([]), "{r}");
    assert_eq!(sha1(&s, 1), Some(sha1_for("/x/a.jpg")), "the master takes its file's SHA-1");
    assert_eq!(sha1(&s, 2), None, "the copy takes none");
    assert_eq!(queued(&s), [1]);
    // master and copy in one reload: the master gets it once
    let mut s = session(vec![photo(1, "/x/a.jpg"), copy_of(2, &photo(1, "/x/a.jpg"))]);
    let r = reload(&mut s, &[1, 2]);
    assert_eq!(r["reloaded"], json!([]), "{r}");
    assert_eq!(queued(&s), [1]);
}

/// The relink is probed before it applies, so the family still names the old file: the master of a
/// relinked virtual copy must not take the new file's SHA-1.
#[test]
fn relinking_a_virtual_copy_gives_no_one_the_new_files_sha1() {
    let dir = lcc_testkit::TempDir::new("lcc-engine-relink-copy-sha1");
    let other = dir.join("other.jpg");
    std::fs::write(&other, b"x").unwrap();
    let master = never_read(1, "/gone/a.jpg");
    let copy = copy_of(2, &master);
    // photo.relink
    let mut s = session(vec![master.clone(), copy.clone()]);
    s.execute("photo.relink", &json!({"id": 2, "path": other.to_string_lossy()})).unwrap();
    assert_eq!((sha1(&s, 1), sha1(&s, 2)), (None, None));
    assert!(queued(&s).is_empty());
    // Find Missing
    let mut s = session(vec![master, copy]);
    let found = json!({"found": [{"id": 2, "from": "/gone/a.jpg", "to": "/found/other.jpg", "by": "name"}], "missing": 0, "ambiguous": []});
    s.execute("library.findMissing", &found).unwrap();
    assert!(matches!(&s.catalog.photo(PhotoId(2)).unwrap().source, Source::File { path } if path == "/found/other.jpg"));
    assert_eq!((sha1(&s, 1), sha1(&s, 2)), (None, None));
    assert!(queued(&s).is_empty());
}

/// [`crate::cmd::convert::reload_ops`] directly: the ids given only a SHA-1 are reported in `hashed`,
/// never in `changed`; a photo that already has one keeps it.
#[test]
fn reload_ops_reports_hashed_photos_apart_from_changed_ones() {
    use crate::cmd::convert::{Sha1To, reload_ops};
    let mut cat = lcc_catalog::Catalog::new();
    let mut hashed_already = photo(2, "/x/b.jpg");
    hashed_already.sha1 = Some("a".repeat(40));
    let mut stale = photo(3, "/x/c.jpg");
    stale.file_size = 99;
    for p in [photo(1, "/x/a.jpg"), hashed_already, stale] {
        cat.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
    }
    let (mut ops, mut changed, mut hashed) = (Vec::new(), Vec::new(), Vec::new());
    for (id, path) in [(1, "/x/a.jpg"), (2, "/x/b.jpg"), (3, "/x/c.jpg")] {
        reload_ops(&cat, PhotoId(id), &info_for(path), Sha1To::Family, &mut ops, &mut changed, &mut hashed);
    }
    assert_eq!(changed, [PhotoId(3)], "only the photo whose file facts changed");
    assert_eq!(hashed, [PhotoId(1), PhotoId(3)], "an existing SHA-1 is never replaced");
    cat.apply(Op::Batch { ops }).unwrap();
    assert_eq!(cat.photo(PhotoId(2)).unwrap().sha1, Some("a".repeat(40)));
    // master and copy of one file in the same run (the commands' planners group them today): the
    // master is given its SHA-1 once
    let master = photo(10, "/x/m.jpg");
    for p in [master.clone(), copy_of(11, &master)] {
        cat.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
    }
    let (mut ops, mut changed, mut hashed) = (Vec::new(), Vec::new(), Vec::new());
    for id in [10, 11] {
        reload_ops(&cat, PhotoId(id), &info_for("/x/m.jpg"), Sha1To::Family, &mut ops, &mut changed, &mut hashed);
    }
    assert_eq!(hashed, [PhotoId(10)]);
    assert_eq!(ops.iter().filter(|o| matches!(o, Op::SetSha1 { .. })).count(), 1);
}

#[test]
fn the_sha1_rule_is_exactly_forty_hex_digits() {
    let ok = "0123456789abcdefABCDEF0123456789abcdef01";
    assert!(lcc_catalog::is_sha1_hex(ok));
    for bad in
        ["", &ok[..39], &format!("{ok}0"), &format!(" {}", &ok[1..]), &format!("+{}", &ok[1..]), &format!("g{}", &ok[1..]), "é".repeat(20).as_str()]
    {
        assert!(!lcc_catalog::is_sha1_hex(bad), "{bad:?}");
        let mut cat = lcc_catalog::Catalog::new();
        cat.apply(Op::AddPhoto { photo: Box::new(photo(1, "/x/a.jpg")) }).unwrap();
        assert!(cat.apply(Op::SetSha1 { id: PhotoId(1), sha1: Some(bad.to_string()) }).is_err(), "the catalog refuses {bad:?} too");
    }
}
