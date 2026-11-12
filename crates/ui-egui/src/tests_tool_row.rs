//! Develop's tools are Lightroom Classic's tool strip: a row under the histogram, each tool's
//! drawer in the Develop panels' place while it is in use; the right panel folds from the top bar
//! (fork-owned).

use std::time::Duration;

use serde_json::json;

use crate::headless::Headless;
use crate::state::RightPanel;
use crate::{LccApp, Services};

const T: Duration = Duration::from_secs(20);
const SETTLE: Duration = Duration::from_secs(120);

fn develop() -> Headless {
    let app = LccApp::new(lcc_engine::Session::with_demo(), Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [1300.0, 1400.0], 1.0);
    let r = h.request("ui.set", json!({"module": "develop", "view": "detail", "right": "edit"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    h
}

fn rect(h: &Headless, id: &str) -> Option<egui::Rect> {
    h.app.widgets.iter().find(|(w, _)| w == id).map(|(_, r)| *r)
}

fn click(h: &mut Headless, id: &str) {
    let r = h.request("ui.clickWidget", json!({"id": id}), T);
    assert_eq!(r["ok"], true, "{id}: {r}");
    h.settle(SETTLE);
}

#[test]
fn the_tools_are_a_row_under_the_histogram() {
    let h = develop();
    let histogram = rect(&h, "histogram").expect("the histogram");
    let tools: Vec<egui::Rect> = ["crop", "remove", "redeye", "masking"].iter().map(|id| rect(&h, &format!("icon:{id}")).expect(id)).collect();
    for r in &tools {
        assert!(r.top() >= histogram.bottom(), "under the histogram");
        assert!((r.center().y - tools[0].center().y).abs() < 1.0, "in one row");
    }
    assert!(tools.windows(2).all(|w| w[0].right() <= w[1].left()), "left to right");
    let basic = rect(&h, "section:light").expect("Basic");
    assert!(tools[0].bottom() <= basic.top(), "above the Develop panels");
    // the right panel folds from the top bar, above the row; Edit is the panels, not a tool
    let fold = rect(&h, "icon:collapseRight").expect("the top bar's fold button");
    assert!(fold.bottom() <= histogram.top(), "in the top bar");
    assert!(rect(&h, "icon:edit").is_none() && rect(&h, "icon:presets").is_none(), "no Edit or Presets button");
}

#[test]
fn a_tools_drawer_opens_in_the_panels_place_and_closes_from_its_button() {
    let mut h = develop();
    click(&mut h, "icon:crop");
    assert_eq!(h.app.ui.right, RightPanel::Crop);
    let close = rect(&h, "button:cropClose").expect("the drawer");
    let crop = rect(&h, "icon:crop").expect("the row stays");
    assert!(crop.bottom() <= close.top(), "the drawer is under the row");
    // the tool in use: a click closes its drawer, back to the panels
    click(&mut h, "icon:crop");
    assert_eq!(h.app.ui.right, RightPanel::Edit);
    assert!(rect(&h, "button:cropClose").is_none());
    assert!(rect(&h, "section:light").is_some(), "the panels are back");
    // its key closes it the same way
    let r = h.request("engine.execute", json!({"command": "panel.masking"}), T);
    assert_eq!(r["ok"], true, "{r}");
    let r = h.request("engine.execute", json!({"command": "panel.masking"}), T);
    assert_eq!(r["ok"], true, "{r}");
    assert_eq!(h.app.ui.right, RightPanel::Edit);
}

/// #618: while a tool is in use the panel holds its controls only, as Lightroom Classic's does;
/// the photo's global panels (Basic, Tone Curve…) wait until the tool is put away.
#[test]
fn a_tools_drawer_hides_the_develop_panels() {
    let mut h = develop();
    assert!(rect(&h, "section:light").is_some(), "Basic with no tool in use");
    for tool in ["crop", "remove", "redeye", "masking"] {
        click(&mut h, &format!("icon:{tool}"));
        assert!(h.app.ui.right != RightPanel::Edit, "{tool} is in use");
        assert!(rect(&h, "section:light").is_none(), "{tool}: no Basic panel under its drawer");
        assert!(rect(&h, "icon:crop").is_some(), "{tool}: the tool row stays");
        click(&mut h, &format!("icon:{tool}"));
        assert_eq!(h.app.ui.right, RightPanel::Edit, "{tool} put away");
        assert!(rect(&h, "section:light").is_some(), "{tool}: the panels are back");
    }
}

#[test]
fn presets_and_the_fold_button_bring_the_tools_back() {
    let mut h = develop();
    // Window ▸ Presets shows the presets in the left column and leaves the tools where they are:
    // the presets never take the Edit panels' place
    let presets = |h: &mut Headless| {
        let r = h.request("engine.execute", json!({"command": "panel.presets"}), T);
        assert_eq!(r["ok"], true, "{r}");
        h.settle(SETTLE);
    };
    let left_of_tools = |h: &Headless| {
        let (panel, crop) = (rect(h, "classicPanel:presets").expect("the Presets panel"), rect(h, "icon:crop").expect("the tools"));
        panel.right() <= crop.left()
    };
    presets(&mut h);
    assert!(crate::panels::presets::shown(&h.app) && h.app.ui.left_panel, "the presets are on screen");
    assert!(left_of_tools(&h), "in the left column, beside the tools");
    assert_eq!(h.app.ui.right, RightPanel::Edit);
    presets(&mut h);
    assert!(!crate::panels::presets::shown(&h.app), "a second time folds the panel");
    assert!(rect(&h, "icon:crop").is_some() && h.app.ui.right == RightPanel::Edit, "and the tools are untouched");
    // E closes the panels; the top bar's fold button brings them back
    let r = h.request("engine.execute", json!({"command": "panel.edit"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    assert_eq!(h.app.ui.right, RightPanel::None);
    click(&mut h, "icon:collapseRight");
    assert_eq!(h.app.ui.right, RightPanel::Edit);
    assert!(!h.app.ui.right_collapsed && rect(&h, "icon:crop").is_some());
}

/// Tab (Window ▸ Panels) and the top bar's fold button are two switches on the same side: the
/// button brings back a side Tab hid, in one click, and the menu's switch undoes the button's fold.
#[test]
fn the_fold_button_and_the_edge_switch_never_leave_the_right_side_stuck_hidden() {
    let mut h = develop();
    let run = |h: &mut Headless, command: &str, params: serde_json::Value| {
        let r = h.request("engine.execute", json!({"command": command, "params": params}), T);
        assert_eq!(r["ok"], true, "{command}: {r}");
        h.settle(SETTLE);
    };
    run(&mut h, "panel.sides", json!({"show": false}));
    assert!(rect(&h, "icon:crop").is_none(), "Tab hid the tools");
    click(&mut h, "icon:collapseRight");
    assert!(rect(&h, "icon:crop").is_some(), "one click on the button brings them back");
    click(&mut h, "icon:collapseRight");
    assert!(rect(&h, "icon:crop").is_none(), "and the next folds them away");
    run(&mut h, "panel.toggle", json!({"edge": "right", "show": true}));
    assert!(rect(&h, "icon:crop").is_some(), "showing the edge unfolds it");
}

#[test]
fn the_edit_button_closes_a_tools_drawer() {
    let mut h = develop();
    let edit = rect(&h, "icon:toolsEdit").expect("the Edit button");
    let crop = rect(&h, "icon:crop").expect("Crop");
    assert!(edit.right() <= crop.left(), "first in the row");
    click(&mut h, "icon:masking");
    assert_eq!(h.app.ui.right, RightPanel::Masking);
    click(&mut h, "icon:toolsEdit");
    assert_eq!(h.app.ui.right, RightPanel::Edit);
    // a second click leaves the panels on screen
    click(&mut h, "icon:toolsEdit");
    assert_eq!(h.app.ui.right, RightPanel::Edit);
}
