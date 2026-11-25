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

/// One session of changes: a slider shows its name and value, a drag updates the one note with its
/// running value, ratings, flags, labels and undo name the change and count the photos, Auto Sync
/// counts every photo reached, and queries and background work never show.
#[test]
fn changes_show_one_note_naming_the_change_its_value_and_the_photos() {
    let mut h = demo();
    open(&mut h, 1);
    run(&mut h, "develop.set", json!({"control": "light.exposure", "value": 0.33}));
    let s = shown(&mut h);
    assert_eq!(s["title"], "Exposure", "{s}");
    assert_eq!(s["value"], "+0.33", "{s}");
    assert_eq!(s["caption"], "", "one photo: no count");
    assert!(s["icon"].is_string(), "the console's icon for Exposure: {s}");
    assert!(widget(&h, "osd:surface").is_some(), "drawn in the window");

    // a drag: live, one note, the running value
    run(&mut h, "develop.beginInteraction", json!({"label": "Exposure"}));
    let mut seqs = Vec::new();
    for i in 1..=5 {
        run(&mut h, "develop.set", json!({"control": "light.exposure", "value": f64::from(i) * 0.2}));
        let s = shown(&mut h);
        assert_eq!(s["title"], "Exposure", "{s}");
        seqs.push(h.app.session.last_change.as_ref().map(|c| (c.seq, c.live)));
    }
    assert!(seqs.iter().all(|s| s.is_some_and(|(_, live)| live)), "live while dragging: {seqs:?}");
    assert_eq!(shown(&mut h)["value"], "+1.00", "the running value");
    run(&mut h, "develop.endInteraction", json!({}));
    assert_eq!(shown(&mut h)["value"], "+1.00");

    // queries and a selection change report nothing
    let before = h.app.session.last_change.as_ref().map(|c| c.seq);
    run(&mut h, "library.info", json!({}));
    run(&mut h, "library.select", json!({"ids": []}));
    assert_eq!(h.app.session.last_change.as_ref().map(|c| c.seq), before, "nothing changed: no report");
    assert!(lcc_engine::change_report::is_background("faces.pump"));

    // ratings, flags, labels and undo name the change and count the photos
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

    // Auto Sync counts every photo the change reached
    open(&mut h, 4);
    h.app.session.auto_sync = true;
    run(&mut h, "develop.set", json!({"control": "light.contrast", "value": 12}));
    let s = shown(&mut h);
    assert_eq!((s["title"].as_str(), s["caption"].as_str()), (Some("Contrast"), Some("on 4 photos")), "{s}");
}

