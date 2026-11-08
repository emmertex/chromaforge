//! `library.hideRejected` (View > Hide Rejected Photos) through the real engine.
use lcc_catalog::{Flag, Op, Photo, PhotoId, Source};
use serde_json::json;

use crate::{Result, Session};

/// Five photos: 1 picked, 2 and 5 rejected, 3 and 4 unflagged; 1, 2 and 4 have five stars.
fn session() -> Result<Session> {
    let mut s = Session::new();
    for (id, rating, flag) in [(1, 5, Flag::Pick), (2, 5, Flag::Reject), (3, 2, Flag::None), (4, 5, Flag::None), (5, 1, Flag::Reject)] {
        let mut photo =
            Photo::new(PhotoId(id), Source::File { path: format!("/hide-rejected/{id}.jpg") }, &format!("{id}.jpg"), "JPEG", 40, 30, "2026-11-02");
        photo.rating = rating;
        photo.flag = flag;
        s.catalog.apply(Op::AddPhoto { photo: Box::new(photo) })?;
    }
    Ok(s)
}

fn shown(s: &mut Session) -> Vec<u64> {
    let mut ids: Vec<u64> = s.visible().iter().map(|p| p.0).collect();
    ids.sort();
    ids
}

#[test]
fn it_hides_the_rejects_and_shows_them_again() {
    let mut s = session().unwrap();
    assert!(!s.rejects_hidden());
    let r = s.execute("library.hideRejected", &json!({})).unwrap();
    assert_eq!((r["hidden"].clone(), r["count"].clone()), (json!(true), json!(3)));
    assert_eq!(shown(&mut s), vec![1, 3, 4]);
    assert!(s.rejects_hidden());
    s.execute("library.hideRejected", &json!({})).unwrap();
    assert_eq!(shown(&mut s), vec![1, 2, 3, 4, 5]);
    assert!(!s.rejects_hidden());
}

#[test]
fn the_rest_of_the_filter_stays_and_a_bad_call_changes_nothing() {
    let mut s = session().unwrap();
    s.execute("library.filter", &json!({"rating": 5})).unwrap();
    s.execute("library.hideRejected", &json!({})).unwrap();
    assert_eq!(shown(&mut s), vec![1, 4]);
    s.execute("library.hideRejected", &json!({"hide": false})).unwrap();
    assert_eq!(shown(&mut s), vec![1, 2, 4]);
    assert!(s.execute("library.hideRejected", &json!({"hide": "yes"})).is_err());
    assert_eq!(shown(&mut s), vec![1, 2, 4]);
}
