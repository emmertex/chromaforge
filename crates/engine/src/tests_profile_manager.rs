//! The Profile Manager (ported from OneSlips): hidden profiles leave the menu and `profiles.list`; a
//! camera body's own camera profile is kept per body and applied to its photos.

use lcc_catalog::{MediaKind, Op, Photo, PhotoId, Source};
use serde_json::json;

use crate::Session;

fn raw(s: &mut Session, name: &str, serial: &str) -> PhotoId {
    let id = s.catalog.alloc_photo_id();
    let mut p = Photo::new(id, Source::File { path: format!("/nonexistent/{name}") }, name, "ARW", 100, 100, "2026-01-01");
    p.kind = MediaKind::Raw;
    p.meta.camera = "Sony ILCE-9".into();
    p.meta.camera_model = "ILCE-9".into();
    p.meta.camera_serial = serial.into();
    s.commit("Add", Op::AddPhoto { photo: Box::new(p) }).unwrap();
    id
}

#[test]
fn hidden_profiles_leave_the_menu_and_the_list() {
    let mut s = Session::new();
    let id = crate::presets::PROFILES[1].id;
    let count = |s: &mut Session, all: bool| s.execute("profiles.list", &json!({"all": all})).unwrap().as_array().unwrap().len();
    let before = count(&mut s, false);
    assert_eq!(s.execute("profile.hide", &json!({"id": id})).unwrap()["hidden"], true);
    assert_eq!(count(&mut s, false), before - 1);
    assert_eq!(count(&mut s, true), before, "`all` lists the hidden ones too");
    let menu = s.execute("profiles.menu", &json!({})).unwrap().to_string();
    assert!(!menu.contains(&format!("\"{id}\"")), "not in the menu");
    let m = s.execute("profiles.manager", &json!({})).unwrap();
    assert!(m["profiles"].as_array().unwrap().iter().any(|p| p["id"] == id && p["hidden"] == true), "the manager still lists it");
    assert_eq!(s.execute("profile.hide", &json!({"id": id})).unwrap()["hidden"], false, "toggles back");
    assert_eq!(count(&mut s, false), before);
    assert!(s.execute("profile.hide", &json!({"id": "nope"})).is_err());
}

#[test]
fn a_body_keeps_its_own_camera_profile() {
    let mut s = Session::new();
    let a = raw(&mut s, "_SA0001.ARW", "");
    let b = raw(&mut s, "DSC0002.ARW", "4012345");
    let bodies = s.execute("profiles.bodies", &json!({})).unwrap();
    let keys: Vec<String> = bodies["bodies"].as_array().unwrap().iter().map(|b| b["body"].as_str().unwrap().to_string()).collect();
    assert_eq!(keys, ["ILCE-9|4012345", "ILCE-9|file:SA"], "a serial, else the file-name key");
    // the app's own colour as body b's profile, then applied to its photos only
    let r = s.execute("profile.setBodyDefault", &json!({"id": "lc", "photo": b.0})).unwrap();
    assert_eq!(r["body"], "ILCE-9|4012345");
    let undo = s.undo.len();
    assert_eq!(s.execute("profile.applyBodyDefaults", &json!({})).unwrap()["changed"], 1);
    assert_eq!(s.undo.len(), undo + 1, "one undo step");
    assert_eq!(s.develop_of(b).unwrap().profile.camera, "lc");
    assert_eq!(s.develop_of(a).unwrap().profile.camera, "", "the other body keeps the camera default");
    // cleared, and bad input refused
    s.execute("profile.setBodyDefault", &json!({"id": "", "body": "ILCE-9|4012345"})).unwrap();
    assert!(s.body_profiles.is_empty());
    assert!(s.execute("profile.setBodyDefault", &json!({"id": "dcp:nope", "photo": b.0})).is_err());
    assert!(s.execute("profile.setBodyDefault", &json!({"id": "lc", "body": ""})).is_err());
    assert!(s.execute("profile.setBodyDefault", &json!({"id": "lc", "photo": "x"})).is_err());
}

#[test]
fn a_body_default_applies_at_import_while_the_photo_is_on_the_camera_default() {
    let mut s = Session::new();
    let id = raw(&mut s, "DSC0003.ARW", "77");
    s.body_profiles.insert("ILCE-9|77".into(), "lc".into());
    let mut p = (**s.catalog.photo(id).unwrap()).clone();
    crate::import::apply_import_defaults(&s, &mut p);
    assert_eq!(p.develop.profile.camera, "lc");
}

