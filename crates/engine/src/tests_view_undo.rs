//! Navigation is on the undo stack: a new source, a new filter and a new selection are steps, so Undo
//! never changes a photo the person can't see.

use lcc_catalog::PhotoId;
use serde_json::json;

use crate::{LibrarySource, Selection, Session};

fn rating(s: &Session, id: PhotoId) -> u8 {
    s.catalog.photo(id).unwrap().rating
}

fn demo_with_album() -> (Session, u64) {
    let mut s = Session::with_demo();
    let id = s.execute("album.create", &json!({"name": "Trip", "addSelected": true})).unwrap()["id"].as_u64().unwrap();
    (s, id)
}

#[test]
fn choosing_a_source_is_an_undo_step() {
    let (mut s, album) = demo_with_album();
    s.execute("library.source", &json!({"kind": "album", "id": album})).unwrap();
    assert_eq!(s.source, LibrarySource::Album(lcc_catalog::AlbumId(album)));
    let label = s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(s.source, LibrarySource::All, "Undo goes back to the previous source: {label}");
    s.execute("edit.redo", &json!({})).unwrap();
    assert_eq!(s.source, LibrarySource::Album(lcc_catalog::AlbumId(album)), "Redo shows it again");
}

#[test]
fn changing_the_filter_is_an_undo_step() {
    let mut s = Session::with_demo();
    s.execute("library.filter", &json!({"rating": 3})).unwrap();
    assert_eq!(s.filter.rating, 3);
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(s.filter.rating, 0, "Undo restores the previous filter");
    s.execute("edit.redo", &json!({})).unwrap();
    assert_eq!(s.filter.rating, 3);
}

#[test]
fn changing_the_selection_is_an_undo_step_and_a_repeat_is_not() {
    let mut s = Session::with_demo();
    let vis = s.visible_cloned();
    s.execute("library.select", &json!({"ids": [vis[0].0]})).unwrap();
    let steps = s.undo.len();
    s.execute("library.select", &json!({"ids": [vis[0].0]})).unwrap();
    assert_eq!(s.undo.len(), steps, "selecting what is selected already adds no step");
    s.execute("library.select", &json!({"ids": [vis[1].0, vis[2].0], "active": vis[2].0})).unwrap();
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(s.selection, Selection::single(vis[0]), "Undo restores the selected photos and the active one");
    s.execute("edit.redo", &json!({})).unwrap();
    assert_eq!(s.selection.ids, vec![vis[1], vis[2]]);
    assert_eq!(s.selection.active, Some(vis[2]));
}

#[test]
fn undo_after_an_edit_and_a_source_change_shows_the_photo_before_it_reverts_it() {
    let (mut s, album) = demo_with_album();
    let vis = s.visible_cloned();
    let edited = vis.iter().copied().find(|id| !s.catalog.albums_of(*id).contains(&lcc_catalog::AlbumId(album))).unwrap();
    s.execute("library.select", &json!({"ids": [edited.0]})).unwrap();
    let before = rating(&s, edited);
    s.execute("photo.rate", &json!({"rating": if before == 4 { 2 } else { 4 }})).unwrap();
    s.execute("library.source", &json!({"kind": "album", "id": album})).unwrap();
    assert!(!s.visible().contains(&edited), "the edited photo is no longer in the view");

    s.execute("edit.undo", &json!({})).unwrap();
    assert_ne!(rating(&s, edited), before, "the first Undo only takes the view back: the edit stays");
    assert!(s.visible().contains(&edited) && s.selection.active == Some(edited), "the edited photo is shown and selected again");

    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(rating(&s, edited), before, "the second Undo reverts the edit on the photo that is on screen");
}

#[test]
fn a_new_navigation_or_edit_clears_redo() {
    let mut s = Session::with_demo();
    s.execute("library.filter", &json!({"rating": 3})).unwrap();
    s.execute("edit.undo", &json!({})).unwrap();
    assert!(!s.redo.is_empty());
    s.execute("library.source", &json!({"kind": "picks"})).unwrap();
    assert!(s.redo.is_empty(), "navigation after an Undo ends the Redo chain, as an edit does");
}

