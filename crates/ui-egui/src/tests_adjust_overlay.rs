//! Ported from OneSlips 0c58de7ad (the user's standard): while any slider or dial adjusts the photo, the overlays and
//! tool graphics hide, even an overlay switched on with O; afterwards they are back as they were.

use std::time::Duration;

use lcc_pipeline::Overlay;
use serde_json::json;

use crate::headless::Headless;
use crate::panels::detail::{adjusting, view_overlay};
use crate::{LccApp, Services};

const T: Duration = Duration::from_secs(20);

fn detail(panel: &str) -> Headless {
    let app = LccApp::new(lcc_engine::Session::with_demo(), Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [1200.0, 1000.0], 1.0);
    let r = h.request("ui.set", json!({"view": "detail", "moduleBar": false}), T);
    assert_eq!(r["ok"], true, "{r}");
    exec(&mut h, panel, json!({}));
    h
}

fn exec(h: &mut Headless, command: &str, params: serde_json::Value) -> serde_json::Value {
    let r = h.request("engine.execute", json!({"command": command, "params": params}), T);
    assert_eq!(r["ok"], true, "{command}: {r}");
    r["result"].clone()
}

fn overlay(h: &Headless) -> Overlay {
    let d = h.app.session.active().and_then(|id| h.app.session.develop_of(id)).unwrap_or_default();
    view_overlay(&h.app, &d, false)
}

#[test]
fn overlays_hide_while_any_slider_adjusts_and_come_back_as_they_were() {
    let mut h = detail("panel.masking");
    exec(&mut h, "mask.add", json!({"kind": "radial", "center": [0.5, 0.5], "rx": 0.2, "ry": 0.2}));
    exec(&mut h, "view.maskOverlay", json!({"show": true}));
    h.step();
    assert!(matches!(overlay(&h), Overlay::Mask { .. }), "{:?}", overlay(&h));
    // a global slider held (it hid nothing before: only a mask slider did)
    h.app.ui.dragging_control = Some("light.exposure".into());
    assert!(adjusting(&h.app));
    assert_eq!(overlay(&h), Overlay::None, "the effect shows, not the overlay");
    // a slider that shapes the mask keeps it
    h.app.ui.dragging_control = Some("feather".into());
    assert!(!adjusting(&h.app));
    assert!(matches!(overlay(&h), Overlay::Mask { .. }));
    // released: the overlay switched on with O is back
    h.app.ui.dragging_control = None;
    assert!(h.app.ui.mask_overlay);
    assert!(matches!(overlay(&h), Overlay::Mask { .. }));
    // Remove: Visualize Spots hides too
    exec(&mut h, "panel.remove", json!({}));
    h.app.ui.visualize_spots = true;
    assert!(matches!(overlay(&h), Overlay::Spots(_)));
    h.app.ui.dragging_control = Some("light.exposure".into());
    assert_eq!(overlay(&h), Overlay::None);
    h.app.ui.dragging_control = Some("ui.removeSize".into());
    assert!(matches!(overlay(&h), Overlay::Spots(_)), "a spot's size keeps them");
    h.app.ui.dragging_control = None;
}

/// The tool graphics (a radial's outline and handles) are not drawn while a slider adjusts.
#[test]
fn the_tool_graphics_hide_while_a_slider_adjusts() {
    let mut h = detail("panel.masking");
    exec(&mut h, "mask.add", json!({"kind": "radial", "center": [0.5, 0.5], "rx": 0.2, "ry": 0.2}));
    h.step();
    h.step();
    let shapes = |h: &Headless| h.view.painted_shapes().len();
    let shown = shapes(&h);
    h.app.ui.dragging_control = Some("light.exposure".into());
    h.step();
    let hidden = shapes(&h);
    h.app.ui.dragging_control = None;
    h.step();
    assert!(hidden < shown, "fewer shapes while adjusting: {hidden} vs {shown}");
    assert!(shapes(&h) > hidden, "back once the slider rests");
}
