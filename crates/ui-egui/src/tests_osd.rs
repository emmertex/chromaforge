//! The on-screen overlay for every change (`panels::surface_osd`, `change_report`).
//!
//! As a photographer sees it: whatever I change, with a key, the mouse, a menu, a slider or the
//! console, a short note in the window says what changed ("Exposure +0.33", "Picked", "3 stars"
//! on 12 photos, "Profile X applied to 350 photos"). Dragging a slider updates the same note.
//! Settings can make it show only what the console does, or nothing; its look is mine to set.

use std::time::Duration;

use serde_json::{Value, json};

use crate::headless::Headless;
use crate::state::{RightPanel, ViewMode};
use crate::{LccApp, Services};

const T: Duration = Duration::from_secs(20);

fn demo() -> Headless {
    let services = Services { png: None, ..Default::default() };
    let app = LccApp::new(lcc_engine::Session::with_demo(), services);
    let mut h = Headless::new(app, [1400.0, 900.0], 1.0);
    h.settle(T);
    h
}

fn run(h: &mut Headless, command: &str, params: Value) -> Value {
    let r = h.request("engine.execute", json!({"command": command, "params": params}), T);
    assert_eq!(r["ok"], true, "{command}: {r}");
    r["result"].clone()
}

fn shown(h: &mut Headless) -> Value {
    h.step();
    run(h, "osd.state", json!({}))["shown"].clone()
}

fn widget(h: &Headless, id: &str) -> Option<egui::Rect> {
    h.app.widgets.iter().find(|(w, _)| w == id).map(|(_, r)| *r)
}

fn open(h: &mut Headless, n: usize) -> Vec<u64> {
    let ids: Vec<u64> = h.app.session.visible_cloned().iter().take(n).map(|p| p.0).collect();
    run(h, "library.select", json!({"ids": ids}));
    h.app.ui.view = ViewMode::Detail;
    h.app.ui.right = RightPanel::Edit;
    h.settle(T);
    ids
}

#[test]
fn a_slider_value_shows_its_name_and_value() {
    let mut h = demo();
    open(&mut h, 1);
    run(&mut h, "develop.set", json!({"control": "light.exposure", "value": 0.33}));
    let s = shown(&mut h);
    assert_eq!(s["title"], "Exposure", "{s}");
    assert_eq!(s["value"], "+0.33", "{s}");
    assert_eq!(s["caption"], "", "one photo: no count");
    assert!(s["icon"].is_string(), "the console's icon for Exposure: {s}");
    assert!(widget(&h, "osd:surface").is_some(), "drawn in the window");
}

#[test]
fn ratings_flags_and_labels_name_the_change_and_count_the_photos() {
    let mut h = demo();
    let ids = open(&mut h, 3);
    run(&mut h, "photo.rate", json!({"rating": 3, "ids": ids}));
    let s = shown(&mut h);
    assert_eq!((s["title"].as_str(), s["caption"].as_str()), (Some("3 stars"), Some("on 3 photos")), "{s}");
    run(&mut h, "photo.pick", json!({}));
    let s = shown(&mut h);
    assert_eq!(s["title"], "Picked", "{s}");
    run(&mut h, "photo.rate", json!({"rating": 1, "ids": [ids[0]]}));
    assert_eq!(shown(&mut h)["title"], "1 star");
    run(&mut h, "photo.label", json!({"label": "red", "ids": [ids[0]]}));
    let s = shown(&mut h);
    assert!(s["title"].as_str().is_some_and(|t| t.starts_with("Label: ")), "{s}");
    run(&mut h, "edit.undo", json!({}));
    let s = shown(&mut h);
    assert!(s["title"].as_str().is_some_and(|t| t.starts_with("Undo")), "{s}");
}

#[test]
fn auto_sync_counts_every_photo_the_change_reached() {
    let mut h = demo();
    open(&mut h, 4);
    h.app.session.auto_sync = true;
    run(&mut h, "develop.set", json!({"control": "light.contrast", "value": 12}));
    let s = shown(&mut h);
    assert_eq!((s["title"].as_str(), s["caption"].as_str()), (Some("Contrast"), Some("on 4 photos")), "{s}");
}

#[test]
fn a_drag_updates_one_note_with_its_running_value() {
    let mut h = demo();
    open(&mut h, 1);
    run(&mut h, "develop.beginInteraction", json!({"label": "Exposure"}));
    let mut seqs = Vec::new();
    for i in 1..=10 {
        run(&mut h, "develop.set", json!({"control": "light.exposure", "value": f64::from(i) * 0.1}));
        let s = shown(&mut h);
        assert_eq!(s["title"], "Exposure", "{s}");
        seqs.push(h.app.session.last_change.as_ref().map(|c| (c.seq, c.live)));
    }
    assert!(seqs.iter().all(|s| s.is_some_and(|(_, live)| live)), "live while dragging: {seqs:?}");
    assert_eq!(shown(&mut h)["value"], "+1.00", "the running value");
    run(&mut h, "develop.endInteraction", json!({}));
    assert_eq!(shown(&mut h)["value"], "+1.00");
}

