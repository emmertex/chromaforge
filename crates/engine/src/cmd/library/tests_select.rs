//! Selecting as in Lightroom: Shift / ⌘-click keep the active photo, a click inside the selection
//! keeps it (client note 19), and Select All keeps up with a view that grows (evening note 1).
use lcc_catalog::{Op, Photo, PhotoId, Source};
use serde_json::json;

use crate::{Result, Session};

fn add(s: &mut Session, id: u64) -> Result<()> {
    let photo = Photo::new(
        PhotoId(id),
        Source::File { path: format!("/lightcraft-select/{id:03}.jpg") },
        &format!("{id:03}.jpg"),
        "JPEG",
        40,
        30,
        "2026-10-22",
    );
    s.catalog.apply(Op::AddPhoto { photo: Box::new(photo) })?;
    Ok(())
}

fn session(n: u64) -> Result<Session> {
    let mut s = Session::new();
    for id in 1..=n {
        add(&mut s, id)?;
    }
    s.execute("library.sort", &json!({"key": "fileName", "ascending": true}))?;
    Ok(s)
}

#[test]
fn shift_and_ctrl_clicks_keep_the_active_photo_and_a_click_inside_keeps_the_selection() -> Result<()> {
    let mut s = session(10)?;
    s.execute("library.select", &json!({"ids": [3], "mode": "click"}))?;
    s.execute("library.select", &json!({"ids": [7], "mode": "range"}))?;
    assert_eq!(s.selection.active, Some(PhotoId(3)), "Shift-click: the loupe stays on 3");
    assert_eq!(s.selection.ids.len(), 5);
    s.execute("library.select", &json!({"ids": [9], "mode": "toggle"}))?;
    assert_eq!(s.selection.active, Some(PhotoId(3)), "⌘-click: the loupe stays on 3");
    assert_eq!(s.selection.ids.len(), 6);
    s.execute("library.select", &json!({"ids": [5], "mode": "click"}))?;
    assert_eq!((s.selection.active, s.selection.ids.len()), (Some(PhotoId(5)), 6), "a click inside: 5 active, the set stays");
    s.execute("library.select", &json!({"ids": [10], "mode": "click"}))?;
    assert_eq!((s.selection.active, s.selection.ids.len()), (Some(PhotoId(10)), 1), "a click outside replaces it");
    Ok(())
}

#[test]
fn select_all_takes_in_the_photos_that_join_the_view() -> Result<()> {
    let mut s = session(5)?;
    s.execute("library.selectAll", &json!({}))?;
    assert_eq!(s.selection.ids.len(), 5);
    // an import still running adds photos to the view
    for id in 6..=9 {
        add(&mut s, id)?;
    }
    assert_eq!(s.visible().len(), 9);
    assert_eq!(s.selection.ids.len(), 9, "every photo of the view stays selected");
    // a selection changed by hand is the user's: new photos don't join it
    s.execute("library.select", &json!({"ids": [2], "mode": "toggle"}))?;
    add(&mut s, 10)?;
    assert_eq!(s.visible().len(), 10);
    assert_eq!(s.selection.ids.len(), 8);
    // nor do they in another view
    s.execute("library.selectAll", &json!({}))?;
    s.execute("library.filter", &json!({"text": "00"}))?;
    add(&mut s, 11)?;
    let _ = s.visible();
    assert_eq!(s.selection.ids.len(), 10);
    Ok(())
}