#[test]
fn an_edit_is_one_step_even_though_it_leaves_the_view_alone() {
    let mut s = Session::with_demo();
    let steps = s.undo.len();
    s.execute("photo.rate", &json!({"rating": 5})).unwrap();
    assert_eq!(s.undo.len(), steps + 1);
}

#[test]
fn a_navigation_step_writes_nothing_to_the_catalog_log() {
    let mut s = Session::with_demo();
    let log = s.pending_log.len();
    s.execute("library.filter", &json!({"rating": 2})).unwrap();
    s.execute("library.source", &json!({"kind": "picks"})).unwrap();
    s.execute("edit.undo", &json!({})).unwrap();
    s.execute("edit.redo", &json!({})).unwrap();
    assert_eq!(s.pending_log.len(), log, "a navigation step and its Undo and Redo are not catalog changes");
}

#[test]
fn bringing_back_a_view_with_a_collection_that_has_gone_shows_all_photos() {
    let mut s = Session::with_demo();
    let mut view = s.view_state();
    view.source = LibrarySource::Album(lcc_catalog::AlbumId(999_999));
    s.apply_view_state(&view);
    assert_eq!(s.source, LibrarySource::All, "no panic, and the view points at what is there");
    // (a step whose collection went is still taken, and the history goes on)
    s.execute("library.filter", &json!({"rating": 2})).unwrap();
    assert!(s.undo_step().is_ok());
}

#[test]
fn typing_in_the_text_filter_is_one_step_for_the_whole_word() {
    let mut s = Session::with_demo();
    let steps = s.undo.len();
    for text in ["b", "be", "bea", "beach"] {
        s.execute("library.filter", &json!({"text": text})).unwrap();
    }
    assert_eq!(s.undo.len(), steps + 1, "one step, not one per key");
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(s.filter.text, "", "Undo takes back the whole word");
}

#[test]
fn stepping_to_the_next_photo_is_labelled_as_lightroom_does() {
    let mut s = Session::with_demo();
    let vis = s.visible_cloned();
    s.execute("library.select", &json!({"ids": [vis[0].0]})).unwrap();
    s.execute("library.next", &json!({})).unwrap();
    assert_eq!(s.undo.last().map(|e| e.label.as_str()), Some("Go to Next Photo"));
}

#[test]
fn a_word_finished_with_enter_starts_a_new_step() {
    let mut s = Session::with_demo();
    let steps = s.undo.len();
    s.execute("library.filter", &json!({"text": "be"})).unwrap();
    s.execute("library.filter", &json!({"text": "beach"})).unwrap();
    s.end_typing();
    s.execute("library.filter", &json!({"text": "beach sunset"})).unwrap();
    assert_eq!(s.undo.len(), steps + 2, "one step for the first word, one for what came after Enter");
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(s.filter.text, "beach");
}

#[test]
fn the_steps_hold_a_bounded_number_of_photo_ids() {
    let mut s = Session::with_demo();
    let everything = |n: u64| Selection { ids: (1..=n).map(PhotoId).collect(), active: None };
    for _ in 0..4 {
        let mut before = s.view_state();
        before.selection = everything(900_000);
        s.view_changed("library.select", before);
    }
    let held: usize = s.undo.iter().filter_map(|e| e.view.as_ref()).map(|v| v.apply.selection.ids.len() + v.inverse.selection.ids.len()).sum();
    assert!(held <= 2_000_000, "the oldest steps give way: {held} ids held");
    assert!(s.undo.last().is_some_and(|e| e.view.is_some()), "the newest step stays");
}

#[test]
fn a_by_hand_change_of_the_view_is_a_step() {
    let mut s = Session::with_demo();
    let vis = s.visible_cloned();
    s.execute("library.select", &json!({"ids": [vis[0].0, vis[1].0], "active": vis[0].0})).unwrap();
    s.navigating(|s| s.selection.active = Some(vis[1]));
    assert_eq!(s.selection.active, Some(vis[1]));
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(s.selection.active, Some(vis[0]));
}

#[test]
fn navigation_goes_before_the_edits_when_the_history_is_full() {
    let mut s = Session::with_demo();
    let vis = s.visible_cloned();
    s.execute("photo.rate", &json!({"rating": 4})).unwrap();
    for i in 0..(crate::UNDO_STEPS + 200) {
        s.execute("library.select", &json!({"ids": [vis[i % 2].0]})).unwrap();
    }
    assert!(s.undo.len() <= crate::UNDO_STEPS);
    assert!(s.undo.iter().any(|e| e.label == "Set Rating"), "holding an arrow key must not push the person's edits out of the history");
}

