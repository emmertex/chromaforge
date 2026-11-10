//! Preparing photos in the background ([`crate::prep`]): a build skips photos whose previews are
//! there and leaves what it built on disk, the background build follows the user, gives way to a
//! build that was asked for, to the photo on screen and to an export, is one cancellable row in the
//! activity stack, prepares only the photos its settings name, and photos whose settings changed
//! off screen are rendered again. The settings are app-wide or a library's own.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use lcc_catalog::PhotoId;
use serde_json::json;

use crate::Session;
use crate::prep::{Mode, Scope};

fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lcc-preview-prep-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A 600 × 400 PNG whose pixels differ for every `n` (no two are duplicates).
fn png_n(p: &Path, n: usize) {
    let img = lcc_raster::Rgba8::from_fn(600, 400, |x, y| [((x + n / 256) % 251) as u8, (y % 241) as u8, (n % 256) as u8, 255]);
    let b =
        crate::export::encode_image(&img, &crate::export::ExportOptions { format: crate::export::ExportFormat::Png, ..Default::default() }).unwrap();
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, b).unwrap();
}

/// A library on disk with `n` photos (600 × 400 PNGs) imported, in name order.
fn library(tag: &str, n: usize) -> (Session, PathBuf, Vec<PhotoId>) {
    let root = temp_dir(tag);
    let paths: Vec<String> = (0..n)
        .map(|i| {
            let p = root.join("shoot").join(format!("p{i:03}.png"));
            png_n(&p, i * 7 + 1);
            p.to_string_lossy().to_string()
        })
        .collect();
    let mut s = Session::new().with_fs();
    s.open_library(root.join("lib"), false).unwrap();
    s.execute("library.import", &json!({"paths": paths})).unwrap();
    let mut ids: Vec<(String, PhotoId)> = s.catalog.photos().map(|p| (p.file_name.clone(), p.id)).collect();
    ids.sort();
    (s, root, ids.into_iter().map(|(_, id)| id).collect())
}

fn ids_json(ids: &[PhotoId]) -> Vec<u64> {
    ids.iter().map(|i| i.0).collect()
}

/// Whether photo `id`'s standard preview (at `edge`) for its current settings is in the cache.
fn has_view(s: &mut Session, id: PhotoId, edge: usize) -> bool {
    let jobs = crate::cmd::previews::jobs_for(s, id, Some(edge));
    crate::cmd::previews::already_built(&jobs)
}

