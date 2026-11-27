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
    // #618: while a tool is in use the panel holds its controls only, as Lightroom Classic's does; the photo's
    // global panels (Basic, Tone Curve…) wait until the tool is put away
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
    // the Edit button, first in the row, closes a drawer
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
    // Tab (Window ▸ Panels) and the fold button are two switches on the same side: the button brings back a side
    // Tab hid, in one click, and the menu's switch undoes the button's fold
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

/// #1251: the row's buttons are part of the panel customisation: Browse Profiles hides as a tool does, and the
/// row follows the saved order, Edit always first.
#[test]
fn the_rows_buttons_hide_and_move_as_the_tools_do() {
    let mut h = develop();
    let run = |h: &mut Headless, command: &str, params: serde_json::Value| {
        let r = h.request("engine.execute", json!({"command": command, "params": params}), T);
        assert_eq!(r["ok"], true, "{command}: {r}");
        h.settle(SETTLE);
    };
    assert!(rect(&h, "icon:profileBrowser").is_some(), "Browse Profiles shows by default");
    run(&mut h, "panel.show", json!({"panel": "profiles", "visible": false}));
    assert!(rect(&h, "icon:profileBrowser").is_none(), "hidden from the row");
    assert!(rect(&h, "icon:crop").is_some() && rect(&h, "icon:toolsEdit").is_some(), "the rest stay");
    run(&mut h, "panel.show", json!({"panel": "profiles", "visible": true}));
    assert!(rect(&h, "icon:profileBrowser").is_some(), "and back");
    // Browse Profiles first, Masking before Crop: Edit keeps its place at the start
    run(&mut h, "panel.order", json!({"order": ["profiles", "masking", "crop", "remove", "redEye"]}));
    let x = |h: &Headless, id: &str| rect(h, id).unwrap_or_else(|| panic!("{id}")).center().x;
    assert!(x(&h, "icon:toolsEdit") < x(&h, "icon:profileBrowser"), "Edit stays first");
    assert!(x(&h, "icon:profileBrowser") < x(&h, "icon:masking"), "Browse Profiles moved left");
    assert!(x(&h, "icon:masking") < x(&h, "icon:crop"), "Masking before Crop");
    // the row's Browse Profiles still opens the browser from its new place
    click(&mut h, "icon:profileBrowser");
    assert_eq!(h.app.ui.right, RightPanel::Profiles);
}

/// #1251: with Settings ▸ Develop ▸ Detach the mask list, the masks sit in a window of their own over the photo, and
/// the selected mask's Refine Edges and Feather stay in the panel.
#[test]
fn a_detached_mask_list_floats_and_leaves_feather_in_the_panel() {
    let mut h = develop();
    let run = |h: &mut Headless, command: &str, params: serde_json::Value| -> serde_json::Value {
        let r = h.request("engine.execute", json!({"command": command, "params": params}), T);
        assert_eq!(r["ok"], true, "{command}: {r}");
        h.settle(SETTLE);
        r["result"].clone()
    };
    run(&mut h, "panel.masking", json!({}));
    run(&mut h, "mask.add", json!({"kind": "radial", "center": [0.5, 0.5], "rx": 0.2, "ry": 0.2}));
    let id = h.app.session.active_mask.expect("the new mask is selected");
    let row = format!("mask:{id}");
    assert!(rect(&h, "window:maskList").is_none(), "docked by default");
    let panel_row = rect(&h, &row).expect("the mask's row in the panel");
    assert!(rect(&h, "slider:maskFeather").is_some() && rect(&h, "slider:refine").is_some(), "Feather and Refine Edges");
    assert_eq!(run(&mut h, "view.masksDetached", json!({"on": true}))["on"], true);
    let window = rect(&h, "window:maskList").expect("the list's own window");
    let row_now = rect(&h, &row).expect("the mask's row");
    assert!(window.contains(row_now.center()), "the row is in the window");
    assert!(row_now.center().x < panel_row.left(), "over the photo, left of the panel");
    let feather = rect(&h, "slider:maskFeather").expect("Feather stays");
    assert!(!window.contains(feather.center()), "in the panel, not the window");
    assert!(rect(&h, "slider:refine").is_some(), "Refine Edges stays");
    // a click on the row in the window still selects and edits the mask
    click(&mut h, &row);
    assert_eq!(h.app.session.active_mask, Some(id));
    // docking puts it back in the panel
    assert_eq!(run(&mut h, "view.masksDetached", json!({}))["on"], false);
    assert!(rect(&h, "window:maskList").is_none());
    assert!(rect(&h, &row).is_some_and(|r| (r.center().x - panel_row.center().x).abs() < 1.0), "back in the panel");
    // a wrong argument is an error, not a switch
    let r = h.request("engine.execute", json!({"command": "view.masksDetached", "params": {"on": "yes"}}), T);
    assert_ne!(r["ok"], true, "{r}");
    assert!(!h.app.ui.settings.masks_detached);
}
