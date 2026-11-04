//! Upright per photo (test notes 2026-10-24 notes 4, 5, 6 and 16): a mode that reaches other photos is
//! analysed on each, in the background in the app; a second click analyses afresh (ported from OneSlips).

use std::sync::Arc;

use lcc_catalog::PhotoId;
use lcc_develop::Upright;
use serde_json::json;

use crate::Session;
use crate::media::SourceLevel;

/// A photo of white posts on a dark wall, the camera rolled by `roll` degrees, as photo `id`'s source.
fn posts(s: &mut Session, id: PhotoId, roll: f64) {
    let (w, h) = (600usize, 400usize);
    let (sn, cs) = roll.to_radians().sin_cos();
    let img = lcc_raster::Rgb32f::from_fn(w, h, |x, y| {
        let (dx, dy) = (x as f64 - 300.0, y as f64 - 200.0);
        // the scene, turned back
        let u = dx * cs + dy * sn;
        let on = (u + 1000.0).rem_euclid(110.0) < 22.0;
        if on { [0.9; 3] } else { [0.08; 3] }
    });
    let img = Arc::new(img);
    s.media.insert(id, SourceLevel::Thumb, img.clone());
    s.media.insert(id, SourceLevel::Preview, img);
}

/// Three demo photos, each with posts rolled differently, all selected (the first active) with Auto Sync.
fn three(background: bool) -> (Session, [PhotoId; 3]) {
    let mut s = Session::with_demo();
    s.upright_jobs.background = background;
    let ids: Vec<PhotoId> = s.visible_cloned().into_iter().take(3).collect();
    let (a, b, c) = (ids[0], ids[1], ids[2]);
    for (id, roll) in [(a, 2.0), (b, -3.0), (c, 1.0)] {
        posts(&mut s, id, roll);
    }
    s.execute("library.select", &json!({"ids": [a.0, b.0, c.0], "active": a.0})).unwrap();
    s.execute("develop.autoSync", &json!({"on": true})).unwrap();
    (s, [a, b, c])
}

fn transform(s: &Session, id: PhotoId) -> Option<lcc_geom::Homography> {
    s.develop_of(id).unwrap().geometry.upright_transform
}