#[test]
fn the_mode_decides_what_shows() {
    let mut h = demo();
    let ids = open(&mut h, 1);
    let r = run(&mut h, "osd.settings", json!({"mode": "surface"}));
    assert_eq!(r["mode"], "surface");
    run(&mut h, "photo.rate", json!({"rating": 4, "ids": ids}));
    assert!(shown(&mut h).is_null(), "control surface only: a key press in the app shows nothing");
    run(&mut h, "osd.mode", json!({}));
    assert_eq!(run(&mut h, "osd.state", json!({}))["settings"]["mode"], "always", "osd.mode steps to the next mode");
    run(&mut h, "photo.rate", json!({"rating": 2, "ids": ids}));
    assert_eq!(shown(&mut h)["title"], "2 stars");
    run(&mut h, "osd.settings", json!({"mode": "off"}));
    run(&mut h, "photo.rate", json!({"rating": 5, "ids": ids}));
    assert!(shown(&mut h).is_null(), "off");
    assert!(!h.app.surface.settings().osd.enabled, "Off also switches the console's readout off");
}

#[test]
fn settings_are_commands_and_hostile_values_are_refused_or_clamped() {
    let mut h = demo();
    let r = run(&mut h, "osd.settings", json!({"size": 1.5, "opacity": 0.5, "displayMs": 2500, "position": "upHigh", "background": "#102030"}));
    assert_eq!(r, json!({"mode": "always", "size": 1.5, "opacity": 0.5, "displayMs": 2500, "position": "upHigh", "background": "#102030"}));
    let r = run(&mut h, "osd.settings", json!({"size": 1e9, "opacity": -3, "displayMs": 1e12}));
    assert_eq!((r["size"].as_f64(), r["opacity"].as_f64(), r["displayMs"].as_u64()), (Some(2.0), Some(0.1), Some(10_000)));
    for bad in [json!({"mode": "loud"}), json!({"position": "left"}), json!({"background": "red"}), json!({"size": "big"})] {
        let r = h.request("engine.execute", json!({"command": "osd.settings", "params": bad}), T);
        assert_eq!(r["ok"], false, "{bad}: {r}");
    }
}

#[test]
fn background_work_and_queries_never_show() {
    let mut h = demo();
    open(&mut h, 1);
    let before = h.app.session.last_change.as_ref().map(|c| c.seq);
    run(&mut h, "library.info", json!({}));
    run(&mut h, "library.select", json!({"ids": []}));
    assert_eq!(h.app.session.last_change.as_ref().map(|c| c.seq), before, "nothing changed: no report");
    assert!(lcc_engine::change_report::is_background("faces.pump"));
}

#[test]
fn the_overlay_draws_at_every_size_and_position() {
    let mut h = demo();
    open(&mut h, 1);
    let mut rects = Vec::new();
    for (size, position) in [(0.5, "down"), (1.0, "downLow"), (2.0, "up"), (1.25, "upHigh")] {
        run(&mut h, "osd.settings", json!({"size": size, "position": position}));
        run(&mut h, "photo.pick", json!({}));
        h.step();
        h.step();
        let r = widget(&h, "osd:surface").expect("drawn");
        rects.push((size, position, r));
    }
    let canvas = h.app.canvas_rect.expect("a canvas");
    for (size, position, r) in &rects {
        assert!(canvas.expand(1.0).contains_rect(*r), "{size} {position}: {r:?} inside {canvas:?}");
    }
    assert!(rects[2].2.height() > rects[0].2.height() * 3.0, "size 2 is four times size 0.5");
    assert!(rects[2].2.center().y < canvas.center().y && rects[0].2.center().y > canvas.center().y, "up is above, down below");
}

/// While an on-photo tool is in use (crop, a brush, a picker, a drag on the photo) the overlay never covers the photo;
/// with the tool put away, the same change shows again (test notes 2026-10-24 note 15).
#[test]
fn the_overlay_stays_off_the_photo_while_a_tool_is_in_use() {
    let mut h = demo();
    open(&mut h, 1);
    let drawn = |h: &mut Headless| {
        h.step();
        h.step();
        widget(h, "osd:surface").is_some()
    };
    // crop: straightening shows its own angle by the pointer, not a second note over the photo
    run(&mut h, "panel.crop", json!({}));
    h.settle(T);
    assert_eq!(h.app.ui.right, RightPanel::Crop);
    run(&mut h, "develop.set", json!({"control": "crop.angle", "value": 2.0}));
    assert!(!drawn(&mut h), "no overlay while cropping");
    assert_eq!(run(&mut h, "osd.state", json!({}))["held"], true);
    run(&mut h, "panel.crop", json!({}));
    h.settle(T);
    // a brush (any armed tool on the photo)
    h.app.ui.right = RightPanel::Masking;
    h.app.ui.tool = "brush".into();
    run(&mut h, "develop.set", json!({"control": "light.exposure", "value": 0.5}));
    assert!(!drawn(&mut h), "no overlay while a brush is armed");
    // put away: the next change shows, once
    h.app.ui.tool.clear();
    h.app.ui.right = RightPanel::Edit;
    run(&mut h, "develop.set", json!({"control": "light.exposure", "value": 0.7}));
    assert!(drawn(&mut h), "a slider change shows again");
    assert_eq!(h.app.widgets.iter().filter(|(w, _)| w == "osd:surface").count(), 1, "one overlay");
    assert_eq!(shown(&mut h)["value"], "+0.70");
}
