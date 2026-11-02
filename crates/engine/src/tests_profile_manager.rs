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
