//! Headless tests of the left sidebar's Keywords section on a large library (issue #1011): with
//! thousands of keywords listed, a frame draws only the rows near the screen, and the rows out
//! of sight still take their room, so scrolling reaches the last keyword.

use std::time::Duration;

use lcc_catalog::{Op, Photo, PhotoId, Source};
use lcc_engine::Session;
use serde_json::json;

use crate::headless::Headless;
use crate::{LccApp, Services};

const T: Duration = Duration::from_secs(30);
const SETTLE: Duration = Duration::from_secs(120);

/// `photos` photos with six keywords each out of `keywords` top-level ones (each with a child).
fn library(photos: u64, keywords: u64) -> Session {
    let mut s = Session::new();
    for i in 0..photos {
        let path = format!("/lcc-kw-test/IMG_{i:05}.jpg");
        let mut p = Photo::new(PhotoId(i + 1), Source::File { path }, &format!("IMG_{i:05}.jpg"), "JPEG", 60, 40, "2026-01-01T00:00:00");
        p.meta.keywords = (0..6).map(|k| format!("Keyword {:05}|Child", (i * 6 + k) % keywords)).collect();
        s.catalog.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
    }
    s
}

fn app(photos: u64, keywords: u64) -> Headless {
    let app = LccApp::new(library(photos, keywords), Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [1400.0, 900.0], 1.0);
    // the Keywords section is upstream's My Photos sidebar's: Develop's "My Photos" panel, alone in its column
    let r = h.request("ui.set", json!({"view": "detail", "right": "edit", "leftPanel": true}), T);
    assert_eq!(r["ok"], true, "{r}");
    crate::tests_panels::my_photos_only(&mut h);
    h
}

/// The keyword rows drawn this frame (every row is listed as `source:…`; only a drawn one has its
/// `label:…`).
fn keyword_rows(h: &Headless) -> Vec<String> {
    h.app.widgets.iter().filter_map(|(w, _)| w.strip_prefix("label:keyword:")).map(str::to_string).collect()
}

/// Given a library with 2,000 keywords, when the sidebar's Keywords section is open, then a frame
/// draws only the few dozen rows that fit on screen (not all 2,000), and scrolling to the end
/// draws the last keyword: the rows out of sight keep their height.
#[test]
fn keywords_section_draws_only_the_rows_near_the_screen() {
    const KEYWORDS: u64 = 2000;
    let mut h = app(KEYWORDS / 6 + 1, KEYWORDS);
    assert_eq!(h.app.caches.keyword_tree(&h.app.session.catalog).len(), KEYWORDS as usize);
    let rows = keyword_rows(&h);
    assert!(!rows.is_empty() && rows.len() < 100, "{} of {KEYWORDS} keyword rows drawn", rows.len());
    assert!(rows.contains(&"Keyword 00000".to_string()), "{rows:?}");
    // all the way down
    crate::tests_panels::scroll_sidebar(&mut h, -(KEYWORDS as f32) * 40.0);
    let rows = keyword_rows(&h);
    assert!(!rows.is_empty() && rows.len() < 100, "{} of {KEYWORDS} keyword rows drawn", rows.len());
    assert!(rows.contains(&"Keyword 01999".to_string()), "the last keyword is reached: {rows:?}");
    assert!(!rows.contains(&"Keyword 00000".to_string()), "{rows:?}");
}

/// A keyword holding line breaks (a camera preset's whole keyword list read as one keyword by
/// 0.8.x: "Beautiful Bride\r\n Bride and Groom\r\n …") was drawn many lines tall over the rows
/// around it: the ghost text in the Keywords section. A row's name is drawn on one line.
#[test]
fn a_keyword_with_line_breaks_is_drawn_on_one_line() {
    let mut s = Session::new();
    let list: Vec<String> = (0..20).map(|i| format!("Wedding Keyword {i:02}")).collect();
    let mut p = Photo::new(PhotoId(1), Source::File { path: "/lcc-kw-test/IMG_1.jpg".into() }, "IMG_1.jpg", "JPEG", 60, 40, "2026-01-01T00:00:00");
    p.meta.keywords = vec![list.join("\r\n "), "Solo".into()];
    s.catalog.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
    let app = LccApp::new(s, Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [1400.0, 900.0], 1.0);
    let r = h.request("ui.set", json!({"view": "detail", "right": "edit", "leftPanel": true}), T);
    assert_eq!(r["ok"], true, "{r}");
    crate::tests_panels::my_photos_only(&mut h);
    h.settle(SETTLE);
    let solo = h.app.widgets.iter().find(|(w, _)| w == "label:keyword:Solo").map(|(_, r)| *r).expect("the Solo row");
    let long = h.app.widgets.iter().find(|(w, _)| w.starts_with("label:keyword:Wedding Keyword 00")).map(|(_, r)| *r).expect("the long row");
    assert!(long.height() <= solo.height() + 0.5, "one line, as tall as any name: {long:?} vs {solo:?}");
}
