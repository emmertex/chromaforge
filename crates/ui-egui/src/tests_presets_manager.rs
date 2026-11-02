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

fn has_prefix(h: &Headless, prefix: &str) -> usize {
    h.app.widgets.iter().filter(|(w, _)| w.starts_with(prefix)).count()
}

fn click(h: &mut Headless, id: &str) {
    assert_eq!(h.request("ui.clickWidget", json!({"id": id}), T)["ok"], true, "{id}");
    h.step();
    h.step();
}

/// LR-PRESET-PICKER: the picker's groups fold (the user's own first), a star toggles a favourite
/// in place, and resting on a preset shows it beside the list without waiting for the render.
#[test]
fn the_picker_folds_stars_and_previews() {
    let mut h = crate::tests_classic::demo();
    h.app.session.add_presets(vec![lcc_develop::Preset {
        id: "user.mine".into(),
        name: "Mine".into(),
        group: "My Looks".into(),
        settings: json!({"light": {"exposure": 1.0}}),
        favorite: false,
        builtin: false,
    }]);
    let id = h.app.session.visible_cloned()[0];
    h.request("engine.execute", json!({"command": "library.select", "params": {"ids": [id.0]}}), T);
    h.step();
    h.step();
    click(&mut h, "button:qd-preset");
    let groups: Vec<String> = h.app.widgets.iter().filter_map(|(w, _)| w.strip_prefix("presetPickerGroup:qd-preset:").map(str::to_string)).collect();
    assert!(!groups.is_empty(), "the picker lists groups");
    assert_eq!(groups.first().map(String::as_str), Some("preset:My Looks"), "the user's own group first: {groups:?}");
    assert_eq!(has_prefix(&h, "presetPick:qd-preset:"), 0, "groups start folded");
    click(&mut h, "presetPickerGroup:qd-preset:preset:My Looks");
    assert!(widget(&mut h, "presetPick:qd-preset:user.mine"), "an opened group shows its presets");
    click(&mut h, "presetPickStar:qd-preset:user.mine");
    assert!(h.app.session.presets.iter().any(|p| p.id == "user.mine" && p.favorite), "the star toggles in place");
    // (the picker stays open: Favorites now lists it too)
    assert!(widget(&mut h, "presetPick:qd-preset:user.mine"), "the picker stays open");
    assert_eq!(h.request("ui.hoverWidget", json!({"id": "presetPick:qd-preset:user.mine"}), T)["ok"], true);
    h.step();
    assert!(widget(&mut h, "presetPickerPreview"), "resting on a preset shows it");
}

/// LR-PRE-MANAGE: renaming a built-in preset in place, hiding a whole group, a new group, dragging
/// a preset onto it, and Restore Built-ins.
#[test]
fn the_manager_renames_groups_drags_and_restores() {
    let app = LccApp::new(lcc_engine::Session::with_demo(), Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [1400.0, 900.0], 1.0);
    h.settle(Duration::from_secs(60));
    let p = h.app.session.presets.iter().find(|p| p.builtin).unwrap().clone();
    assert_eq!(h.request("engine.execute", json!({"command": "dialog.presetsManager", "params": {"query": p.name}}), T)["ok"], true);
    h.step();
    h.step();
    click(&mut h, &format!("button:presetsManagerRename:{}", p.id));
    let field = format!("field:presetsManagerName:{}", p.id);
    assert!(widget(&mut h, &field), "renamed in place");
    h.request("ui.text", json!({"text": " Renamed"}), T);
    h.request("ui.key", json!({"key": "Enter"}), T);
    h.step();
    h.step();
    let now = h.app.session.presets.iter().find(|q| q.id == p.id).unwrap().clone();
    assert!(now.name.ends_with("Renamed") && now.builtin, "a built-in renamed: {}", now.name);
    // hide the group whole
    click(&mut h, &format!("button:presetsManagerGroupHide:{}", now.group));
    assert!(h.app.session.preset_hidden.contains(&p.id));
    // a new group, and the preset dragged onto it
    h.app.session.execute("preset.createGroup", &json!({"name": "Dropped Here"})).unwrap();
    if let Some(crate::state::Dialog::PresetsManager { query, .. }) = h.app.ui.dialog.as_mut() {
        query.clear();
    }
    h.step();
    h.step();
    assert!(widget(&mut h, "presetsManagerGroup:Dropped Here"), "an empty group of the user's is a place to drop");
    h.app.session.execute("preset.move", &json!({"id": p.id, "group": "Dropped Here"})).unwrap();
    click(&mut h, "button:presetsManagerRestore");
    click(&mut h, "button:presetsManagerRestoreConfirm");
    let back = h.app.session.presets.iter().find(|q| q.id == p.id).unwrap().clone();
    assert_eq!((back.name, back.group), (p.name.clone(), p.group.clone()), "Restore Built-ins");
    assert!(!h.app.session.preset_hidden.contains(&p.id));
}

/// LR-PRE-CREATE: Update Preset's toast carries an Undo button that puts the old settings back.
#[test]
fn update_preset_toast_undoes() {
    let mut h = crate::tests_classic::demo();
    let id = h.app.session.visible_cloned()[0];
    h.request("engine.execute", json!({"command": "library.select", "params": {"ids": [id.0]}}), T);
    h.app.session.execute("preset.create", &json!({"name": "Mine", "group": "User Presets"})).unwrap();
    let pid = h.app.session.presets.iter().find(|p| p.name == "Mine").unwrap().id.clone();
    let before = h.app.session.presets.iter().find(|p| p.id == pid).unwrap().settings.clone();
    h.app.session.execute("develop.set", &json!({"values": {"light.exposure": 2.0}})).unwrap();
    assert_eq!(h.request("engine.execute", json!({"command": "dialog.updatePreset", "params": {"id": pid}}), T)["ok"], true);
    h.step();
    h.step();
    click(&mut h, "button:dialogOk");
    assert!(h.app.ui.toast_action.is_some(), "a toast with Undo");
    assert_ne!(h.app.session.presets.iter().find(|p| p.id == pid).unwrap().settings, before);
    click(&mut h, "button:toastAction");
    assert_eq!(h.app.session.presets.iter().find(|p| p.id == pid).unwrap().settings, before, "Undo puts the old settings back");
    assert!(h.app.ui.toast_action.is_none());
}
