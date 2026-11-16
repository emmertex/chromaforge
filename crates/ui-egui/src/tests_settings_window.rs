//! LR-PREF-WINDOW: Settings opens on the section shown last, and Ctrl+Tab / Ctrl+Shift+Tab step
//! through its sections.

use std::time::Duration;

use serde_json::json;

use crate::headless::Headless;
use crate::state::Dialog;
use crate::{LccApp, Services};

const T: Duration = Duration::from_secs(10);

fn open_tab(h: &Headless) -> String {
    match &h.app.ui.dialog {
        Some(Dialog::Settings { tab }) => crate::settings_screen::section_of(tab).to_string(),
        _ => String::new(),
    }
}

#[test]
fn settings_reopens_on_the_last_section_and_ctrl_tab_steps_sections() {
    let app = LccApp::new(lcc_engine::Session::new(), Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [1400.0, 900.0], 1.0);
    h.settle(Duration::from_secs(60));
    assert_eq!(h.request("engine.execute", json!({"command": "app.settings", "params": {"tab": "display"}}), T)["ok"], true);
    h.step();
    assert_eq!(open_tab(&h), "display");
    h.request("ui.key", json!({"key": "Tab", "ctrl": true, "cmd": !cfg!(target_os = "macos")}), T);
    h.step();
    let next = open_tab(&h);
    assert_ne!(next, "display", "Ctrl+Tab moves on");
    h.request("ui.key", json!({"key": "Tab", "ctrl": true, "cmd": !cfg!(target_os = "macos"), "shift": true}), T);
    h.step();
    assert_eq!(open_tab(&h), "display", "Ctrl+Shift+Tab comes back");
    h.request("ui.key", json!({"key": "PageDown", "ctrl": true, "cmd": !cfg!(target_os = "macos")}), T);
    h.step();
    assert_eq!(open_tab(&h), next);
    h.app.ui.dialog = None;
    h.step();
    assert_eq!(h.request("engine.execute", json!({"command": "app.settings", "params": {}}), T)["ok"], true);
    h.step();
    assert_eq!(open_tab(&h), next, "Settings… opens where it was left");
}
