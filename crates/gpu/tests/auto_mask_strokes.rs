//! Auto Mask strokes kept between renders (`strokes.rs`): a return to 100 % and the tiles of a
//! zoomed view take strokes worked out before. A binary of its own: the store is one per process,
//! and other tests' renders would push these tests' strokes out. Skips without a GPU adapter.

use std::sync::{Arc, Mutex, PoisonError};

use lcc_develop::{BrushStroke, DevelopSettings, Mask, MaskComponent, MaskOp, MaskShape};
use lcc_geom::Point;
use lcc_pipeline::{RenderRequest, SourceInfo, render};
use lcc_raster::{Rgb32f, Rgba8};

const MEAN_LSB: f64 = 0.5;
const MAX_LSB: u8 = 3;

/// The tests here take turns: each fills much of the store.
static TURN: Mutex<()> = Mutex::new(());

fn gpu() -> bool {
    let ok = lcc_gpu::available();
    if !ok {
        let why = lcc_gpu::unavailable_reason().unwrap_or_default();
        assert!(!why.contains("kernels failed to build"), "{why}");
        lcc_testkit::skip!(Gpu, "no GPU adapter");
    }
    ok
}

fn scene(i: usize, w: usize, h: usize) -> Arc<Rgb32f> {
    Arc::new(lcc_scenes::demo_library()[i].render(w, h))
}

/// (mean |Δ|, max |Δ|).
fn diff(a: &Rgba8, b: &Rgba8) -> (f64, u8) {
    assert_eq!((a.width, a.height), (b.width, b.height));
    let (mut sum, mut max) = (0u64, 0u8);
    for (p, q) in a.data.iter().zip(&b.data) {
        for c in 0..3 {
            let d = p[c].abs_diff(q[c]);
            sum += d as u64;
            max = max.max(d);
        }
    }
    (sum as f64 / (a.data.len() * 3) as f64, max)
}

/// After Auto Mask mouse strokes with a big brush, each return to 100 % of a 45 MP photo refined
/// every stroke again over the whole view, all queued ahead of the window's frames (over a second
/// at 4K, with no stage cache or once its geometries were pushed out). Each stroke is now refined
/// over the rectangle it reaches, one stroke queued at a time, and kept on the device between
/// renders: Fit → 100 % again takes no refine.
#[test]
fn returning_to_100_percent_after_big_auto_mask_strokes_is_quick() {
    if !gpu() {
        return;
    }
    let _turn = TURN.lock().unwrap_or_else(PoisonError::into_inner);
    use lcc_pipeline::PixelWindow;
    let mut strokes = Vec::new();
    for k in 0..12 {
        let y0 = 0.25 + 0.04 * k as f64;
        let points: Vec<Point> = (0..=200)
            .map(|i| {
                let t = i as f64 / 200.0;
                Point::new(0.3 + 0.4 * (t * 6.0 * std::f64::consts::PI).sin().abs(), y0 + 0.08 * t)
            })
            .collect();
        strokes.push(BrushStroke { points, size: 0.06, auto_mask: true, ..Default::default() });
    }
    let s = DevelopSettings {
        masks: vec![Mask {
            components: vec![MaskComponent { anchor: None, name: None, op: MaskOp::Add, invert: false, shape: MaskShape::Brush { strokes } }],
            adjust: lcc_develop::LocalAdjustments { exposure: 0.5, ..Default::default() },
            ..Default::default()
        }],
        ..Default::default()
    };
    let src = scene(1, 2048, 1366);
    let info = SourceInfo { raw: true, ..Default::default() };
    let win = RenderRequest { window: Some(PixelWindow { x: 1500, y: 1200, w: 1920, h: 1080 }), ..RenderRequest::fit(8192, 5464) };
    // (the kernels and the source warmed up first)
    let _ = lcc_gpu::render(&src, &info, &s, &win, None).expect("gpu render");
    // the same strokes moved a little: every one is worked out anew
    let mut moved = s.clone();
    if let MaskShape::Brush { strokes } = &mut moved.masks[0].components[0].shape {
        strokes.iter_mut().flat_map(|st| st.points.iter_mut()).for_each(|p| p.x += 0.002);
    }
    let timed = |s: &DevelopSettings| {
        let t = std::time::Instant::now();
        let _ = lcc_gpu::render(&src, &info, s, &win, None).expect("gpu render");
        t.elapsed().as_secs_f64() * 1000.0
    };
    let first = timed(&moved);
    let _ = lcc_gpu::render(&src, &info, &moved, &RenderRequest::fit(1600, 1067), None).expect("gpu render");
    let refined = lcc_gpu::refined_auto_mask_strokes();
    let again = timed(&moved);
    assert_eq!(lcc_gpu::refined_auto_mask_strokes(), refined, "the return refined strokes again");
    eprintln!("100 % with 12 big Auto Mask strokes: {first:.0} ms; Fit → 100 % again: {again:.0} ms");
    assert!(again < 0.5 * first && again < 1500.0, "returning to 100 % took {again:.0} ms (the first 100 %: {first:.0} ms)");
}

