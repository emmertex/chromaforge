//! Icon buttons answer the pointer: the pointing hand while enabled, the arrow otherwise.

use std::time::Duration;

use serde_json::json;

use crate::headless::Headless;
use crate::{LccApp, Services};

const T: Duration = Duration::from_secs(20);
const SETTLE: Duration = Duration::from_secs(120);

#[test]
fn icon_buttons_show_the_pointing_hand_on_hover() {
    let app = LccApp::new(lcc_engine::Session::with_demo(), Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [1200.0, 800.0], 1.0);
    h.settle(SETTLE);
    let r = h.request("ui.hoverWidget", json!({"id": "icon:sidebar"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    h.step();
    h.step();
    assert_eq!(h.last_cursor, egui::CursorIcon::PointingHand);
    let r = h.request("ui.move", json!({"x": 2.0, "y": 790.0}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    assert_ne!(h.last_cursor, egui::CursorIcon::PointingHand);
}
