//! The app-wide store (LR-PREF-SHARED / LR-PREF-HABITS): shared by every library, each library
//! merged once (read only), profile ids remapped, marks moved out of presets.json, damaged files
//! reported, never a crash.

use std::path::{Path, PathBuf};

use serde_json::json;

use crate::Session;

fn temp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lc-engine-store-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

fn with_store(store: &Path, lib: &Path) -> Session {
    let mut s = Session::new().with_app_store(store);
    s.open_library(lib, false).unwrap();
    s
}

fn own(lib: &Path) -> Session {
    let mut s = Session::new();
    s.open_library(lib, false).unwrap();
    s
}

fn user_preset(id: &str, name: &str) -> lcc_develop::Preset {
    lcc_develop::Preset {
        id: id.into(),
        name: name.into(),
        group: "Mine".into(),
        settings: json!({"light": {"exposure": 0.5}}),
        favorite: false,
        builtin: false,
    }
}

fn export_preset(name: &str, quality: u32) -> crate::export::ExportPreset {
    serde_json::from_value(json!({"name": name, "params": {"quality": quality}})).unwrap()
}

#[test]
fn presets_and_export_presets_made_in_one_library_are_in_the_next() {
    let root = temp("share");
    let (store, a, b) = (root.join("store"), root.join("a"), root.join("b"));
    let mut s = with_store(&store, &a);
    s.presets.push(user_preset("user.warm", "Warm"));
    s.export_presets.push(export_preset("Web", 80));
    s.save_prefs().unwrap();
    s.persist().unwrap();
    drop(s);
    // a library's own presets.json isn't written while the store holds them
    assert!(!a.join("presets.json").exists());
    let s = with_store(&store, &b);
    assert!(s.presets.iter().any(|p| p.id == "user.warm"));
    assert!(s.export_presets.iter().any(|p| p.name == "Web"));
    drop(s);
    // without the store a library has its own (nothing of the store's)
    let s = own(&b);
    assert!(!s.presets.iter().any(|p| p.id == "user.warm"));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_library_merges_once_read_only_and_its_photos_follow_a_remapped_profile() {
    let root = temp("merge");
    let (store, a, b) = (root.join("store"), root.join("a"), root.join("b"));
    // two libraries made without the store, each with its own `lut:look` (different files),
    // the same export preset, and a clashing one
    for (dir, body, q) in [
        (&a, "LUT_3D_SIZE 2\n0 0 0\n1 0 0\n0 1 0\n1 1 0\n0 0 1\n1 0 1\n0 1 1\n1 1 1\n", 80),
        (&b, "LUT_3D_SIZE 2\n0 0 0\n0.5 0 0\n0 1 0\n1 1 0\n0 0 1\n1 0 1\n0 1 1\n1 1 1\n", 60),
    ] {
        let mut s = own(dir);
        std::fs::create_dir_all(dir.join("Profiles")).unwrap();
        let f = dir.join("Profiles").join("look.cube");
        std::fs::write(&f, body).unwrap();
        s.lut_profiles.push(crate::cmd::lut_profiles::LutProfile {
            id: "lut:look".into(),
            name: "Look".into(),
            group: "G".into(),
            file: f.to_string_lossy().into(),
        });
        s.export_presets.push(export_preset("Web", 80));
        s.export_presets.push(export_preset("Print", q));
        let id = s.catalog.alloc_photo_id();
        let mut p =
            lcc_catalog::Photo::new(id, lcc_catalog::Source::File { path: "/x/a.jpg".into() }, "a.jpg", "JPEG", 60, 40, "2026-01-01T10:00:00");
        std::sync::Arc::make_mut(&mut p.develop).profile.id = "lut:look".into();
        s.commit("Add", lcc_catalog::Op::AddPhoto { photo: Box::new(p) }).unwrap();
        s.save_prefs().unwrap();
        s.persist().unwrap();
    }
    let b_prefs = std::fs::read(b.join("prefs.json")).unwrap();
    drop(with_store(&store, &a));
    let mut s = with_store(&store, &b);
    // b's profile is another file: it got a new id, and b's photo follows it
    let ids: Vec<_> = s.lut_profiles.iter().map(|p| p.id.clone()).collect();
    assert_eq!(ids.len(), 2, "{ids:?}");
    let new_id = ids.iter().find(|i| *i != "lut:look").unwrap().clone();
    assert!(s.catalog.photos().all(|p| p.develop.profile.id == new_id));
    // the same export preset once, the clash kept as "Print 2"
    let names: Vec<_> = s.export_presets.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names.iter().filter(|n| **n == "Web").count(), 1);
    assert!(names.contains(&"Print") && names.contains(&"Print 2"), "{names:?}");
    // the merge only read b: its prefs.json keeps its own lists, also after a save
    s.save_prefs().unwrap();
    let after: serde_json::Value = serde_json::from_slice(&std::fs::read(b.join("prefs.json")).unwrap()).unwrap();
    let before: serde_json::Value = serde_json::from_slice(&b_prefs).unwrap();
    for k in crate::app_store::SHARED_PREFS_KEYS {
        if k != "import" {
            assert_eq!(after.get(k), before.get(k), "{k}");
        }
    }
    // merged once: asking again skips it
    let r = s.execute("store.mergeLibraries", &json!({"paths": [b.to_string_lossy()]})).unwrap();
    assert_eq!(r["merged"][0]["skipped"], true);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn at_import_and_hidden_marks_move_from_a_library_into_the_store() {
    let root = temp("marks");
    let (store, a, b) = (root.join("store"), root.join("a"), root.join("b"));
    let mut s = own(&a);
    let id = s.presets.first().unwrap().id.clone();
    s.execute("preset.forImport", &json!({"id": id})).unwrap();
    s.execute("preset.hide", &json!({"id": id})).unwrap();
    drop(s);
    drop(with_store(&store, &a));
    let shared: serde_json::Value = serde_json::from_slice(&std::fs::read(store.join("shared.json")).unwrap()).unwrap();
    assert_eq!(shared["presetMarks"]["atImport"], json!([id]));
    let presets: serde_json::Value = serde_json::from_slice(&std::fs::read(store.join("presets.json")).unwrap()).unwrap();
    assert!(presets.get("atImport").is_none() && presets.get("hidden").is_none());
    let s = with_store(&store, &b);
    assert_eq!(s.preset_at_import, std::slice::from_ref(&id));
    assert_eq!(s.preset_hidden, [id]);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_damaged_or_unreadable_store_is_reported_never_a_panic() {
    let root = temp("hostile");
    for (i, bytes) in [&b"{not json"[..], b"\xff\xfe\x00", b"[]", b"{\"lutProfiles\": 7}", &[b'['; 10_000][..], b""].iter().enumerate() {
        let store = root.join(format!("store{i}"));
        std::fs::create_dir_all(&store).unwrap();
        std::fs::write(store.join("shared.json"), bytes).unwrap();
        std::fs::write(store.join("presets.json"), bytes).unwrap();
        let mut s = Session::new().with_app_store(&store);
        let w = s.take_library_warnings();
        assert!(w.iter().any(|w| w.contains("shared.json") && w.contains("damaged")), "{w:?}");
        // kept aside, and the store works with defaults
        assert!(std::fs::read_dir(&store).unwrap().any(|e| e.unwrap().file_name().to_string_lossy().starts_with("shared.json.corrupt-")));
        s.export_presets.push(export_preset("Web", 80));
        s.save_prefs().unwrap();
    }
    // a store file that can't be read (a folder in its place) is never overwritten: an error, not a panic
    let store = root.join("blocked");
    std::fs::create_dir_all(store.join("shared.json")).unwrap();
    let mut s = Session::new().with_app_store(&store);
    assert!(!s.take_library_warnings().is_empty());
    s.export_presets.push(export_preset("Web", 80));
    let e = s.save_prefs().unwrap_err().to_string();
    assert!(e.contains("shared.json"), "{e}");
    // a library merged from hostile files: reported, nothing panics
    let lib = root.join("badlib");
    drop(own(&lib));
    std::fs::write(lib.join("prefs.json"), b"{\"exportPresets\": 5").unwrap();
    let mut s = Session::new().with_app_store(&root.join("store-ok"));
    let r = s.execute("store.mergeLibraries", &json!({"paths": [lib.to_string_lossy(), "/no/such/library"]})).unwrap();
    assert!(!r["merged"][0]["problems"].as_array().unwrap().is_empty());
    assert!(!r["merged"][1]["problems"].as_array().unwrap().is_empty());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn use_library_keeps_a_copy_and_gather_reads_backups() {
    let root = temp("use");
    let (store, a, old) = (root.join("store"), root.join("a"), root.join("old"));
    let mut s = with_store(&store, &a);
    s.export_presets.push(export_preset("Web", 80));
    s.save_prefs().unwrap();
    s.execute("store.useLibrary", &json!({"keep": true})).unwrap();
    assert!(!s.uses_app_store());
    drop(s);
    let s = with_store(&store, &a);
    assert!(s.library.as_ref().unwrap().own_store);
    assert!(s.export_presets.iter().any(|p| p.name == "Web"));
    drop(s);
    // an older library's export preset only in a backup is gathered once
    drop(own(&old));
    let backup = old.join("Backups").join("2026-01-01");
    std::fs::create_dir_all(&backup).unwrap();
    std::fs::write(backup.join("prefs.json"), serde_json::to_vec(&json!({"exportPresets": [{"name": "Archive", "params": {}}]})).unwrap()).unwrap();
    let mut s = Session::new().with_app_store(&store);
    let r = s.execute("store.gatherPresets", &json!({"paths": [old.to_string_lossy()]})).unwrap();
    assert_eq!(r["count"], 1);
    assert!(s.export_presets.iter().any(|p| p.name == "Archive"));
    let r = s.execute("store.gatherPresets", &json!({"paths": [old.to_string_lossy()]})).unwrap();
    assert_eq!(r["count"], 0);
    let _ = std::fs::remove_dir_all(&root);
}
