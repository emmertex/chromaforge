//! Read Originals, Reload, the preview-only recheck and relinking keep the SHA-1 their file probe
//! computed (adaptive-I/O spec, PR 0), instead of leaving it to the background back-fill to read every
//! file again: for the photos the back-fill would hash (`convert::takes_sha1`), only a well-formed
//! value, queued for the Immich checksum lookup once committed. Device-sync re-keying:
//! `device_sync::tests::read_originals_and_reload_key_photos_by_the_sha1_they_compute`; the
//! vignetting reference: `tests_vignetting_calibration_commands`.

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

/// A Reload that finds only the SHA-1 new adds no undo step: the redo history survives it, and the
/// next Undo is the user's own edit, not a step that would only drop a correct SHA-1.
#[test]
fn reload_records_the_sha1_alone_without_an_undo_step() {
    let mut s = session(vec![photo(1, "/x/a.jpg"), photo(2, "/x/bad.jpg"), photo(3, "/x/upper.jpg")]);
    s.selection = crate::Selection::single(PhotoId(1));
    s.execute("photo.rate", &json!({"rating": 3})).unwrap();
    s.execute("edit.undo", &json!({})).unwrap();
    let (undo, redo) = (s.undo.len(), s.redo.len());
    assert_eq!(redo, 1, "a step to redo");
    let r = reload(&mut s, &[1, 2, 3]);
    assert_eq!(r["reloaded"], json!([]), "a SHA-1 alone is not a reload: {r}");
    assert_eq!(sha1(&s, 1), Some(sha1_for("/x/a.jpg")));
    assert_eq!(sha1(&s, 2), None, "a malformed SHA-1 is skipped, not failing the batch");
    assert_eq!(sha1(&s, 3), Some(sha1_for("/x/upper.jpg")), "upper case is a SHA-1 (stored lower case)");
    assert_eq!(queued(&s), [1, 3], "queued for the Immich checksum lookup");
    assert_eq!((s.undo.len(), s.redo.len()), (undo, redo), "no undo step, and the redo history is kept");
    s.execute("edit.redo", &json!({})).unwrap();
    assert_eq!(s.catalog.photo(PhotoId(1)).unwrap().rating, 3, "the user's step is still there to redo");
    assert_eq!(sha1(&s, 1), Some(sha1_for("/x/a.jpg")));
}

/// A Reload that finds the file changed is one undo step for the file facts; the SHA-1 of the bytes
/// on disk stays when it is undone (undo doesn't change the file).
#[test]
fn undoing_a_reload_keeps_the_sha1_of_the_file_on_disk() {
    let mut stale = photo(1, "/x/a.jpg");
    stale.file_size = 99;
    let mut s = session(vec![stale]);
    let undo = s.undo.len();
    let r = reload(&mut s, &[1]);
    assert_eq!(r["reloaded"], json!([1]), "{r}");
    assert_eq!(s.undo.len(), undo + 1, "the file facts are one undo step");
    assert_eq!(sha1(&s, 1), Some(sha1_for("/x/a.jpg")));
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(s.catalog.photo(PhotoId(1)).unwrap().file_size, 99, "the file facts are undone");
    assert_eq!(sha1(&s, 1), Some(sha1_for("/x/a.jpg")), "the SHA-1 is not");
}

