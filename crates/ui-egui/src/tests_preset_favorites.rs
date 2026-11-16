//! Presets panel: starred presets get a Favorites group at the top, as profiles do (issue #1999).

use std::time::Duration;

use serde_json::json;

use crate::headless::Headless;
use crate::{LccApp, Services};

const SETTLE: Duration = Duration::from_secs(120);

fn rect(h: &Headless, id: &str) -> Option<egui::Rect> {
    h.app.widgets.iter().find(|(w, _)| w == id).map(|(_, r)| *r)
}

#[test]
fn a_starred_preset_is_listed_first_in_favorites_and_stays_in_its_group() {
    let app = LccApp::new(lcc_engine::Session::with_demo(), Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [1200.0, 800.0], 1.0);
    h.settle(SETTLE);
    h.app.run("panel.presets", json!({})).unwrap();
    // groups start folded here except the one holding the current preset: open B&W
    h.app.ui.group_open.insert("preset:B&W".into(), true);
    h.step();
    h.step();
    assert!(rect(&h, "preset:lc.bw-selenium").is_some(), "the B&W group is in view");
    assert!(rect(&h, "presetFavorites").is_none(), "no Favorites group without favourites");

    h.app.run("preset.favorite", json!({"id": "lc.bw-selenium"})).unwrap();
    h.step();
    h.step();
    let header = rect(&h, "presetFavorites").expect("a Favorites group");
    let groups: Vec<egui::Rect> = h.app.widgets.iter().filter(|(w, _)| w.starts_with("presetGroup:")).map(|(_, r)| *r).collect();
    assert!(!groups.is_empty());
    assert!(groups.iter().all(|g| header.top() < g.top()), "Favorites comes before every group");
    let fav = rect(&h, "presetFav:lc.bw-selenium").expect("the preset under Favorites");
    let own = rect(&h, "preset:lc.bw-selenium").expect("the preset still in B&W");
    assert!(fav.top() < own.top());
    if let Some(path) = std::env::var_os("PRESET_FAVORITES_SCREENSHOT_DIR") {
        let pixels = h.paint();
        let image = lcc_raster::Rgba8 { width: pixels.size[0], height: pixels.size[1], data: pixels.pixels.iter().map(|c| c.to_array()).collect() };
        let png = lcc_codecs::encode_png(&lcc_codecs::EncodeImage::rgba8(&image), &lcc_codecs::EncodeMeta::default()).unwrap();
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(std::path::Path::new(&path).join("lcc-presetfav.png"), png).unwrap();
    }

    h.app.run("preset.favorite", json!({"id": "lc.bw-selenium"})).unwrap();
    h.step();
    h.step();
    assert!(rect(&h, "presetFavorites").is_none(), "un-starring the last favourite removes the group");
    assert!(rect(&h, "presetFav:lc.bw-selenium").is_none());
}

/// A headless app on the demo library with the Presets panel open and laid out.
fn app() -> Headless {
    let app = LccApp::new(lcc_engine::Session::with_demo(), Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [1400.0, 1000.0], 1.0);
    h.app.run("panel.presets", json!({})).unwrap();
    for _ in 0..5 {
        h.step();
    }
    h
}

/// A user group named "Favorites" keeps its own header (a typed "favorites" joins it: groups match
/// in any case), apart from the section's: a
/// click aimed at either id lands on the right row (the widget lookup takes the last match).
#[test]
fn user_groups_named_favorites_keep_their_own_header() {
    let mut h = app();
    let id = h.app.session.presets.first().map(|p| p.id.clone()).unwrap();
    h.app.run("preset.favorite", json!({"id": id})).unwrap();
    for group in ["Favorites", "favorites"] {
        h.app.run("preset.create", json!({"name": format!("Mine in {group}"), "group": group})).unwrap();
    }
    for _ in 0..3 {
        h.step();
    }
    let section = rect(&h, "presetFavorites").unwrap();
    assert!(h.app.session.presets.iter().filter(|p| !p.builtin).all(|p| p.group == "Favorites"));
    let id = "presetGroup:Favorites";
    let user = rect(&h, id).expect("the user's Favorites group is listed");
    assert_ne!(user, section, "{id} is its own row");
    assert_eq!(h.app.widgets.iter().filter(|(w, _)| *w == id).count(), 1, "{id} names one row");
    assert_eq!(h.app.widgets.iter().filter(|(w, _)| w == "presetFavorites").count(), 1);
}
