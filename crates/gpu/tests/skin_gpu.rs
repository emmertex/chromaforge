//! Smooth Skin on the GPU matches the CPU's (`lcc_pipeline::skin`), so a photo with Smooth
//! Skin no longer has its whole linear image read back, smoothed on the processor and uploaded
//! again: one face, faces of different sizes (two buckets, each pixel by the face it belongs to),
//! every slider, an orientation, a zoomed window and a full-size render. Bounds as the other
//! equivalence tests: mean |Δ| < 0.5 LSB, max ≤ 3 LSB in 8-bit sRGB. Skips without a GPU adapter.

use std::sync::Arc;

use lcc_develop::{DevelopSettings, SegMask, SkinAnalysis, SkinFace, SkinSmoothing};
use lcc_geom::Orientation;
use lcc_pipeline::{PixelWindow, RenderRequest, SourceInfo, render};
use lcc_raster::{Rgb32f, Rgba8};

fn gpu() -> bool {
    let ok = lcc_gpu::available();
    if !ok {
        let why = lcc_gpu::unavailable_reason().unwrap_or_default();
        assert!(!why.contains("kernels failed to build"), "{why}");
        lcc_testkit::skip!(Gpu, "no GPU adapter");
    }
    ok
}

/// A soft blob segmentation on a `side` grid: high inside the ellipses `(cx, cy, rx, ry)`.
fn blobs(side: usize, ellipses: &[(f64, f64, f64, f64)]) -> SegMask {
    let logits: Vec<f32> = (0..side * side)
        .map(|i| {
            let (x, y) = (((i % side) as f64 + 0.5) / side as f64, ((i / side) as f64 + 0.5) / side as f64);
            let d = ellipses.iter().map(|(cx, cy, rx, ry)| ((x - cx) / rx).powi(2) + ((y - cy) / ry).powi(2)).fold(f64::MAX, f64::min);
            (8.0 * (1.0 - d)).clamp(-12.0, 12.0) as f32
        })
        .collect();
    SegMask::from_logits(side, &logits)
}

/// Smooth Skin at `k` on faces `(x, y, eyes)`, their skin an ellipse around each.
fn skin(k: SkinSmoothing, faces: &[(f64, f64, f64)]) -> DevelopSettings {
    let ellipses: Vec<_> = faces.iter().map(|(x, y, e)| (*x, *y, e * 3.0, e * 4.0)).collect();
    let mut s = DevelopSettings { skin: k, ..Default::default() };
    s.skin_analysis = Some(SkinAnalysis {
        area: s.skin.area,
        person: s.skin.person,
        orientation: s.orientation,
        seg: blobs(256, &ellipses),
        faces: faces.iter().map(|(x, y, eyes)| SkinFace { x: *x, y: *y, eyes: *eyes }).collect(),
    });
    s
}

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

/// The GPU's render of `s` against the CPU's; the skin must have changed the picture.
fn check(name: &str, src: &Arc<Rgb32f>, s: &DevelopSettings, req: &RenderRequest) {
    let info = SourceInfo { raw: true, ..Default::default() };
    let cpu = render(src, &info, s, req).image;
    let gpu = lcc_gpu::try_render(src, &info, s, req, None).unwrap_or_else(|e| panic!("{name}: the GPU did not render it: {e:?}")).image;
    let (mean, max) = diff(&cpu, &gpu);
    let plain = render(src, &info, &DevelopSettings { skin: SkinSmoothing::default(), skin_analysis: None, ..s.clone() }, req).image;
    let (changed, _) = diff(&cpu, &plain);
    eprintln!("{name:<32} {}x{}  mean {mean:.4}  max {max}  (the skin changed {changed:.3} LSB on average)", cpu.width, cpu.height);
    assert!(changed > 0.01, "{name}: Smooth Skin changed nothing to compare");
    assert!(mean < 0.5 && max <= 3, "{name}: mean {mean:.4} LSB, max {max} LSB");
}

#[test]
fn smooth_skin_on_the_gpu_matches_the_cpu() {
    if !gpu() {
        return;
    }
    let (w, h) = (1200, 800);
    let src = Arc::new(lcc_scenes::demo_library()[5].render(w, h));
    let req = RenderRequest::fit(w, h);
    let k = SkinSmoothing { amount: 60.0, ..Default::default() };
    check("one face", &src, &skin(k, &[(0.5, 0.45, 0.06)]), &req);
    let every = SkinSmoothing { amount: 80.0, texture: -30.0, even_tone: 50.0, redness: 40.0, shine: 60.0, ..Default::default() };
    check("every slider", &src, &skin(every, &[(0.5, 0.45, 0.06)]), &req);
    let more_texture = SkinSmoothing { texture: 40.0, ..every };
    check("texture up", &src, &skin(more_texture, &[(0.5, 0.45, 0.06)]), &req);
    // a couple close up and a guest far behind: two buckets, pixels owned by their nearest face
    let group = [(0.35, 0.4, 0.07), (0.55, 0.42, 0.065), (0.85, 0.5, 0.015)];
    check("faces of two sizes", &src, &skin(every, &group), &req);
    // turned, and a zoomed window (the region clipped to it)
    let mut turned = skin(k, &group);
    turned.orientation = Orientation::Rotate90;
    if let Some(a) = turned.skin_analysis.as_mut() {
        a.orientation = Orientation::Rotate90;
    }
    check("turned", &src, &turned, &RenderRequest::fit(w, h));
    let win = RenderRequest { window: Some(PixelWindow { x: 300, y: 150, w: 500, h: 400 }), ..RenderRequest::fit(2 * w, 2 * h) };
    check("zoomed window", &src, &skin(every, &group), &win);
}

/// A full-size photo: the regions of a portrait (tens of megabytes of bands) on the device.
#[test]
fn smooth_skin_on_a_full_size_photo_matches() {
    if !gpu() {
        return;
    }
    let (w, h) = (4000, 6000);
    let src = Arc::new(lcc_scenes::demo_library()[1].render(w, h));
    let s = skin(SkinSmoothing { amount: 50.0, even_tone: 30.0, ..Default::default() }, &[(0.5, 0.35, 0.05), (0.3, 0.6, 0.02)]);
    check("full size portrait", &src, &s, &RenderRequest::fit(w, h));
}