/// A zoomed view renders each tile as a window of the frame with a wide margin, and big Auto Mask
/// strokes reach most of every tile: kept per window, each tile refined every stroke again and the
/// tiles pushed each other's strokes out, so a return to 100 % found none. Strokes are kept in the
/// frame's coordinates: the next tile takes what the first one worked out, and still renders what
/// the CPU does away from its own margin.
#[test]
fn tiles_of_a_zoomed_view_reuse_each_others_auto_mask_strokes() {
    if !gpu() {
        return;
    }
    let _turn = TURN.lock().unwrap_or_else(PoisonError::into_inner);
    use lcc_pipeline::PixelWindow;
    let mut strokes = Vec::new();
    for k in 0..6 {
        let y0 = 0.3 + 0.07 * k as f64;
        let points: Vec<Point> = (0..=100).map(|i| Point::new(0.35 + 0.3 * i as f64 / 100.0, y0 + 0.03 * (i as f64 * 0.2).sin())).collect();
        strokes.push(BrushStroke { points, size: 0.05, auto_mask: true, ..Default::default() });
    }
    let s = DevelopSettings {
        masks: vec![Mask {
            components: vec![MaskComponent { anchor: None, name: None, op: MaskOp::Add, invert: false, shape: MaskShape::Brush { strokes } }],
            adjust: lcc_develop::LocalAdjustments { exposure: 1.0, ..Default::default() },
            ..Default::default()
        }],
        ..Default::default()
    };
    let src = scene(2, 1536, 1024);
    let info = SourceInfo { raw: true, ..Default::default() };
    // two neighbouring tiles of a 6144-wide frame, each with its margin
    let tile = |x| RenderRequest { window: Some(PixelWindow { x, y: 1000, w: 3200, h: 2400 }), ..RenderRequest::fit(6144, 4096) };
    let (a, b) = (tile(1000), tile(2024));
    let _ = lcc_gpu::render(&src, &info, &s, &a, None).expect("gpu render");
    let before = lcc_gpu::reused_stroke_blocks();
    let gpu = lcc_gpu::render(&src, &info, &s, &b, None).expect("gpu render").image;
    let reused = lcc_gpu::reused_stroke_blocks() - before;
    assert!(reused > 0, "the second tile took none of the first one's stroke blocks");
    // away from its margin (where each window's refine meets its own edge) it is the CPU's tile
    let cpu = render(&src, &info, &s, &b).image;
    let m = 600;
    let inner = |img: &Rgba8| Rgba8::from_fn(img.width - 2 * m, img.height - 2 * m, |x, y| img.get(x + m, y + m));
    let (mean, max) = diff(&inner(&cpu), &inner(&gpu));
    eprintln!("second tile with {reused} kept blocks: mean {mean:.4} LSB, max {max} LSB");
    assert!(mean < MEAN_LSB && max <= MAX_LSB, "mean {mean:.4} LSB, max {max} LSB");
    // and back to it after Fit: every block it needs is kept
    let _ = lcc_gpu::render(&src, &info, &s, &RenderRequest::fit(1536, 1024), None).expect("gpu render");
    let refined = lcc_gpu::refined_auto_mask_strokes();
    let again = lcc_gpu::render(&src, &info, &s, &b, None).expect("gpu render").image;
    assert_eq!(lcc_gpu::refined_auto_mask_strokes(), refined, "the return refined strokes again");
    let (_, max) = diff(&again, &gpu);
    assert!(max <= 1, "the tile from its kept blocks differs by {max} LSB");
}
