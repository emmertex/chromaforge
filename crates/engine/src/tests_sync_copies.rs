//! Sync Copies (`develop.sync {copies}`) and Sync Snapshots (`version.sync`).

use lcc_catalog::PhotoId;
use serde_json::json;

use crate::Session;

fn select(s: &mut Session, id: PhotoId) {
    s.execute("library.select", &json!({"ids": [id.0], "active": id.0})).unwrap();
}

fn exposure(s: &Session, id: PhotoId) -> f64 {
    s.develop_of(id).unwrap().light.exposure
}

/// Syncing from a virtual copy reaches the master and the other copies, not unrelated photos,
/// and undoes in one step.
#[test]
fn sync_copies_reaches_the_master_and_every_copy_only() {
    let mut s = Session::with_demo();
    let ids = s.visible_cloned();
    let (master, other) = (ids[0], ids[1]);
    select(&mut s, master);
    let c1 = PhotoId(s.execute("photo.virtualCopy", &json!({})).unwrap()["ids"][0].as_u64().unwrap());
    select(&mut s, master);
    let c2 = PhotoId(s.execute("photo.virtualCopy", &json!({})).unwrap()["ids"][0].as_u64().unwrap());
    select(&mut s, c1);
    s.execute("develop.set", &json!({"control": "light.exposure", "value": 1.0})).unwrap();
    s.execute("develop.sync", &json!({"groups": ["light"], "copies": true})).unwrap();
    assert_eq!(exposure(&s, master), 1.0);
    assert_eq!(exposure(&s, c2), 1.0);
    assert_eq!(exposure(&s, other), 0.0, "an unrelated photo is untouched");
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(exposure(&s, master), 0.0);
    assert_eq!(exposure(&s, c2), 0.0);
    assert_eq!(exposure(&s, c1), 1.0);
}

/// Sync Snapshots applies only the chosen groups of the current edit to every snapshot, in one undo.
#[test]
fn sync_snapshots_applies_chosen_groups_to_each_version() {
    let mut s = Session::with_demo();
    let id = s.visible_cloned()[0];
    select(&mut s, id);
    assert!(s.execute("version.sync", &json!({"groups": ["light"]})).is_err(), "no snapshots");
    s.execute("version.create", &json!({"name": "A"})).unwrap();
    s.execute("version.create", &json!({"name": "B"})).unwrap();
    s.execute("develop.set", &json!({"control": "light.exposure", "value": 0.7})).unwrap();
    s.execute("develop.set", &json!({"control": "color.vibrance", "value": 40.0})).unwrap();
    s.execute("version.sync", &json!({"groups": ["light"]})).unwrap();
    let versions = s.catalog.photo(id).unwrap().versions.clone();
    assert_eq!(versions.len(), 2);
    for v in &versions {
        assert_eq!(v.settings.light.exposure, 0.7);
        assert_eq!(v.settings.color.vibrance, 0.0, "unticked groups stay");
    }
    s.execute("edit.undo", &json!({})).unwrap();
    assert!(s.catalog.photo(id).unwrap().versions.iter().all(|v| v.settings.light.exposure == 0.0));
}
