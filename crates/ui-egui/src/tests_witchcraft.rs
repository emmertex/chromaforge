//! The Witchcraft grading mode's Primaries palette: the four colourist wheels, the Color | Log
//! variant switch, and that Eboda keeps Lightroom's own panels.

use std::time::Duration;

use serde_json::json;

use crate::headless::Headless;
use crate::state::GradingMode;
use crate::{LccApp, Services};

const T: Duration = Duration::from_secs(20);
const SETTLE: Duration = Duration::from_secs(120);

fn demo() -> Headless {
    let services = Services { png: None, ..Default::default() };
    let app = LccApp::new(lcc_engine::Session::with_demo(), services);
    let mut h = Headless::new(app, [1400.0, 2400.0], 1.0);
    let r = h.request("ui.set", json!({"view": "detail", "right": "edit"}), T);
    assert_eq!(r["ok"], true, "{r}");
    let r = h.request("engine.execute", json!({"command": "view.gradingMode", "params": {"mode": "witchcraft"}}), T);
    assert_eq!(r["ok"], true, "{r}");
    // the Primaries palette, on its own (the fork's Resolve-style panels)
    h.app.ui.solo_section("primaries");
    h.settle(SETTLE);
    h
}

fn has(h: &Headless, id: &str) -> bool {
    h.app.widgets.iter().any(|(w, _)| w == id)
}

fn any_has(h: &Headless, part: &str) -> bool {
    h.app.widgets.iter().any(|(w, _)| w.contains(part))
}

fn run(h: &mut Headless, id: &str, params: serde_json::Value) -> serde_json::Value {
    let r = h.request("engine.execute", json!({"command": id, "params": params}), T);
    assert_eq!(r["ok"], true, "{id}: {r}");
    h.step();
    h.step();
    r["result"].clone()
}

#[test]
fn witchcraft_shows_the_palette_eboda_keeps_lightrooms_panels() {
    let mut h = demo();
    for key in ["lift", "gamma", "gain", "offset"] {
        assert!(has(&h, &format!("wheel:primaries-{key}")), "wheel {key}");
    }
    assert!(has(&h, "button:primariesWheels-log"), "the Color | Log tabs");
    // back in Eboda the palette is away: Lightroom's own panels only
    run(&mut h, "view.gradingMode", json!({"mode": "eboda"}));
    h.step();
    assert!(!has(&h, "wheel:primaries-lift"));
    assert_eq!(h.app.ui.settings.grading_mode, GradingMode::Eboda);
}

