//! The Map, Book, Slideshow and Print menu items that used to be "Coming soon": toggles show their
//! state, Delete GPS Coordinates clears the position, Book's page navigation stops at the ends.

use serde_json::json;

use crate::menubar::checked;
use crate::{LccApp, Services};

fn app() -> LccApp {
    let mut session = lcc_engine::Session::with_demo();
    session.selection = lcc_engine::Selection::single(session.visible_cloned()[0]);
    LccApp::new(session, Services { png: None, ..Default::default() })
}

#[test]
fn delete_gps_clears_the_selected_photos_position() {
    let mut a = app();
    a.run("map.geotag", json!({"lat": 48.85, "lon": 2.35})).unwrap();
    let id = a.session.selection.ids[0];
    assert!(a.session.catalog.photo(id).unwrap().meta.gps.is_some());
    a.run("map.geotag", json!({"clear": true})).unwrap();
    assert!(a.session.catalog.photo(id).unwrap().meta.gps.is_none());
}

#[test]
fn map_toggles_flip_and_show_their_state() {
    let mut a = app();
    assert_eq!(checked(&a, "map.toggleLockMarkers"), Some(false));
    assert_eq!(checked(&a, "map.toggleSavedLocations"), Some(true));
    a.run("map.toggleLockMarkers", json!({})).unwrap();
    a.run("map.toggleSavedLocations", json!({})).unwrap();
    assert_eq!(checked(&a, "map.toggleLockMarkers"), Some(true));
    assert_eq!(checked(&a, "map.toggleSavedLocations"), Some(false));
    // locking drops a pin drag already under way
    a.map.dragging_pin = Some(vec![1]);
    a.run("map.toggleLockMarkers", json!({})).unwrap();
    assert!(a.map.dragging_pin.is_none());
    assert!(!a.map.focus_search);
    a.run("map.focusSearch", json!({})).unwrap();
    assert!(a.map.focus_search);
}

#[test]
fn book_guides_toggle_and_page_navigation_stops_at_the_ends() {
    let mut a = app();
    a.run("book.autoLayout", json!({"preset": "Two Per Page"})).unwrap();
    let before = checked(&a, "book.toggleGuides").unwrap();
    a.run("book.toggleGuides", json!({})).unwrap();
    assert_eq!(checked(&a, "book.toggleGuides"), Some(!before));
    let n = a.ui.book.book.all_pages().len();
    assert!(n >= 2, "{n} pages");
    a.run("book.go", json!({"delta": 1000})).unwrap();
    let last = a.ui.book.current;
    a.run("book.go", json!({"delta": 1})).unwrap();
    assert_eq!(a.ui.book.current, last, "Next Page at the end stays put");
    a.run("book.go", json!({"delta": -1000})).unwrap();
    let first = a.ui.book.current;
    a.run("book.go", json!({"delta": -1})).unwrap();
    assert_eq!(a.ui.book.current, first);
    a.run("book.go", json!({"delta": 1})).unwrap();
    assert_ne!(a.ui.book.current, first);
    assert!(a.run("book.go", json!({})).is_err());
    assert!(a.run("book.refreshCaptions", json!({})).is_ok());
}

#[test]
fn slideshow_and_print_guides_and_rulers_toggle() {
    let mut a = app();
    let g = checked(&a, "slideshow.toggleGuides").unwrap();
    a.run("slideshow.toggleGuides", json!({})).unwrap();
    assert_eq!(checked(&a, "slideshow.toggleGuides"), Some(!g));

    assert_eq!(checked(&a, "printui.toggleGuides"), Some(true));
    a.run("printui.toggleGuides", json!({})).unwrap();
    a.run("printui.toggleRulers", json!({})).unwrap();
    assert_eq!(checked(&a, "printui.toggleGuides"), Some(false));
    assert_eq!(checked(&a, "printui.toggleRulers"), Some(false));
    a.run("printui.guides", json!({"show": true, "rulers": true})).unwrap();
    assert_eq!(checked(&a, "printui.toggleGuides"), Some(true));
    assert_eq!(checked(&a, "printui.toggleRulers"), Some(true));
}
