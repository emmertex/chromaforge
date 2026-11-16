//! Edit-menu selection commands (Invert Selection, Deselect Active Photo, Deselect Unflagged
//! Photos) and Remove Photo from Catalog: behaviour on real selections, and on none at all.

use lcc_catalog::{Flag, PhotoId};
use serde_json::json;

use crate::{Selection, Session};

fn demo() -> Session {
    let mut s = Session::with_demo();
    s.selection = Selection::default();
    s
}

fn select(s: &mut Session, ids: &[PhotoId]) {
    let ids: Vec<u64> = ids.iter().map(|i| i.0).collect();
    s.execute("library.select", &json!({"ids": ids, "active": ids[0]})).unwrap();
}

#[test]
fn invert_selection_swaps_selected_and_unselected_in_view() {
    let mut s = demo();
    let vis = s.visible_cloned();
    select(&mut s, &vis[..3]);
    let r = s.execute("library.invertSelection", &json!({})).unwrap();
    assert_eq!(r["selected"].as_u64(), Some(vis.len() as u64 - 3));
    assert!(vis[..3].iter().all(|id| !s.selection.ids.contains(id)));
    assert!(vis[3..].iter().all(|id| s.selection.ids.contains(id)));
    assert!(s.selection.active.is_some_and(|a| s.selection.ids.contains(&a)), "the active photo is one of the selected");
    // twice: back where it was
    s.execute("library.invertSelection", &json!({})).unwrap();
    let mut got = s.selection.ids.clone();
    got.sort();
    let mut want = vis[..3].to_vec();
    want.sort();
    assert_eq!(got, want);
}

#[test]
fn invert_selection_of_everything_or_nothing() {
    let mut s = demo();
    let vis = s.visible_cloned();
    s.execute("library.selectAll", &json!({})).unwrap();
    let r = s.execute("library.invertSelection", &json!({})).unwrap();
    assert_eq!(r["selected"], 0);
    assert!(s.selection.ids.is_empty());
    assert_eq!(s.selection.active, None);
    // nothing selected -> everything
    let r = s.execute("library.invertSelection", &json!({})).unwrap();
    assert_eq!(r["selected"].as_u64(), Some(vis.len() as u64));
}

#[test]
fn deselect_active_hands_over_to_the_next_selected_photo() {
    let mut s = demo();
    let vis = s.visible_cloned();
    s.execute("library.select", &json!({"ids": [vis[0].0, vis[2].0, vis[4].0], "active": vis[2].0})).unwrap();
    s.execute("library.deselectActive", &json!({})).unwrap();
    assert_eq!(s.selection.ids, vec![vis[0], vis[4]]);
    assert_eq!(s.selection.active, Some(vis[4]), "the next selected photo is active");
    // the last one wraps round to the first
    s.execute("library.deselectActive", &json!({})).unwrap();
    assert_eq!(s.selection.ids, vec![vis[0]]);
    assert_eq!(s.selection.active, Some(vis[0]));
    // the only selected photo: nothing stays selected
    let r = s.execute("library.deselectActive", &json!({})).unwrap();
    assert_eq!(r["selected"], 0);
    assert_eq!(s.selection, Selection::default());
}

#[test]
fn deselect_active_without_an_active_photo_is_an_error_not_a_panic() {
    let mut s = demo();
    assert!(s.execute("library.deselectActive", &json!({})).is_err());
    assert_eq!(s.selection, Selection::default());
    // an active photo that isn't in the list (stale state) is left alone
    let vis = s.visible_cloned();
    s.selection = Selection { ids: vec![vis[1]], active: Some(vis[0]) };
    assert!(s.execute("library.deselectActive", &json!({})).is_ok());
    assert_eq!(s.selection.ids, vec![vis[1]]);
}

#[test]
fn deselect_unflagged_keeps_only_the_picks() {
    let mut s = demo();
    let vis = s.visible_cloned();
    select(&mut s, &vis[..4]);
    s.execute("photo.flag", &json!({"ids": [vis[1].0, vis[3].0], "flag": "pick"})).unwrap();
    s.execute("photo.flag", &json!({"ids": [vis[0].0, vis[2].0], "flag": "none"})).unwrap();
    s.execute("library.select", &json!({"ids": [vis[0].0, vis[1].0, vis[2].0, vis[3].0], "active": vis[0].0})).unwrap();
    let r = s.execute("library.deselectUnflagged", &json!({})).unwrap();
    assert_eq!(r["selected"], 2);
    assert_eq!(s.selection.ids, vec![vis[1], vis[3]]);
    assert!(s.selection.active.is_some_and(|a| a == vis[1] || a == vis[3]));
    assert!(s.selection.ids.iter().all(|id| s.catalog.photo(*id).unwrap().flag == Flag::Pick));
}

#[test]
fn deselect_unflagged_with_no_pick_or_no_selection_does_not_panic() {
    let mut s = demo();
    let vis = s.visible_cloned();
    select(&mut s, &vis[..2]);
    s.execute("photo.flag", &json!({"ids": [vis[0].0, vis[1].0], "flag": "none"})).unwrap();
    s.execute("library.select", &json!({"ids": [vis[0].0, vis[1].0], "active": vis[1].0})).unwrap();
    s.execute("library.deselectUnflagged", &json!({})).unwrap();
    assert_eq!(s.selection, Selection::single(vis[1]), "acts as Deselect All");
    s.selection = Selection::default();
    assert!(s.execute("library.deselectUnflagged", &json!({})).is_ok());
    assert_eq!(s.selection, Selection::default());
}

#[test]
fn remove_from_catalog_removes_without_trashing_and_undoes() {
    let mut s = demo();
    let vis = s.visible_cloned();
    let n = vis.len();
    s.execute("library.select", &json!({"ids": [vis[2].0, vis[3].0], "active": vis[2].0})).unwrap();
    let snapshot = s.catalog.to_snapshot();
    let r = s.execute("photo.removeFromCatalog", &json!({})).unwrap();
    assert_eq!(r["deleted"], 2);
    assert!(s.catalog.photo(vis[2]).is_none() && s.catalog.photo(vis[3]).is_none(), "gone, not in Recently Deleted");
    assert_eq!(s.visible_cloned().len(), n - 2);
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(s.catalog.to_snapshot(), snapshot);
}

#[test]
fn remove_from_catalog_hostile_input_is_an_error() {
    let mut s = demo();
    assert!(s.execute("photo.removeFromCatalog", &json!({})).is_err(), "no selection");
    let snapshot = s.catalog.to_snapshot();
    for bad in [json!({"ids": [999_999]}), json!({"ids": "x"}), json!({"ids": [-1]}), json!({"ids": [null]})] {
        let _ = s.execute("photo.removeFromCatalog", &bad);
        assert_eq!(s.catalog.to_snapshot(), snapshot, "{bad}");
    }
}
