//! Import ▸ Import from lists This PC: every drive and volume as a folder tree (tests see one fake
//! drive, the temporary folder, so they never touch this machine's drives), above the usual places.

use std::time::Duration;

use serde_json::json;

use crate::LccApp;
use crate::headless::Headless;
use crate::state::ViewMode;

const T: Duration = Duration::from_secs(60);

#[test]
fn import_from_lists_the_drives_as_folder_trees() {
    let mut app = LccApp::new(lcc_engine::Session::new(), crate::Services { png: None, ..Default::default() });
    app.ui.view = ViewMode::PhotoGrid;
    let mut h = Headless::new(app, [1400.0, 900.0], 1.0);
    h.settle(T);
    let r = h.request("engine.execute", json!({"command": "dialog.import", "params": {}}), T);
    assert_eq!(r["ok"], true, "{r}");
    let root = crate::import_tab::test_drive();
    std::fs::create_dir_all(root.join("DCIM")).unwrap();
    let drive = root.to_string_lossy().to_string();
    let row = format!("source:local:{drive}");
    // the drives are read on a worker thread: they show a few frames later
    h.step_until(Duration::from_secs(10), |h| h.app.widgets.iter().any(|(w, _)| *w == row));
    assert!(h.app.widgets.iter().any(|(w, _)| *w == row), "the drive's row in Import from");
    assert!(h.app.widgets.iter().any(|(w, _)| *w == format!("folderToggle:{drive}")), "it opens as a tree");
    // a click browses it
    assert_eq!(h.request("ui.clickWidget", json!({"id": row}), T)["ok"], true);
    h.settle(T);
    let browsed = h.app.session.browse.as_ref().map(|b| std::path::PathBuf::from(&b.path));
    assert_eq!(browsed.as_deref(), Some(root.as_path()));
    let _ = std::fs::remove_dir_all(&root);
}
