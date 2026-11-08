//! Ported from OneSlips 0c58de7ad (MASK-TOOLS-OVERLAY, the parts engine PR 1 left out): in the Compact layout the
//! mask overlay has a switch with its own icon, lit while the overlay is on to stay, and the photo says "Overlay on ·
//! O" then; picking how the overlay looks never switches it on for good.

use std::time::Duration;

use serde_json::json;

use crate::headless::Headless;
use crate::{LccApp, Services};

const T: Duration = Duration::from_secs(20);

fn masking(compact: bool) -> Headless {
    let app = LccApp::new(lcc_engine::Session::with_demo(), Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [1200.0, 1000.0], 1.0);
    let r = h.request("ui.set", json!({"view": "detail", "moduleBar": false}), T);
    assert_eq!(r["ok"], true, "{r}");
    exec(&mut h, "view.compactLayout", json!({"on": compact}));
    exec(&mut h, "panel.masking", json!({}));
    exec(&mut h, "mask.add", json!({"kind": "radial", "center": [0.5, 0.5], "rx": 0.2, "ry": 0.2}));
    // (a new mask shows the overlay; each test starts with it off)
    exec(&mut h, "view.maskOverlay", json!({"show": false}));
    h.step();
    h.step();
    h
}

fn exec(h: &mut Headless, command: &str, params: serde_json::Value) -> serde_json::Value {
    let r = h.request("engine.execute", json!({"command": command, "params": params}), T);
    assert_eq!(r["ok"], true, "{command}: {r}");
    r["result"].clone()
}

fn has(h: &Headless, id: &str) -> bool {
    h.app.widgets.iter().any(|(w, _)| w == id)
}

/// The overlay switched on with O: the photo says so; while a slider adjusts it steps aside with the overlay, and
/// comes back after. Classic draws no such note.
#[test]
fn compact_overlay_on_is_named_on_the_photo_and_steps_aside_while_adjusting() {
    let mut h = masking(true);
    assert!(!has(&h, "canvas:overlayOn"), "the overlay isn't on to stay yet");
    exec(&mut h, "view.maskOverlay", json!({"show": true}));
    h.step();
    assert!(has(&h, "canvas:overlayOn"), "the canvas says the overlay is on (O)");
    h.app.ui.dragging_control = Some("light.exposure".into());
    h.step();
    assert!(!has(&h, "canvas:overlayOn"), "no note while a slider adjusts");
    h.app.ui.dragging_control = None;
    h.step();
    assert!(has(&h, "canvas:overlayOn"), "back once the slider is let go");

    let mut classic = masking(false);
    exec(&mut classic, "view.maskOverlay", json!({"show": true}));
    classic.step();
    assert!(!has(&classic, "canvas:overlayOn"), "Classic is as it was");
}

/// The Compact switch has its own icon button; a click turns the overlay on to stay and another off. Classic keeps its
/// checkbox.
#[test]
fn compact_overlay_switch_is_a_button_of_its_own() {
    let mut h = masking(true);
    assert!(has(&h, "icon:maskOverlayOn"), "the overlay's own button");
    assert!(!h.app.ui.mask_overlay);
    let r = h.request("ui.clickWidget", json!({"id": "icon:maskOverlayOn"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    assert!(h.app.ui.mask_overlay, "on to stay");
    let r = h.request("ui.clickWidget", json!({"id": "check:maskOverlay"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    assert!(!h.app.ui.mask_overlay, "the same switch turns it off");
    let classic = masking(false);
    assert!(!has(&classic, "icon:maskOverlayOn") && has(&classic, "check:maskOverlay"), "Classic keeps its checkbox");
}

/// Choosing how the overlay looks (the overlay menu) leaves it as it was, which is how it got stuck on; ⇧O, cycling
/// the look from the keyboard, still shows it.
#[test]
fn the_overlay_look_menu_does_not_latch_the_overlay_on() {
    let mut h = masking(false);
    assert!(!h.app.ui.mask_overlay);
    exec(&mut h, "view.maskOverlayMode", json!({"mode": "whiteOnBlack", "show": false}));
    assert!(!h.app.ui.mask_overlay, "picking a look leaves the overlay as it was");
    exec(&mut h, "view.maskOverlayMode", json!({}));
    assert!(h.app.ui.mask_overlay, "⇧O cycles the look and shows it");
}
