//! LR-PRE-MANAGE / LR-PRE-ONIMPORT / LR-PRESET-PICKER: the Presets Manager marks presets At
//! Import and hides them; hidden presets leave the Presets panel, At Import ones get their group.

use std::time::Duration;

use serde_json::json;

use crate::headless::Headless;
use crate::{LccApp, Services};

const T: Duration = Duration::from_secs(10);

fn widget(h: &mut Headless, id: &str) -> bool {
    h.app.widgets.iter().any(|(w, _)| w == id)
}

#[test]
fn the_manager_marks_at_import_and_hides_presets() {
    let app = LccApp::new(lcc_engine::Session::with_demo(), Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [1400.0, 900.0], 1.0);
    h.settle(Duration::from_secs(60));
    let p = h.app.session.presets.first().unwrap().clone();
    assert_eq!(h.request("engine.execute", json!({"command": "dialog.presetsManager", "params": {"query": p.name}}), T)["ok"], true);
    h.step();
    h.step();
    let at_import = format!("check:presetsManagerAtImport:{}", p.id);
    assert!(widget(&mut h, &at_import), "a search opens the group it finds");
    assert_eq!(h.request("ui.clickWidget", json!({"id": at_import}), T)["ok"], true);
    h.step();
    h.step();
    assert!(h.app.session.preset_at_import.contains(&p.id));
    let show = format!("check:presetsManagerShow:{}", p.id);
    assert_eq!(h.request("ui.clickWidget", json!({"id": show}), T)["ok"], true);
    h.step();
    h.step();
    assert!(h.app.session.preset_hidden.contains(&p.id));
    // the Presets panel leaves a hidden preset out, the At Import group too
    h.app.ui.dialog = None;
    h.app.session.execute("preset.hide", &json!({"id": p.id, "hidden": false})).unwrap();
    h.app.run("panel.presets", json!({})).unwrap();
    h.step();
    h.step();
    assert!(widget(&mut h, "presetAtImport"), "At Import is a group of the Presets panel");
    h.app.session.execute("preset.hide", &json!({"id": p.id, "hidden": true})).unwrap();
    h.step();
    h.step();
    assert!(!widget(&mut h, "presetAtImport"), "a hidden preset leaves the panel");
}
