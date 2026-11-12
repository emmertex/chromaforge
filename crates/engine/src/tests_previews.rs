//! True 1:1 (P1.6): a window of the loupe at 1:1 has the pixels of the full-size export, and the
//! preview store's 1:1 previews, settings and discards.

use lcc_pipeline::{OutputDepth, OutputSpace, PixelWindow};
use serde_json::json;

use crate::Session;
use crate::cmd::previews::{DiscardAfter, discard_full};

/// The largest per-channel difference between the `visible` pixels of `part` (a render of
/// window `win`) and the same pixels of `full`.
fn max_diff(full: &lcc_raster::Rgba8, part: &lcc_raster::Rgba8, win: PixelWindow, visible: PixelWindow) -> u8 {
    let mut worst = 0u8;
    for y in visible.y..visible.y + visible.h {
        for x in visible.x..visible.x + visible.w {
            let a = part.data[(y - win.y) * part.width + (x - win.x)];
            let b = full.data[y * full.width + x];
            for c in 0..3 {
                worst = worst.max(a[c].abs_diff(b[c]));
            }
        }
    }
    worst
}

/// Render photo `id` at full size for export and windows of its 1:1 view, and compare them.
fn check_one_to_one(s: &mut Session, id: lcc_catalog::PhotoId) {
    let p = s.catalog.photo(id).unwrap().clone();
    let export = s.render_export(id, p.width as usize, p.height as usize, OutputSpace::Srgb, OutputDepth::U8).unwrap().image;
    let (w, h) = (export.width, export.height);
    assert!(w.max(h) >= (p.width.max(p.height) as usize) - 1, "export is full size: {w}×{h} of {}×{}", p.width, p.height);
    // the 1:1 frame is the export's frame: the loupe asks for windows of (w, h), with the margin
    // of context it renders around what is on screen (as `region::margin_for` in the UI)
    let margin = ((w.max(h) as f64 * 0.045).min(768.0) as usize).div_ceil(256).max(1) * 256;
    for (fx, fy) in [(0.0, 0.0), (0.37, 0.52), (0.8, 0.7)] {
        let x = ((w as f64 * fx) as usize).min(w.saturating_sub(512));
        let y = ((h as f64 * fy) as usize).min(h.saturating_sub(384));
        let visible = PixelWindow { x, y, w: 512.min(w), h: 384.min(h) };
        let (wx, wy) = (x.saturating_sub(margin), y.saturating_sub(margin));
        let win = PixelWindow { x: wx, y: wy, w: (x + visible.w + margin).min(w) - wx, h: (y + visible.h + margin).min(h) - wy };
        let job = s.region_job(id, w, h, win, true).expect("a window of the 1:1 view");
        let part = job.run().rendered.unwrap().image;
        assert_eq!((part.width, part.height), (win.w, win.h));
        let d = max_diff(&export, &part, win, visible);
        assert!(d <= 2, "1:1 window {win:?} differs from the export by {d} levels");
    }
    // the loupe's tile grid (`region::tiles_for` in the UI): each 1024 px tile is rendered with its
    // margin and cropped back; across a tile boundary — the seam between two renders — the pixels
    // on both sides still equal the export's
    let tile = 1024usize;
    if w > tile && h > tile {
        let (row_y, seam_x) = ((h / tile).saturating_sub(1).min(1) * tile, tile);
        for col in [0usize, 1] {
            let t = PixelWindow { x: col * tile, y: row_y, w: tile.min(w - col * tile), h: tile.min(h - row_y) };
            let (wx, wy) = (t.x.saturating_sub(margin), t.y.saturating_sub(margin));
            let win = PixelWindow { x: wx, y: wy, w: (t.x + t.w + margin).min(w) - wx, h: (t.y + t.h + margin).min(h) - wy };
            let part = s.region_job(id, w, h, win, true).expect("a tile of the 1:1 view").run().rendered.unwrap().image;
            // the 64 px strip of this tile that touches the seam
            let strip = if col == 0 { PixelWindow { x: seam_x - 64, w: 64, ..t } } else { PixelWindow { x: seam_x, w: 64.min(t.w), ..t } };
            let d = max_diff(&export, &part, win, strip);
            assert!(d <= 2, "tile {col} at the seam x = {seam_x} differs from the export by {d} levels");
        }
    }
}

// Given a procedural photo with edits, every 1:1 window equals the full-resolution export
#[test]
fn one_to_one_pixels_equal_full_resolution_export_pixels() {
    let mut s = Session::with_demo();
    let id = s.active().unwrap();
    s.execute("develop.set", &json!({"control": "light.exposure", "value": 0.4})).unwrap();
    s.execute("develop.set", &json!({"control": "effects.clarity", "value": 20})).unwrap();
    s.execute("develop.set", &json!({"control": "detail.sharpenAmount", "value": 60})).ok();
    check_one_to_one(&mut s, id);
}