#[test]
fn profiles_are_renamed_regrouped_reordered_removed_and_restored() {
    let mut s = Session::new();
    let (a, b) = (crate::presets::PROFILES[0].id, crate::presets::PROFILES[2].id);
    let list = |s: &mut Session| s.execute("profiles.list", &json!({})).unwrap().as_array().unwrap().clone();
    let n = list(&mut s).len();
    s.execute("profile.rename", &json!({"id": a, "name": "  My Colour "})).unwrap();
    s.execute("profile.setGroup", &json!({"id": a, "group": "Mine"})).unwrap();
    let menu = s.execute("profiles.menu", &json!({})).unwrap();
    let mine = menu["groups"].as_array().unwrap().iter().find(|g| g["name"] == "Mine").unwrap().clone();
    assert_eq!(mine["profiles"][0]["name"], "My Colour");
    assert_eq!(mine["imported"], true, "the user's group is shown as named");
    assert_eq!(s.profile_info(a), Some(("My Colour", "Mine")));
    // b first
    s.execute("profile.reorder", &json!({"ids": [b]})).unwrap();
    assert_eq!(list(&mut s)[0]["id"], b);
    // a removed built-in look leaves every list until restored
    s.execute("profile.remove", &json!({"id": b})).unwrap();
    assert_eq!(list(&mut s).len(), n - 1);
    assert!(s.execute("profiles.manager", &json!({})).unwrap()["removed"].as_array().unwrap().iter().any(|r| r == b));
    assert_eq!(s.execute("profile.restoreBuiltins", &json!({})).unwrap()["restored"], 1);
    assert_eq!(list(&mut s).len(), n);
    // a whole group: renamed, hidden, removed
    let g = crate::presets::PROFILES[1].group;
    let members = crate::presets::PROFILES.iter().filter(|p| p.group == g && p.id != a).count();
    assert_eq!(s.execute("profile.groupRename", &json!({"group": g, "name": "Renamed"})).unwrap()["profiles"], members);
    assert_eq!(s.execute("profile.groupHide", &json!({"group": "Renamed"})).unwrap()["hidden"], true);
    assert_eq!(list(&mut s).len(), n - members);
    s.execute("profile.groupHide", &json!({"group": "Renamed", "hidden": false})).unwrap();
    assert_eq!(s.execute("profile.groupRemove", &json!({"group": "Renamed"})).unwrap()["removed"], members);
    assert_eq!(list(&mut s).len(), n - members);
    // clearing a name gives the profile its own back
    s.execute("profile.rename", &json!({"id": a, "name": ""})).unwrap();
    assert_eq!(s.profile_info(a).map(|x| x.0), Some(crate::presets::PROFILES[0].name));
}

#[test]
fn hostile_profile_manager_requests_are_errors() {
    let mut s = Session::new();
    let a = crate::presets::PROFILES[0].id;
    for (cmd, p) in [
        ("profile.rename", json!({"id": "nope", "name": "x"})),
        ("profile.rename", json!({"id": a})),
        ("profile.rename", json!({"id": a, "name": 3})),
        ("profile.rename", json!({"id": a, "name": "x".repeat(500)})),
        ("profile.rename", json!({"id": a, "name": "a\u{0}b"})),
        ("profile.setGroup", json!({"id": a, "group": ["x"]})),
        ("profile.reorder", json!({"ids": "all"})),
        ("profile.reorder", json!({"ids": [1, 2]})),
        ("profile.reorder", json!({"ids": ["nope"]})),
        ("profile.remove", json!({})),
        ("profile.groupRename", json!({"group": "No Such Group", "name": "x"})),
        ("profile.groupRename", json!({"group": crate::presets::PROFILES[0].group})),
        ("profile.groupHide", json!({"group": ""})),
        ("profile.groupRemove", json!({"group": null})),
    ] {
        assert!(s.execute(cmd, &p).is_err(), "{cmd} {p}");
    }
    assert!(s.profile_edits.is_empty(), "nothing changed");
    // a hostile prefs file is cut to the limits
    let big = crate::profile_edits::ProfileEdits { order: vec!["x".into(); 20_000], ..Default::default() }.bounded();
    assert_eq!(big.order.len(), crate::profile_edits::MAX_ENTRIES);
}