/// The overlay's settings are commands (hostile values refused or clamped), and it draws inside
/// the canvas at every size and position.
#[test]
fn settings_are_checked_and_the_overlay_draws_at_every_size_and_position() {
    let mut h = demo();
    let r = run(&mut h, "osd.settings", json!({"size": 1.5, "opacity": 0.5, "displayMs": 2500, "position": "upHigh", "background": "#102030"}));
    assert_eq!(r, json!({"mode": "always", "size": 1.5, "opacity": 0.5, "displayMs": 2500, "position": "upHigh", "background": "#102030"}));
    let r = run(&mut h, "osd.settings", json!({"size": 1e9, "opacity": -3, "displayMs": 1e12}));
    assert_eq!((r["size"].as_f64(), r["opacity"].as_f64(), r["displayMs"].as_u64()), (Some(2.0), Some(0.1), Some(10_000)));
    for bad in [json!({"mode": "loud"}), json!({"position": "left"}), json!({"background": "red"}), json!({"size": "big"})] {
        let r = h.request("engine.execute", json!({"command": "osd.settings", "params": bad}), T);
        assert_eq!(r["ok"], false, "{bad}: {r}");
    }

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

#[test]
fn a_key_shows_one_note_not_a_toast_as_well_and_the_mode_decides_what_shows() {
    let mut h = demo();
    open(&mut h, 1);
    let press = |h: &mut Headless, key: &str, cmd: bool| {
        h.app.ui.toasts.clear();
        let r = h.request("ui.key", json!({"key": key, "cmd": cmd}), T);
        assert_eq!(r["ok"], true, "{key}: {r}");
        h.step();
        h.step();
    };
    // Always (the default): the overlay names the change and no toast repeats it
    press(&mut h, "p", false);
    assert_eq!(shown(&mut h)["title"], "Picked");
    assert!(h.app.ui.toast().is_none(), "one note on P, not two: {:?}", h.app.ui.toasts);
    press(&mut h, "x", false);
    assert_eq!(shown(&mut h)["title"], "Rejected");
    assert!(h.app.ui.toast().is_none(), "{:?}", h.app.ui.toasts);
    press(&mut h, "z", true);
    assert!(shown(&mut h)["title"].as_str().is_some_and(|t| t.starts_with("Undo")));
    assert!(h.app.ui.toast().is_none(), "{:?}", h.app.ui.toasts);
    // with the overlay for the control surface only, a key shows nothing
    run(&mut h, "osd.settings", json!({"mode": "surface"}));
    press(&mut h, "p", false);
    assert!(shown(&mut h).is_null());
    assert!(h.app.ui.toast().is_none(), "and no toast either: {:?}", h.app.ui.toasts);

    // the mode decides what shows: osd.mode steps Control surface only → Always, and Off shows nothing
    let ids = [h.app.session.active().map_or(0, |p| p.0)];
    run(&mut h, "photo.rate", json!({"rating": 4, "ids": ids}));
    assert!(shown(&mut h).is_null(), "control surface only: a change made in the app shows nothing");
    run(&mut h, "osd.mode", json!({}));
    assert_eq!(run(&mut h, "osd.state", json!({}))["settings"]["mode"], "always", "osd.mode steps to the next mode");
    run(&mut h, "photo.rate", json!({"rating": 2, "ids": ids}));
    assert_eq!(shown(&mut h)["title"], "2 stars");
    let r = run(&mut h, "osd.settings", json!({"mode": "off"}));
    assert_eq!(r["mode"], "off");
    run(&mut h, "photo.rate", json!({"rating": 5, "ids": ids}));
    assert!(shown(&mut h).is_null(), "off");
    assert!(!h.app.surface.settings().osd.enabled, "Off also switches the console's readout off");
}

/// A script can see what the window is saying: `ui.inspect` lists the toasts still showing with the time each has
/// left, the toast with a button, and the overlay's readout, so a test or an agent can time and compare them.
#[test]
fn inspect_lists_the_toasts_the_button_toast_and_the_overlay() {
    let mut h = demo();
    let inspect = |h: &mut Headless| h.request("ui.inspect", json!({}), T)["result"].clone();
    let i = inspect(&mut h);
    assert_eq!(i["toasts"], json!([]), "nothing showing yet: {}", i["toasts"]);
    assert!(i["toastAction"].is_null() && i["osd"]["shown"].is_null(), "{} {}", i["toastAction"], i["osd"]);

    let ctx = h.view.ctx.clone();
    h.app.toast_for(&ctx, "Saved", 6.0);
    h.app.toast_with_action(&ctx, "Preset updated", "Undo", "edit.undo", json!({}), 10.0);
    let i = inspect(&mut h);
    assert_eq!(i["toasts"][0]["text"], "Saved", "{}", i["toasts"]);
    let left = i["toasts"][0]["secsLeft"].as_f64().unwrap_or(0.0);
    assert!(left > 0.0 && left <= 6.0, "the time left counts down from the 6 s it was given: {left}");
    assert_eq!(i["toastAction"]["button"], "Undo", "{}", i["toastAction"]);
    assert_eq!(i["toastAction"]["command"], "edit.undo");

    open(&mut h, 1);
    run(&mut h, "osd.settings", json!({"mode": "always"}));
    run(&mut h, "develop.set", json!({"control": "light.exposure", "value": 0.33}));
    h.step();
    let i = inspect(&mut h);
    assert_eq!(i["osd"]["shown"]["title"], "Exposure", "the overlay's readout is in the inspect reply: {}", i["osd"]);
    assert_eq!(i["osd"]["settings"]["mode"], "always");
}

/// `ui.toast` fires each kind of message so it can be looked at and timed: a plain one for the time given, an error
/// for the longer error time, and one with a button that runs its command once; bad input is refused.
#[test]
fn ui_toast_fires_a_message_an_error_and_a_message_with_a_button() {
    let mut h = demo();
    let toast = |h: &mut Headless, params: Value| h.request("ui.toast", params, T);
    let inspect = |h: &mut Headless| h.request("ui.inspect", json!({}), T)["result"].clone();

    assert_eq!(toast(&mut h, json!({"text": "Saved"}))["ok"], true);
    assert_eq!(toast(&mut h, json!({"text": "Export failed", "kind": "error"}))["ok"], true);
    let i = inspect(&mut h);
    let left = |text: &str| i["toasts"].as_array().and_then(|t| t.iter().find(|t| t["text"] == text)).and_then(|t| t["secsLeft"].as_f64());
    assert!(left("Saved").is_some_and(|s| s > 0.0 && s <= 1.4), "a plain message stays the default 1.4 s: {}", i["toasts"]);
    assert!(left("Export failed").is_some_and(|s| s > 1.4 && s <= 6.0), "an error stays 6 s: {}", i["toasts"]);

    assert_eq!(toast(&mut h, json!({"text": "Held", "seconds": 30}))["ok"], true);
    assert!(
        inspect(&mut h)["toasts"]
            .as_array()
            .is_some_and(|t| t.iter().any(|t| t["text"] == "Held" && t["secsLeft"].as_f64().is_some_and(|s| s > 20.0))),
        "seconds sets the time"
    );

    assert_eq!(toast(&mut h, json!({"text": "Preset updated", "kind": "action", "command": "edit.undo"}))["ok"], true);
    let i = inspect(&mut h);
    assert_eq!(i["toastAction"]["text"], "Preset updated", "{}", i["toastAction"]);
    assert_eq!(i["toastAction"]["button"], "Undo");
    assert_eq!(i["toastAction"]["command"], "edit.undo");

    assert_eq!(toast(&mut h, json!({"text": "Long", "kind": "action", "command": "edit.undo", "button": "B".repeat(500)}))["ok"], true);
    assert_eq!(inspect(&mut h)["toastAction"]["button"].as_str().map(|b| b.chars().count()), Some(40), "a button label is cut to 40 characters");

    for bad in [
        json!({}),
        json!({"text": ""}),
        json!({"text": "x", "kind": "toast"}),
        json!({"text": "x", "seconds": 0}),
        json!({"text": "x", "seconds": 1e9}),
        json!({"text": "x", "seconds": "3"}),
        json!({"text": "x", "kind": "action"}),
        json!({"text": "x", "kind": 5}),
    ] {
        assert_eq!(toast(&mut h, bad.clone())["ok"], false, "{bad} is refused");
    }
}

/// Most people have never met a control surface, so Settings ▸ Interface says what "Control surface only" is for and
/// where one is set up; the Controls page shows the same overlay settings and leaves the pointer out.
#[test]
fn show_changes_points_to_the_controls_page_from_settings_but_not_on_it() {
    let pointer = |h: &Headless| h.painted_text().iter().any(|t| t.contains("Set one up in Settings ▸ Controls"));
    let mut h = Headless::new(LccApp::new(lcc_engine::Session::with_demo(), Services { png: None, ..Default::default() }), [1400.0, 2400.0], 1.0);
    h.settle(T);
    run(&mut h, "app.settings", json!({"page": "interface"}));
    h.settle(T);
    // the overlay settings are at the bottom of a long page: scroll the dialog down to them
    h.request("ui.move", json!({"x": 700.0, "y": 1000.0}), T);
    h.request("ui.scroll", json!({"dx": 0.0, "dy": -4000.0}), T);
    h.settle(T);
    assert!(h.painted_text().iter().any(|t| t == "On-Screen Overlay"), "the overlay settings are on the page: {:?}", h.painted_text());
    assert!(pointer(&h), "Interface says where a control surface is set up");

    run(&mut h, "app.settings", json!({"page": "controls"}));
    h.app.surface.editor.section = "readout".into();
    h.settle(T);
    assert!(h.painted_text().iter().any(|t| t == "On-Screen Overlay"), "the Controls page has the overlay settings too: {:?}", h.painted_text());
    assert!(!pointer(&h), "but doesn't point to itself");
    assert!(
        h.painted_text().iter().any(|t| t == "Flash a message for each change: what changed, and how many photos were affected."),
        "it keeps the note: {:?}",
        h.painted_text()
    );
}

/// A change the overlay reports is never a toast as well, in any mode: the overlay says it when it is on for the
/// window and nothing does otherwise. Each command runs on its own through `LccApp::run`, the way the window runs
/// it, with nothing else showing. (Toasts a panel adds after its own call are covered where a test drives that panel.)
#[test]
fn a_change_the_overlay_reports_is_never_also_a_toast() {
    let mut h = demo();
    let ids = open(&mut h, 3);
    run(&mut h, "osd.settings", json!({"mode": "always"}));
    let preset = run(&mut h, "preset.create", json!({"name": "Survey", "group": "User Presets"}))["id"].clone();
    let single = json!({"ids": [ids[0]]});
    let several = json!({"ids": ids});
    let cases: Vec<(&str, Value)> = vec![
        ("photo.rate", json!({"rating": 4, "ids": [ids[0]]})),
        ("photo.rate", json!({"rating": 2, "ids": ids})),
        ("photo.increaseRating", single.clone()),
        ("photo.decreaseRating", single.clone()),
        ("photo.label", json!({"label": "red", "ids": [ids[0]]})),
        ("photo.label", json!({"label": "blue", "ids": ids})),
        ("photo.clearLabel", several.clone()),
        ("photo.pick", single.clone()),
        ("photo.reject", several.clone()),
        ("photo.setMeta", json!({"keywords": ["survey"], "ids": [ids[0]]})),
        ("photo.setMeta", json!({"removeKeywords": ["survey"], "ids": [ids[0]]})),
        ("preset.apply", json!({"id": preset, "amount": 100, "ids": ids})),
        ("map.geotag", json!({"ids": [ids[0]], "lat": 48.1, "lon": 11.5})),
        ("photo.unflag", single.clone()),
        ("photo.cycleFlag", single.clone()),
    ];
    for (command, params) in &cases {
        h.app.ui.toasts.clear();
        h.app.run(command, params.clone()).unwrap_or_else(|e| panic!("{command} {params}: {e}"));
        let s = shown(&mut h);
        assert!(s.is_object(), "{command} {params}: the overlay should report it");
        assert!(h.app.ui.toast().is_none(), "{command} {params}: the overlay says {s}, so no toast: {:?}", h.app.ui.toasts);
    }
    // and in the other modes no toast takes its place: the overlay's mode is the only say
    for mode in ["surface", "off"] {
        run(&mut h, "osd.settings", json!({"mode": mode}));
        for (command, params) in &cases {
            h.app.ui.toasts.clear();
            h.app.run(command, params.clone()).unwrap_or_else(|e| panic!("{command} {params}: {e}"));
            assert!(h.app.ui.toast().is_none(), "{mode} {command} {params}: {:?}", h.app.ui.toasts);
        }
    }
}

/// A scratch folder that goes away with the test, however it ends.
struct Scratch(std::path::PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn scratch(name: &str) -> Scratch {
    let d = std::env::temp_dir().join(format!("lc-ui-osd-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    Scratch(d)
}

fn gradient_jpeg(path: &std::path::Path, shade: u8) {
    let (w, h) = (256usize, 192usize);
    let img = lcc_raster::Rgba8 {
        width: w,
        height: h,
        data: (0..w * h).map(|i| [((i % w) * 255 / w) as u8, ((i / w) * 255 / h) as u8, shade, 255]).collect(),
    };
    let bytes = lcc_codecs::encode_jpeg(&lcc_codecs::EncodeImage::rgba8(&img), 90, Default::default(), &Default::default()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

/// Moving photos or folders on disk, and pointing a folder at its new place, are reported by the overlay, which is
/// why the panels that run them (`panels/left.rs`, `grid.rs`) toast only a move that changed nothing. This checks
/// the reporting; the panels' own toasts are not driven here.
#[test]
fn the_overlay_reports_moves_and_relinks_on_disk() {
    let src = scratch("files");
    for (d, shade) in [("a", 40u8), ("b", 140), ("c", 220)] {
        std::fs::create_dir_all(src.0.join(d)).unwrap();
        gradient_jpeg(&src.0.join(d).join("x.jpg"), shade);
        gradient_jpeg(&src.0.join(d).join("y.jpg"), shade.wrapping_add(7));
    }
    let services = Services { png: None, ..Default::default() };
    let mut h = Headless::new(LccApp::new(lcc_engine::Session::with_demo().with_fs(), services), [1300.0, 900.0], 1.0);
    let r = h.request(
        "engine.execute",
        json!({"command": "library.import", "params": {"paths": [src.0.to_string_lossy()], "mode": "add", "importDuplicates": true}}),
        T,
    );
    let all: Vec<u64> =
        r["result"]["imported"].as_array().map(|a| a.iter().filter_map(|v| v.as_u64().or_else(|| v["id"].as_u64())).collect()).unwrap_or_default();
    assert_eq!(all.len(), 6, "{r}");
    h.settle(T);
    run(&mut h, "osd.settings", json!({"mode": "always"}));
    let dest = src.0.join("dest");
    std::fs::create_dir_all(&dest).unwrap();
    let moved = std::env::temp_dir().join(format!("lc-ui-osd-moved-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&moved);
    std::fs::rename(src.0.join("c"), &moved).unwrap();
    let _gone = Scratch(moved.clone());
    let steps = [
        ("photo.moveToFolder", json!({"folder": dest.to_string_lossy(), "ids": &all[..2]})),
        ("folder.move", json!({"path": src.0.join("b").to_string_lossy(), "into": dest.to_string_lossy()})),
        ("folder.relocate", json!({"path": src.0.join("c").to_string_lossy(), "to": moved.to_string_lossy()})),
    ];
    for (command, params) in steps {
        h.app.ui.toasts.clear();
        h.app.run(command, params.clone()).unwrap_or_else(|e| panic!("{command}: {e}"));
        let s = shown(&mut h);
        assert!(s.is_object(), "{command}: the overlay should report it");
        assert!(h.app.ui.toast().is_none(), "{command}: the overlay says {s}, so no toast: {:?}", h.app.ui.toasts);
    }
}