// Given corpus raws (`cargo xtask corpus --download`), the same holds for real files; skipped
// cleanly without them
#[test]
fn one_to_one_pixels_equal_export_pixels_on_corpus_raws() {
    let dir = std::env::var_os("LCC_CORPUS_RAW")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/raw"));
    let Ok(rd) = std::fs::read_dir(&dir) else {
        eprintln!("skipped: no raw corpus at {}", dir.display());
        return;
    };
    // (LCC_CORPUS_RAW_COUNT files, sorted by name; default 3: each is a full-size render)
    let count = std::env::var("LCC_CORPUS_RAW_COUNT").ok().and_then(|n| n.parse().ok()).unwrap_or(3);
    let mut files: Vec<_> = rd.flatten().map(|e| e.path()).filter(|p| p.is_file()).collect();
    files.sort();
    let step = (files.len() / count.max(1)).max(1);
    let files: Vec<_> = files.into_iter().step_by(step).take(count).collect();
    if files.is_empty() {
        eprintln!("skipped: empty raw corpus");
        return;
    }
    // (the filesystem loader: without it no file decodes)
    let mut s = Session::new().with_fs();
    let paths: Vec<String> = files.iter().map(|p| p.to_string_lossy().to_string()).collect();
    s.execute("library.import", &json!({"paths": paths})).unwrap();
    let ids: Vec<_> = s.catalog.photos().map(|p| p.id).collect();
    assert_eq!(ids.len(), files.len(), "every corpus raw imported");
    for id in &ids {
        check_one_to_one(&mut s, *id);
    }
    eprintln!("1:1 equals export on {} corpus raws", ids.len());
}

// Given 1:1 previews built, they are kept apart from the standard ones and discarded on their own
#[test]
fn one_to_one_previews_are_discarded_on_their_own() {
    let _keys = crate::sensor_maps::keep_generation();
    let mut s = Session::with_demo();
    let id = s.active().unwrap();
    let p = s.catalog.photo(id).unwrap().clone();
    s.execute("library.buildPreviews", &json!({"size": "full", "ids": [id.0], "wait": true})).unwrap();
    let full = Session::full_view_key(&p);
    // (the view preview is written once the render hands it over)
    assert!(s.media.rendered.contains(full), "1:1 preview built");
    let img = s.media.rendered.get(full).unwrap();
    assert_eq!(img.width.max(img.height), p.width.max(p.height) as usize, "at the photo's own size");
    // a month's policy keeps a fresh one
    assert_eq!(discard_full(&s, &[id], DiscardAfter::Month.max_age()).removed, 0);
    let r = s.execute("library.discardPreviews", &json!({"ids": [id.0]})).unwrap();
    assert_eq!(r["removed"], 1);
    assert!(!s.media.rendered.contains(full));
    let r = s.execute("library.discardPreviews", &json!({"ids": [id.0]})).unwrap();
    assert_eq!(r["removed"], 0, "nothing left to discard");
}

// Given the preview settings, they are validated, applied and reported
#[test]
fn preview_settings_are_validated_and_applied() {
    let mut s = Session::with_demo();
    let r = s.execute("library.previewSettings", &json!({})).unwrap();
    assert_eq!(r, json!({"standardEdge": 2048, "discardFull": "month", "atImport": "minimal"}));
    let r = s.execute("library.previewSettings", &json!({"standardEdge": 1440, "discardFull": "never", "atImport": "full"})).unwrap();
    assert_eq!(r, json!({"standardEdge": 1440, "discardFull": "never", "atImport": "full"}));
    assert!(s.execute("library.previewSettings", &json!({"standardEdge": 10})).is_err());
    assert!(s.execute("library.previewSettings", &json!({"discardFull": "fortnight"})).is_err());
    assert!(s.execute("library.previewSettings", &json!({"atImport": 3})).is_err());
    assert!(s.execute("library.discardPreviews", &json!({"size": "standard"})).is_err());
    assert_eq!(s.preview_prefs.standard_edge(), 1440, "a refused change leaves the settings");
}

/// An absurd export size is refused, not a multiply overflow (it panicked before the fix).
#[test]
fn huge_export_size_is_an_error() {
    let mut s = Session::with_demo();
    let id = s.catalog.photos().next().unwrap().id;
    let huge = usize::MAX >> 8;
    assert!(s.render_export(id, huge, huge, OutputSpace::Srgb, OutputDepth::U8).is_err());
    assert!(s.render_export(id, 70_000, 10, OutputSpace::Srgb, OutputDepth::U8).is_err());
    assert!(s.render_now(id, huge, huge).is_err());
    assert!(s.render_now(id, 10, 70_000).is_err());
}

// Given a photo's preview jobs, the grid thumbnail is made from the source the view decoded, not
// by reading the file again (here: a thumbnail job pointing at a file that is not there)
#[test]
fn preview_build_makes_the_thumbnail_from_the_views_source() {
    use crate::cmd::previews::{jobs_for, run_photo};
    use crate::media::SourceRef;
    let mut s = Session::with_demo();
    let id = s.active().unwrap();
    let mut jobs = jobs_for(&mut s, id, Some(512));
    assert_eq!(jobs.len(), 2, "view and thumbnail");
    assert!(jobs[0].view_cache.is_some(), "the view renders first");
    jobs[1].source = SourceRef::File {
        path: "/nonexistent/photo.arw".into(),
        max_edge: 512,
        loader: None,
        fallback: None,
        denoise: None,
        options: Default::default(),
        reduced: None,
    };
    assert!(run_photo(jobs), "the thumbnail rendered from the view's decoded source");
}

// Given many photos, a standard preview build renders them side by side and counts every one
#[test]
fn preview_build_of_many_photos_counts_each_once() {
    let mut s = Session::with_demo();
    let ids: Vec<u64> = s.catalog.photos().map(|p| p.id.0).collect();
    assert!(ids.len() > 4);
    let r = s.execute("library.buildPreviews", &json!({"size": "standard", "edge": 512, "ids": ids, "wait": true})).unwrap();
    assert_eq!(r["done"], json!(ids.len()), "{r}");
    assert_eq!(r["total"], json!(ids.len()));
    assert_eq!(r["failed"], json!(0));
}