/// Wait (at most 30 s) until `b` is done.
fn wait_done(b: &crate::cmd::previews::PreviewBuild) -> bool {
    for _ in 0..1500 {
        if !b.running() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    false
}

/// A build skips photos whose previews are there (built before, or by the loupe), renders those
/// whose preview is too small, and its previews are on disk when it is done, for the next session.
#[test]
fn a_build_skips_what_is_there_and_leaves_it_on_disk() {
    let _keys = crate::sensor_maps::keep_generation();
    let (mut s, root, ids) = library("skip", 4);
    let r = s.execute("library.buildPreviews", &json!({"ids": ids_json(&ids), "edge": 300, "wait": true})).unwrap();
    assert_eq!((r["done"].as_u64(), r["skipped"].as_u64(), r["failed"].as_u64()), (Some(4), Some(0), Some(0)), "{r}");
    for id in &ids {
        assert!(has_view(&mut s, *id, 300), "built");
    }
    // again: nothing is rendered
    let r = s.execute("library.buildPreviews", &json!({"ids": ids_json(&ids), "edge": 300, "wait": true})).unwrap();
    assert_eq!((r["done"].as_u64(), r["skipped"].as_u64()), (Some(4), Some(4)), "{r}");
    // forced, or bigger: rendered
    let r = s.execute("library.buildPreviews", &json!({"ids": [ids[0].0], "edge": 300, "wait": true, "force": true})).unwrap();
    assert_eq!(r["skipped"], 0, "{r}");
    let r = s.execute("library.buildPreviews", &json!({"ids": ids_json(&ids), "edge": 500, "wait": true})).unwrap();
    assert_eq!(r["skipped"], 0, "too small for what is asked: built again {r}");
    // a library opened again finds them on disk (memory is empty)
    drop(s);
    let mut s = Session::new().with_fs();
    s.open_library(root.join("lib"), false).unwrap();
    let r = s.execute("library.buildPreviews", &json!({"ids": ids_json(&ids), "edge": 500, "wait": true})).unwrap();
    assert_eq!(r["skipped"], 4, "{r}");
    assert!(s.loupe_job(ids[1], 400, 400, true).unwrap().cached_view().is_some(), "the loupe uses the preview built in the last session");
    let _ = std::fs::remove_dir_all(root);
}

/// The background preparation builds standard previews at least as big as the loupe on this
/// screen asks for, so the loupe can show them.
#[test]
fn background_previews_are_as_big_as_the_loupe() {
    let _keys = crate::sensor_maps::keep_generation();
    let mut s = Session::with_demo();
    let standard = s.preview_prefs.standard_edge();
    assert_eq!(crate::cmd::previews::standard_edge(&s), standard);
    s.loupe_edge = standard + 900;
    assert_eq!(crate::cmd::previews::standard_edge(&s), standard + 900);
    s.loupe_edge = 100;
    assert_eq!(crate::cmd::previews::standard_edge(&s), standard);
}

/// A background build goes from where the user is, in the direction they step, and follows them
/// when they jump.
#[test]
fn the_background_build_starts_where_the_user_is() {
    use crate::cmd::previews::PreviewBuild;
    let _keys = crate::sensor_maps::keep_generation();
    let b = PreviewBuild { order: (10..20).collect(), ..Default::default() };
    let mut queue: Vec<Option<u64>> = (10..20).map(Some).collect();
    b.focus_on(PhotoId(14), false);
    let mut took = Vec::new();
    for _ in 0..3 {
        let i = b.next_index(&queue).unwrap();
        took.push(queue[i].take().unwrap());
    }
    assert_eq!(took, vec![14, 15, 16], "forward from the photo on screen");
    b.focus_on(PhotoId(12), true);
    let i = b.next_index(&queue).unwrap();
    assert_eq!(queue[i], Some(12), "jumped back: from there, backwards");
    queue[i] = None;
    assert_eq!(b.next_index(&queue).map(|i| queue[i]), Some(Some(11)));
    b.focus_on(PhotoId(19), false);
    for _ in 0..6 {
        let i = b.next_index(&queue).unwrap();
        queue[i] = None;
    }
    assert!(b.next_index(&queue).is_none(), "all taken");
}

/// The background build waits while the photo on screen is being made, shows as one row in the
/// activity stack that can be cancelled, and gives way to a build that is asked for.
#[test]
fn the_background_build_waits_for_the_photo_on_screen_and_gives_way() {
    let _keys = crate::sensor_maps::keep_generation();
    let (mut s, root, ids) = library("replace", 3);
    // held back: the photo on screen is being made all along
    let hold = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let h = hold.clone();
    let holder = std::thread::spawn(move || {
        while h.load(Ordering::Relaxed) {
            crate::prep::foreground();
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    });
    s.execute("library.buildPreviews", &json!({"ids": ids_json(&ids), "edge": 300, "background": true})).unwrap();
    let bg = s.preview_build.clone().unwrap();
    assert!(bg.background && bg.running());
    std::thread::sleep(std::time::Duration::from_millis(600));
    assert_eq!(bg.done.load(Ordering::Relaxed), 0, "nothing built while the photo on screen is being made");
    let row = s.activity.list().into_iter().find(|t| t.label == "Preparing photos in the background").expect("its row");
    assert!(row.cancellable, "{row:?}");
    // a second background build is refused; one that is asked for replaces it
    assert!(s.execute("library.buildPreviews", &json!({"ids": ids_json(&ids), "background": true})).is_err());
    hold.store(false, Ordering::Relaxed);
    holder.join().unwrap();
    let r = s.execute("library.buildPreviews", &json!({"ids": ids_json(&ids), "edge": 300, "wait": true})).unwrap();
    assert_eq!(r["done"], 3, "{r}");
    assert!(bg.cancel.load(Ordering::Relaxed), "the background build was stopped");
    assert!(wait_done(&bg));
    // its row's ✕ stops it: the same flag
    s.execute("library.buildPreviews", &json!({"ids": ids_json(&ids), "edge": 400, "background": true})).unwrap();
    let bg = s.preview_build.clone().unwrap();
    let row = s.activity.list().into_iter().find(|t| t.label == "Preparing photos in the background").expect("its row");
    assert!(s.activity.cancel(row.id).is_ok(), "the row can be stopped");
    assert!(bg.cancel.load(Ordering::Relaxed));
    assert!(wait_done(&bg));
    let _ = std::fs::remove_dir_all(root);
}

/// The background preparation starts once the user steps through photos in the loupe (not when a
/// library opens), builds the view's previews, and is not started again for the same view.
#[test]
fn the_background_preparation_starts_when_culling() {
    let _keys = crate::sensor_maps::keep_generation();
    let (mut s, root, ids) = library("prep", 4);
    let mode = Mode::Library;
    s.prep_tick(mode, None, false);
    s.prep_tick(mode, Some(ids[0]), false);
    assert!(s.preview_build.is_none(), "opening and looking at one photo starts nothing");
    s.prep_tick(mode, Some(ids[1]), false);
    s.prep_tick(mode, Some(ids[2]), false);
    let b = s.preview_build.clone().expect("culling: the view is prepared");
    assert!(b.background);
    assert_eq!(b.total, 4);
    for _ in 0..600 {
        s.prep_tick(mode, Some(ids[2]), false);
        if !b.running() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(!b.running(), "built");
    assert_eq!(b.done.load(Ordering::Relaxed), 4);
    s.prep_tick(mode, Some(ids[3]), false);
    assert!(s.preview_build.as_ref().is_some_and(|p| std::sync::Arc::ptr_eq(p, &b)), "not started again for the same view");
    let _ = std::fs::remove_dir_all(root);
}

/// "Which photos: Picks, rated and labelled" prepares only those; unflagged photos are left alone.
/// "Only the photo on screen" prepares nothing in the background.
#[test]
fn picks_only_leaves_the_unflagged_photos_alone() {
    let _keys = crate::sensor_maps::keep_generation();
    let (mut s, root, ids) = library("marked", 6);
    s.selection = crate::Selection { ids: ids.clone(), active: Some(ids[0]) };
    s.execute("photo.flag", &json!({"ids": [ids[1].0], "flag": "pick"})).unwrap();
    s.execute("photo.rate", &json!({"ids": [ids[4].0], "rating": 3})).unwrap();
    s.execute("prefs.setBackgroundPrep", &json!({"settings": {"library": {"scope": "marked"}}})).unwrap();
    for at in [ids[0], ids[1], ids[2]] {
        s.prep_tick(Mode::Library, Some(at), false);
    }
    let b = s.preview_build.clone().expect("prepared");
    let mut order = b.order.clone();
    order.sort();
    assert_eq!(order, vec![ids[1].0, ids[4].0], "only the pick and the rated photo");
    assert!(wait_done(&b));
    // only the photo on screen: nothing in the background, nothing ahead
    let (mut s, root2, ids) = library("onscreen", 4);
    s.execute("prefs.setBackgroundPrep", &json!({"settings": {"library": {"scope": "onScreen"}}})).unwrap();
    for at in [ids[0], ids[1], ids[2], ids[3]] {
        s.prep_tick(Mode::Library, Some(at), false);
    }
    assert!(s.preview_build.is_none());
    assert_eq!(s.prep_settings().ahead(Mode::Library), 0);
    assert_eq!(s.prep_settings().eye_ahead(Mode::Library), 0);
    let _ = std::fs::remove_dir_all(root);
    let _ = std::fs::remove_dir_all(root2);
}

/// The settings round-trip: app-wide ones in the app's shared settings, and a library's own (a
/// timelapse library prepares nothing while a wedding library prepares everything) saved with it;
/// a reset takes the library back to the app-wide ones.
#[test]
fn the_settings_round_trip_app_wide_and_per_library() {
    let _keys = crate::sensor_maps::keep_generation();
    let root = temp_dir("own");
    let store = root.join("store");
    let mut s2 = Session::new().with_fs().with_app_store(&store);
    s2.open_library(root.join("lib"), false).unwrap();
    // app-wide
    let r = s2.execute("prefs.setBackgroundPrep", &json!({"settings": {"library": {"ahead": 10, "eyeAhead": 3}}})).unwrap();
    assert_eq!(r["own"], false, "{r}");
    assert_eq!(r["settings"]["library"]["ahead"], 10);
    // this library's own
    let r = s2.execute("prefs.setBackgroundPrep", &json!({"library": true, "settings": {"culling": {"previews": false, "ahead": 0}}})).unwrap();
    assert_eq!(r["own"], true, "{r}");
    assert_eq!(r["settings"]["culling"]["ahead"], 0);
    assert_eq!(r["settings"]["library"]["ahead"], 10, "the library's own start from the app-wide ones");
    assert_eq!(r["app"]["culling"]["ahead"], 5, "the app-wide ones are unchanged");
    // out of range and malformed values
    let r = s2.execute("prefs.setBackgroundPrep", &json!({"library": true, "settings": {"culling": {"eyeAhead": 200}}})).unwrap();
    assert_eq!(r["settings"]["culling"]["eyeAhead"], crate::prep::MAX_AHEAD);
    assert!(s2.execute("prefs.setBackgroundPrep", &json!({"settings": 3})).is_err());
    assert!(s2.execute("prefs.setBackgroundPrep", &json!({"settings": {"library": {"scope": "sometimes"}}})).is_err());
    drop(s2);
    // both come back: the library's own from its prefs, the app-wide ones from the store
    let mut s = Session::new().with_fs().with_app_store(&store);
    s.open_library(root.join("lib"), false).unwrap();
    assert!(s.prep.library.is_some(), "kept with the library");
    assert_eq!(s.prep_settings().culling.ahead, 0);
    assert!(!s.prep_settings().culling.previews);
    assert_eq!(s.prep_settings().ahead(Mode::Culling), 0);
    assert_eq!(s.prep.app.library.ahead, 10, "the app-wide ones kept in the store");
    assert_eq!(s.prep.app.library.eye_ahead, 3);
    assert!(s.execute("prefs.setBackgroundPrep", &json!({"reset": true, "settings": {"culling": {"ahead": 3}}})).is_err());
    assert_eq!(s.prep_settings().culling.ahead, 0, "reset with valid settings keeps the library setting");
    assert!(s.execute("prefs.setBackgroundPrep", &json!({"reset": true, "settings": 3})).is_err());
    assert_eq!(s.prep_settings().culling.ahead, 0, "invalid reset keeps the library setting");
    s.execute("prefs.setBackgroundPrep", &json!({"reset": true})).unwrap();
    assert!(s.prep.library.is_none());
    assert_eq!(s.prep_settings().culling.ahead, 5, "back to the app-wide settings");
    assert_eq!(s.prep_settings().library.ahead, 10);
    assert_eq!(s.prep_settings().scope(Mode::Library), Scope::All);
    let _ = std::fs::remove_dir_all(root);
}

/// `library` and `reset` that aren't true or false, and an unknown setting, are errors that
/// leave the app-wide and the library's own settings as they were.
#[test]
fn background_prep_flags_must_be_booleans() {
    let _keys = crate::sensor_maps::keep_generation();
    let root = temp_dir("flags");
    let mut s = Session::new().with_fs();
    s.open_library(root.join("lib"), false).unwrap();
    s.execute("prefs.setBackgroundPrep", &json!({"settings": {"culling": {"ahead": 2}}})).unwrap();
    s.execute("prefs.setBackgroundPrep", &json!({"library": true, "settings": {"culling": {"ahead": 1}}})).unwrap();
    let before = s.execute("prefs.backgroundPrep", &json!({})).unwrap();
    for p in [
        json!({"library": "yes", "settings": {"culling": {"ahead": 4}}}),
        json!({"library": 1, "settings": {"culling": {"ahead": 4}}}),
        json!({"reset": "true"}),
        json!({"reset": 1}),
        json!({"settings": {"culling": {"ahead": 4}, "unknown": true}}),
        json!({"library": true, "settings": {"develop": {"ahead": 0, "unknown": true}}}),
    ] {
        assert!(s.execute("prefs.setBackgroundPrep", &p).is_err(), "{p}");
        let after = s.execute("prefs.backgroundPrep", &json!({})).unwrap();
        assert_eq!(after["app"], before["app"], "{p} keeps the app-wide settings");
        assert_eq!(after["library"], before["library"], "{p} keeps the library's own settings");
    }
    let _ = std::fs::remove_dir_all(root);
}

/// Resetting a library's settings with no library open is an error and leaves the app-wide
/// settings as they were.
#[test]
fn background_prep_reset_needs_an_open_library() {
    let _keys = crate::sensor_maps::keep_generation();
    let mut s = Session::new();
    s.execute("prefs.setBackgroundPrep", &json!({"settings": {"culling": {"ahead": 2}}})).unwrap();
    assert!(s.execute("prefs.setBackgroundPrep", &json!({"reset": true})).is_err());
    let r = s.execute("prefs.backgroundPrep", &json!({})).unwrap();
    assert_eq!(r["app"]["culling"]["ahead"], 2, "{r}");
    assert_eq!(r["own"], false, "{r}");
}

/// Photos whose settings changed while they were not on screen (Auto Sync, Sync, paste, presets)
/// are rendered again in the background once the edits settle; the photo on screen is left to the
/// loupe.
#[test]
fn photos_changed_off_screen_are_rendered_again_in_the_background() {
    let _keys = crate::sensor_maps::keep_generation();
    let (mut s, root, ids) = library("resync", 5);
    let edge = crate::cmd::previews::standard_edge(&s);
    for at in [ids[0], ids[1], ids[0]] {
        s.prep_tick(Mode::Library, Some(at), false);
    }
    let first = s.preview_build.clone().expect("the view is prepared");
    for _ in 0..1500 {
        s.prep_tick(Mode::Library, Some(ids[0]), false);
        if !first.running() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(has_view(&mut s, ids[3], edge));
    // synced from photo 0: photos 2 and 3 (and 0 itself, on screen) change
    for id in [ids[0], ids[2], ids[3]] {
        let mut d = (*s.catalog.photo(id).unwrap().develop).clone();
        d.light.exposure = 0.8;
        s.set_develop(id, d, "Sync").unwrap();
    }
    assert!(!has_view(&mut s, ids[2], edge) && !has_view(&mut s, ids[3], edge));
    let mut again = None;
    for _ in 0..1500 {
        s.prep_tick(Mode::Library, Some(ids[0]), false);
        if let Some(b) = s.preview_build.clone().filter(|b| !std::sync::Arc::ptr_eq(b, &first) && !b.running()) {
            again = Some(b);
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let b = again.expect("a build for the changed photos ran");
    assert!(b.background);
    assert_eq!(b.total, 2, "only the photos changed off screen");
    assert!(has_view(&mut s, ids[2], edge) && has_view(&mut s, ids[3], edge), "their previews are made for the new settings");
    let _ = std::fs::remove_dir_all(root);
}

/// While the user's own work runs (an export), the background build pauses completely and its row
/// says so; it carries on once the export ends.
#[test]
fn the_background_build_pauses_while_an_export_runs() {
    let _keys = crate::sensor_maps::keep_generation();
    let (mut s, root, ids) = library("pause", 3);
    let export = s.activity.start_batch("export", "Exporting", crate::activity::Cancel::Yes);
    assert_eq!(s.activity.user_work(), Some("export"));
    s.execute("library.buildPreviews", &json!({"ids": ids_json(&ids), "edge": 300, "background": true})).unwrap();
    let b = s.preview_build.clone().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(800));
    assert_eq!(b.done.load(Ordering::Relaxed), 0, "nothing prepared while the export runs");
    let row = s.activity.list().into_iter().find(|t| t.label == "Preparing photos in the background").expect("its row");
    assert_eq!(row.detail, "Paused while exporting");
    drop(export);
    assert_eq!(s.activity.user_work(), None);
    assert!(wait_done(&b));
    assert_eq!(b.done.load(Ordering::Relaxed), 3, "carried on once the export ended");
    let _ = std::fs::remove_dir_all(root);
}

/// In a library the background build prepares the photos around the one on screen, not the whole
/// (already edited) gallery.
#[test]
fn the_background_build_prepares_around_the_photo_on_screen() {
    let _keys = crate::sensor_maps::keep_generation();
    let (mut s, root, ids) = library("around", 130);
    let at = ids[100];
    for a in [ids[98], ids[99], at] {
        s.prep_tick(Mode::Library, Some(a), false);
    }
    let b = s.preview_build.clone().expect("prepared");
    b.cancel.store(true, Ordering::Relaxed);
    assert!(b.total <= 3 * crate::prep::AROUND && b.total < ids.len(), "{} of {}", b.total, ids.len());
    assert!(b.order.contains(&at.0) && b.order.contains(&ids[129].0) && !b.order.contains(&ids[0].0));
    assert!(wait_done(&b));
    let _ = std::fs::remove_dir_all(root);
}

/// Two folders of `n` photos each (600 × 400 PNGs), not imported: a library on disk to browse them in.
fn two_folders(tag: &str, n: usize) -> (Session, PathBuf, [String; 2]) {
    let root = temp_dir(tag);
    let dirs = ["first", "second"].map(|d| root.join(d));
    for (k, dir) in dirs.iter().enumerate() {
        for i in 0..n {
            png_n(&dir.join(format!("p{i:03}.png")), k * 100 + i * 7 + 1);
        }
    }
    let mut s = Session::new().with_fs();
    s.open_library(root.join("lib"), false).unwrap();
    (s, root, dirs.map(|d| d.to_string_lossy().to_string()))
}

/// The ids of the photos in view, in order.
fn in_view(s: &mut Session) -> Vec<PhotoId> {
    s.visible_cloned()
}

/// A folder browsed after another is prepared, not the folder left behind: the build of the first
/// folder stops and one of the second starts (2026-11-09: a wedding browsed after a calibration
/// folder kept "Preparing photos in the background" on the calibration folder's 323 photos, and
/// nothing started for the wedding when it ended).
#[test]
fn browsing_another_folder_prepares_that_folder() {
    let _keys = crate::sensor_maps::keep_generation();
    let (mut s, root, [first, second]) = two_folders("refolder", 4);
    // held back all along, so the first folder's build is still running when the user moves on
    let hold = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let h = hold.clone();
    let holder = std::thread::spawn(move || {
        while h.load(Ordering::Relaxed) {
            crate::prep::foreground();
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    });
    s.execute("library.browse", &json!({"path": first})).unwrap();
    let a = in_view(&mut s);
    assert_eq!(Mode::of(&s), Mode::Culling);
    s.prep_tick(Mode::Culling, Some(a[0]), false);
    let first_build = s.preview_build.clone().expect("the first folder is prepared");
    assert!(first_build.order.contains(&a[0].0));
    s.execute("library.browse", &json!({"path": second})).unwrap();
    let b = in_view(&mut s);
    assert!(b.iter().all(|id| !a.contains(id)), "another folder's photos");
    let mut second_build = None;
    for _ in 0..500 {
        s.prep_tick(Mode::Culling, Some(b[0]), false);
        if let Some(p) = s.preview_build.clone().filter(|p| !std::sync::Arc::ptr_eq(p, &first_build)) {
            second_build = Some(p);
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(first_build.cancel.load(Ordering::Relaxed), "the folder left behind is no longer prepared");
    let second_build = second_build.expect("the folder in view is prepared");
    assert!(second_build.background);
    assert_eq!(second_build.total, 4);
    assert!(b.iter().all(|id| second_build.order.contains(&id.0)), "{:?}", second_build.order);
    hold.store(false, Ordering::Relaxed);
    holder.join().unwrap();
    assert!(wait_done(&first_build) && wait_done(&second_build));
    let _ = std::fs::remove_dir_all(root);
}

/// A folder browsed to be culled is prepared as soon as one of its photos is looked at, in the
/// loupe or the grid: the user waited on photo 1 for minutes and nothing was prepared (a library
/// still waits for a few steps, [`crate::prep::STEPS_BEFORE_BUILD`]).
#[test]
fn a_folder_being_culled_is_prepared_from_the_first_photo() {
    let _keys = crate::sensor_maps::keep_generation();
    let (mut s, root, [first, _]) = two_folders("first-photo", 3);
    s.execute("library.browse", &json!({"path": first})).unwrap();
    let a = in_view(&mut s);
    s.prep_tick(Mode::Culling, Some(a[0]), false);
    let b = s.preview_build.clone().expect("prepared from the first photo in the loupe");
    assert_eq!(b.total, 3);
    assert!(wait_done(&b));
    let (mut s2, root2, [first2, _]) = two_folders("first-photo-grid", 3);
    s2.execute("library.browse", &json!({"path": first2})).unwrap();
    let a2 = in_view(&mut s2);
    s2.execute("library.select", &json!({"ids": [a2[1].0]})).unwrap();
    s2.prep_tick(Mode::Culling, None, false);
    let b2 = s2.preview_build.clone().expect("prepared from the photo selected in the grid");
    assert_eq!(b2.order.get(b2.focus.load(Ordering::Relaxed)), Some(&a2[1].0), "built from the selected photo: {:?}", b2.order);
    assert!(wait_done(&b2));
    let _ = std::fs::remove_dir_all(root);
    let _ = std::fs::remove_dir_all(root2);
}

/// Stepping through a folder keeps the photo on screen being made all the time: the background
/// build still gets ahead, one photo at a time, instead of waiting for good (2026-11-09: 16 photos
/// in 70 s of culling a wedding).
#[test]
fn the_background_build_keeps_going_while_the_user_steps() {
    let _keys = crate::sensor_maps::keep_generation();
    let (mut s, root, ids) = library("steady", 2);
    let hold = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let h = hold.clone();
    let holder = std::thread::spawn(move || {
        while h.load(Ordering::Relaxed) {
            crate::prep::foreground();
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    });
    s.execute("library.buildPreviews", &json!({"ids": ids_json(&ids), "edge": 300, "background": true})).unwrap();
    let b = s.preview_build.clone().unwrap();
    let done = wait_done(&b);
    hold.store(false, Ordering::Relaxed);
    holder.join().unwrap();
    assert!(done, "built while the photo on screen was being made all along");
    assert_eq!(b.done.load(Ordering::Relaxed), 2);
    let _ = std::fs::remove_dir_all(root);
}

/// The background build's decodes are background work: at the memory gate they wait behind the
/// photo on screen's instead of making it wait (left out of the port; with a lane that keeps
/// building while the user steps, the photo on screen never finished in a 1.5 s look).
#[test]
fn the_background_builds_decodes_are_background_work() {
    let _keys = crate::sensor_maps::keep_generation();
    let (s, root, ids) = library("bg-decode", 1);
    // a session that has not decoded the photo yet (the import's decode is kept in memory)
    drop(s);
    let mut s = Session::new().with_fs();
    s.open_library(root.join("lib"), false).unwrap();
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<bool>::new()));
    let log = seen.clone();
    // (every decode through this loader: no reduced-size or embedded-preview shortcut)
    s.media.reduced_loader = None;
    s.media.preview_loader = None;
    s.media.file_loader = Some(std::sync::Arc::new(move |_, _, _| {
        log.lock().unwrap().push(crate::memory::is_background());
        let mut image = lcc_raster::Rgb32f::new(600, 400);
        image.data.fill([0.3, 0.2, 0.1]);
        Ok((image, Default::default()))
    }));
    s.execute("library.buildPreviews", &json!({"ids": ids_json(&ids), "edge": 300, "background": true})).unwrap();
    let b = s.preview_build.clone().unwrap();
    assert!(wait_done(&b));
    let seen = seen.lock().unwrap().clone();
    assert!(
        !seen.is_empty(),
        "the photo was decoded: done {} skipped {} failed {}",
        b.done.load(Ordering::Relaxed),
        b.skipped.load(Ordering::Relaxed),
        b.failed.load(Ordering::Relaxed)
    );
    assert!(seen.iter().all(|bg| *bg), "every decode of the build is background work: {seen:?}");
    let _ = std::fs::remove_dir_all(root);
}

/// The background preparation starts one past the photo on screen, which the loupe renders
/// itself: building it as well decoded the same raw twice while the user stepped. It is built last
/// on that side, never left out.
#[test]
fn the_background_build_leaves_the_photo_on_screen_to_the_loupe() {
    use crate::cmd::previews::PreviewBuild;
    let _keys = crate::sensor_maps::keep_generation();
    let b = PreviewBuild { order: (10..20).collect(), background: true, ..Default::default() };
    let mut queue: Vec<Option<u64>> = (10..20).map(Some).collect();
    b.focus_on(PhotoId(14), false);
    let mut took = Vec::new();
    while let Some(i) = b.next_index(&queue) {
        took.push(queue[i].take().unwrap());
    }
    assert_eq!(took, vec![15, 16, 17, 18, 19, 14, 13, 12, 11, 10], "ahead first, then the photo on screen, then behind");
    let mut queue: Vec<Option<u64>> = (10..20).map(Some).collect();
    b.focus_on(PhotoId(14), true);
    let mut took = Vec::new();
    while let Some(i) = b.next_index(&queue) {
        took.push(queue[i].take().unwrap());
    }
    assert_eq!(took, vec![13, 12, 11, 10, 14, 15, 16, 17, 18, 19], "stepping back: the same, backwards");
    // at either end of the folder
    let mut queue: Vec<Option<u64>> = (10..20).map(Some).collect();
    b.focus_on(PhotoId(10), true);
    assert_eq!(b.next_index(&queue).map(|i| queue[i]), Some(Some(10)));
    b.focus_on(PhotoId(19), false);
    let i = b.next_index(&queue).unwrap();
    assert_eq!(queue[i].take(), Some(19));
}
