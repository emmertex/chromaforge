//! Ported from OneSlips 1138a5edd: Add / Subtract / Intersect offer every kind of mask Create New Mask does, in the
//! same order (Background, People, Remembered and Color were missing), and a People or Remembered pick from there
//! combines with the selected mask.

use std::time::Duration;

use lcc_develop::{MaskOp, MaskShape};
use serde_json::json;

use crate::headless::Headless;
use crate::{LccApp, Services};

const T: Duration = Duration::from_secs(20);

fn masking() -> Headless {
    let app = LccApp::new(lcc_engine::Session::with_demo(), Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [1200.0, 1000.0], 1.0);
    let r = h.request("ui.set", json!({"view": "detail", "moduleBar": false}), T);
    assert_eq!(r["ok"], true, "{r}");
    let r = h.request("engine.execute", json!({"command": "panel.masking"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h
}

fn exec(h: &mut Headless, command: &str, params: serde_json::Value) -> serde_json::Value {
    let r = h.request("engine.execute", json!({"command": command, "params": params}), T);
    assert_eq!(r["ok"], true, "{command}: {r}");
    r["result"].clone()
}

fn develop(h: &Headless) -> lcc_develop::DevelopSettings {
    (*h.app.session.develop_of(h.app.session.active().unwrap()).unwrap_or_default()).clone()
}

fn click(h: &mut Headless, id: &str) {
    let r = h.request("ui.clickWidget", json!({"id": id}), T);
    assert_eq!(r["ok"], true, "{id}: {r}");
}

fn widgets(h: &Headless, prefix: &str) -> Vec<String> {
    let mut v: Vec<String> = h.app.widgets.iter().filter(|(id, _)| id.starts_with(prefix)).map(|(id, _)| id.replace(prefix, "")).collect();
    v.dedup();
    v
}

#[test]
fn add_subtract_and_intersect_offer_every_mask_kind() {
    let mut h = masking();
    let tiles: Vec<String> = widgets(&h, "maskNew:");
    assert_eq!(tiles.len(), 12, "{tiles:?}");
    exec(&mut h, "mask.add", json!({"kind": "radial", "center": [0.5, 0.5]}));
    h.step();
    for (button, op) in [("button:maskAddComp", "add"), ("button:maskSubComp", "subtract"), ("button:maskIntComp", "intersect")] {
        click(&mut h, button);
        h.step();
        let menu = widgets(&h, &format!("maskComp:{op}:"));
        // (painting only adds or erases)
        let want: Vec<String> = tiles.iter().filter(|k| op != "intersect" || *k != "brush").cloned().collect();
        assert_eq!(menu, want, "{op}");
        let r = h.request("ui.key", json!({"key": "Escape"}), T);
        assert_eq!(r["ok"], true, "{r}");
        h.step();
    }
    // the user's case: a radial, minus the background
    let before = develop(&h).masks[0].components.len();
    click(&mut h, "button:maskSubComp");
    h.step();
    click(&mut h, "maskComp:subtract:background");
    let m = &develop(&h).masks[0];
    assert_eq!(m.components.len(), before + 1);
    assert!(matches!(m.components.last().map(|c| (&c.shape, c.op)), Some((MaskShape::Background, MaskOp::Subtract))));
    // Remembered from Add opens its picker for the selected mask; the Remembered tile (a new mask) forgets the op
    click(&mut h, "button:maskAddComp");
    h.step();
    click(&mut h, "maskComp:add:concept");
    h.step();
    assert!(h.app.ui.concept_pick);
    assert_eq!(h.app.mask_pick_op.as_deref(), Some("add"));
    click(&mut h, "maskNew:concept");
    h.step();
    assert!(!h.app.ui.concept_pick && h.app.mask_pick_op.is_none());
}
