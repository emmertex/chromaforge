//! Ported from OneSlips 0d9d4fb7d (test notes 2026-10-25 notes 8 and 9): entering Masks lands on a mask, and a tool
//! armed for a new mask that is put away without drawing gives the earlier mask back.

use std::time::Duration;

use serde_json::json;

use crate::headless::Headless;
use crate::state::RightPanel;
use crate::{LccApp, Services};

const T: Duration = Duration::from_secs(20);

fn masking() -> Headless {
    let app = LccApp::new(lcc_engine::Session::with_demo(), Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [1200.0, 1000.0], 1.0);
    let r = h.request("ui.set", json!({"view": "detail", "moduleBar": false}), T);
    assert_eq!(r["ok"], true, "{r}");
    exec(&mut h, "panel.masking", json!({}));
    h.step();
    h
}

fn exec(h: &mut Headless, command: &str, params: serde_json::Value) -> serde_json::Value {
    let r = h.request("engine.execute", json!({"command": command, "params": params}), T);
    assert_eq!(r["ok"], true, "{command}: {r}");
    r["result"].clone()
}

fn nth(h: &Headless, n: usize) -> u32 {
    let id = h.app.session.active().unwrap();
    h.app.session.develop_of(id).unwrap().masks[n].id
}

/// Leave Masks (the Edit panel) and enter it again.
fn reenter(h: &mut Headless) {
    exec(h, "panel.edit", json!({}));
    h.step();
    assert_ne!(h.app.ui.right, RightPanel::Masking);
    exec(h, "panel.masking", json!({}));
    h.step();
    assert_eq!(h.app.ui.right, RightPanel::Masking);
}

/// Note 8: entering Masks lands on a mask when the photo has any: the only one; the one selected last time on this
/// photo (the third of five stays the third); else the first. It opened with none selected.
#[test]
fn entering_masks_lands_on_a_mask() {
    let mut h = masking();
    exec(&mut h, "mask.add", json!({"kind": "radial", "center": [0.3, 0.4], "rx": 0.1, "ry": 0.1}));
    // one mask: away to another photo, back, deselected, into Masks
    exec(&mut h, "panel.edit", json!({}));
    exec(&mut h, "library.next", json!({}));
    exec(&mut h, "library.previous", json!({}));
    exec(&mut h, "mask.select", json!({"id": null}));
    h.step();
    assert_eq!(h.app.session.active_mask, None);
    exec(&mut h, "panel.masking", json!({}));
    h.step();
    assert_eq!(h.app.session.active_mask, Some(nth(&h, 0)), "the only mask");
    // five masks, the third selected: leaving and coming back keeps it, also after a photo away
    for x in [0.4, 0.5, 0.6, 0.7] {
        exec(&mut h, "mask.add", json!({"kind": "radial", "center": [x, 0.6], "rx": 0.05, "ry": 0.05}));
    }
    let third = nth(&h, 2);
    exec(&mut h, "mask.select", json!({"id": third}));
    h.step();
    reenter(&mut h);
    assert_eq!(h.app.session.active_mask, Some(third), "leaving and coming back keeps the third");
    exec(&mut h, "panel.edit", json!({}));
    exec(&mut h, "library.next", json!({}));
    exec(&mut h, "library.previous", json!({}));
    exec(&mut h, "mask.select", json!({"id": null}));
    exec(&mut h, "panel.masking", json!({}));
    h.step();
    assert_eq!(h.app.session.active_mask, Some(third), "the one selected last time on this photo");
    // a photo whose masks were never selected in Masks: the first
    exec(&mut h, "panel.edit", json!({}));
    exec(&mut h, "library.next", json!({}));
    h.step();
    for x in [0.3, 0.6] {
        exec(&mut h, "mask.add", json!({"kind": "linear", "start": [x, 0.2], "end": [x, 0.6]}));
    }
    exec(&mut h, "mask.select", json!({"id": null}));
    h.step();
    exec(&mut h, "panel.masking", json!({}));
    h.step();
    assert_eq!(h.app.session.active_mask, Some(nth(&h, 0)), "the first");
    // deselecting in Masks stays deselected
    exec(&mut h, "mask.select", json!({"id": null}));
    h.step();
    h.step();
    assert_eq!(h.app.session.active_mask, None, "a choice of none is kept while Masks stays open");
}

/// Note 9: arming a new-mask tool deselects the selected mask; putting it away without drawing (Esc, or the tool
/// again) selects the mask selected before, unchanged.
#[test]
fn a_tool_put_away_without_drawing_gives_the_earlier_mask_back() {
    let mut h = masking();
    exec(&mut h, "mask.add", json!({"kind": "linear", "start": [0.5, 0.1], "end": [0.5, 0.4], "name": "Top"}));
    let top = h.app.session.active_mask.expect("selected");
    h.step();
    exec(&mut h, "tool.radial", json!({}));
    h.step();
    assert_eq!(h.app.ui.tool, "radial");
    assert_eq!(h.app.session.active_mask, None, "arming deselects");
    let r = h.request("ui.key", json!({"key": "Escape"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    assert!(h.app.ui.mask_new.is_none(), "Esc puts the tool away");
    assert_eq!(h.app.session.active_mask, Some(top), "the mask selected before");
    // a mask drawn with the tool is the one selected
    exec(&mut h, "tool.radial", json!({}));
    h.step();
    exec(&mut h, "mask.add", json!({"kind": "radial", "center": [0.3, 0.6], "rx": 0.05, "ry": 0.05}));
    h.step();
    h.step();
    let made = nth(&h, 1);
    assert_eq!(h.app.session.active_mask, Some(made), "the new mask stays selected");
}
