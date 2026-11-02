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
