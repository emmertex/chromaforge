//! A picture that stands in for a photo already shows the photo's
//! current crop and look. A picture made with the same look (small slider differences aside) is
//! drawn cut to the current crop, straighten angle and flips at once, as a texture transform with no
//! render; one made with another look (the browse-time preview of a photo imported with a preset)
//! never stands in: the fast draft with the photo's settings does.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use lcc_catalog::{Op, Photo, PhotoId, Source};
use serde_json::json;

use crate::headless::Headless;
use crate::render::Slot;
use crate::{LccApp, Services};

const T: Duration = Duration::from_secs(30);

/// A headless app with `n` raws: their camera JPEGs read at once, a raw decode taking `delay` ms
/// (changeable while the test runs).
fn raws(n: u64, delay: Arc<AtomicU64>) -> Headless {
    let mut s = lcc_engine::Session::new();
    s.media.preview_loader = Some(Arc::new(|_: &str, edge: usize| {
        let w = edge.clamp(1, 300);
        let h = (w * 2 / 3).max(1);
        Some(lcc_raster::Rgba8 { width: w, height: h, data: vec![[200, 60, 200, 255]; w * h] })
    }));
    s.media.file_loader = Some(Arc::new(move |_, _, _| {
        std::thread::sleep(Duration::from_millis(delay.load(Ordering::Relaxed)));
        let mut img = lcc_raster::Rgb32f::new(600, 400);
        img.data.fill([0.18, 0.18, 0.18]);
        Ok((img, Default::default()))
    }));
    for id in 1..=n {
        let mut p =
            Photo::new(PhotoId(id), Source::File { path: format!("shoot/c{id}.cr3") }, &format!("c{id}.cr3"), "CR3", 6000, 4000, "2026-10-25");
        p.kind = lcc_catalog::MediaKind::Raw;
        p.develop = Arc::new(p.camera_defaults());
        s.catalog.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
    }
    let app = LccApp::new(s, Services { png: None, ..Default::default() });
    Headless::new(app, [1400.0, 900.0], 1.0)
}

fn exec(h: &mut Headless, command: &str, params: serde_json::Value) {
    let r = h.request("engine.execute", json!({"command": command, "params": params}), T);
    assert_eq!(r["ok"], true, "{command}: {r}");
}

fn rendered(h: &Headless, id: PhotoId) -> bool {
    h.app.loupe_shown == Some((id, "render"))
        && !h.app.renderer.is_pending(Slot::Main)
        && h.app
            .renderer
            .textures
            .get(&Slot::Main)
            .is_some_and(|t| t.photo == id && matches!(t.quick, None | Some(lcc_engine::media::QuickSource::Cached)))
}

