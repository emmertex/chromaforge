//! Develop's tools are Lightroom Classic's tool strip: a row under the histogram, each tool's
//! drawer above the Develop panels, which stay under it; the right panel folds from the top bar
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
    let tools: Vec<egui::Rect> =
        ["edit", "crop", "remove", "redeye", "masking"].iter().map(|id| rect(&h, &format!("icon:{id}")).expect(id)).collect();
    for r in &tools {
        assert!(r.top() >= histogram.bottom(), "under the histogram");
        assert!((r.center().y - tools[0].center().y).abs() < 1.0, "in one row");
    }
    assert!(tools.windows(2).all(|w| w[0].right() <= w[1].left()), "left to right");
    let basic = rect(&h, "section:light").expect("Basic");
    assert!(tools[0].bottom() <= basic.top(), "above the Develop panels");
    // the right panel folds from the top bar, above the row; Edit is in the tool row
    let fold = rect(&h, "icon:collapseRight").expect("the top bar's fold button");
    assert!(fold.bottom() <= histogram.top(), "in the top bar");
    assert!(rect(&h, "icon:edit").is_some() && rect(&h, "icon:presets").is_none(), "Edit in row, no Presets button");
}

#[test]
fn edit_button_returns_to_develop_panels_and_stays_open() {
    let mut h = develop();
    // initially on Edit
    assert_eq!(h.app.ui.right, RightPanel::Edit);
    // clicking Edit while on Edit keeps Edit open
    click(&mut h, "icon:edit");
    assert_eq!(h.app.ui.right, RightPanel::Edit);

    // opening Crop drawer
    click(&mut h, "icon:crop");
    assert_eq!(h.app.ui.right, RightPanel::Crop);
    assert!(rect(&h, "button:cropClose").is_some());

    // clicking Edit returns to the Develop panels
    click(&mut h, "icon:edit");
    assert_eq!(h.app.ui.right, RightPanel::Edit);
    assert!(rect(&h, "button:cropClose").is_none());
}

#[test]
fn tool_indicators_show_applied_adjustments() {
    let mut h = develop();
    // initially clean demo photo
    assert!(rect(&h, "dot:crop").is_none(), "crop not applied yet");
    assert!(rect(&h, "dot:edit").is_none(), "edit not applied yet");
    assert!(rect(&h, "dot:remove").is_none(), "remove not applied yet");
    assert!(rect(&h, "dot:masking").is_none(), "masking not applied yet");

    // apply a crop angle
    let r = h.request("engine.execute", json!({"command": "crop.straighten", "params": {"angle": 5.0}}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    assert!(rect(&h, "dot:crop").is_some(), "crop indicator appears");
    assert!(rect(&h, "dot:edit").is_none(), "edit indicator still none");

    // apply an exposure edit
    let r = h.request("engine.execute", json!({"command": "develop.set", "params": {"control": "light.exposure", "value": 1.0}}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    assert!(rect(&h, "dot:edit").is_some(), "edit indicator appears");

    // reset crop
    let r = h.request("engine.execute", json!({"command": "crop.reset"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    assert!(rect(&h, "dot:crop").is_none(), "crop indicator clears after reset");
    assert!(rect(&h, "dot:edit").is_some(), "edit indicator remains");
}

#[test]
fn a_tools_drawer_opens_over_the_panels_and_closes_from_its_button() {
    let mut h = develop();
    click(&mut h, "icon:crop");
    assert_eq!(h.app.ui.right, RightPanel::Crop);
    let close = rect(&h, "button:cropClose").expect("the drawer");
    let crop = rect(&h, "icon:crop").expect("the row stays");
    assert!(crop.bottom() <= close.top(), "the drawer is under the row");
    let basic = rect(&h, "section:light").expect("the panels under the drawer");
    assert!(close.bottom() <= basic.top(), "under the drawer");
    // the tool in use: a click closes its drawer, back to the panels
    click(&mut h, "icon:crop");
    assert_eq!(h.app.ui.right, RightPanel::Edit);
    assert!(rect(&h, "button:cropClose").is_none());
    // its key closes it the same way
    let r = h.request("engine.execute", json!({"command": "panel.masking"}), T);
    assert_eq!(r["ok"], true, "{r}");
    let r = h.request("engine.execute", json!({"command": "panel.masking"}), T);
    assert_eq!(r["ok"], true, "{r}");
    assert_eq!(h.app.ui.right, RightPanel::Edit);
}

#[test]
fn presets_and_the_fold_button_bring_the_tools_back() {
    let mut h = develop();
    // Window ▸ Presets puts the presets in the panels' place, and back
    let presets = |h: &mut Headless| {
        let r = h.request("engine.execute", json!({"command": "panel.presets"}), T);
        assert_eq!(r["ok"], true, "{r}");
        h.settle(SETTLE);
    };
    presets(&mut h);
    assert!(h.app.ui.presets, "Presets in the panels' place");
    assert!(rect(&h, "icon:crop").is_none());
    presets(&mut h);
    assert!(!h.app.ui.presets, "a second time goes back to the tools");
    assert!(rect(&h, "icon:crop").is_some());
    // E closes the panels; the top bar's fold button brings them back
    let r = h.request("engine.execute", json!({"command": "panel.edit"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    assert_eq!(h.app.ui.right, RightPanel::None);
    click(&mut h, "icon:collapseRight");
    assert_eq!(h.app.ui.right, RightPanel::Edit);
    assert!(!h.app.ui.right_collapsed && rect(&h, "icon:crop").is_some());
}
