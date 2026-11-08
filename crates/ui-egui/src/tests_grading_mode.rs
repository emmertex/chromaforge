//! The Edit panel's grading modes (Eboda, the Lightroom Classic panels; Witchcraft, the DaVinci
//! Resolve style palettes): the `view.gradingMode` command, the header's segmented control, and
//! that both modes edit the same photo settings.

use std::time::Duration;

use serde_json::json;

use crate::headless::Headless;
use crate::state::{AppSettings, GradingMode};
use crate::{LccApp, Services};

const T: Duration = Duration::from_secs(20);
const SETTLE: Duration = Duration::from_secs(120);

fn demo() -> Headless {
    let services = Services { png: None, ..Default::default() };
    let app = LccApp::new(lcc_engine::Session::with_demo(), services);
    let mut h = Headless::new(app, [1400.0, 2400.0], 1.0);
    let r = h.request("ui.set", json!({"view": "detail", "right": "edit"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    h
}

fn grade(h: &mut Headless, params: serde_json::Value) -> serde_json::Value {
    let r = h.request("engine.execute", json!({"command": "view.gradingMode", "params": params}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    r["result"].clone()
}

#[test]
fn the_command_cycles_selects_and_rejects_unknown_modes() {
    let mut h = demo();
    assert_eq!(h.app.ui.settings.grading_mode, GradingMode::Eboda);
    let r = grade(&mut h, json!({}));
    assert_eq!(r["mode"], "witchcraft");
    assert_eq!(h.app.ui.settings.grading_mode, GradingMode::Witchcraft);
    let r = grade(&mut h, json!({"mode": "eboda"}));
    assert_eq!(r["mode"], "eboda");
    assert_eq!(h.app.ui.settings.grading_mode, GradingMode::Eboda);
    let r = h.request("engine.execute", json!({"command": "view.gradingMode", "params": {"mode": "nope"}}), T);
    assert_eq!(r["ok"], false, "an unknown mode is an error, not a panic: {r}");
}

#[test]
fn the_header_segmented_control_switches_modes() {
    let mut h = demo();
    let r = h.request("ui.clickWidget", json!({"id": "button:gradingMode-witchcraft"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    assert_eq!(h.app.ui.settings.grading_mode, GradingMode::Witchcraft);
    let r = h.request("ui.clickWidget", json!({"id": "button:gradingMode-eboda"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    assert_eq!(h.app.ui.settings.grading_mode, GradingMode::Eboda);
}

#[test]
fn both_modes_edit_the_same_photo_settings() {
    let mut h = demo();
    grade(&mut h, json!({"mode": "witchcraft"}));
    crate::tests_classic::run(&mut h, "develop.set", json!({"control": "light.exposure", "value": 1.25}));
    let id = h.app.session.active().unwrap();
    let in_witchcraft = h.app.session.develop_of(id).unwrap_or_default();
    // back in Eboda the edit is still there: one recipe, two presentations
    grade(&mut h, json!({"mode": "eboda"}));
    let in_eboda = h.app.session.develop_of(id).unwrap_or_default();
    assert_eq!(in_eboda.light.exposure, in_witchcraft.light.exposure);
    assert!((in_eboda.light.exposure - 1.25).abs() < 1e-9);
}

#[test]
fn grading_mode_round_trips_and_unknown_values_read_as_eboda() {
    let s: AppSettings = serde_json::from_str(r#"{"gradingMode":"witchcraft"}"#).unwrap();
    assert_eq!(s.grading_mode, GradingMode::Witchcraft);
    let s: AppSettings = serde_json::from_str(r#"{"gradingMode":"nope"}"#).unwrap();
    assert_eq!(s.grading_mode, GradingMode::Eboda);
    let j = serde_json::to_value(AppSettings { grading_mode: GradingMode::Witchcraft, ..Default::default() }).unwrap();
    assert_eq!(j["gradingMode"], "witchcraft");
}
