//! Headless tests of a mask's Recolor rows (the flyout, its sliders, the swatch and the
//! eyedroppers) in the Masking panel.
#![allow(clippy::indexing_slicing, reason = "test code: clippy.toml allows indexing in tests, but its test check misses cfg(all(test, ...))")]
#![allow(clippy::expect_used, reason = "test code: clippy.toml allows expect in tests, but its test check misses cfg(all(test, ...))")]
#![allow(clippy::unwrap_used, reason = "test code: clippy.toml allows unwrap in tests, but its test check misses cfg(all(test, ...))")]

use std::time::Duration;

use serde_json::json;

use crate::LccApp;
use crate::headless::Headless;

const T: Duration = Duration::from_secs(20);
const SETTLE: Duration = Duration::from_secs(5);

fn masking() -> Headless {
    let services = crate::Services { png: None, ..Default::default() };
    let app = LccApp::new(lcc_engine::Session::with_demo(), services);
    let mut h = Headless::new(app, [1200.0, 2000.0], 1.0);
    let r = h.request("ui.set", json!({"view": "detail", "moduleBar": false}), T);
    assert_eq!(r["ok"], true, "ui.set: {r}");
    let r = h.request("engine.execute", json!({"command": "panel.masking", "params": json!({})}), T);
    assert_eq!(r["ok"], true);
    let r = h.request("engine.execute", json!({"command": "mask.add", "params": json!({"kind": "radial"})}), T);
    assert_eq!(r["ok"], true);
    h.settle(SETTLE);
    h
}

fn exec(h: &mut Headless, command: &str, params: serde_json::Value) -> serde_json::Value {
    let r = h.request("engine.execute", json!({"command": command, "params": params}), T);
    assert_eq!(r["ok"], true, "{command}: {r}");
    r
}

/// A control-channel UI action (`ui.*`), not an engine command.
fn ui_req(h: &mut Headless, method: &str, params: serde_json::Value) {
    let r = h.request(method, params, T);
    assert_eq!(r["ok"], true, "{method}: {r}");
}

fn has(h: &mut Headless, id: &str) -> bool {
    h.request("ui.widgets", json!({"filter": id}), T).to_string().contains(&format!("\"{id}\""))
}

fn adjust(h: &mut Headless) -> lcc_develop::LocalAdjustments {
    let id = h.app.session.active().expect("active photo");
    let d = h.app.session.develop_of(id).unwrap_or_default();
    d.masks.first().map(|m| m.adjust).unwrap_or_default()
}

#[test]
fn the_recolor_rows_show_the_swatch_and_set_the_colour() {
    let mut h = masking();
    assert!(has(&mut h, "flyout:maskRecolor"), "the Recolor row is there");
    assert!(has(&mut h, "maskRecolor:swatch"), "with its swatch");
    // closed by default: the sliders come with the flyout
    assert!(!has(&mut h, "slider:recolor"), "closed: no Amount slider");
    ui_req(&mut h, "ui.clickWidget", json!({"id": "flyout:maskRecolor"}));
    h.settle(SETTLE);
    assert!(has(&mut h, "slider:recolor") && has(&mut h, "slider:recolorRange"), "open: Amount and Range");
    assert!(has(&mut h, "icon:maskRecolorPicker") && has(&mut h, "icon:maskRecolorFromPicker"), "both eyedroppers");
    assert!(has(&mut h, "field:maskRecolorHex") || has(&mut h, "maskRecolorHex"), "the hex field");
    // the commands the rows run
    exec(&mut h, "mask.recolor", json!({"color": "#7a3fb0", "amount": 100}));
    h.settle(SETTLE);
    let a = adjust(&mut h);
    assert_eq!(a.recolor, 100.0);
    assert!(a.recolor_b > a.recolor_g, "purple stored: {a:?}");
    exec(&mut h, "mask.recolorPick", json!({"x": 0.5, "y": 0.2, "target": "from"}));
    h.settle(SETTLE);
    let a = adjust(&mut h);
    assert!(a.recolor_from_r > 0.0 || a.recolor_from_g > 0.0 || a.recolor_from_b > 0.0, "a pick stored: {a:?}");
    // an amount back to 0 clears the swatch's fill but keeps the row
    exec(&mut h, "mask.recolor", json!({"amount": 0}));
    h.settle(SETTLE);
    assert!(has(&mut h, "flyout:maskRecolor"));
    assert_eq!(adjust(&mut h).recolor, 0.0);
}
