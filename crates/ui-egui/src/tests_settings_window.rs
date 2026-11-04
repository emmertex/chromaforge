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

fn back_button(h: &Headless) -> egui::Rect {
    h.app.widgets.iter().find(|(w, _)| w == "settings:back").map(|(_, r)| *r).unwrap_or_else(|| panic!("no settings:back widget"))
}

/// Fork #793: on macOS the window's close, minimise and zoom buttons sit over the top-left corner
/// of the window, so Settings' Back button starts to their right, as the top bar's first button
/// does; in macOS full screen they are gone and it keeps the corner, as on other platforms.
#[test]
fn settings_back_button_clears_the_macos_window_controls() {
    let app = LccApp::new(lcc_engine::Session::new(), Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [1400.0, 900.0], 1.0);
    h.settle(Duration::from_secs(60));
    assert_eq!(h.request("engine.execute", json!({"command": "app.settings", "params": {}}), T)["ok"], true);
    h.step();
    let plain = back_button(&h);
    h.app.integrated_titlebar = true;
    h.step();
    let under_controls = back_button(&h);
    assert!(under_controls.left() >= 78.0, "right of the window controls: {under_controls:?}");
    assert_eq!(under_controls.top(), plain.top(), "same row");
    h.app.window_is_fullscreen = true;
    h.step();
    assert_eq!(back_button(&h), plain, "macOS full screen hides the window controls");
    h.app.integrated_titlebar = false;
    h.app.window_is_fullscreen = false;
    h.step();
    assert_eq!(back_button(&h), plain, "other platforms keep the corner");
}
