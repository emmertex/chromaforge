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

#[test]
fn each_component_row_inverts_that_component_alone() {
    let mut h = masking();
    exec(&mut h, "mask.add", json!({"kind": "radial", "center": [0.5, 0.5]}));
    exec(&mut h, "mask.addComponent", json!({"op": "subtract", "kind": "linear", "start": [0.5, 0.0], "end": [0.5, 0.4]}));
    let before = masks(&h)[0].invert;
    let edited = h.app.ui.active_mask_component;
    let r = h.request("ui.clickWidget", json!({"id": "componentInvert:1"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    let m = &masks(&h)[0];
    assert!(m.components[1].invert && !m.components[0].invert, "only that component");
    assert_eq!(m.invert, before, "the mask itself is left alone");
    assert_eq!(h.app.ui.active_mask_component, edited, "inverting doesn't switch the component edited");
    let r = h.request("ui.clickWidget", json!({"id": "componentInvert:1"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    assert!(!masks(&h)[0].components[1].invert, "a second click turns it back");
}

#[test]
fn component_rows_are_named_after_their_source() {
    use crate::panels::masking::component_label;
    use lcc_develop::recipe::Who;
    use lcc_develop::{MaskComponent, MaskShape};
    let comp = |shape: MaskShape| MaskComponent { name: None, anchor: None, op: lcc_develop::MaskOp::Add, invert: false, shape };
    let groom = MaskShape::People { person: 0, parts: vec!["Clothes".into(), "Hair".into()], who: Some(Who::Role("Groom".into())), seg: None };
    assert_eq!(component_label(&comp(groom)), "Groom - Clothes, Hair", "a person's role and parts");
    assert_eq!(component_label(&comp(MaskShape::Concept { name: "Wedding dress".into(), seg: None })), "Wedding dress", "a taught object's name");
    assert_eq!(component_label(&comp(MaskShape::Landscape { class: "water".into() })), "Water", "a landscape part's class");
    assert_eq!(component_label(&comp(MaskShape::Sky)), "Sky");
    // a name the photographer gave wins; an empty one doesn't count
    let named = MaskComponent { name: Some("Veil".into()), ..comp(MaskShape::Sky) };
    assert_eq!(component_label(&named), "Veil");
    let blank = MaskComponent { name: Some("  ".into()), ..comp(MaskShape::Sky) };
    assert_eq!(component_label(&blank), "Sky");
}

#[test]
fn only_the_selected_masks_branch_opens_by_itself() {
    let mut h = masking();
    exec(&mut h, "mask.add", json!({"kind": "radial", "center": [0.3, 0.5]}));
    exec(&mut h, "mask.add", json!({"kind": "linear", "start": [0.5, 0.0], "end": [0.5, 0.4]}));
    let [first, second] = [masks(&h)[0].id, masks(&h)[1].id];
    assert_eq!(h.app.session.active_mask, Some(second));
    h.step();
    let at = |h: &Headless, id: &str| h.app.widgets.iter().find(|(w, _)| w == id).map(|(_, r)| *r).unwrap_or_else(|| panic!("no {id}"));
    assert_eq!(count(&h, "component:0"), 1, "one branch open: the selected mask's");
    assert!(at(&h, "component:0").top() >= at(&h, &format!("mask:{second}")).bottom(), "under the selected mask");
    // the chevron opens another mask's branch as well
    let r = h.request("ui.clickWidget", json!({"id": format!("button:maskCollapse:{first}")}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    assert_eq!(count(&h, "component:0"), 2, "both branches open");
    let r = h.request("ui.clickWidget", json!({"id": format!("button:maskCollapse:{first}")}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    assert_eq!(count(&h, "component:0"), 1, "and folds it again");
    // selecting the other mask moves the open branch to it
    exec(&mut h, "mask.select", json!({"id": first}));
    h.step();
    assert_eq!(count(&h, "component:0"), 1);
    assert!(at(&h, "component:0").bottom() <= at(&h, &format!("mask:{second}")).top(), "under the first mask now");
}

#[test]
fn the_eye_is_always_on_the_mask_row_and_switches_its_effect() {
    let mut h = masking();
    exec(&mut h, "mask.add", json!({"kind": "radial", "center": [0.5, 0.5]}));
    exec(&mut h, "mask.adjust", json!({"values": {"exposure": 1.0}}));
    let mid = masks(&h)[0].id;
    // the pointer away from the list
    let r = h.request("ui.move", json!({"x": 5.0, "y": 5.0}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    assert_eq!(count(&h, &format!("maskVisible:{mid}")), 1, "the eye shows without the pointer on the row");
    let r = h.request("ui.clickWidget", json!({"id": format!("maskVisible:{mid}")}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    assert!(!masks(&h)[0].visible, "the eye turns the mask's effect off");
    assert_eq!(masks(&h)[0].adjust.exposure, 1.0, "and keeps its settings");
}
