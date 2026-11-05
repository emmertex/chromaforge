//! Settings ▸ Profiles, the Profile Manager (ported from OneSlips): its Hide button takes a profile out of
//! the menu, and each camera body of the library a row for its own camera profile.

use serde_json::json;

use crate::tests_classic::{click, demo, has, run};

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
    // the dialog's content slides into place over its first frames: click once it has settled
    h.settle(std::time::Duration::from_secs(60));
    // the profiles are under the bodies and the plain page's rows: scroll the page until the button shows
    let at = |h: &crate::headless::Headless, id: &str| h.app.widgets.iter().rev().find(|(w, _)| w == id).map(|(_, r)| *r);
    for _ in 0..30 {
        let (Some(rows), Some(button)) = (at(&h, "settings:rows"), at(&h, &hide)) else { break };
        if rows.contains_rect(button) {
            break;
        }
        h.request("ui.move", json!({"x": rows.center().x, "y": rows.center().y}), std::time::Duration::from_secs(20));
        h.request("ui.scroll", json!({"dy": -200.0}), std::time::Duration::from_secs(20));
        for _ in 0..4 {
            h.step();
        }
    }
    click(&mut h, &hide);
    h.step();
    assert_eq!(h.app.session.profile_hidden, [profile.to_string()], "the click hid that profile");
    assert!(has(&h, &hide), "a hidden profile stays in the manager, to show it again");
    assert!(h.app.session.execute("profiles.list", &json!({})).unwrap().as_array().unwrap().iter().all(|p| p["id"] != profile), "out of the list");
}
