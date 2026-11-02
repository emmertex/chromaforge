//! Time a zoomed (100 %) window render of a 45 MP photo with a brush mask of many dabs (TEST-NOTES
//! item 28: a region render with a dress brushed over took 9–55 s; budget ~150 ms).
//!
//! `cargo run --release -p lcc-pipeline --example brush_region [-- --auto]`
//! (`LIGHTCRAFT_PROFILE=1` prints the stages.)

use lcc_develop::{BrushStroke, DevelopSettings, Mask, MaskComponent, MaskOp, MaskShape};
use lcc_geom::Point;
use lcc_pipeline::{PixelWindow, RenderRequest, SourceInfo, render};
use lcc_raster::Rgb32f;

fn main() {
    let auto = std::env::args().any(|a| a == "--auto");
    let (w, h) = (8192usize, 5464usize);
    let mut src = Rgb32f::new(w, h);
    for (i, p) in src.data.iter_mut().enumerate() {
        let (x, y) = ((i % w) as f32 / w as f32, (i / w) as f32 / h as f32);
        *p = [0.1 + 0.4 * x, 0.15 + 0.3 * y, 0.2 + 0.1 * ((x * 40.0).sin() * (y * 30.0).cos())];
    }
    let mut s = DevelopSettings::default();
    s.masks.push(Mask { id: 1, components: vec![dress(auto)], ..Default::default() });
    s.masks[0].adjust.exposure = 0.5;
    let dabs: usize = match &s.masks[0].components[0].shape {
        MaskShape::Brush { strokes } => strokes.iter().map(|st| st.points.len()).sum(),
        _ => 0,
    };
    println!("brush: {dabs} points");
    let win = PixelWindow { x: 3000, y: 2000, w: 2048, h: 1792 };
    // the brush alone: its dabs in the window's pixels, stamped
    let frame = lcc_pipeline::geometry::Frame::new(w, h, &s, true);
    if let MaskShape::Brush { strokes } = &s.masks[0].components[0].shape {
        let (img, l) = (Rgb32f::new(win.w, win.h), lcc_raster::Plane::new(win.w, win.h));
        let t = std::time::Instant::now();
        let mut n = 0;
        for st in strokes {
            let mut bd = lcc_pipeline::masks::brush_dabs(st, &frame, w, h);
            for d in &mut bd.dabs {
                *d = Point::new(d.x - win.x as f64, d.y - win.y as f64);
            }
            n += bd.dabs.len();
            let mut out = lcc_raster::Plane::new(win.w, win.h);
            lcc_pipeline::masks::stamp_dabs(&bd, win.w, win.h, &img, &l, &mut out);
        }
        println!("stamping {n} dabs into the window: {:.0} ms", t.elapsed().as_secs_f64() * 1000.0);
    }
    for q in [lcc_pipeline::Quality::Draft, lcc_pipeline::Quality::Full] {
        let req = RenderRequest { window: Some(win), quality: q, ..RenderRequest::fit(w, h) };
        let t = std::time::Instant::now();
        let r = render(&src, &SourceInfo::default(), &s, &req);
        println!("{q:?}: {}×{} in {:.0} ms", r.image.width, r.image.height, t.elapsed().as_secs_f64() * 1000.0);
    }
}

/// Back-and-forth strokes over a "dress" (the middle of the frame), as a user paints one.
pub fn dress(auto: bool) -> MaskComponent {
    let mut strokes = Vec::new();
    for k in 0..12 {
        let y0 = 0.25 + 0.04 * k as f64;
        let points: Vec<Point> = (0..=200)
            .map(|i| {
                let t = i as f64 / 200.0;
                // zig-zag across the dress
                let x = 0.3 + 0.4 * (t * 6.0 * std::f64::consts::PI).sin().abs();
                Point::new(x, y0 + 0.08 * t)
            })
            .collect();
        strokes.push(BrushStroke { points, size: 0.02, auto_mask: auto, ..Default::default() });
    }
    MaskComponent { name: None, op: MaskOp::Add, invert: false, shape: MaskShape::Brush { strokes } }
}
