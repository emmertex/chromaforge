//! Switching module is a step of the history, as in Lightroom Classic (Edit ▸ Undo Module Change):
//! Undo goes back to the module the person came from, Redo returns.

use std::time::Duration;

use serde_json::json;

use crate::headless::Headless;
use crate::module::ModuleId;
use crate::{LccApp, Services};

const SETTLE: Duration = Duration::from_secs(120);

fn library() -> Headless {
    let app = LccApp::new(lcc_engine::Session::with_demo(), Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [1300.0, 900.0], 1.0);
    h.settle(SETTLE);
    // start in the Library with a history of its own
    h.app.run("module.switch", json!({"module": "library"})).unwrap();
    h.settle(SETTLE);
    h.app.session.undo.clear();
    h.app.session.redo.clear();
    h
}

#[test]
fn switching_module_is_an_undo_step_and_undo_goes_back() {
    let mut h = library();
    assert_eq!(h.app.ui.module, ModuleId::Library);
    h.app.run("module.switch", json!({"module": "develop"})).unwrap();
    h.settle(SETTLE);
    assert_eq!(h.app.ui.module, ModuleId::Develop);
    assert_eq!(h.app.session.undo.last().map(|e| e.label.as_str()), Some("Module Change"));

    h.app.run("edit.undo", json!({})).unwrap();
    h.settle(SETTLE);
    assert_eq!(h.app.ui.module, ModuleId::Library, "Undo returns to the module the person came from");

    h.app.run("edit.redo", json!({})).unwrap();
    h.settle(SETTLE);
    assert_eq!(h.app.ui.module, ModuleId::Develop, "Redo goes to it again");
}

#[test]
fn undoing_a_module_change_adds_no_step_of_its_own() {
    let mut h = library();
    h.app.run("module.switch", json!({"module": "develop"})).unwrap();
    h.settle(SETTLE);
    let steps = h.app.session.undo.len();
    h.app.run("edit.undo", json!({})).unwrap();
    h.settle(SETTLE);
    assert_eq!(h.app.session.undo.len(), steps - 1, "the switch back that Undo makes is not a step");
    assert!(!h.app.session.redo.is_empty(), "so Redo can bring Develop back");
}

#[test]
fn the_apps_own_moves_of_the_view_are_not_steps() {
    let mut h = library();
    let vis = h.app.session.visible_cloned();
    let steps = h.app.session.undo.len();
    h.app.without_view_steps(|app| app.run("library.select", json!({"ids": [vis[1].0]})).unwrap());
    assert_eq!(h.app.session.undo.len(), steps, "what an import does to show its photos is not the person's navigation");
    h.app.run("library.select", json!({"ids": [vis[2].0]})).unwrap();
    assert_eq!(h.app.session.undo.len(), steps + 1, "the person's own selecting is");
}