/// Step frames until `done` (at most `max`), handing every frame to `each`.
fn frames(h: &mut Headless, max: Duration, mut each: impl FnMut(&Headless), done: impl Fn(&Headless) -> bool) -> bool {
    let t0 = Instant::now();
    while t0.elapsed() < max {
        h.step();
        each(h);
        if done(h) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    false
}

/// Show photo `id` in the loupe and wait for its render.
fn show(h: &mut Headless, id: u64) {
    exec(h, "library.select", json!({"ids": [id]}));
    let r = h.request("ui.set", json!({"view": "detail"}), T);
    assert_eq!(r["ok"], true, "{r}");
    let ok = frames(h, T, |_| {}, |h| rendered(h, PhotoId(id)));
    let log: Vec<_> = h.app.renderer.loupe_log.iter().rev().take(6).map(|r| (r.width, r.height, r.draft)).collect();
    let main = h.app.renderer.textures.get(&Slot::Main).map(|t| (t.photo, t.quick, t.size));
    assert!(ok, "photo {id} rendered: {:?}, main {main:?}, pending {}, log {log:?}", h.app.loupe_shown, h.app.renderer.is_pending(Slot::Main));
}

/// What the loupe drew this frame, for photo `id`: the source and how it was drawn.
fn drawn(h: &Headless, id: PhotoId) -> Option<(&'static str, crate::panels::detail::LoupeDrawn)> {
    let (p, what) = h.app.loupe_shown?;
    (p == id && what != "none").then_some(())?;
    Some((what, h.app.loupe_drawn?))
}

/// The view's picture fills its rect: no pixel of the frame is empty background.
fn fills(d: &crate::panels::detail::LoupeDrawn, view: egui::Rect) -> bool {
    crate::panels::detail::quad_covers(&d.quad, view.shrink(1.0))
}

/// A photo imported with a preset that crops (its browse preview made as shot, before the
/// preset) shows its crop and look in its first picture: the draft with its settings, never the
/// as-shot preview.
#[test]
fn a_preset_that_crops_shows_cropped_and_toned_in_the_first_picture() {
    let delay = Arc::new(AtomicU64::new(0));
    let mut h = raws(3, delay.clone());
    // the browse-time preview: photo 2 seen as shot
    show(&mut h, 2);
    show(&mut h, 1);
    // the preset: a crop and a look
    let mut d = (*h.app.session.catalog.photo(PhotoId(2)).unwrap().develop).clone();
    d.light.exposure = 0.9;
    d.light.contrast = 30.0;
    d.crop.geometry.rect = lcc_geom::Rect { x0: 0.2, y0: 0.0, x1: 0.8, y1: 1.0 };
    h.app.session.set_develop(PhotoId(2), d, "Preset").unwrap();
    // the raw is slow: what shows first is a stand-in
    delay.store(3000, Ordering::Relaxed);
    exec(&mut h, "library.select", json!({"ids": [2]}));
    let mut seen = Vec::new();
    let mut aspects = Vec::new();
    frames(
        &mut h,
        Duration::from_secs(2),
        |h| {
            if let Some((what, d)) = drawn(h, PhotoId(2)) {
                seen.push((what, d.known));
                aspects.push(h.app.image_rect.map(|r| r.width() / r.height()));
            }
        },
        |h| h.app.loupe_shown == Some((PhotoId(2), "draft")),
    );
    assert!(!seen.is_empty(), "something showed");
    // (the draft, or the filmstrip's thumbnail made with the preset when that comes first)
    assert!(matches!(seen[0].0, "draft" | "thumb") && seen.iter().all(|s| s.1), "the first picture has the preset: {seen:?}");
    assert!(!seen.iter().any(|s| s.0 == "last"), "the as-shot preview never stands in: {seen:?}");
    // the crop's shape: 0.6 of a 3:2 frame across, all of it down = 0.9
    for a in aspects.iter().flatten() {
        assert!((a - 0.9).abs() < 0.03, "cropped: {aspects:?}");
    }
    delay.store(0, Ordering::Relaxed);
}

/// A crop synced to photos rendered before is on screen at once on each of them, drawn from
/// their last picture: no render comes before the first cropped frame.
#[test]
fn stepping_through_synced_crops_shows_each_crop_with_no_render() {
    let delay = Arc::new(AtomicU64::new(0));
    let mut h = raws(6, delay.clone());
    for id in 1..=6 {
        show(&mut h, id);
    }
    show(&mut h, 1);
    let crop = lcc_geom::CropGeometry { rect: lcc_geom::Rect { x0: 0.1, y0: 0.3, x1: 0.6, y1: 0.9 }, angle: 2.0 };
    for id in 1..=6 {
        let mut d = (*h.app.session.catalog.photo(PhotoId(id)).unwrap().develop).clone();
        d.crop.geometry = crop;
        h.app.session.set_develop(PhotoId(id), d, "Sync Crop").unwrap();
    }
    delay.store(3000, Ordering::Relaxed);
    for id in 2..=6u64 {
        let pid = PhotoId(id);
        let renders = h.app.renderer.loupe_log.len();
        exec(&mut h, "library.select", json!({"ids": [id]}));
        let mut first = None;
        // every frame until the render: cut to the crop and filling the view, whatever stands in
        let mut every = Vec::new();
        let ok = frames(
            &mut h,
            Duration::from_secs(2),
            |h| {
                if let Some((what, d)) = drawn(h, pid) {
                    if first.is_none() {
                        first = Some((what, d, h.app.image_rect, h.app.renderer.loupe_log.len()));
                    }
                    let full = h.app.image_rect.is_some_and(|v| fills(&d, v));
                    if !(d.placed && full) {
                        every.push((what, d.placed, full));
                    }
                }
            },
            |_| false,
        );
        let _ = ok;
        let (what, d, view, log) = first.unwrap_or_else(|| {
            let pre = h.app.renderer.textures.get(&Slot::Preview).map(|t| (t.photo, t.quick, t.size, t.placement.is_some()));
            panic!(
                "photo {id}: nothing shown: {:?}, preview {pre:?}, pending preview {} main {}, prefetches {}, workers {} queued {} in flight {}",
                h.app.loupe_shown,
                h.app.renderer.is_pending(Slot::Preview),
                h.app.renderer.is_pending(Slot::Main),
                h.app.renderer.prefetches_pending(),
                h.app.renderer.live_workers(),
                h.app.renderer.queued(),
                h.app.renderer.in_flight()
            )
        });
        assert_eq!(log, renders, "photo {id}: no render before its first frame");
        assert!(d.placed, "photo {id}: drawn cut to the synced crop ({what}): {d:?}");
        let view = view.expect("a view");
        assert!(fills(&d, view), "photo {id}: the frame is full: {d:?} in {view:?}");
        assert!((view.width() / view.height() - 0.5 * 1.5 / 0.6).abs() < 0.03, "photo {id}: the crop's shape: {view:?}");
        assert!(every.is_empty(), "photo {id}: frames not cut to the crop or not full: {every:?}");
    }
    delay.store(0, Ordering::Relaxed);
}

/// After Straighten with Constrain Crop and a crop dragged into a corner, leaving Crop
/// while the cropped render is held back shows the crop from the crop tool's picture, with no
/// empty strip.
#[test]
fn leaving_a_constrained_straightened_crop_never_shows_empty_strips() {
    let delay = Arc::new(AtomicU64::new(0));
    let mut h = raws(2, delay.clone());
    show(&mut h, 1);
    exec(&mut h, "panel.crop", json!({}));
    // Constrain Crop first, then the crop: turned on in the same edit, it makes the crop again from
    // the whole frame (`Session::fit_constrained_crop`), and a whole frame turned 3° (no warp for
    // Constrain Crop to shrink it into) has corners off the photo: no picture of the photo fills
    // them, and only a thumbnail made with that crop (drawn as it is) passed, when it came in time
    let mut d = (*h.app.session.catalog.photo(PhotoId(1)).unwrap().develop).clone();
    d.geometry.constrain_crop = true;
    h.app.session.set_develop(PhotoId(1), d, "Constrain Crop").unwrap();
    let mut d = (*h.app.session.catalog.photo(PhotoId(1)).unwrap().develop).clone();
    // dragged into the bottom-right corner, as far as it goes turned 3° (all of it on the photo)
    d.crop.geometry = lcc_geom::CropGeometry { rect: lcc_geom::Rect { x0: 0.55, y0: 0.5, x1: 0.93, y1: 0.92 }, angle: 3.0 };
    h.app.session.set_develop(PhotoId(1), d, "Crop").unwrap();
    assert_eq!(h.app.session.catalog.photo(PhotoId(1)).unwrap().develop.crop.geometry.rect.x1, 0.93, "the crop is kept as drawn");
    assert!(frames(&mut h, T, |_| {}, |h| rendered(h, PhotoId(1))), "the crop tool's render: {:?}", h.app.loupe_shown);
    delay.store(3000, Ordering::Relaxed);
    h.app.run("tool.done", json!({})).expect("done");
    let mut bad = Vec::new();
    let mut shown = 0;
    frames(
        &mut h,
        Duration::from_millis(1500),
        |h| {
            if let (Some((what, d)), Some(view)) = (drawn(h, PhotoId(1)), h.app.image_rect) {
                shown += 1;
                if !fills(&d, view) {
                    bad.push((what, d, view));
                }
            }
        },
        |_| false,
    );
    assert!(shown > 0, "the photo is on screen");
    assert!(bad.is_empty(), "frames with empty background: {bad:?}");
    delay.store(0, Ordering::Relaxed);
}

/// The same, the photo cropped before: the crop tool opens and the render is held back the whole
/// time (a slow raw). Leaving it shows the new straightened, constrained crop in its own shape from
/// the pictures on hand, with no empty strip and nothing of the old crop.
#[test]
fn a_crop_made_while_renders_are_held_back_shows_in_its_own_shape() {
    let delay = Arc::new(AtomicU64::new(0));
    let mut h = raws(2, delay.clone());
    let mut d = (*h.app.session.catalog.photo(PhotoId(1)).unwrap().develop).clone();
    d.crop.geometry.rect = lcc_geom::Rect { x0: 0.1, y0: 0.1, x1: 0.9, y1: 0.9 };
    h.app.session.set_develop(PhotoId(1), d, "Crop").unwrap();
    show(&mut h, 1);
    // the filmstrip's cells have their pictures too
    frames(&mut h, Duration::from_secs(2), |_| {}, |h| h.app.renderer.thumb(PhotoId(1)).is_some_and(|t| t.quick.is_none()));
    delay.store(4000, Ordering::Relaxed);
    exec(&mut h, "panel.crop", json!({}));
    frames(&mut h, Duration::from_millis(300), |_| {}, |_| false);
    let mut d = (*h.app.session.catalog.photo(PhotoId(1)).unwrap().develop).clone();
    d.geometry.constrain_crop = true;
    // (all of it on the photo, turned 3°: a crop past the photo's edge has corners no picture of
    // the photo fills; the render paints them a light neutral)
    d.crop.geometry = lcc_geom::CropGeometry { rect: lcc_geom::Rect { x0: 0.5, y0: 0.45, x1: 0.95, y1: 0.93 }, angle: 3.0 };
    h.app.session.set_develop(PhotoId(1), d, "Crop").unwrap();
    // the picture on hand with the new crop is the filmstrip's thumbnail (the loupe's renders are
    // held back): leaving Crop before it is made has nothing to show the new crop from, so wait for
    // it rather than for a time a loaded machine may not make it in
    let cropped = h.app.session.placement_of(PhotoId(1), true).expect("a placement");
    let ok = frames(
        &mut h,
        T,
        |_| {},
        |h| h.app.renderer.thumb(PhotoId(1)).is_some_and(|t| t.quick.is_none() && t.placement.is_some_and(|p| p.uv_to_norm == cropped.uv_to_norm)),
    );
    assert!(ok, "the filmstrip's thumbnail of the new crop");
    h.app.run("tool.done", json!({})).expect("done");
    let want = h.app.session.placement_of(PhotoId(1), true).expect("a placement");
    let mut bad = Vec::new();
    let mut shown = 0;
    frames(
        &mut h,
        Duration::from_millis(1500),
        |h| {
            if let (Some((what, d)), Some(view)) = (drawn(h, PhotoId(1)), h.app.image_rect) {
                shown += 1;
                let shape = view.width() / view.height() / want.aspect - 1.0;
                if !fills(&d, view) || !d.known || shape.abs() > 0.02 {
                    bad.push((what, d, view, shape));
                }
            }
        },
        |_| false,
    );
    assert!(shown > 0, "the photo is on screen");
    assert!(bad.is_empty(), "frames with empty background, the old crop or another shape: {bad:?}");
    delay.store(0, Ordering::Relaxed);
}
