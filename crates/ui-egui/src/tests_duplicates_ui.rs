//! Headless tests of the Find Duplicates window: a re-imported copy groups with its original,
//! the best copy stays unmarked, the marks drive the buttons, and Remove sends what is marked to
//! Recently Deleted.

use std::time::Duration;

use serde_json::json;

use crate::headless::Headless;
use crate::{LccApp, Services};

const T: Duration = Duration::from_secs(20);

/// A scratch folder that goes away with the test, however it ends.
struct Scratch(std::path::PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn scratch(name: &str) -> Scratch {
    let d = std::env::temp_dir().join(format!("lc-ui-duplicates-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    Scratch(d)
}

fn gradient_jpeg(path: &std::path::Path, w: usize, h: usize) {
    let img = lcc_raster::Rgba8 {
        width: w,
        height: h,
        data: (0..w * h).map(|i| [((i % w) * 255 / w) as u8, ((i / w) * 255 / h) as u8, 96, 255]).collect(),
    };
    let bytes = lcc_codecs::encode_jpeg(&lcc_codecs::EncodeImage::rgba8(&img), 90, Default::default(), &Default::default()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

#[test]
fn a_reimported_copy_groups_and_remove_sends_the_marked_photo_away() {
    let src = scratch("remove");
    gradient_jpeg(&src.0.join("one.jpg"), 512, 384);
    std::fs::copy(src.0.join("one.jpg"), src.0.join("one-again.jpg")).unwrap();

    let services = Services { png: None, ..Default::default() };
    let mut h = Headless::new(LccApp::new(lcc_engine::Session::with_demo().with_fs(), services), [1300.0, 900.0], 1.0);
    let r = h.request(
        "engine.execute",
        json!({"command": "library.import", "params": {"paths": [src.0.to_string_lossy()], "mode": "add", "importDuplicates": true}}),
        T,
    );
    assert_eq!(r["result"]["imported"].as_array().map(Vec::len), Some(2), "{r}");

    // the window opens with one group: the original first and unmarked, the copy marked
    let r = h.request("engine.execute", json!({"command": "library.findDuplicates", "params": {}}), T);
    assert_eq!(r["result"]["groups"].as_array().map(Vec::len), Some(1), "{r}");
    let marked = r["result"]["groups"][0]["photos"][1]["id"].as_u64().expect("the marked copy's id");
    h.settle(Duration::from_millis(300));
    let f = h.app.duplicates.as_ref().expect("the window is open");
    assert!(f.open);
    assert_eq!(f.groups.len(), 1);
    assert_eq!(f.groups[0].members.len(), 2);
    assert!(!f.groups[0].members[0].marked, "the best copy starts unmarked");
    assert!(f.groups[0].members[1].marked, "the other copy starts marked");
    assert!(h.app.widgets.iter().any(|(w, _)| w == "button:duplicates.remove"), "the Remove button is shown");
    assert!(h.app.widgets.iter().any(|(w, _)| w.starts_with("duplicates:")), "the members are shown");

    // Remove: the marked photo lands in Recently Deleted, the group is gone, the keeper stays
    assert_eq!(h.request("ui.clickWidget", json!({"id": "button:duplicates.remove"}), T)["ok"], true);
    h.step();
    assert!(h.app.session.catalog.photo(lcc_catalog::PhotoId(marked)).is_some_and(|p| p.deleted), "removed to Recently Deleted");
    assert!(h.app.duplicates.as_ref().is_some_and(|f| f.groups.is_empty()), "no group is left");
}

#[test]
fn clicking_a_photo_changes_what_is_marked() {
    let src = scratch("toggle");
    gradient_jpeg(&src.0.join("one.jpg"), 512, 384);
    std::fs::copy(src.0.join("one.jpg"), src.0.join("one-again.jpg")).unwrap();

    let services = Services { png: None, ..Default::default() };
    let mut h = Headless::new(LccApp::new(lcc_engine::Session::with_demo().with_fs(), services), [1300.0, 900.0], 1.0);
    h.request(
        "engine.execute",
        json!({"command": "library.import", "params": {"paths": [src.0.to_string_lossy()], "mode": "add", "importDuplicates": true}}),
        T,
    );
    h.request("engine.execute", json!({"command": "library.findDuplicates", "params": {}}), T);
    h.settle(Duration::from_millis(300));

    // unmark the copy, mark the keeper instead: the buttons follow
    assert_eq!(h.request("ui.clickWidget", json!({"id": "duplicates:1"}), T)["ok"], true);
    h.step();
    let f = h.app.duplicates.as_ref().expect("the window is open");
    assert!(!f.groups[0].members[1].marked, "the copy is unmarked");
    assert_eq!(h.request("ui.clickWidget", json!({"id": "duplicates:0"}), T)["ok"], true);
    h.step();
    let f = h.app.duplicates.as_ref().expect("the window is open");
    assert!(f.groups[0].members[0].marked && !f.groups[0].members[1].marked, "the keeper is marked instead");

    assert_eq!(h.request("ui.clickWidget", json!({"id": "button:duplicates.remove"}), T)["ok"], true);
    h.step();
    let kept = h.app.duplicates.as_ref().expect("the window stays").groups.is_empty();
    assert!(kept, "the group went with the removal");
    let deleted: Vec<u64> = h.app.session.catalog.photos().filter(|p| p.deleted).map(|p| p.id.0).collect();
    assert_eq!(deleted.len(), 1, "one photo in Recently Deleted");
}