/// Note 4a: Auto Sync carries the MODE; every photo is analysed on its own pixels, off the UI thread with one
/// activity row, the results merged into the settings as they arrive; one undo step for all.
#[test]
fn auto_sync_analyses_upright_on_each_photo_in_the_background() {
    let (mut s, [a, b, c]) = three(true);
    let undo = s.undo.len();
    s.execute("geometry.upright", &json!({"mode": "vertical"})).unwrap();
    assert_eq!(s.undo.len(), undo + 1, "one undo step");
    assert!(transform(&s, a).is_some(), "the active photo is analysed at once");
    for id in [b, c] {
        assert_eq!(s.develop_of(id).unwrap().geometry.upright, Upright::Vertical, "the mode travels");
    }
    // (the worker may be quick, but the results only land when polled)
    assert!(transform(&s, b).is_none() && transform(&s, c).is_none(), "not the active photo's correction");
    assert_eq!(s.upright_jobs.pending(), 2);
    assert!(s.activity.list().iter().any(|t| t.kind == "upright"), "an activity row while it runs");
    let t0 = std::time::Instant::now();
    while s.upright_jobs.pending() > 0 {
        s.upright_poll();
        assert!(t0.elapsed().as_secs() < 60, "the analyses finish");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(!s.activity.list().iter().any(|t| t.kind == "upright"), "the row goes when done");
    let (ta, tb, tc) = (transform(&s, a).unwrap(), transform(&s, b).unwrap(), transform(&s, c).unwrap());
    assert_eq!(Some(tb), s.upright_now(b, Upright::Vertical).ok(), "b's own correction");
    assert_eq!(Some(tc), s.upright_now(c, Upright::Vertical).ok(), "c's own correction");
    assert!(ta != tb && tb != tc, "each photo its own");
    assert_eq!(s.undo.len(), undo + 1, "the results are no undo step of their own");
    s.execute("edit.undo", &json!({})).unwrap();
    for id in [a, b, c] {
        assert_eq!(s.develop_of(id).unwrap().geometry.upright, Upright::Off, "one undo takes all back");
    }
}

/// Note 4a: Paste (and presets, Sync) carry Upright as a recipe too: the photo pasted on gets its own
/// correction, never the copied photo's; without a worker (CLI, MCP) at once.
#[test]
fn pasted_upright_is_analysed_on_the_photo_it_lands_on() {
    let (mut s, [a, b, _]) = three(false);
    s.execute("develop.autoSync", &json!({"on": false})).unwrap();
    s.execute("library.select", &json!({"ids": [a.0], "active": a.0})).unwrap();
    s.execute("geometry.upright", &json!({"mode": "vertical"})).unwrap();
    s.execute("develop.copy", &json!({"groups": ["geometry"]})).unwrap();
    s.execute("library.select", &json!({"ids": [b.0], "active": b.0})).unwrap();
    s.execute("develop.paste", &json!({})).unwrap();
    let gb = s.develop_of(b).unwrap().geometry.clone();
    assert_eq!(gb.upright, Upright::Vertical);
    assert_eq!(gb.upright_transform, s.upright_now(b, Upright::Vertical).ok(), "b's own correction");
    assert_ne!(gb.upright_transform, transform(&s, a), "not a's");
}

/// Decision 2026-10-25 (item 12): Guided Upright is drawn on one photo's lines, so syncing it is skipped: Auto
/// Sync, Paste and presets leave the other photos with the Upright they have (mode, correction, guides); the
/// Transform sliders still travel.
#[test]
fn guided_upright_is_not_synced_the_other_photos_keep_their_own() {
    let (mut s, [a, b, c]) = three(false);
    s.execute("develop.autoSync", &json!({"on": false})).unwrap();
    s.execute("library.select", &json!({"ids": [b.0], "active": b.0})).unwrap();
    s.execute("geometry.upright", &json!({"mode": "vertical"})).unwrap();
    let gb = s.develop_of(b).unwrap().geometry.clone();
    assert!(gb.upright_transform.is_some());
    // Auto Sync: Guided on a, b keeps Vertical with its own correction, c stays Off
    s.execute("library.select", &json!({"ids": [a.0, b.0, c.0], "active": a.0})).unwrap();
    s.execute("develop.autoSync", &json!({"on": true})).unwrap();
    s.execute("geometry.guides", &json!({"guides": [[0.2, 0.1, 0.25, 0.9], [0.8, 0.1, 0.75, 0.9]]})).unwrap();
    s.execute("develop.set", &json!({"control": "geometry.rotate", "value": 2.0})).unwrap();
    assert_eq!(s.develop_of(a).unwrap().geometry.upright, Upright::Guided);
    let g = s.develop_of(b).unwrap().geometry.clone();
    assert_eq!((g.upright, g.upright_transform, g.guides.len()), (Upright::Vertical, gb.upright_transform, 0), "b keeps its own");
    assert_eq!(g.rotate, 2.0, "the Transform sliders still travel");
    assert_eq!(s.develop_of(c).unwrap().geometry.upright, Upright::Off, "c keeps Off");
    // Copy / Paste (and presets: the same recipe rule) of a Guided photo
    s.execute("develop.autoSync", &json!({"on": false})).unwrap();
    s.execute("library.select", &json!({"ids": [a.0], "active": a.0})).unwrap();
    s.execute("develop.copy", &json!({"groups": ["geometry"]})).unwrap();
    s.execute("library.select", &json!({"ids": [b.0], "active": b.0})).unwrap();
    s.execute("develop.paste", &json!({})).unwrap();
    let g = s.develop_of(b).unwrap().geometry.clone();
    assert_eq!((g.upright, g.upright_transform, g.guides.len()), (Upright::Vertical, gb.upright_transform, 0), "paste keeps b's own");
    // a mode that isn't Guided still travels
    s.execute("library.select", &json!({"ids": [a.0], "active": a.0})).unwrap();
    s.execute("geometry.upright", &json!({"mode": "level"})).unwrap();
    s.execute("develop.copy", &json!({"groups": ["geometry"]})).unwrap();
    s.execute("library.select", &json!({"ids": [c.0], "active": c.0})).unwrap();
    s.execute("develop.paste", &json!({})).unwrap();
    assert_eq!(s.develop_of(c).unwrap().geometry.upright, Upright::Level);
}

/// Note 6 and decision 2026-10-25 (item 11): a second click on the mode that is on analyses it afresh and
/// (Guided) clears the guides, one undo step; the Transform sliders the photographer set stay.
#[test]
fn a_second_click_on_the_upright_mode_re_analyses_and_keeps_the_sliders() {
    let (mut s, [a, ..]) = three(false);
    s.execute("library.select", &json!({"ids": [a.0], "active": a.0})).unwrap();
    s.execute("geometry.upright", &json!({"mode": "vertical"})).unwrap();
    s.execute("develop.set", &json!({"control": "geometry.rotate", "value": 4.0})).unwrap();
    s.execute("develop.set", &json!({"control": "geometry.vertical", "value": 20.0})).unwrap();
    s.execute("geometry.upright", &json!({"mode": "vertical", "again": true})).unwrap();
    let g = s.develop_of(a).unwrap().geometry.clone();
    assert_eq!((g.rotate, g.vertical), (4.0, 20.0), "the Transform sliders are kept");
    assert_eq!(g.upright, Upright::Vertical);
    assert_eq!(g.upright_transform, s.upright_now(a, Upright::Vertical).ok());
    // Guided: the guides go
    s.execute("geometry.guides", &json!({"guides": [[0.2, 0.1, 0.25, 0.9], [0.8, 0.1, 0.75, 0.9]]})).unwrap();
    assert_eq!(s.develop_of(a).unwrap().geometry.guides.len(), 2);
    let undo = s.undo.len();
    s.execute("geometry.upright", &json!({"mode": "guided", "again": true})).unwrap();
    let g = s.develop_of(a).unwrap().geometry.clone();
    assert_eq!(g.upright, Upright::Guided);
    assert!(g.guides.is_empty(), "a second click on Guided clears the guides");
    assert_eq!((g.rotate, g.vertical), (4.0, 20.0), "and keeps the sliders");
    assert_eq!(s.undo.len(), undo + 1, "one undo step");
    // a plain run (the Update button, an agent) keeps the sliders
    s.execute("geometry.upright", &json!({"mode": "vertical"})).unwrap();
    s.execute("develop.set", &json!({"control": "geometry.rotate", "value": 3.0})).unwrap();
    s.execute("geometry.upright", &json!({"mode": "vertical"})).unwrap();
    assert_eq!(s.develop_of(a).unwrap().geometry.rotate, 3.0);
}
