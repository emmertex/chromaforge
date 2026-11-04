//! Panning a zoomed photo in the Masking panel with no tool armed: a drag that grabs no pin,
//! handle or gradient pans, as in the loupe (Space+drag over an armed tool: `tests_space_hand`).

use std::time::Duration;

use serde_json::json;

use crate::headless::Headless;
use crate::{LccApp, Services};

const T: Duration = Duration::from_secs(20);
const SETTLE: Duration = Duration::from_secs(120);

fn exec(h: &mut Headless, command: &str) {
    let r = h.request("engine.execute", json!({"command": command, "params": {}}), T);
    assert_eq!(r["ok"], true, "{command}: {r}");
    h.step();
}

#[test]
fn a_drag_pans_in_the_masking_panel_with_no_tool_armed() {
    let services = Services { png: None, ..Default::default() };
    let mut app = LccApp::new(lcc_engine::Session::with_demo(), services);
    app.ui.settings.gpu = false;
    let mut h = Headless::new(app, [1300.0, 900.0], 1.0);
    let r = h.request("ui.set", json!({"view": "detail", "right": "edit"}), T);
    assert_eq!(r["ok"], true, "{r}");
    exec(&mut h, "panel.masking");
    exec(&mut h, "view.zoom100");
    h.settle(SETTLE);
    let (img, canvas) = (h.app.image_rect.expect("photo"), h.app.canvas_rect.expect("loupe"));
    assert!(crate::panels::detail::zoomed(img, canvas), "zoomed in past the pane");
    assert!(h.app.ui.tool.is_empty());
    let pan = h.app.ui.pan;
    let c = canvas.center();
    let r = h.request("ui.drag", json!({"x": c.x, "y": c.y, "toX": c.x - 200.0, "toY": c.y - 120.0, "steps": 12}), T);
    assert_eq!(r["ok"], true, "{r}");
    for _ in 0..4 {
        h.step();
    }
    let now = h.app.ui.pan;
    assert!((pan.0 - now.0).abs() > 0.01 || (pan.1 - now.1).abs() > 0.01, "the drag panned: {pan:?} -> {now:?}");
    assert!(h.app.gesture.is_none(), "the pan ends with the drag");
    let id = h.app.session.active().expect("a photo");
    assert_eq!(h.app.session.develop_of(id).map_or(0, |d| d.masks.len()), 0, "and made no mask");
}
