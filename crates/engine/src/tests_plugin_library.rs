//! What plug-ins learn of the Library and add to it: the `source` a command gets, and
//! `library.add` (a tool's file added from the folder of its photo, stacked on it or in an album
//! that holds it).

use std::path::{Path, PathBuf};

use lcc_catalog::{AlbumId, PhotoId};
use lcc_plugins::HostApi;
use serde_json::{Value, json};

use crate::Session;
use crate::cmd::plugins::SessionHost;

fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lcc-plugin-library-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn write_png(path: &Path, seed: u8, w: usize, h: usize) {
    let data: Vec<[u8; 4]> = (0..w * h).map(|i| [(i % w * 6) as u8, (i / w * 9) as u8, seed, 255]).collect();
    let img = lcc_raster::Rgba8 { width: w, height: h, data };
    let bytes = lcc_codecs::encode_png(&lcc_codecs::EncodeImage::rgba8(&img), &lcc_codecs::EncodeMeta::default()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

fn import(s: &mut Session, p: &Path) -> PhotoId {
    let r = s.execute("library.import", &json!({"paths": [p.to_string_lossy()]})).unwrap();
    PhotoId(r["imported"][0].as_u64().unwrap())
}

fn album(s: &mut Session, name: &str, photos: &[PhotoId]) -> AlbumId {
    let a = s.execute("album.create", &json!({"name": name})).unwrap()["id"].as_u64().unwrap();
    if !photos.is_empty() {
        s.execute("album.addPhotos", &json!({"id": a, "ids": photos.iter().map(|p| p.0).collect::<Vec<_>>()})).unwrap();
    }
    AlbumId(a)
}

/// A library with one photo, `<dir>/beach.png`, in the album "Trip".
struct Fixture {
    dir: PathBuf,
    s: Session,
    photo: PhotoId,
    trip: AlbumId,
}

impl Fixture {
    fn new(tag: &str) -> Fixture {
        let dir = temp_dir(tag);
        write_png(&dir.join("beach.png"), 9, 32, 20);
        let mut s = Session::new().with_fs();
        let photo = import(&mut s, &dir.join("beach.png"));
        let trip = album(&mut s, "Trip", &[photo]);
        Fixture { dir, s, photo, trip }
    }

    /// A file the tool made: `<dir>/<name>`, folders made as needed.
    fn made(&self, name: &str, seed: u8) -> PathBuf {
        let p = self.dir.join(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        write_png(&p, seed, 32, 20);
        p
    }

    fn add(&mut self, p: Value) -> Result<Value, String> {
        SessionHost(&mut self.s).library_add(&p)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn photo_of(r: &Value) -> PhotoId {
    PhotoId(r["photo"].as_str().and_then(|p| p.parse().ok()).unwrap_or_else(|| panic!("a photo id: {r}")))
}

/// A module whose command answers with the request it got (`{"ok": <request>}`, built around the
/// request where the host put it).
fn echo_module(manifest: &Value) -> Vec<u8> {
    let manifest = manifest.to_string();
    let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    let src = format!(
        r#"(module (memory (export "memory") 2)
  (data (i32.const 16) "{m}") (data (i32.const 4096) "{{\"ok\":")
  (func (export "lcc_abi_version") (result i32) (i32.const 1))
  (func (export "lcc_manifest") (result i64) (i64.or (i64.shl (i64.const {ml}) (i64.const 32)) (i64.const 16)))
  (func (export "lcc_alloc") (param $n i32) (result i32) (local $old i32)
    (local.set $old (memory.grow (i32.shr_u (i32.add (local.get $n) (i32.const 65535)) (i32.const 16))))
    (if (result i32) (i32.eq (local.get $old) (i32.const -1)) (then (i32.const 0)) (else (i32.mul (local.get $old) (i32.const 65536)))))
  (func (export "lcc_handle") (param $ptr i32) (param $len i32) (result i64) (local $i i32)
    (block $done (loop $copy
      (br_if $done (i32.ge_u (local.get $i) (i32.const 6)))
      (i32.store8 (i32.add (i32.sub (local.get $ptr) (i32.const 6)) (local.get $i)) (i32.load8_u (i32.add (i32.const 4096) (local.get $i))))
      (local.set $i (i32.add (local.get $i) (i32.const 1)))
      (br $copy)))
    (i32.store8 (i32.add (local.get $ptr) (local.get $len)) (i32.const 125))
    (i64.or (i64.shl (i64.extend_i32_u (i32.add (local.get $len) (i32.const 7))) (i64.const 32))
      (i64.extend_i32_u (i32.sub (local.get $ptr) (i32.const 6))))))"#,
        m = esc(&manifest),
        ml = manifest.len()
    );
    wat::parse_str(&src).unwrap()
}

/// A module whose every hook answers `{"ok": {"message": "<the request, as JSON text>"}}`: what a
/// `command.prepare` step got comes back through `plugin.prepare`.
fn quoting_echo_module(manifest: &Value) -> Vec<u8> {
    let manifest = manifest.to_string();
    let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    // the request is copied to 65536 after the 18-byte prefix, `"` and `\` escaped, and `"}}` closes it
    let src = format!(
        r#"(module (memory (export "memory") 2)
  (data (i32.const 16) "{m}") (data (i32.const 4096) "{{\"ok\":{{\"message\":\"")
  (func (export "lcc_abi_version") (result i32) (i32.const 1))
  (func (export "lcc_manifest") (result i64) (i64.or (i64.shl (i64.const {ml}) (i64.const 32)) (i64.const 16)))
  (func (export "lcc_alloc") (param $n i32) (result i32) (local $old i32)
    (local.set $old (memory.grow (i32.shr_u (i32.add (local.get $n) (i32.const 65535)) (i32.const 16))))
    (if (result i32) (i32.eq (local.get $old) (i32.const -1)) (then (i32.const 0)) (else (i32.mul (local.get $old) (i32.const 65536)))))
  (func (export "lcc_handle") (param $ptr i32) (param $len i32) (result i64) (local $i i32) (local $o i32) (local $c i32)
    (memory.copy (i32.const 65536) (i32.const 4096) (i32.const 18))
    (local.set $o (i32.const 65554))
    (block $done (loop $copy
      (br_if $done (i32.ge_u (local.get $i) (local.get $len)))
      (local.set $c (i32.load8_u (i32.add (local.get $ptr) (local.get $i))))
      (if (i32.or (i32.eq (local.get $c) (i32.const 34)) (i32.eq (local.get $c) (i32.const 92)))
        (then (i32.store8 (local.get $o) (i32.const 92)) (local.set $o (i32.add (local.get $o) (i32.const 1)))))
      (i32.store8 (local.get $o) (local.get $c))
      (local.set $o (i32.add (local.get $o) (i32.const 1)))
      (local.set $i (i32.add (local.get $i) (i32.const 1)))
      (br $copy)))
    (i32.store8 (local.get $o) (i32.const 34))
    (i32.store8 (i32.add (local.get $o) (i32.const 1)) (i32.const 125))
    (i32.store8 (i32.add (local.get $o) (i32.const 2)) (i32.const 125))
    (i64.or (i64.shl (i64.extend_i32_u (i32.sub (i32.add (local.get $o) (i32.const 3)) (i32.const 65536))) (i64.const 32))
      (i64.const 65536))))"#,
        m = esc(&manifest),
        ml = manifest.len()
    );
    wat::parse_str(&src).unwrap()
}

/// A command learns the Library source it was started from: the album (a regular one, the Quick
/// Collection, a smart one), the folder, All Photographs or another view, whatever search is on
/// top. `plugin.run` from an agent gets the session's source too, and a `source` it passes
/// replaces it.
#[test]
fn plugin_commands_learn_the_library_source_they_ran_from() {
    let mut f = Fixture::new("source");
    let s = &mut f.s;
    let smart = s.execute("album.createSmart", &json!({"name": "Rated", "rules": {"rating": 1}})).unwrap()["id"].as_u64().unwrap();
    s.execute("album.toggleTarget", &json!({"ids": [f.photo.0]})).unwrap();
    let quick = s.catalog.quick_collection().unwrap();
    let source = crate::cmd::plugin_library::source;

    s.execute("library.source", &json!({"kind": "all"})).unwrap();
    assert_eq!(source(s), json!({"kind": "allPhotos"}));
    s.execute("library.source", &json!({"kind": "album", "id": f.trip.0})).unwrap();
    let trip = json!({"kind": "album", "id": f.trip.0.to_string(), "name": "Trip"});
    assert_eq!(source(s), trip);
    s.execute("library.filter", &json!({"text": "beach"})).unwrap();
    assert_eq!(source(s), trip, "a search on top of the album keeps it");
    s.execute("library.filter", &json!({"text": ""})).unwrap();
    s.execute("library.source", &json!({"kind": "quickCollection"})).unwrap();
    assert_eq!(source(s), json!({"kind": "album", "id": quick.0.to_string(), "name": "Quick Collection", "quick": true}));
    s.execute("library.source", &json!({"kind": "album", "id": smart})).unwrap();
    assert_eq!(source(s), json!({"kind": "smartAlbum", "id": smart.to_string(), "name": "Rated"}));
    let folder = f.dir.to_string_lossy().to_string();
    s.execute("library.source", &json!({"kind": "libraryFolder", "path": folder})).unwrap();
    assert_eq!(source(s), json!({"kind": "folder", "path": folder}));
    s.execute("library.browse", &json!({"path": folder})).unwrap();
    assert_eq!(source(s), json!({"kind": "folder", "path": folder}), "a folder browsed on disk");
    s.execute("library.source", &json!({"kind": "picks"})).unwrap();
    assert_eq!(source(s), json!({"kind": "other", "name": "picks"}));
    let set = s.execute("album.create", &json!({"name": "Trips", "folder": true})).unwrap()["id"].as_u64().unwrap();
    s.source = crate::LibrarySource::Album(AlbumId(set));
    assert_eq!(source(s), json!({"kind": "other", "name": "albumFolder"}), "an album folder holds no photos to file into");

    // through plugin.run, as an agent or the menu runs it
    let _turn = crate::cmd::plugins::tests::turn();
    let manifest = json!({"id": "test.source", "name": "Source", "version": "1.0.0", "requires": ["libraryAdd"],
        "commands": [{"id": "go", "label": "Go"}]});
    let path = f.dir.join("source.wasm");
    std::fs::write(&path, echo_module(&manifest)).unwrap();
    s.execute("plugin.install", &json!({"path": path})).unwrap();
    s.execute("library.source", &json!({"kind": "album", "id": f.trip.0})).unwrap();
    let r = s.execute("plugin.run", &json!({"plugin": "test.source", "command": "go"})).unwrap();
    assert_eq!((r["hook"].as_str(), &r["source"]), (Some("command"), &trip), "the command gets the source shown: {r}");
    let given = json!({"kind": "other", "name": "search"});
    let r = s.execute("plugin.run", &json!({"plugin": "test.source", "command": "go", "source": given})).unwrap();
    assert_eq!(r["source"], given, "a source passed replaces it");
    for bad in [json!("album"), json!({"kind": "planet"}), json!({"kind": "album", "id": 7}), json!({"kind": "album", "extra": "x"})] {
        assert!(s.execute("plugin.run", &json!({"plugin": "test.source", "command": "go", "source": bad})).is_err(), "{bad} refused");
    }
    s.execute("plugin.uninstall", &json!({"id": "test.source"})).unwrap();

    // and its prepare step, through plugin.prepare
    let manifest = json!({"id": "test.prepsource", "name": "Prepare", "version": "1.0.0", "requires": ["libraryAdd"],
        "commands": [{"id": "go", "label": "Go", "prepare": true}]});
    let path = f.dir.join("prepsource.wasm");
    std::fs::write(&path, quoting_echo_module(&manifest)).unwrap();
    s.execute("plugin.install", &json!({"path": path})).unwrap();
    let prepared = |s: &mut Session, p: Value| {
        let r = s.execute("plugin.prepare", &p).unwrap();
        serde_json::from_str::<Value>(r["message"].as_str().unwrap_or_else(|| panic!("a message: {r}"))).unwrap()
    };
    let req = prepared(s, json!({"plugin": "test.prepsource", "command": "go"}));
    assert_eq!((req["hook"].as_str(), &req["source"]), (Some("command.prepare"), &trip), "the prepare step gets the source shown: {req}");
    let req = prepared(s, json!({"plugin": "test.prepsource", "command": "go", "source": given}));
    assert_eq!(req["source"], given, "a source passed to plugin.prepare replaces it");
    s.execute("plugin.uninstall", &json!({"id": "test.prepsource"})).unwrap();
}

/// A tool's file from the photo's folder (or below it) is added, stacked on its photo and put in
/// an album that holds the photo; it doesn't take the photo's develop settings, the user's
/// selection stays, and it is one undo step.
#[test]
fn library_add_stacks_a_tool_file_on_its_photo_and_files_it_in_the_album() {
    let mut f = Fixture::new("add");
    f.s.execute("develop.set", &json!({"ids": [f.photo.0], "control": "light.exposure", "value": 1.0})).unwrap();
    let other = import(&mut f.s, &{
        let p = f.dir.join("other.png");
        write_png(&p, 40, 8, 8);
        p
    });
    f.s.execute("library.select", &json!({"ids": [other.0]})).unwrap();
    let made = f.made("DxO/beach-out.png", 30);
    let undo = f.s.undo.len();

    let r = f.add(json!({"path": made, "from": f.photo.0.to_string(), "stack": true, "album": f.trip.0.to_string()})).unwrap();
    assert_eq!((r["added"].as_bool(), r["reloaded"].as_bool()), (Some(true), Some(false)), "{r}");
    let new = photo_of(&r);
    assert_ne!(new, f.photo);
    let st = f.s.catalog.stack_of(new).expect("stacked on its photo");
    assert!(st.photos.contains(&f.photo), "in the photo's stack");
    assert!(f.s.catalog.album(f.trip).unwrap().photos.contains(&new), "in the album the command came from");
    let exposure = |s: &mut Session, id: PhotoId| s.execute("photo.inspect", &json!({"id": id.0})).unwrap()["develop"]["light"]["exposure"].clone();
    assert_ne!(exposure(&mut f.s, new), exposure(&mut f.s, f.photo), "a new rendering base: the photo's settings aren't copied");
    assert_eq!(f.s.active(), Some(other), "the user's selection stays");
    assert_eq!(f.s.undo.len(), undo + 1, "one undo step");
    f.s.execute("edit.undo", &json!({})).unwrap();
    assert!(f.s.catalog.photo(new).is_none_or(|p| !p.in_library()), "undo takes it out again");

    // without `stack` nor `album`: only added
    let plain = f.made("beach-plain.png", 31);
    let r = f.add(json!({"path": plain, "from": f.photo.0})).unwrap();
    let new = photo_of(&r);
    assert!(f.s.catalog.stack_of(new).is_none(), "not stacked");
    assert!(!f.s.catalog.album(f.trip).unwrap().photos.contains(&new), "not in the album");
}

/// Everything is checked first: a file outside the photo's folder, a missing or unreadable file,
/// the photo's own original, a photo that left the library, and an album that can't take it
/// (smart, not holding the photo, gone) each give an error and add nothing.
#[test]
fn library_add_refuses_and_adds_nothing() {
    let mut f = Fixture::new("refuse");
    let elsewhere = temp_dir("refuse-elsewhere");
    write_png(&elsewhere.join("x.png"), 50, 8, 8);
    let ok = f.made("ok.png", 51);
    std::fs::write(f.dir.join("notes.png"), b"not an image").unwrap();
    let smart = f.s.execute("album.createSmart", &json!({"name": "Rated", "rules": {"rating": 1}})).unwrap()["id"].as_u64().unwrap();
    let empty = album(&mut f.s, "Empty", &[]);
    let set = f.s.execute("album.create", &json!({"name": "Trips", "folder": true})).unwrap()["id"].as_u64().unwrap();
    std::fs::create_dir_all(f.dir.join("sub")).unwrap();
    let outside = format!("sub/../../{}/x.png", elsewhere.file_name().unwrap().to_string_lossy());
    let gone = album(&mut f.s, "Gone", &[f.photo]);
    f.s.execute("album.delete", &json!({"id": gone.0})).unwrap();
    let from = f.photo.0.to_string();
    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut cases = vec![
        (json!({"path": elsewhere.join("x.png"), "from": from}), "not in the folder"),
        (json!({"path": f.dir.join(&outside), "from": from}), "not in the folder"),
        (json!({"path": "ok.png", "from": from}), "absolute"),
        (json!({"path": f.dir.join("missing.png"), "from": from}), "can't be found"),
        (json!({"path": f.dir.join("notes.png"), "from": from}), "could not be added"),
        (json!({"path": f.dir.join("beach.png"), "from": from}), "own original"),
        (json!({"path": ok, "from": "999999"}), "not in the catalog"),
        (json!({"path": ok, "from": "beach"}), "not an id"),
        (json!({"path": ok}), "from"),
        (json!({"from": from}), "path"),
        (json!({"path": ok, "from": from, "stack": "yes"}), "stack"),
        (json!({"path": ok, "from": from, "album": smart}), "can't hold photos"),
        (json!({"path": ok, "from": from, "album": set}), "can't hold photos"),
        (json!({"path": ok, "from": from, "album": empty.0}), "doesn't hold"),
        (json!({"path": ok, "from": from, "album": gone.0}), "no album"),
        (json!({"path": ok, "from": from, "album": "x"}), "not an id"),
    ];
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(elsewhere.join("x.png"), f.dir.join("link.png")).unwrap();
        cases.push((json!({"path": f.dir.join("link.png"), "from": from}), "not in the folder"));
    }
    let photos = f.s.catalog.len();
    let undo = f.s.undo.len();
    for (p, needle) in cases {
        let e = f.add(p.clone()).unwrap_err();
        assert!(e.contains(needle), "{p}: {e}");
        assert_eq!((f.s.catalog.len(), f.s.undo.len()), (photos, undo), "{p}: nothing added");
    }
    // a photo in Recently Deleted is not in the library
    f.s.execute("photo.delete", &json!({"ids": [f.photo.0]})).unwrap();
    let photos = f.s.catalog.len();
    let e = f.add(json!({"path": ok, "from": from})).unwrap_err();
    assert!(e.contains("not in the catalog"), "{e}");
    assert_eq!(f.s.catalog.len(), photos);
    let _ = std::fs::remove_dir_all(&elsewhere);
}

/// Adding a file the library has gives that photo back: `stack` and `album` still apply, nothing
/// is listed twice, and a file the tool wrote over is read again (as Reload from Disk does),
/// keeping the photo's id, settings, stack and albums.
#[test]
fn library_add_again_reuses_the_photo_and_reads_a_changed_file() {
    let mut f = Fixture::new("again");
    let made = f.made("beach-out.png", 60);
    let from = f.photo.0.to_string();
    let first = f.add(json!({"path": made, "from": from})).unwrap();
    let new = photo_of(&first);

    let r = f.add(json!({"path": made, "from": from, "stack": true, "album": f.trip.0})).unwrap();
    assert_eq!((photo_of(&r), r["added"].as_bool(), r["reloaded"].as_bool()), (new, Some(false), Some(false)), "{r}");
    let trip = f.trip;
    let count = |s: &Session| {
        let st = s.catalog.stack_of(new).map(|st| st.photos.iter().filter(|p| **p == new).count());
        let al = s.catalog.album(trip).map(|a| a.photos.iter().filter(|p| **p == new).count());
        (st, al)
    };
    assert_eq!(count(&f.s), (Some(1), Some(1)), "stacked and in the album once");
    let undo = f.s.undo.len();
    let r = f.add(json!({"path": made, "from": from, "stack": true, "album": f.trip.0})).unwrap();
    assert_eq!((r["added"].as_bool(), r["reloaded"].as_bool()), (Some(false), Some(false)));
    assert_eq!(count(&f.s), (Some(1), Some(1)), "still once");
    assert_eq!(f.s.undo.len(), undo, "nothing changed, no undo step");

    // the tool writes over its earlier result
    f.s.execute("develop.set", &json!({"ids": [new.0], "control": "light.exposure", "value": 0.5})).unwrap();
    let stack = f.s.catalog.stack_of(new).unwrap().id;
    write_png(&made, 61, 48, 30);
    let r = f.add(json!({"path": made, "from": from})).unwrap();
    assert_eq!((photo_of(&r), r["added"].as_bool(), r["reloaded"].as_bool()), (new, Some(false), Some(true)), "{r}");
    let ph = f.s.catalog.photo(new).unwrap();
    assert_eq!((ph.width, ph.height), (48, 30), "read again");
    assert_eq!(ph.file_size, std::fs::metadata(&made).unwrap().len());
    let v = f.s.execute("photo.inspect", &json!({"id": new.0})).unwrap();
    assert_eq!(v["develop"]["light"]["exposure"], 0.5, "its settings stay");
    assert_eq!(f.s.catalog.stack_of(new).map(|s| s.id), Some(stack), "its stack stays");
    assert!(f.s.catalog.album(f.trip).unwrap().photos.contains(&new), "its album stays");
}

/// A `program.end` hook runs on a worker: its `library.add` goes to the session's thread.
#[test]
fn library_add_from_a_worker_goes_through_the_session_thread() {
    let mut f = Fixture::new("relay");
    let made = f.made("beach-out.png", 70);
    let (mut host, rx) = crate::cmd::plugins::relay_host(&f.s);
    let p = json!({"path": made, "from": f.photo.0.to_string(), "stack": true});
    let worker = std::thread::spawn(move || host.library_add(&p));
    rx.recv().unwrap().answer(&mut f.s);
    let r = worker.join().unwrap().unwrap();
    assert_eq!(r["added"], true);
    assert!(f.s.catalog.stack_of(photo_of(&r)).is_some(), "stacked by the session");
}

/// A file reached through a link inside the photo's folder is added under the path the folder's
/// own files have, so adding it again by that path finds the same photo.
#[cfg(unix)]
#[test]
fn library_add_files_a_linked_path_under_the_folder_s_own_spelling() {
    let mut f = Fixture::new("spelling");
    let made = f.made("DxO/beach-out.png", 80);
    std::os::unix::fs::symlink(f.dir.join("DxO"), f.dir.join("alias")).unwrap();
    let from = f.photo.0.to_string();
    let r = f.add(json!({"path": f.dir.join("alias/beach-out.png"), "from": from})).unwrap();
    let new = photo_of(&r);
    let path = match &f.s.catalog.photo(new).unwrap().source {
        lcc_catalog::Source::File { path } => path.clone(),
        other => panic!("a file: {other:?}"),
    };
    assert_eq!(Path::new(&path), made, "filed under the folder's own path");
    let r = f.add(json!({"path": made, "from": from})).unwrap();
    assert_eq!((photo_of(&r), r["added"].as_bool()), (new, Some(false)), "the same photo, not a second one: {r}");
}

/// A result the user deleted comes back from Recently Deleted when the tool makes it again, and
/// is read again when the tool wrote over it.
#[test]
fn library_add_brings_a_deleted_result_back_and_reads_it_again() {
    let mut f = Fixture::new("restore");
    let made = f.made("beach-out.png", 90);
    let from = f.photo.0.to_string();
    let new = photo_of(&f.add(json!({"path": made, "from": from})).unwrap());
    f.s.execute("photo.delete", &json!({"ids": [new.0]})).unwrap();
    assert!(!f.s.catalog.photo(new).unwrap().in_library(), "in Recently Deleted");
    write_png(&made, 91, 48, 30);
    let r = f.add(json!({"path": made, "from": from})).unwrap();
    assert_eq!((photo_of(&r), r["added"].as_bool(), r["reloaded"].as_bool()), (new, Some(true), Some(true)), "{r}");
    let ph = f.s.catalog.photo(new).unwrap();
    assert!(ph.in_library(), "back in the library");
    assert_eq!((ph.width, ph.height), (48, 30), "read again");
}

/// What a `library.add` that failed half-way did is undone, leaving no redo step behind.
#[test]
fn a_failed_library_add_takes_back_what_it_did() {
    let mut f = Fixture::new("take-back");
    let made = f.made("beach-out.png", 95);
    let photos = f.s.catalog.photos().filter(|p| p.in_library()).count();
    let undo = f.s.undo.len();
    let r = f.s.execute("library.import", &json!({"paths": [made.to_string_lossy()]})).unwrap();
    let new = PhotoId(r["imported"][0].as_u64().unwrap());
    f.s.execute("album.addPhotos", &json!({"id": f.trip.0, "ids": [new.0]})).unwrap();
    let steps = f.s.undo.len() - undo;
    crate::cmd::plugin_library::take_back(&mut f.s, steps).unwrap();
    assert_eq!(f.s.catalog.photos().filter(|p| p.in_library()).count(), photos, "the import is undone");
    assert!(!f.s.catalog.album(f.trip).unwrap().photos.contains(&new), "the album is as it was");
    assert_eq!((f.s.undo.len(), f.s.redo.len()), (undo, 0), "no step left to undo or redo");
    assert!(made.exists(), "the tool's file stays on disk");
}

/// With the undo history full (it keeps the last 1000 steps), an add is still one undo step, and
/// undoing it takes the whole add back.
#[test]
fn library_add_is_one_undo_step_when_the_undo_history_is_full() {
    let mut f = Fixture::new("full-undo");
    for i in 0..1001 {
        f.s.execute("develop.set", &json!({"ids": [f.photo.0], "control": "light.exposure", "value": (i % 7) as f64 * 0.1})).unwrap();
    }
    let full = f.s.undo.len();
    let made = f.made("beach-out.png", 97);
    let r = f.add(json!({"path": made, "from": f.photo.0, "stack": true, "album": f.trip.0})).unwrap();
    let new = photo_of(&r);
    assert_eq!(f.s.undo.last().map(|e| e.label.as_str()), Some("Add from Plug-in"));
    assert!(f.s.undo.len() < full, "the add's steps are folded into one: {} of {full}", f.s.undo.len());
    f.s.execute("edit.undo", &json!({})).unwrap();
    assert!(f.s.catalog.photo(new).is_none_or(|p| !p.in_library()), "one undo takes the add back");
    assert!(!f.s.catalog.album(f.trip).unwrap().photos.contains(&new));
}

/// An add whose import stacks the file by its own link (`xmpMM:DerivedFrom`, as Edit In writes)
/// folds that into its own undo step and leaves the user's last step alone.
#[test]
fn library_add_leaves_the_user_s_last_undo_step_alone() {
    let mut f = Fixture::new("own-step");
    // a TIFF Edit In wrote, copied: the copy keeps its link to beach.png
    f.s.workflow = crate::cmd::actions::Workflow::in_dir(Some(f.dir.join("settings")));
    let r = f.s.execute("photo.editIn", &json!({"id": f.photo.0, "bitDepth": 8})).unwrap();
    let made = f.dir.join("beach-out.tif");
    std::fs::copy(r["path"].as_str().unwrap(), &made).unwrap();
    f.s.execute("develop.set", &json!({"ids": [f.photo.0], "control": "light.exposure", "value": 1.0})).unwrap();
    let before: Vec<String> = f.s.undo.iter().map(|e| e.label.clone()).collect();

    let new = photo_of(&f.add(json!({"path": made, "from": f.photo.0})).unwrap());
    assert!(f.s.catalog.stack_of(new).is_some_and(|st| st.photos.contains(&f.photo)), "the import stacked it by its link");
    let after: Vec<String> = f.s.undo.iter().map(|e| e.label.clone()).collect();
    assert_eq!(after, [before, vec!["Add from Plug-in".to_string()]].concat(), "one step of its own after the user's");
    f.s.execute("edit.undo", &json!({})).unwrap();
    let v = f.s.execute("photo.inspect", &json!({"id": f.photo.0})).unwrap();
    assert_eq!(v["develop"]["light"]["exposure"], 1.0, "undoing the add keeps the user's edit");
}
