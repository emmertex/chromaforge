//! Pickers (targeted adjustments, Point Color) are put away when another tab is chosen, a slider is touched or Esc
//! is pressed; with no tool armed a click on the photo zooms, in Masks as in Edit (test notes 2026-10-24 notes 8 and
//! 23).

use std::time::Duration;

use serde_json::json;

use crate::headless::Headless;
use crate::state::{RightPanel, Zoom};
use crate::{LccApp, Services};

const T: Duration = Duration::from_secs(20);
const SETTLE: Duration = Duration::from_secs(120);

fn edit() -> Headless {
    let services = Services { png: None, ..Default::default() };
    let app = LccApp::new(lcc_engine::Session::with_demo(), services);
    let mut h = Headless::new(app, [1300.0, 2600.0], 1.0);
    let r = h.request("ui.set", json!({"view": "detail", "right": "edit"}), T);
    assert_eq!(r["ok"], true, "{r}");
    run(&mut h, "view.colorMixer", json!({}));
    h.settle(SETTLE);
    h
}

fn run(h: &mut Headless, command: &str, params: serde_json::Value) -> serde_json::Value {
    let r = h.request("engine.execute", json!({"command": command, "params": params}), T);
    assert_eq!(r["ok"], true, "{command}: {r}");
    h.step();
    h.step();
    r["result"].clone()
}

fn click(h: &mut Headless, id: &str) {
    let r = h.request("ui.clickWidget", json!({"id": id}), T);
    assert_eq!(r["ok"], true, "{id}: {r}");
    h.step();
    h.step();
}

fn widget(h: &Headless, id: &str) -> egui::Rect {
    h.app.widgets.iter().find(|(w, _)| w == id).map(|(_, r)| *r).unwrap_or_else(|| panic!("{id} drawn"))
}

/// A press, a short drag and a release at `from`.
fn drag(h: &mut Headless, from: egui::Pos2, by: egui::Vec2) {
    let m = egui::Modifiers::NONE;
    h.events.push(egui::Event::PointerMoved(from));
    h.events.push(egui::Event::PointerButton { pos: from, button: egui::PointerButton::Primary, pressed: true, modifiers: m });
    h.step();
    for i in 1..=6 {
        h.events.push(egui::Event::PointerMoved(from + by * (i as f32 / 6.0)));
        h.step();
    }
    h.events.push(egui::Event::PointerButton { pos: from + by, button: egui::PointerButton::Primary, pressed: false, modifiers: m });
    h.step();
    h.step();
}

fn click_at(h: &mut Headless, at: egui::Pos2) {
    let m = egui::Modifiers::NONE;
    h.events.push(egui::Event::PointerMoved(at));
    h.events.push(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers: m });
    h.step();
    h.events.push(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers: m });
    h.step();
    h.step();
}

/// The Color Mixer's other tab (Mixer ↔ Point Color).
fn point_tab(h: &mut Headless) {
    h.app.ui.toggle_flyout("pointColor");
    h.step();
    h.step();
}

#[test]
fn a_picker_is_put_away_by_another_tab_a_slider_or_esc() {
    let mut h = edit();
    click(&mut h, "button:mixerGroup-lum");
    click(&mut h, "icon:tatMixer-lum");
    assert_eq!(h.app.ui.tool, "tat:lum");
    h.step();
    // another group: put away
    click(&mut h, "button:mixerGroup-sat");
    assert_eq!(h.app.ui.tool, "", "Saturation chosen: the Luminance picker is put away");
    // a slider touched
    click(&mut h, "icon:tatMixer-sat");
    assert_eq!(h.app.ui.tool, "tat:sat");
    let s = widget(&h, "slider:mixer.red.sat");
    drag(&mut h, s.center(), egui::vec2(30.0, 0.0));
    assert_eq!(h.app.ui.tool, "", "a slider touched puts the picker away");
    // Esc
    click(&mut h, "icon:tatMixer-sat");
    assert_eq!(h.app.ui.tool, "tat:sat");
    let r = h.request("ui.key", json!({"key": "escape"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    assert_eq!(h.app.ui.tool, "", "Esc puts it away");
    // the Point Color tab: its picker goes when the Mixer tab is chosen again, and doesn't follow into Masks
    point_tab(&mut h);
    click(&mut h, "icon:pointColorPicker");
    assert_eq!(h.app.ui.tool, "pointColor");
    point_tab(&mut h);
    assert!(!h.app.ui.flyout_open("pointColor"));
    assert_eq!(h.app.ui.tool, "");
    point_tab(&mut h);
    click(&mut h, "icon:pointColorPicker");
    assert_eq!(h.app.ui.tool, "pointColor");
    run(&mut h, "panel.masking", json!({}));
    assert_eq!(h.app.ui.right, RightPanel::Masking);
    assert_eq!(h.app.ui.tool, "", "an Edit picker doesn't follow into Masks");
}

#[test]
fn with_no_tool_armed_a_click_zooms_in_masks_too() {
    let mut h = edit();
    run(&mut h, "panel.masking", json!({}));
    run(&mut h, "mask.add", json!({"kind": "radial", "center": {"x": 0.5, "y": 0.5}, "rx": 0.2, "ry": 0.2}));
    h.app.ui.tool.clear();
    h.settle(SETTLE);
    assert_eq!(h.app.ui.right, RightPanel::Masking);
    assert!(matches!(h.app.ui.zoom, Zoom::Fit));
    let img = widget(&h, "canvas:image");
    // away from the mask's pin and handles
    click_at(&mut h, img.min + egui::vec2(img.width() * 0.1, img.height() * 0.1));
    assert!(matches!(h.app.ui.zoom, Zoom::Percent(_)), "a click zooms in Masks: {:?}", h.app.ui.zoom);
    assert_eq!(h.app.session.develop_of(h.app.session.active().unwrap()).map(|d| d.masks.len()), Some(1), "and makes no mask");
}
