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
use crate::toast::{self, ToastKind};
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
    assert_eq!(
        r,
        json!({"mode": "always", "inDevelop": true, "elsewhere": true, "size": 1.5, "opacity": 0.5, "displayMs": 2500, "position": "upHigh", "background": "#102030"})
    );
    let r = run(&mut h, "osd.settings", json!({"size": 1e9, "opacity": -3, "displayMs": 1e12}));
    assert_eq!((r["size"].as_f64(), r["opacity"].as_f64(), r["displayMs"].as_u64()), (Some(2.0), Some(0.1), Some(10_000)));
    for bad in [
        json!({"mode": "loud"}),
        json!({"position": "left"}),
        json!({"background": "red"}),
        json!({"size": "big"}),
        json!({"inDevelop": "yes"}),
        json!({"inDevelop": false, "elsewhere": false}),
    ] {
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
    assert_eq!(h.app.surface.settings().osd.mode, lcc_surface::settings::OsdMode::Off, "Off is the mode itself");
}

/// "In Develop" and "Elsewhere" each switch off one kind of change message, and the last one on can't be switched
/// off; with Show changes Off, nothing shows whatever they say.
#[test]
fn each_overlay_area_option_hides_only_its_own_changes() {
    let mut h = demo();
    let ids = open(&mut h, 1);
    let develop = |h: &mut Headless, v: f64| {
        run(h, "develop.set", json!({"control": "light.exposure", "value": v}));
        shown(h)
    };
    let rate = |h: &mut Headless, n: u8| {
        run(h, "photo.rate", json!({"rating": n, "ids": ids}));
        shown(h)
    };
    assert_eq!(develop(&mut h, 0.3)["title"], "Exposure", "both on by default");
    assert_eq!(rate(&mut h, 2)["title"], "2 stars");

    run(&mut h, "osd.settings", json!({"inDevelop": false}));
    let s = develop(&mut h, 0.5);
    assert!(s["title"] != "Exposure", "In Develop off hides a develop change: {s}");
    assert_eq!(rate(&mut h, 4)["title"], "4 stars", "and leaves the rest");

    run(&mut h, "osd.settings", json!({"inDevelop": true, "elsewhere": false}));
    assert_eq!(develop(&mut h, 0.7)["title"], "Exposure");
    let s = rate(&mut h, 1);
    assert!(s["title"] != "1 star", "Elsewhere off hides a rating: {s}");

    let r = h.request("engine.execute", json!({"command": "osd.settings", "params": {"inDevelop": false}}), T);
    assert_eq!(r["ok"], false, "the last one on can't be turned off: {r}");
    let st = run(&mut h, "osd.state", json!({}))["settings"].clone();
    assert_eq!((st["inDevelop"].as_bool(), st["elsewhere"].as_bool()), (Some(true), Some(false)), "{st}");

    run(&mut h, "osd.settings", json!({"mode": "off", "elsewhere": true}));
    assert!(develop(&mut h, 0.9).is_null() && rate(&mut h, 5).is_null(), "Off shows nothing");
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
    h.app.toast_error(&ctx, "Saved");
    h.app.toast_with_action(&ctx, "Preset updated", "Undo", "edit.undo", json!({}));
    let i = inspect(&mut h);
    assert_eq!(i["toasts"][0]["text"], "Saved", "{}", i["toasts"]);
    let left = i["toasts"][0]["secsLeft"].as_f64().unwrap_or(0.0);
    assert!(left > 0.0 && left <= toast::stay_seconds(ToastKind::Error, "Saved", 1.4), "the time left counts down from the policy's time: {left}");
    assert_eq!(i["toasts"][0]["kind"], "error", "the kind is in the reply: {}", i["toasts"]);
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

/// The overlay's settings and the messages' are one page, Settings ▸ Notifications, after Interface; Interface no
/// longer has them and Controls points to the page where they were.
#[test]
fn notifications_has_the_overlay_and_message_settings_and_the_other_pages_point_to_it() {
    let mut h = Headless::new(LccApp::new(lcc_engine::Session::with_demo(), Services { png: None, ..Default::default() }), [1400.0, 2400.0], 1.0);
    h.settle(T);
    let tabs: Vec<&str> = crate::panels::settings::TABS.iter().map(|(id, _)| *id).collect();
    let at = |id: &str| tabs.iter().position(|t| *t == id);
    assert_eq!(at("notifications"), at("interface").map(|i| i + 1), "after Interface: {tabs:?}");

    run(&mut h, "app.settings", json!({"page": "notifications"}));
    h.settle(T);
    let text = h.painted_text();
    for want in ["Image and Library edits", "Messages", "Information", "Confirmation", "Warnings", "Persistent", "In Develop", "Elsewhere"] {
        assert!(text.iter().any(|t| t == want), "{want} is on the page: {text:?}");
    }
    assert!(text.iter().any(|t| t == "Flash a message for each change: what changed, and how many photos were affected."), "{text:?}");
    assert!(!text.iter().any(|t| t.to_lowercase().contains("toast")), "the word toast is not in the UI: {text:?}");

    run(&mut h, "app.settings", json!({"page": "interface"}));
    h.settle(T);
    assert!(!h.painted_text().iter().any(|t| t == "On-Screen Overlay"), "the overlay settings left Interface");

    run(&mut h, "app.settings", json!({"page": "controls"}));
    h.app.surface.editor.section = "readout".into();
    h.settle(T);
    let text = h.painted_text();
    assert!(!text.iter().any(|t| t == "On-Screen Overlay"), "Controls doesn't have them either: {text:?}");
    assert!(text.iter().any(|t| t.contains("Settings ▸ Notifications")), "it points to the page: {text:?}");
}

/// With Show changes Off nothing under it can be used; the last of In Develop and Elsewhere that is on can't be
/// turned off; each test button fires its kind through `notifications.testFire`.
#[test]
fn the_notifications_page_greys_what_cannot_be_used_and_each_test_button_fires_its_kind() {
    let mut h = demo();
    run(&mut h, "app.settings", json!({"page": "notifications"}));
    h.settle(T);
    let click = |h: &mut Headless, id: &str| {
        h.request("ui.clickWidget", json!({"id": id}), T);
        h.settle(T);
    };
    let pill = |h: &mut Headless| run(h, "osd.state", json!({}))["settings"].clone();

    // the last one on can't be turned off: Elsewhere goes, then In Develop stays
    click(&mut h, "check:notifications.elsewhere");
    assert_eq!((pill(&mut h)["elsewhere"].as_bool(), pill(&mut h)["inDevelop"].as_bool()), (Some(false), Some(true)));
    click(&mut h, "check:notifications.inDevelop");
    assert_eq!(pill(&mut h)["inDevelop"], true, "the last one on stays on: {}", pill(&mut h));
    click(&mut h, "check:notifications.elsewhere");
    assert_eq!(pill(&mut h)["elsewhere"], true);

    // Show changes Off: the controls below it do nothing
    click(&mut h, "button:osdMode-0");
    assert_eq!(pill(&mut h)["mode"], "off");
    click(&mut h, "check:notifications.elsewhere");
    assert_eq!(pill(&mut h)["elsewhere"], true, "greyed out while Off");
    click(&mut h, "button:testFire.develop");
    assert!(shown(&mut h).is_null(), "the test button is greyed too");
    let fired = run(&mut h, "notifications.testFire", json!({"kind": "develop"}));
    assert_eq!(fired["shown"], false, "and the command says a test shows nothing while Off: {fired}");
    assert!(shown(&mut h).is_null(), "Off shows nothing, even a test");
    // Control surface only hides the tests of what the app reports too
    click(&mut h, "button:osdMode-1");
    click(&mut h, "button:testFire.elsewhere");
    assert!(shown(&mut h).is_null(), "control surface only: a change in the app, and so its test, shows nothing");
    click(&mut h, "button:osdMode-2");

    // every kind shows itself through the one command, with its time in the text
    for (button, kind) in [("develop", "develop"), ("elsewhere", "elsewhere")] {
        click(&mut h, &format!("button:testFire.{button}"));
        assert!(!shown(&mut h).is_null(), "{kind} shows the pill");
    }
    click(&mut h, "button:testFire.information");
    click(&mut h, "button:testFire.warning");
    click(&mut h, "button:testFire.confirmation");
    click(&mut h, "button:testFire.persistent");
    let i = h.request("ui.inspect", json!({}), T)["result"].clone();
    let kinds: Vec<&str> = i["toasts"].as_array().into_iter().flatten().filter_map(|t| t["kind"].as_str()).collect();
    assert!(kinds.contains(&"message") && kinds.contains(&"error") && kinds.contains(&"sticky"), "{}", i["toasts"]);
    assert!(
        i["toastAction"]["text"].as_str().is_some_and(|t| t.contains("9.8")),
        "a message with a button, its time in the text: {}",
        i["toastAction"]
    );
    let text =
        |kind: &str| i["toasts"].as_array().into_iter().flatten().find(|t| t["kind"] == kind).and_then(|t| t["text"].as_str().map(str::to_string));
    assert!(text("message").is_some_and(|t| t.contains("1.4")), "the information message says its time: {:?}", text("message"));
    assert!(text("error").is_some_and(|t| t.contains("6.0")), "the warning says its time: {:?}", text("error"));

    // a dismissed persistent test message shows again when fired again
    h.request("ui.clickWidget", json!({"id": "button:toastDismiss:test-fire"}), T);
    h.settle(T);
    assert!(h.app.ui.toasts.iter().all(|t| t.key.as_deref() != Some("test-fire")), "dismissed");
    click(&mut h, "button:testFire.persistent");
    assert!(h.app.ui.toasts.iter().any(|t| t.key.as_deref() == Some("test-fire")), "fired again: {:?}", h.app.ui.toasts);
    // and the command is refused for a kind it doesn't know
    let r = h.request("engine.execute", json!({"command": "notifications.testFire", "params": {"kind": "loud"}}), T);
    assert_eq!(r["ok"], false, "{r}");
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
        (
            "map.reverseGeocode",
            json!({"provider": "given", "places": [{"id": ids[0], "city": "Lisbon", "country": "Portugal", "countryCode": "PT"}]}),
        ),
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

/// A failure is said with `toast_error` or `LccApp::report`, never a plain `toast`: given the
/// error itself (a binding named `e`, `err` or `error`, bare or inside a format, on one line or several), it would
/// vanish in 1.4 s instead of staying the error time. The scan reads the UI source, so a new call site can't slip
/// back. (A failure kept in a variable of another name is not seen; `report` and a behaviour test cover that.)
#[test]
fn a_failure_is_never_a_plain_toast() {
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let mut files = Vec::new();
    walk(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut files);
    let mut found = Vec::new();
    for path in files {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name.starts_with("tests") || name == "headless.rs" || name == "tests.rs" {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        // a unit-test module at the end of a file is not product code
        let product = text.split("#[cfg(test)]\nmod ").next().unwrap_or("");
        let lines: Vec<&str> = product.lines().collect();
        for (n, line) in lines.iter().enumerate() {
            let call = ".toast(";
            let Some(at) = line.find(call) else { continue };
            // the call's arguments, read on over up to 6 lines to its closing parenthesis
            let mut text = line[at + call.len()..].to_string();
            for next in lines.iter().skip(n + 1).take(6) {
                text.push(' ');
                text.push_str(next);
            }
            let mut depth = 1;
            let end = text
                .char_indices()
                .find(|(_, c)| {
                    depth += i32::from(*c == '(') - i32::from(*c == ')');
                    depth == 0
                })
                .map_or(text.len(), |(i, _)| i);
            let args = &text[..end];
            if args.split(|c: char| !is_word(c)).any(|w| matches!(w, "e" | "err" | "error")) {
                found.push(format!("{}:{}: {}", path.display(), n + 1, line.trim()));
            }
        }
    }
    assert!(found.is_empty(), "a failure given to a plain toast (use `toast_error` or `report`):\n{}", found.join("\n"));
}

/// `report` says a failure for the error time and a success not at all, and hands back the value.
#[test]
fn report_says_a_failure_for_the_error_time_and_a_success_not_at_all() {
    let mut h = demo();
    let ctx = h.view.ctx.clone();
    assert_eq!(h.app.report(&ctx, Ok::<_, String>(7)), Some(7));
    assert!(h.app.ui.toasts.is_empty(), "{:?}", h.app.ui.toasts);
    assert_eq!(h.app.report(&ctx, Err::<u8, _>("Couldn't do it")), None);
    let r = h.request("ui.inspect", json!({}), T)["result"]["toasts"].clone();
    let left = r[0]["secsLeft"].as_f64().unwrap_or(0.0);
    assert_eq!(r[0]["text"], "Couldn't do it", "{r}");
    assert!(left > 5.0, "a failure stays the error time, not the 1.4 s of a notice: {left}");
}

/// How long a message stays is decided in one place by its kind and its length: a failure outlasts a note, a message
/// with a button outlasts both, and a long text gets time to be read, up to a limit.
#[test]
fn a_message_stays_for_the_time_its_kind_and_length_give_it() {
    let stay_seconds = |kind, text: &str| crate::toast::stay_seconds(kind, text, 1.4);
    let short = "Saved";
    let (note, button, error) =
        (stay_seconds(ToastKind::Message, short), stay_seconds(ToastKind::Action, short), stay_seconds(ToastKind::Error, short));
    assert!(note > 0.0 && note < error && error < button, "a note {note} s, a failure {error} s, a button {button} s");
    let long = "word ".repeat(60);
    for kind in [ToastKind::Message, ToastKind::Action, ToastKind::Error] {
        let (brief, read) = (stay_seconds(kind, short), stay_seconds(kind, &long));
        assert!(read > brief, "{kind:?}: a long text stays longer than a short one ({read} s against {brief} s)");
        assert!(stay_seconds(kind, &"x".repeat(100_000)) <= 30.0, "{kind:?}: nothing stays for ever");
    }
    // the call that raises a message gets the policy's time, whoever the caller is
    let mut h = demo();
    let ctx = h.view.ctx.clone();
    h.app.toast(&ctx, short);
    h.app.toast_error(&ctx, long.clone());
    let now = ctx.input(|i| i.time);
    let until = |h: &Headless, text: &str| h.app.ui.toasts.iter().find(|t| t.text == text).map(|t| t.until - now);
    let near = |got: Option<f64>, want: f64| got.is_some_and(|g| (g - want).abs() < 1e-6);
    assert!(near(until(&h, short), note), "{:?}", h.app.ui.toasts);
    assert!(near(until(&h, &long), stay_seconds(ToastKind::Error, &long)), "{:?}", h.app.ui.toasts);
}

/// Every message is drawn from one look: the kinds share the box (corner, padding, least height, text size), and an
/// error is told apart by its colour alone.
#[test]
fn every_message_has_one_look_and_an_error_a_different_colour() {
    let look = |t: &_, kind, fade| crate::toast::look(t, kind, fade, &crate::toast::MessageSettings::default());
    let h = demo();
    let t = crate::theme::Tokens::get(&h.view.ctx);
    let (message, action, error) = (look(&t, ToastKind::Message, 1.0), look(&t, ToastKind::Action, 1.0), look(&t, ToastKind::Error, 1.0));
    for other in [action, error] {
        assert_eq!(
            (message.radius, message.pad_x, message.pad_y, message.min_height, message.font),
            (other.radius, other.pad_x, other.pad_y, other.min_height, other.font)
        );
    }
    assert_eq!(message.fill, action.fill, "a message and a message with a button are one box");
    assert_ne!(message.fill, error.fill, "an error is told apart");
    assert_ne!(message.stroke, error.stroke, "an error has its own edge");
    assert_eq!(look(&t, ToastKind::Error, 0.0).fill.a(), 0, "a faded message is clear");
    // and the box is the text's size plus the padding, never under the least height
    let size = message.size(egui::vec2(100.0, 10.0));
    assert_eq!((size.x, size.y), (100.0 + 2.0 * message.pad_x, message.min_height));
}

/// The boxes on screen come from `toast::look`, whichever way they are drawn: a message is the least height for one
/// line, and the box with a button is its button's height plus the same padding, centred like the others.
#[test]
fn the_drawn_boxes_of_a_message_and_a_message_with_a_button_follow_the_one_look() {
    let mut h = demo();
    let ctx = h.view.ctx.clone();
    h.app.toast(&ctx, "Saved");
    h.app.toast_error(&ctx, "Failed");
    h.app.toast_with_action(&ctx, "Preset updated", "Undo", "edit.undo", json!({}));
    h.step();
    let rect = |h: &mut Headless, id: &str| {
        let w = h.request("ui.widgets", json!({"filter": id}), T)["result"].clone();
        let r = w.as_array().and_then(|w| w.iter().find(|w| w["id"] == id)).map(|w| w["rect"].clone()).unwrap_or_default();
        let v: Vec<f32> = r.as_array().into_iter().flatten().filter_map(|n| n.as_f64()).map(|n| n as f32).collect();
        (v.len() == 4).then(|| egui::Rect::from_min_size(egui::pos2(v[0], v[1]), egui::vec2(v[2], v[3])))
    };
    let t = crate::theme::Tokens::get(&ctx);
    let look = crate::toast::look(&t, ToastKind::Message, 1.0, &Default::default());
    let (message, error, action, button) =
        (rect(&mut h, "toast:message"), rect(&mut h, "toast:error"), rect(&mut h, "toast:action"), rect(&mut h, "button:toastAction"));
    let (Some(message), Some(error), Some(action), Some(button)) = (message, error, action, button) else {
        panic!("every box is drawn and listed: {:?} {:?} {:?} {:?}", message, error, action, button);
    };
    assert!((message.height() - look.min_height).abs() < 0.6, "a one-line message is the least height: {message:?}");
    assert!((error.height() - message.height()).abs() < 0.6, "an error box is the message's size: {error:?}");
    let want = (button.height() + 2.0 * look.pad_y).max(look.min_height);
    assert!((action.height() - want).abs() < 1.5, "the button's box is its row plus the look's padding: {action:?} {button:?}");
    let centre = h.app.canvas_rect.expect("a canvas").center().x;
    for r in [message, error, action] {
        assert!((r.center().x - centre).abs() < 1.5, "centred on the canvas: {r:?} {centre}");
    }
}

/// A sticky message is raised and cleared by key through `ui.toast`, shows a dismiss button, has no end, and stays
/// through a long idle time.
#[test]
fn a_sticky_message_stays_until_dismissed_or_cleared() {
    let mut h = demo();
    let toast = |h: &mut Headless, params: Value| h.request("ui.toast", params, T);
    let sticky = |h: &mut Headless| -> Vec<Value> {
        let i = h.request("ui.inspect", json!({}), T)["result"]["toasts"].clone();
        i.as_array().into_iter().flatten().filter(|t| t["kind"] == "sticky").cloned().collect()
    };

    assert_eq!(toast(&mut h, json!({"text": "Not saved", "kind": "sticky", "key": "save-failed"}))["ok"], true);
    assert_eq!(toast(&mut h, json!({"text": "Not saved (2)", "kind": "sticky", "key": "save-failed"}))["ok"], true);
    h.step();
    let now = sticky(&mut h);
    assert_eq!(now.len(), 1, "the same key stays one message: {now:?}");
    assert_eq!((now[0]["key"].as_str(), now[0]["text"].as_str()), (Some("save-failed"), Some("Not saved (2)")));
    assert!(now[0].get("secsLeft").is_none(), "no end time: {now:?}");
    assert!(widget(&h, "button:toastDismiss:save-failed").is_some(), "a dismiss button");

    // far longer than any message stays (the longest is 20 s; a frame is 1/60 s)
    for _ in 0..1500 {
        h.step();
    }
    assert_eq!(sticky(&mut h).len(), 1, "a sticky message stays through a long idle time");

    // the code that raised it clears the key
    assert_eq!(toast(&mut h, json!({"clear": true, "key": "save-failed"}))["ok"], true);
    h.step();
    assert!(sticky(&mut h).is_empty());

    // the person dismisses it with its button
    assert_eq!(toast(&mut h, json!({"text": "Waiting", "kind": "sticky", "key": "sidecars-waiting"}))["ok"], true);
    h.step();
    h.step();
    assert_eq!(h.request("ui.clickWidget", json!({"id": "button:toastDismiss:sidecars-waiting"}), T)["ok"], true);
    h.step();
    h.step();
    assert!(sticky(&mut h).is_empty(), "the button removes it");

    for bad in [json!({"text": "x", "kind": "sticky"}), json!({"clear": true}), json!({"text": "x", "kind": "sticky", "key": ""})] {
        assert_eq!(toast(&mut h, bad.clone())["ok"], false, "{bad} is refused");
    }
}

/// The Information time is the one setting for how long messages stay: a message with a button stays 7 times as
/// long, a warning 4 times and at least 6 s, none more than 30 s; a sticky one never ends.
#[test]
fn message_times_follow_the_information_time_by_fixed_multiples() {
    let at = |kind, ms: f64| toast::stay_seconds(kind, "Saved", ms / 1000.0);
    assert_eq!((at(ToastKind::Message, 2000.0), at(ToastKind::Action, 2000.0), at(ToastKind::Error, 2000.0)), (2.0, 14.0, 8.0));
    assert_eq!(at(ToastKind::Error, 500.0), 6.0, "a warning stays at least 6 s");
    assert_eq!((at(ToastKind::Action, 10_000.0), at(ToastKind::Error, 10_000.0)), (30.0, 30.0), "capped at 30 s");
    assert!(at(ToastKind::Sticky, 2000.0).is_infinite());

    // and a real message gets it, whoever raised it
    let mut h = demo();
    let r = run(&mut h, "messages.settings", json!({"displayMs": 3000}));
    assert_eq!(r["displayMs"], 3000);
    let ctx = h.view.ctx.clone();
    h.app.toast(&ctx, "Saved");
    h.app.toast_error(&ctx, "Failed");
    h.app.toast_with_action(&ctx, "Done", "Undo", "edit.undo", json!({}));
    let now = ctx.input(|i| i.time);
    let left = |text: &str| h.app.ui.toasts.iter().find(|t| t.text == text).map(|t| t.until - now);
    let near = |got: Option<f64>, want: f64| got.is_some_and(|g| (g - want).abs() < 1e-6);
    assert!(near(left("Saved"), 3.0) && near(left("Failed"), 12.0), "{:?}", h.app.ui.toasts);
    let action = h.app.ui.toast_action.as_ref().map(|a| a.until - now);
    assert!(near(action, 21.0), "a message with a button stays 7 times the Information time: {action:?}");
}

/// The messages' settings are a command (hostile values refused or clamped), are saved in `ui.json`, and a file from
/// before them, or with nonsense in them, reads as the defaults.
#[test]
fn message_settings_are_checked_saved_with_the_ui_and_read_leniently() {
    let mut h = demo();
    let r = run(&mut h, "messages.settings", json!({"size": 1.5, "opacity": 0.5, "displayMs": 2500, "position": "upHigh", "background": "#102030"}));
    assert_eq!(r, json!({"size": 1.5, "opacity": 0.5, "displayMs": 2500, "position": "upHigh", "background": "#102030"}));
    let r = run(&mut h, "messages.settings", json!({"size": 1e9, "opacity": -3, "displayMs": 1e12}));
    assert_eq!((r["size"].as_f64(), r["opacity"].as_f64(), r["displayMs"].as_u64()), (Some(2.0), Some(0.1), Some(10_000)));
    for bad in [json!({"position": "left"}), json!({"background": "red"}), json!({"size": "big"}), json!({"displayMs": null})] {
        let r = h.request("engine.execute", json!({"command": "messages.settings", "params": bad}), T);
        assert_eq!(r["ok"], false, "{bad}: {r}");
    }
    // saved with the other app settings, not with the control surfaces
    let saved = serde_json::to_value(&h.app.ui.settings).unwrap();
    assert_eq!(saved["messages"]["displayMs"], 10_000, "{}", saved["messages"]);
    assert!(h.app.surface.settings().osd.display_ms != 10_000, "the pill's own time is untouched");
    let back: crate::state::AppSettings = serde_json::from_value(saved).unwrap();
    assert_eq!(back.messages, h.app.ui.settings.messages);
    // a file from before the setting, and one with nonsense in it
    let old: crate::state::AppSettings = serde_json::from_value(json!({})).unwrap();
    assert_eq!(old.messages, toast::MessageSettings::default());
    let odd: crate::state::AppSettings =
        serde_json::from_value(json!({"messages": {"size": 99, "opacity": "no", "displayMs": 1, "background": "zzz"}})).unwrap();
    assert_eq!(odd.messages, toast::MessageSettings::default(), "a value that isn't one of its own is the default");
    let odd: crate::state::AppSettings = serde_json::from_value(json!({"messages": {"size": 99, "displayMs": 1, "background": "zzz"}})).unwrap();
    assert_eq!((odd.messages.size, odd.messages.display_ms, odd.messages.background.as_str()), (2.0, 500, "#000000"), "out of range is clamped");
}

/// The page's one struct reads and writes both halves, each where it is saved.
#[test]
fn the_notification_settings_struct_writes_the_pill_and_the_messages_to_their_own_files() {
    let mut h = demo();
    let mut all = crate::notifications::NotificationSettings::read(&mut h.app);
    all.pill.size = 1.75;
    all.messages.size = 0.75;
    all.write(&mut h.app);
    assert_eq!(h.app.surface.settings().osd.size, 1.75, "the pill's, with the control surfaces");
    assert_eq!(h.app.ui.settings.messages.size, 0.75, "the messages', with the UI");
    assert_ne!(h.app.surface.settings().osd.size, h.app.ui.settings.messages.size);
    // a half-typed colour stays in its field without being applied
    let mut all = crate::notifications::NotificationSettings::read(&mut h.app);
    all.messages.background = "#12".into();
    all.pill.background = "#34".into();
    all.write(&mut h.app);
    assert_eq!(h.app.ui.settings.messages.background, "#12");
    assert_eq!(h.app.surface.settings().osd.background, "#34");
}

/// Messages are drawn as the settings say: size scales the box and its text, opacity its fill, the background colour
/// a note's box (a warning keeps its red), and Up puts the stack at the top.
#[test]
fn messages_are_drawn_as_their_settings_say() {
    let h = demo();
    let t = crate::theme::Tokens::get(&h.view.ctx);
    let base = toast::MessageSettings::default();
    let big = toast::MessageSettings { size: 2.0, opacity: 0.4, background: "#102030".into(), ..base.clone() };
    let (a, b) = (toast::look(&t, ToastKind::Message, 1.0, &base), toast::look(&t, ToastKind::Message, 1.0, &big));
    assert_eq!((b.font, b.pad_x, b.min_height), (a.font * 2.0, a.pad_x * 2.0, a.min_height * 2.0), "twice the size");
    assert!(b.fill.a() < a.fill.a(), "less opaque");
    let [r, g, bl, _] = b.fill.to_srgba_unmultiplied();
    assert!(r.abs_diff(0x10) <= 2 && g.abs_diff(0x20) <= 2 && bl.abs_diff(0x30) <= 2, "the chosen colour: {r} {g} {bl}");
    let (e0, e1) = (toast::look(&t, ToastKind::Error, 1.0, &base), toast::look(&t, ToastKind::Error, 1.0, &big));
    let (red0, red1) = (e0.fill.to_srgba_unmultiplied(), e1.fill.to_srgba_unmultiplied());
    assert!(red0.iter().zip(red1).take(3).all(|(a, b)| a.abs_diff(b) <= 2), "a warning keeps its red whatever the background: {red0:?} {red1:?}");
    assert!(base.edge_gap() < big.edge_gap());
    assert!(!base.from_top(false) && base.from_top(true), "a slideshow puts them at the top");
    assert!(toast::MessageSettings { position: lcc_surface::settings::OsdPosition::Top, ..base }.from_top(false));
}
