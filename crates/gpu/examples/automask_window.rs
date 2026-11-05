//! Time a 100 % window render of a 45 MP photo with 12 Auto Mask mouse strokes, then Fit and
//! 100 % again (each return to 100 % used to refine every stroke over the whole window again).
//!
//! `<PREFIX>_GPU_ADAPTER=4070 cargo run --release -p lcc-gpu --example automask_window -- [size] [ww] [wh] [--cpu] [--stages]`
//! (brush size as a fraction of the long side, default 0.02; window, default 2048 × 1792; `--cpu`: the
//! CPU pipeline; `--stages`: one view's stage cache, as the app passes; `<PREFIX>_PROFILE=1` for the
//! stages and how many strokes were kept)

use std::sync::Arc;
use std::time::Instant;

use lcc_develop::{BrushStroke, DevelopSettings, Mask, MaskComponent, MaskOp, MaskShape};
use lcc_geom::Point;
use lcc_pipeline::{PixelWindow, Quality, RenderRequest, SourceInfo};
use lcc_raster::Rgb32f;

fn main() {
    let cpu = std::env::args().any(|a| a == "--cpu");
    if !cpu && !lcc_gpu::available() {
        eprintln!("no GPU: {}", lcc_gpu::unavailable_reason().unwrap_or_default());
        return;
    }
    if !cpu {
        println!("card: {}", lcc_gpu::adapter_name().unwrap_or_default());
    }
    let (w, h) = (8192usize, 5464usize);
    let mut src = Rgb32f::new(w, h);
    for (i, p) in src.data.iter_mut().enumerate() {
        let (x, y) = ((i % w) as f32 / w as f32, (i / w) as f32 / h as f32);
        *p = [0.1 + 0.4 * x, 0.15 + 0.3 * y, 0.2 + 0.1 * ((x * 40.0).sin() * (y * 30.0).cos())];
    }
    let src = Arc::new(src);
    let a: Vec<f64> = std::env::args().skip(1).filter_map(|v| v.parse().ok()).collect();
    let size = a.first().copied().unwrap_or(0.02);
    let (ww, wh) = (a.get(1).copied().unwrap_or(2048.0) as usize, a.get(2).copied().unwrap_or(1792.0) as usize);
    let mut strokes = Vec::new();
    for k in 0..12 {
        let y0 = 0.25 + 0.04 * k as f64;
        let points: Vec<Point> = (0..=200)
            .map(|i| {
                let t = i as f64 / 200.0;
                Point::new(0.3 + 0.4 * (t * 6.0 * std::f64::consts::PI).sin().abs(), y0 + 0.08 * t)
            })
            .collect();
        strokes.push(BrushStroke { points, size, auto_mask: true, ..Default::default() });
    }
    let comp = MaskComponent { anchor: None, name: None, op: MaskOp::Add, invert: false, shape: MaskShape::Brush { strokes } };
    let mut s = DevelopSettings::default();
    s.masks.push(Mask { id: 1, components: vec![comp], ..Default::default() });
    s.masks[0].adjust.exposure = 0.5;
    let info = SourceInfo::default();
    let win = RenderRequest { window: Some(PixelWindow { x: 1500, y: 1200, w: ww, h: wh }), quality: Quality::Full, ..RenderRequest::fit(w, h) };
    let fit = RenderRequest::fit(1600, 1067);
    // `--stages`: one view's stage cache (two sizes kept), as the app passes
    let cache = std::env::args().any(|a| a == "--stages").then(lcc_pipeline::StageCache::default);
    for (name, req) in [("Fit (warm-up)", &fit), ("100 %", &win), ("Fit", &fit), ("100 % again", &win)] {
        let t = Instant::now();
        let how = if cpu {
            let _ = lcc_pipeline::render(&src, &info, &s, req);
            "CPU"
        } else if lcc_gpu::try_render(&src, &info, &s, req, cache.as_ref()).is_ok() {
            "GPU"
        } else {
            "fell back"
        };
        println!("{name}: {how} in {:.0} ms", t.elapsed().as_secs_f64() * 1000.0);
    }
}