#[test]
fn an_import_gives_its_raws_the_camera_profile_chosen_per_body_or_for_all() {
    let dir = std::env::temp_dir().join(format!("lcc-import-bodyprof-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let corners = lcc_camprof::ChartCorners::from_rect(10.0, 10.0, 40.0, 30.0);
    let write = |name: &str, serial: &str| {
        let p = dir.join(name);
        std::fs::write(&p, crate::chart_profiling::demo_chart_dng_of_body(64, 48, &corners, "D55", true, Some(serial)).unwrap()).unwrap();
        p.to_string_lossy().to_string()
    };
    let (a, b) = (write("SA0001.dng", "SN1"), write("SB0001.dng", "SN2"));
    let mut s = Session::new().with_fs();
    let r = s.execute("library.import", &json!({"paths": [a]})).unwrap();
    let id = PhotoId(r["imported"][0].as_u64().unwrap());
    let ph = s.catalog.photo(id).unwrap();
    let key = crate::profile_bodies::key_of(ph).unwrap();
    // (its serial, else the file names' key)
    let (mine, theirs) = if key.ends_with("|SN1") { ("SN1", "SN2") } else { ("file:SA", "file:SB") };
    assert!(key.ends_with(mine), "{key}");
    // per body, for this import only: SN2's body gets the built-in colour, SN1 isn't named
    s.execute("library.remove", &json!({"ids": [id.0]})).ok();
    let other = key.replace(mine, theirs);
    let plan = s.execute("import.profilePlan", &json!({"paths": [&b], "cameraProfile": {"perBody": {&other: "lc"}}})).unwrap();
    assert_eq!(plan["rows"].as_array().unwrap().len(), 1, "{plan}");
    assert_eq!(
        (plan["rows"][0]["body"].as_str(), plan["rows"][0]["profile"].as_str(), plan["rows"][0]["how"].as_str()),
        (Some(other.as_str()), Some("lc"), Some("chosen"))
    );
    assert!(s.execute("import.profilePlan", &json!({"paths": "x"})).is_err());
    assert!(s.execute("import.profilePlan", &json!({"paths": [&b], "cameraProfile": 7})).is_err());
    let r = s.execute("library.import", &json!({"paths": [b], "cameraProfile": {"perBody": {other: "lc"}}})).unwrap();
    assert_eq!(r["cameraProfiles"]["lc"]["photos"], 1, "{r}");
    let idb = PhotoId(r["imported"][0].as_u64().unwrap());
    assert_eq!(s.catalog.photo(idb).unwrap().develop.profile.camera, "lc");
    assert!(s.body_profiles.is_empty(), "nothing is remembered for the body");
    for bad in [
        json!("manual"),
        json!({"all": "nope"}),
        json!({"all": 3}),
        json!({"perBody": []}),
        json!({"perBody": {"": "lc"}}),
        json!({"all": "lc", "x": 1}),
    ] {
        assert!(s.execute("library.import", &json!({"paths": [b], "cameraProfile": bad})).is_err(), "{bad}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// The Import tab imports browsed photos (OneSlips LR-IMP-PROFILE, 21c4b5644): an unedited browsed raw
/// takes the per-body choice made for the import, and a body default set after the folder was
/// browsed; a photo edited while browsing keeps its own.
#[test]
fn browsed_raws_take_the_import_profile_choice_and_a_later_body_default() {
    let dir = std::env::temp_dir().join(format!("lcc-import-browsed-bodyprof-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let card = dir.join("card");
    std::fs::create_dir_all(&card).unwrap();
    let corners = lcc_camprof::ChartCorners::from_rect(10.0, 10.0, 40.0, 30.0);
    for (name, serial) in [("SA0001.dng", "SN1"), ("SB0001.dng", "SN2"), ("SC0001.dng", "SN2")] {
        std::fs::write(card.join(name), crate::chart_profiling::demo_chart_dng_of_body(64, 48, &corners, "D55", true, Some(serial)).unwrap())
            .unwrap();
    }
    let mut s = Session::new().with_fs();
    s.open_library(dir.join("lib"), true).unwrap();
    s.execute("library.browse", &json!({"path": card.to_string_lossy()})).unwrap();
    let id_of = |s: &Session, n: &str| s.catalog.photos().find(|p| p.file_name == n).unwrap().id;
    let (a, b, c) = (id_of(&s, "SA0001.dng"), id_of(&s, "SB0001.dng"), id_of(&s, "SC0001.dng"));
    assert!(s.catalog.photo(a).unwrap().develop.profile.camera.is_empty(), "no body default while browsing");
    let key_b = crate::profile_bodies::key_of(s.catalog.photo(b).unwrap()).unwrap();
    // a body default for A's body, set after browsing
    s.execute("profile.setBodyDefault", &json!({"id": "lc", "photo": a.0})).unwrap();
    let path = |s: &Session, id: PhotoId| match &s.catalog.photo(id).unwrap().source {
        Source::File { path } => path.clone(),
        _ => panic!("unexpected case in a test"),
    };
    s.execute("library.import", &json!({"paths": [path(&s, a)]})).unwrap();
    assert!(!s.catalog.photo(a).unwrap().local);
    assert_eq!(s.catalog.photo(a).unwrap().develop.profile.camera, "lc", "the body's default as it is at import");
    // C was edited while browsing: it keeps its own
    s.selection = crate::Selection::single(c);
    s.execute("develop.set", &json!({"control": "light.exposure", "value": 0.5})).unwrap();
    s.execute("library.import", &json!({"paths": [path(&s, b), path(&s, c)], "cameraProfile": {"perBody": {key_b: "lc"}}})).unwrap();
    assert_eq!(s.catalog.photo(b).unwrap().develop.profile.camera, "lc", "the import's per-body choice");
    assert!(s.catalog.photo(c).unwrap().develop.profile.camera.is_empty(), "an edited photo keeps its profile");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A wedding import (IMPORT-STACK, ported from OneSlips LR-IMP-WINDOW / LR-IMP-PROFILE): a stack of
/// presets applies in order, the old single `preset` first, one History step each; each body keeps
/// the camera profile chosen for it through a preset that names another, unless `presetProfile`.
#[test]
fn an_import_applies_a_preset_stack_in_order_and_each_body_keeps_its_profile() {
    let dir = std::env::temp_dir().join(format!("lcc-import-stack-bodies-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let corners = lcc_camprof::ChartCorners::from_rect(10.0, 10.0, 40.0, 30.0);
    let write = |name: &str, serial: &str| {
        let p = dir.join(name);
        std::fs::write(&p, crate::chart_profiling::demo_chart_dng_of_body(64, 48, &corners, "D55", true, Some(serial)).unwrap()).unwrap();
        p.to_string_lossy().to_string()
    };
    let mut s = Session::new().with_fs();
    let preset = |id: &str, settings: serde_json::Value| lcc_develop::Preset {
        id: id.into(),
        name: id.into(),
        group: "Wedding".into(),
        settings,
        favorite: false,
        builtin: false,
    };
    let ids = s.add_presets(vec![
        preset("user.base", json!({"light": {"exposure": 0.3, "contrast": 10.0}})),
        // names the camera's default profile: the body's own must stay
        preset("user.look", json!({"light": {"exposure": 0.6}, "profile": {"camera": ""}})),
        preset("user.finish", json!({"light": {"contrast": 25.0}, "color": {"vibrance": 15.0}})),
    ]);
    assert_eq!(ids.len(), 3, "{ids:?}");
    let key_of_file = |s: &mut Session, path: &str| {
        let plan = s.execute("import.profilePlan", &json!({"paths": [path]})).unwrap();
        plan["rows"][0]["body"].as_str().unwrap().to_string()
    };
    let (a, b) = (write("SA0001.dng", "SN1"), write("SB0001.dng", "SN2"));
    let key_a = key_of_file(&mut s, &a);
    let params = json!({
        "paths": [&a, &b],
        "preset": ids[0],
        "presets": [ids[1], ids[2]],
        "cameraProfile": {"perBody": {&key_a: "lc"}},
    });
    let r = s.execute("library.import", &params).unwrap();
    let got: Vec<PhotoId> = r["imported"].as_array().unwrap().iter().map(|v| PhotoId(v.as_u64().unwrap())).collect();
    assert_eq!(got.len(), 2, "{r}");
    for id in &got {
        let p = s.catalog.photo(*id).unwrap();
        let d = &p.develop;
        assert!((d.light.exposure - 0.6).abs() < 1e-6, "the later preset's exposure wins: {}", d.light.exposure);
        assert!((d.light.contrast - 25.0).abs() < 1e-6, "the last preset's contrast: {}", d.light.contrast);
        assert!((d.color.vibrance - 15.0).abs() < 1e-6);
        let steps: Vec<&str> = p.history.iter().filter(|h| h.label.starts_with("Preset: ")).map(|h| h.label.as_str()).collect();
        assert_eq!(steps, ["Preset: user.base", "Preset: user.look", "Preset: user.finish"], "one History step each, in order");
        let body = crate::profile_bodies::key_of(p).unwrap();
        let want = if body == key_a { "lc" } else { "" };
        assert_eq!(d.profile.camera, want, "{body}: its own profile through the stack");
    }
    // the old single `preset` alone still works (the same synthetic bytes: imported as a copy)
    let c = write("SA0002.dng", "SN1");
    let r = s.execute("library.import", &json!({"paths": [&c], "preset": ids[0], "importDuplicates": true})).unwrap();
    let p = s.catalog.photo(PhotoId(r["imported"][0].as_u64().unwrap())).unwrap();
    assert!((p.develop.light.exposure - 0.3).abs() < 1e-6 && (p.develop.light.contrast - 10.0).abs() < 1e-6);
    // presetProfile: the preset's profile replaces the body's
    let e = write("SA0003.dng", "SN1");
    let r = s
        .execute("library.import", &json!({"paths": [&e], "presets": [ids[1]], "presetProfile": true, "importDuplicates": true, "cameraProfile": {"perBody": {&key_a: "lc"}}}))
        .unwrap();
    let p = s.catalog.photo(PhotoId(r["imported"][0].as_u64().unwrap())).unwrap();
    assert_eq!(p.develop.profile.camera, "", "the preset's profile");
    assert!(s.execute("library.import", &json!({"paths": [&e], "presets": ["no.such.preset"]})).is_err(), "an unknown preset refuses the import");
    let _ = std::fs::remove_dir_all(&dir);
}