#[test]
fn the_newest_step_stays_even_when_it_alone_is_over_the_id_budget() {
    let mut s = Session::with_demo();
    let mut before = s.view_state();
    before.selection = Selection { ids: (1..=3_000_000).map(PhotoId).collect(), active: None };
    s.view_changed("library.select", before);
    assert!(s.undo.last().is_some_and(|e| e.is_navigation()), "the last thing the person did can still be undone");
}

#[test]
fn a_run_that_folds_its_steps_leaves_the_navigation_between_them_as_steps() {
    let mut s = Session::with_demo();
    let vis = s.visible_cloned();
    let steps = s.undo.len();
    s.execute("photo.rate", &json!({"rating": 1})).unwrap();
    s.execute("library.select", &json!({"ids": [vis[1].0]})).unwrap();
    s.execute("photo.rate", &json!({"rating": 2})).unwrap();
    s.merge_undo(s.undo.len() - steps, "Import");
    let labels: Vec<&str> = s.undo.iter().skip(steps).map(|e| e.label.as_str()).collect();
    assert_eq!(labels, ["Import", "Selection"], "the two edits are one step, the move stays");
}

#[test]
fn while_the_app_moves_the_view_by_itself_nothing_is_a_step() {
    let mut s = Session::with_demo();
    let vis = s.visible_cloned();
    let steps = s.undo.len();
    let was = s.record_navigation(false);
    s.execute("library.select", &json!({"ids": [vis[1].0]})).unwrap();
    s.navigating(|s| s.selection = Selection::single(vis[2]));
    s.record_navigation(was);
    assert_eq!(s.undo.len(), steps);
    s.execute("library.select", &json!({"ids": [vis[0].0]})).unwrap();
    assert_eq!(s.undo.len(), steps + 1, "and it is back on afterwards");
}

#[test]
fn stepping_back_a_photo_is_labelled_too() {
    let mut s = Session::with_demo();
    let vis = s.visible_cloned();
    s.execute("library.select", &json!({"ids": [vis[2].0]})).unwrap();
    s.execute("library.previous", &json!({})).unwrap();
    assert_eq!(s.undo.last().map(|e| e.label.as_str()), Some("Go to Previous Photo"));
}

#[test]
fn undoing_back_to_a_collection_that_was_deleted_shows_all_photos_and_the_history_goes_on() {
    let (mut s, album) = demo_with_album();
    s.execute("library.source", &json!({"kind": "album", "id": album})).unwrap();
    s.execute("library.source", &json!({"kind": "all"})).unwrap();
    s.execute("album.delete", &json!({"id": album})).unwrap();
    s.execute("edit.undo", &json!({})).unwrap(); // the deletion comes back
    s.execute("album.delete", &json!({"id": album})).unwrap();
    // the step that shows the deleted collection is still on the stack
    let mut steps = 0;
    while s.undo_step().is_ok() && steps < 10 {
        steps += 1;
        assert!(
            match s.source {
                LibrarySource::Album(a) => s.catalog.album(a).is_some(),
                _ => true,
            },
            "the view never points at a collection that is gone"
        );
    }
    assert!(steps >= 2, "Undo kept going through the steps around the deleted collection");
}

#[test]
fn a_navigation_step_and_its_undo_and_redo_commit_nothing() {
    let mut s = Session::with_demo();
    let commits = s.commits();
    s.execute("library.filter", &json!({"rating": 2})).unwrap();
    s.execute("edit.undo", &json!({})).unwrap();
    s.execute("edit.redo", &json!({})).unwrap();
    assert_eq!(s.commits(), commits, "the library is not marked changed by navigation");
}

#[test]
fn an_edit_after_an_undo_clears_redo() {
    let mut s = Session::with_demo();
    s.execute("library.filter", &json!({"rating": 3})).unwrap();
    s.execute("edit.undo", &json!({})).unwrap();
    assert!(!s.redo.is_empty());
    s.execute("photo.rate", &json!({"rating": 5})).unwrap();
    assert!(s.redo.is_empty(), "a new edit ends the Redo chain");
}
