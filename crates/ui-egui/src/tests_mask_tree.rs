//! The Masks list as a tree: each mask's components hang under it as
//! branches, with how each one combines (+ − ∩) in a column of its own.

use std::time::Duration;

use lcc_develop::MaskOp;
use serde_json::json;

use crate::headless::Headless;
use crate::{LccApp, Services};

const T: Duration = Duration::from_secs(20);
const SETTLE: Duration = Duration::from_secs(120);

fn masking() -> Headless {
    let services = Services { png: None, ..Default::default() };
    let app = LccApp::new(lcc_engine::Session::with_demo(), services);
    let mut h = Headless::new(app, [1200.0, 1000.0], 1.0);
    let r = h.request("ui.set", json!({"view": "detail", "moduleBar": false}), T);
    assert_eq!(r["ok"], true, "{r}");
    let r = h.request("engine.execute", json!({"command": "panel.masking"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    h
}

fn exec(h: &mut Headless, command: &str, params: serde_json::Value) -> serde_json::Value {
    let r = h.request("engine.execute", json!({"command": command, "params": params}), T);
    assert_eq!(r["ok"], true, "{command}: {r}");
    h.settle(SETTLE);
    r["result"].clone()
}

fn masks(h: &Headless) -> Vec<lcc_develop::Mask> {
    let id = h.app.session.active().expect("a photo");
    h.app.session.develop_of(id).expect("settings").masks.clone()
}

fn count(h: &Headless, id: &str) -> usize {
    h.app.widgets.iter().filter(|(w, _)| w == id).count()
}

#[test]
fn the_first_component_starts_the_mask_and_the_others_show_how_they_combine() {
    use crate::panels::masking::component_op_sign;
    assert_eq!(component_op_sign(0, MaskOp::Subtract), "", "the first starts the mask");
    assert_eq!(component_op_sign(1, MaskOp::Add), "+");
    assert_eq!(component_op_sign(2, MaskOp::Subtract), "−");
    assert_eq!(component_op_sign(3, MaskOp::Intersect), "∩");
}

#[test]
fn components_hang_under_the_mask_with_their_operation_in_a_column() {
    let mut h = masking();
    exec(&mut h, "mask.add", json!({"kind": "radial", "center": [0.5, 0.5]}));
    exec(&mut h, "mask.addComponent", json!({"op": "subtract", "kind": "linear", "start": [0.5, 0.0], "end": [0.5, 0.4]}));
    h.step();
    assert_eq!(masks(&h)[0].components.len(), 2);
    let at = |h: &Headless, id: &str| h.app.widgets.iter().find(|(w, _)| w == id).map(|(_, r)| *r).unwrap_or_else(|| panic!("no {id}"));
    assert_eq!(count(&h, "componentOp:0"), 0, "the first component has no sign");
    let op = at(&h, "componentOp:1");
    let row = at(&h, "component:1");
    let thumb = at(&h, "componentIcon:1");
    assert!(row.contains_rect(op), "the sign is on its row");
    assert!(op.right() <= thumb.left(), "the sign before the thumbnail, in a column of its own");
    // the sign and the thumbnail line up on every row
    assert!((at(&h, "componentIcon:0").left() - thumb.left()).abs() < 0.5);
}