/// Two Reloads of one photo overlap (the app reads on a worker thread): only the one started last
/// records the SHA-1, whichever finishes first, so the SHA-1 of bytes an older read saw can't be
/// fixed before the newer read applies (it could not replace it then).
#[test]
fn overlapping_reloads_record_the_newest_reads_sha1() {
    use crate::cmd::reload as r;
    let mut s = session(vec![photo(1, "/x/a.jpg")]);
    let older = r::plan(&mut s, &json!({"ids": [1]})).unwrap();
    let newer = r::plan(&mut s, &json!({"ids": [1]})).unwrap();
    let (old_bytes, new_bytes) = (info_for("/x/old"), info_for("/x/a.jpg"));
    // the older read applies first: its file facts apply, its SHA-1 doesn't
    r::apply(&mut s, older.clone(), vec![Ok(old_bytes.clone())]).unwrap();
    assert_eq!(s.catalog.photo(PhotoId(1)).unwrap().content_hash.as_deref(), Some("hash-/x/old"));
    assert_eq!(sha1(&s, 1), None, "left to the newer reload");
    assert!(queued(&s).is_empty());
    r::apply(&mut s, newer, vec![Ok(new_bytes)]).unwrap();
    assert_eq!(s.catalog.photo(PhotoId(1)).unwrap().content_hash.as_deref(), Some("hash-/x/a.jpg"));
    assert_eq!(sha1(&s, 1), Some(sha1_for("/x/a.jpg")), "the SHA-1 of the bytes the photo now describes");
    assert_eq!(queued(&s), [1]);
    // the newer one first: the older is skipped whole
    let mut s = session(vec![photo(1, "/x/a.jpg")]);
    let older = r::plan(&mut s, &json!({"ids": [1]})).unwrap();
    let newer = r::plan(&mut s, &json!({"ids": [1]})).unwrap();
    r::apply(&mut s, newer, vec![Ok(info_for("/x/a.jpg"))]).unwrap();
    r::apply(&mut s, older, vec![Ok(old_bytes)]).unwrap();
    assert_eq!(sha1(&s, 1), Some(sha1_for("/x/a.jpg")));
    assert_eq!(s.catalog.photo(PhotoId(1)).unwrap().content_hash.as_deref(), Some("hash-/x/a.jpg"));
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

fn pending(s: &Session) -> Vec<u64> {
    crate::remote::sha1_pending(s, 100).iter().map(|(id, _)| id.0).collect()
}

#[test]
fn link_only_local_deleted_and_virtual_copies_get_no_sha1() {
    let mut link_only = photo(1, "/cache/preview.jpg");
    link_only.preview_only = Some(crate::remote::LINK_ONLY.into());
    let mut local = photo(2, "/x/local.jpg");
    local.local = true;
    let master = photo(3, "/x/master.jpg");
    // a virtual copy relinked to a file of its own: no family member shares its file
    let mut own_file = copy_of(4, &master);
    own_file.source = Source::File { path: "/x/own.jpg".into() };
    own_file.content_hash = Some("hash-/x/own.jpg".into());
    let mut deleted = photo(5, "/x/deleted.jpg");
    deleted.deleted = true;
    let mut s = session(vec![link_only, local, master, own_file, deleted]);
    // the back-fill hashes only the photo a reload may give a SHA-1 to
    assert_eq!(pending(&s), [3]);
    reload(&mut s, &[1, 2, 4, 5]);
    for id in [1, 2, 3, 4, 5] {
        assert_eq!(sha1(&s, id), None, "{id}");
    }
    assert!(queued(&s).is_empty());
    // and the same rule directly, for a deleted photo the planner might list
    let (mut ops, mut changed, mut hashed) = (Vec::new(), Vec::new(), Vec::new());
    crate::cmd::convert::reload_ops(
        &s.catalog,
        PhotoId(5),
        &info_for("/x/deleted.jpg"),
        crate::cmd::convert::Reach::Family,
        &mut ops,
        &mut changed,
        &mut hashed,
    );
    assert!(hashed.is_empty() && !ops.iter().any(|o| matches!(o, Op::SetSha1 { .. })));
    // a generated demo photo has no file to hash; the rule's own photo passes it
    use crate::cmd::convert::takes_sha1;
    assert!(!takes_sha1(&Photo::new(PhotoId(9), Source::Demo { scene: 0 }, "a.jpg", "JPEG", 30, 20, "2026-01-01T00:00:00")));
    assert!(takes_sha1(&photo(9, "/x/f.jpg")));
}

/// Reload leaves an Immich link-only photo alone: its file is the server's preview, so reading it
/// would take the flag off and let the next Reload (or the back-fill) record the preview's SHA-1
/// as the original's.
#[test]
fn reload_leaves_a_link_only_photo_link_only() {
    let mut link_only = photo(1, "/cache/preview.jpg");
    link_only.preview_only = Some(crate::remote::LINK_ONLY.into());
    let mut s = session(vec![link_only]);
    for _ in 0..2 {
        reload(&mut s, &[1]);
        let p = s.catalog.photo(PhotoId(1)).unwrap();
        assert_eq!(p.preview_only.as_deref(), Some(crate::remote::LINK_ONLY), "still link-only");
        assert_eq!(p.sha1, None);
        assert!(pending(&s).is_empty(), "nor does the back-fill hash it");
    }
    assert!(queued(&s).is_empty());
}

/// A photo whose original is still to be read is left to read-originals, which records its SHA-1
/// from the same read; the back-fill hashing it as well would read each such file twice while the
/// two run side by side. One device sync asked for first is hashed anyway.
#[test]
fn the_back_fill_leaves_unread_originals_to_read_originals() {
    let mut s = session(vec![never_read(1, "/x/a.jpg"), photo(2, "/x/b.jpg"), never_read(3, "/x/c.jpg")]);
    assert_eq!(pending(&s), [2], "only the photo already read");
    s.remote.sha1_first.insert(PhotoId(3));
    assert_eq!(pending(&s), [3, 2], "device sync's first, then the read one");
    s.remote.sha1_first.clear();
    s.execute("library.readOriginals", &json!({})).unwrap();
    assert_eq!((sha1(&s, 1), sha1(&s, 3)), (Some(sha1_for("/x/a.jpg")), Some(sha1_for("/x/c.jpg"))));
    assert_eq!(pending(&s), [2], "read now, and hashed by that read");
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
/// relinked virtual copy must not take the new file's SHA-1, nor any other of its facts (content
/// hash, size, dimensions): it stays a never-read photo that read-originals will read.
#[test]
fn relinking_a_virtual_copy_gives_no_one_else_the_new_files_facts() {
    let dir = lcc_testkit::TempDir::new("lcc-engine-relink-copy-sha1");
    let other = dir.join("other.jpg");
    std::fs::write(&other, b"x").unwrap();
    let master = never_read(1, "/gone/a.jpg");
    let copy = copy_of(2, &master);
    let untouched = |s: &Session| {
        let m = s.catalog.photo(PhotoId(1)).unwrap();
        assert_eq!((m.sha1.as_deref(), m.content_hash.as_deref(), m.file_size, m.width), (None, None, 0, 0), "the master is as it was");
        assert!(crate::cmd::read_originals::is_unread(m), "and still to be read");
    };
    // photo.relink
    let mut s = session(vec![master.clone(), copy.clone()]);
    s.execute("photo.relink", &json!({"id": 2, "path": other.to_string_lossy()})).unwrap();
    assert_eq!((sha1(&s, 1), sha1(&s, 2)), (None, None));
    assert_eq!(s.catalog.photo(PhotoId(2)).unwrap().width, 30, "the relinked copy takes its new file's facts");
    untouched(&s);
    assert!(queued(&s).is_empty());
    // Find Missing
    let mut s = session(vec![master.clone(), copy.clone()]);
    let found = json!({"found": [{"id": 2, "from": "/gone/a.jpg", "to": "/found/other.jpg", "by": "name"}], "missing": 0, "ambiguous": []});
    s.execute("library.findMissing", &found).unwrap();
    assert!(matches!(&s.catalog.photo(PhotoId(2)).unwrap().source, Source::File { path } if path == "/found/other.jpg"));
    assert_eq!((sha1(&s, 1), sha1(&s, 2)), (None, None));
    untouched(&s);
    assert!(queued(&s).is_empty());
    // relinking the master leaves its copy, still on the old file, as it was
    let mut s = session(vec![master, copy]);
    s.execute("photo.relink", &json!({"id": 1, "path": other.to_string_lossy()})).unwrap();
    assert_eq!(sha1(&s, 1), Some(sha1_for(&other.to_string_lossy())));
    let c = s.catalog.photo(PhotoId(2)).unwrap();
    assert_eq!((c.content_hash.as_deref(), c.width), (None, 0), "the copy keeps the old file's (lack of) facts");
    assert_eq!(queued(&s), [1]);
}

/// [`crate::cmd::convert::reload_ops`] directly: the ids given only a SHA-1 are reported in `hashed`,
/// never in `changed`; a photo whose file still matches the SHA-1 it has keeps it.
#[test]
fn reload_ops_reports_hashed_photos_apart_from_changed_ones() {
    use crate::cmd::convert::{Reach, reload_ops};
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
        reload_ops(&cat, PhotoId(id), &info_for(path), Reach::Family, &mut ops, &mut changed, &mut hashed);
    }
    assert_eq!(changed, [PhotoId(3)], "only the photo whose file facts changed");
    assert_eq!(hashed, [PhotoId(1), PhotoId(3)], "a photo whose file still matches its SHA-1 keeps it");
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
        reload_ops(&cat, PhotoId(id), &info_for("/x/m.jpg"), Reach::Family, &mut ops, &mut changed, &mut hashed);
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

/// What [`lcc_catalog::is_sha1_hex`] lets the catalog store parses back to the same digest through
/// `lcc_hash`, the parser device sync keys photos with (looser: it trims, and takes base64 too), so a
/// stored SHA-1 never silently falls back to the content key.
#[test]
fn every_stored_sha1_parses_back_where_device_sync_reads_it() {
    let ok = "0123456789abcdefABCDEF0123456789abcdef01";
    for s in [ok, &ok.to_ascii_lowercase(), &ok.to_ascii_uppercase(), &"f".repeat(40), &"0".repeat(40)] {
        assert!(lcc_catalog::is_sha1_hex(s), "{s}");
        let d = lcc_hash::Sha1Digest::from_hex(s).unwrap_or_else(|| panic!("lcc_hash refuses {s}"));
        assert_eq!(d.to_hex(), s.to_ascii_lowercase());
        assert_eq!(lcc_hash::Sha1Digest::parse(s).map(|d| d.to_hex()), Some(s.to_ascii_lowercase()));
    }
    // the other way round lcc_hash is looser, which is the safe direction
    for loose in [format!(" {ok}"), format!("{ok}\n"), format!("+{}", &ok[1..])] {
        assert!(!lcc_catalog::is_sha1_hex(&loose), "{loose:?}");
    }
}

/// The background back-fill's result applies by the same rule its list was made by
/// ([`crate::cmd::convert::takes_sha1`]): a photo hashed meanwhile by a Reload, or deleted
/// meanwhile, is left as it is; only a recorded SHA-1 is counted and queued for the Immich lookup.
#[test]
fn a_back_fill_result_obeys_the_rule_and_queues_only_what_it_recorded() {
    let mut deleted = photo(2, "/x/b.jpg");
    deleted.deleted = true;
    let mut s = session(vec![photo(1, "/x/a.jpg"), deleted, photo(3, "/x/c.jpg"), photo(4, "/x/d.jpg")]);
    reload(&mut s, &[1]);
    s.remote.link_queue.clear();
    let result = |id: u64, path: &str, h: &str| (PhotoId(id), path.to_string(), Some(h.to_string()));
    let msg = crate::remote::Msg::Sha1(vec![
        result(1, "/x/a.jpg", &"1".repeat(40)),
        result(2, "/x/b.jpg", &"2".repeat(40)),
        result(3, "/x/c.jpg", &"3".repeat(40)),
        result(4, "/x/d.jpg", "not-a-sha1"),
    ]);
    s.remote.sender().send(msg).unwrap();
    s.remote.sha1_busy = true;
    // (no lookup is started, so the queue can be looked at)
    s.remote.link_checking = true;
    s.execute("remote.pump", &json!({"sync": false, "sha1": false})).unwrap();
    assert_eq!(sha1(&s, 1), Some(sha1_for("/x/a.jpg")), "the Reload's SHA-1 stays");
    assert_eq!(sha1(&s, 2), None, "a deleted photo takes none");
    assert_eq!(sha1(&s, 3), Some("3".repeat(40)));
    assert_eq!(sha1(&s, 4), None, "refused by the catalog");
    assert_eq!(queued(&s), [3], "only what was recorded is queued");
    assert_eq!(s.remote.sha1_done, 1);
    assert!(s.remote.sha1_failed.contains(&PhotoId(4)), "a refused one isn't listed again this session");
}

/// Read Originals fills a never-read photo's empty camera serial and lens range too (a Lightroom
/// catalog import has neither: the serial keys the body's sensor maps); one already known is kept.
#[test]
fn read_originals_fills_the_camera_serial_and_lens_range() {
    let mut known = never_read(2, "/x/b.jpg");
    known.meta.camera_serial = "KEEP".into();
    known.meta.lens_range = Some((24.0, 70.0));
    let mut s = session(vec![never_read(1, "/x/a.jpg"), known]);
    s.media.file_probe = Some(Arc::new(|path: &str| {
        let mut i = info_for(path);
        i.meta.camera_serial = "1234567".into();
        i.meta.lens_range = Some((14.0, 30.0));
        Ok(i)
    }));
    s.execute("library.readOriginals", &json!({})).unwrap();
    let m = |id| s.catalog.photo(PhotoId(id)).unwrap().meta.clone();
    assert_eq!((m(1).camera_serial.as_str(), m(1).lens_range), ("1234567", Some((14.0, 30.0))));
    assert_eq!((m(2).camera_serial.as_str(), m(2).lens_range), ("KEEP", Some((24.0, 70.0))), "nothing set is overwritten");
}