#[test]
fn the_log_wheels_are_their_own_set_and_both_render() {
    let mut h = demo();
    // a Color wheel and a Log wheel are separate edits that both hold their values
    run(&mut h, "develop.set", json!({"control": "primaries.lift.sat", "value": 40.0}));
    assert!(!any_has(&h, "primaries.low"), "the Color view has no range sliders");
    let r = h.request("ui.clickWidget", json!({"id": "button:primariesWheels-log"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    assert_eq!(h.app.ui.primaries_tab, "log");
    // the Log view shows its own wheels and the range boundaries
    assert!(has(&h, "wheel:primaries-logWheels.shadow"));
    assert!(any_has(&h, "primaries.low") && any_has(&h, "primaries.high"));
    assert!(!has(&h, "wheel:primaries-lift"), "the Color wheels are in the other view");
    // the Color edit survives the view switch — both sets are live at once
    let id = h.app.session.active().unwrap();
    let p = h.app.session.develop_of(id).unwrap_or_default().primaries;
    assert_eq!(p.lift.sat, 40.0);
    run(&mut h, "develop.set", json!({"control": "primaries.logWheels.shadow.sat", "value": 55.0}));
    let p = h.app.session.develop_of(id).unwrap_or_default().primaries;
    assert_eq!((p.lift.sat, p.log_wheels.shadow.sat), (40.0, 55.0), "two sets, each with its own values");
    // back to the Color view: nothing reset
    let r = h.request("ui.clickWidget", json!({"id": "button:primariesWheels-color"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    assert!(has(&h, "wheel:primaries-lift"));
    let p = h.app.session.develop_of(id).unwrap_or_default().primaries;
    assert_eq!((p.lift.sat, p.log_wheels.shadow.sat), (40.0, 55.0));
}

#[test]
fn the_hsl_curves_hold_points_anywhere() {
    let mut h = demo();
    h.app.ui.solo_section("hsl");
    h.settle(SETTLE);
    assert!(has(&h, "curve:hsl"), "the curves plot");
    run(&mut h, "develop.hslCurves", json!({"channel": "sat", "points": [[0.6, 0.8], [0.3, -0.5]]}));
    h.step();
    let id = h.app.session.active().unwrap();
    let c = &h.app.session.develop_of(id).unwrap_or_default().hsl_curves;
    // stored in the order the curve runs, y kept in −1..1
    assert_eq!(c.sat.len(), 2);
    assert!((c.sat[0].x - 0.3).abs() < 1e-9 && (c.sat[0].y + 0.5).abs() < 1e-9);
    // the panel shows the points of its tab's own curve (the sat one), not the mixer's choice
    let r = h.request("ui.clickWidget", json!({"id": "button:hslCurves-sat"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    assert_eq!(h.app.ui.hsl_curve, "sat");
    assert!(has(&h, "hslPoint:sat-0") && has(&h, "hslPoint:sat-1"));
    let r = h.request("ui.clickWidget", json!({"id": "button:hslCurves-hue"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    assert_eq!(h.app.ui.hsl_curve, "hue");
    assert!(!has(&h, "hslPoint:sat-0"), "the hue tab shows the hue curve's points");
    // each channel is its own curve; clearing one leaves the others
    run(&mut h, "develop.hslCurves", json!({"channel": "hue", "points": [[0.2, 0.4]]}));
    run(&mut h, "develop.hslCurves", json!({"channel": "sat", "points": []}));
    let c = &h.app.session.develop_of(id).unwrap_or_default().hsl_curves;
    assert!(c.sat.is_empty() && c.hue.len() == 1 && c.hue[0].y == 0.4);
}

#[test]
fn the_node_strip_lists_the_stages_and_its_badges_bypass_them() {
    let mut h = demo();
    for id in ["light", "primaries", "hsl", "curve", "mixer", "grading", "effects", "output"] {
        assert!(has(&h, &format!("node:{id}")), "node {id}");
    }
    let r = h.request("ui.clickWidget", json!({"id": "nodeBadge:grading"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    let id = h.app.session.active().unwrap();
    let d = h.app.session.develop_of(id).unwrap_or_default();
    assert!(!d.section_enabled("grading"), "the badge is the section's eye");
    assert!(d.effective().grading.is_neutral(), "a bypassed stage renders neutral");
    let r = h.request("ui.clickWidget", json!({"id": "node:curve"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    assert!(h.app.ui.section_open("curve"), "a node click opens its panel");
}

#[test]
fn the_node_strip_lists_the_photo_masks() {
    let mut h = demo();
    run(&mut h, "mask.add", json!({"kind": "radial", "center": {"x": 0.2, "y": 0.7}}));
    h.step();
    assert!(has(&h, "node:mask1"), "a radial mask is a node");
    let r = h.request("ui.clickWidget", json!({"id": "nodeBadge:mask1"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    let id = h.app.session.active().unwrap();
    let d = h.app.session.develop_of(id).unwrap_or_default();
    assert!(!d.masks.first().is_none_or(|m| m.visible), "the mask's badge hides the mask");
}

#[test]
fn the_wheels_write_the_primaries_controls_and_the_eye_switches_the_palette_off() {
    let mut h = demo();
    run(&mut h, "develop.set", json!({"control": "primaries.lift.hue", "value": 220.0}));
    run(&mut h, "develop.set", json!({"control": "primaries.lift.sat", "value": 60.0}));
    run(&mut h, "develop.set", json!({"control": "primaries.gain.lum", "value": 25.0}));
    let id = h.app.session.active().unwrap();
    let p = h.app.session.develop_of(id).unwrap_or_default().primaries;
    assert_eq!((p.lift.hue, p.lift.sat, p.gain.lum), (220.0, 60.0, 25.0));
    // the section's eye: the palette's settings stop contributing to the render
    run(&mut h, "develop.sectionEnabled", json!({"section": "primaries", "enabled": false}));
    let d = h.app.session.develop_of(id).unwrap_or_default();
    assert!(!d.section_enabled("primaries"));
    assert!(d.effective().primaries.is_neutral(), "an eye-off palette renders neutral");
}
