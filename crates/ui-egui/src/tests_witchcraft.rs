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
    assert!(has(&h, "button:primariesVariant-log"), "the Color | Log switch");
    // back in Eboda the palette is away: Lightroom's own panels only
    run(&mut h, "view.gradingMode", json!({"mode": "eboda"}));
    h.step();
    assert!(!has(&h, "wheel:primaries-lift"));
    assert_eq!(h.app.ui.settings.grading_mode, GradingMode::Eboda);
}

#[test]
fn the_log_variant_shows_the_range_boundaries() {
    let mut h = demo();
    assert!(!any_has(&h, "primaries.low"), "the Color variant has no range sliders");
    let r = run(&mut h, "develop.primaries", json!({"log": true}));
    assert_eq!(r["log"], true);
    let id = h.app.session.active().unwrap();
    assert!(h.app.session.develop_of(id).unwrap_or_default().primaries.log);
    // the Log wheels' boundary sliders appear beside the wheels
    assert!(any_has(&h, "primaries.low"));
    assert!(any_has(&h, "primaries.high"));
    let r = run(&mut h, "develop.primaries", json!({}));
    assert_eq!(r["log"], false);
    assert!(!any_has(&h, "primaries.low"));
}

#[test]
fn the_scope_switch_drives_the_histograms_mode() {
    let mut h = demo();
    assert!(has(&h, "button:witchScopes-parade"), "the scope switch leads the panels");
    let r = h.request("ui.clickWidget", json!({"id": "button:witchScopes-parade"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    assert_eq!(h.app.ui.histogram_mode, "parade");
    assert!(h.app.ui.histogram, "the histogram shows for the scope to be seen");
    let r = h.request("ui.clickWidget", json!({"id": "button:witchScopes-vectorscope"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    assert_eq!(h.app.ui.histogram_mode, "vectorscope");
}

#[test]
fn the_hsl_curves_write_the_mixer_bands() {
    let mut h = demo();
    h.app.ui.solo_section("hsl");
    h.settle(SETTLE);
    assert!(has(&h, "curve:hsl"), "the curves plot");
    assert!(has(&h, "button:hslCurves-sat"), "the Hue vs Sat tab");
    let r = h.request("ui.clickWidget", json!({"id": "button:hslCurves-sat"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    assert_eq!(h.app.ui.mixer_group, "sat");
    run(&mut h, "develop.set", json!({"control": "mixer.blue.sat", "value": -55.0}));
    let id = h.app.session.active().unwrap();
    let bands = h.app.session.develop_of(id).unwrap_or_default().mixer.bands();
    assert_eq!(bands[5].sat, -55.0, "the curves sit on the mixer's own controls");
    // the tabs are the mixer's own choice, so the Color Mixer panel follows
    assert_eq!(h.app.ui.mixer_group, "sat");
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
