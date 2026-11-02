//! Settings ▸ Profiles, the Profile Manager (ported from OneSlips): every profile has a Hide button and
//! each camera body of the library a row for its own camera profile.

use serde_json::json;

use crate::tests_classic::{demo, has, run};

#[test]
fn the_profile_manager_hides_profiles_and_sets_body_profiles() {
    let mut h = demo();
    let id = h.app.session.catalog.alloc_photo_id();
    let mut p = lcc_catalog::Photo::new(id, lcc_catalog::Source::Demo { scene: 0 }, "DSC0001.ARW", "ARW", 300, 200, "2026-01-01T10:00:00");
    p.kind = lcc_catalog::MediaKind::Raw;
    p.meta.camera = "Sony ILCE-9".into();
    p.meta.camera_model = "ILCE-9".into();
    p.meta.camera_serial = "4012345".into();
    h.app.session.catalog.apply(lcc_catalog::Op::AddPhoto { photo: Box::new(p) }).unwrap();
    run(&mut h, "app.settings", json!({"tab": "profiles"}));
    h.step();
    h.step();
    let profile = lcc_engine::presets::PROFILES[0].id;
    let hide = format!("button:profileHide:{profile}");
    assert!(has(&h, "combo:bodyProfile:ILCE-9|4012345"), "the body has a row");

    assert!(has(&h, &hide), "every profile has a Hide button");
    run(&mut h, "profile.hide", json!({"id": profile}));
    assert!(has(&h, &hide), "a hidden profile stays in the manager, to show it again");
    assert!(h.app.session.execute("profiles.list", &json!({})).unwrap().as_array().unwrap().iter().all(|p| p["id"] != profile), "out of the list");
}
