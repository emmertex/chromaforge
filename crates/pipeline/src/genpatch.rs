//! Generative fill patches in the render (`docs/generative-fill.md`).
//!
//! A [`SpotMode::Generate`](lcc_develop::SpotMode::Generate) spot that has been generated
//! shows a [`PatchLayer`]: pixels of the photo's own (EXIF-oriented, as-shot, scene-linear) source,
//! placed in normalized source coordinates. It is composited first thing in the scene-linear
//! stage, right after the one geometric resample and before the raw's local tone map, white
//! balance, defringe, the other spots, red eye, noise reduction, exposure and everything after
//! them: those all apply to the patch as they apply to the photo. Sampling it through the frame's
//! own mapping (orientation, lens corrections, perspective, crop, the window of a zoomed view) is
//! the same as having pasted it into the source before that resample, so crops, rotations and
//! lens corrections done later carry it along.
//!
//! Each render then adds the sensor noise the patch lacks, measured around it in the image being
//! rendered ([`lcc_genfill::noise`]), so noise reduction treats it like the rest.

use std::sync::Arc;

use lcc_genfill::PatchLayer;
use lcc_genfill::patch::{SourcePos, composite};
use lcc_geom::Point;
use lcc_raster::Rgb32f;

use crate::geometry::Frame;

/// Where a render finds the patch files of a photo (the engine reads them from the library).
pub trait PatchSource: Send + Sync {
    /// Variation `variant` of generation `id`; `None` when its file is missing or unreadable.
    fn patch(&self, id: &str, variant: u32) -> Option<Arc<PatchLayer>>;
}

/// A photo's patches, for [`crate::SourceInfo::generated`].
#[derive(Clone)]
pub struct GenStore(pub Arc<dyn PatchSource>);

impl std::fmt::Debug for GenStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("GenStore")
    }
}

impl PartialEq for GenStore {
    fn eq(&self, o: &Self) -> bool {
        std::ptr::addr_eq(Arc::as_ptr(&self.0), Arc::as_ptr(&o.0))
    }
}

/// The patch each spot shows (`None` for spots that show none: not generative, not generated yet,
/// moved since, or whose file is missing).
pub fn shown(spots: &[lcc_develop::Spot], store: Option<&GenStore>) -> Vec<Option<Arc<PatchLayer>>> {
    spots.iter().map(|sp| sp.shown_gen().and_then(|g| store?.0.patch(&g.id, g.variant))).collect()
}

/// A point of the transformed image (normalized, as spots are) in source pixels (EXIF-oriented),
/// for a source of `src_w × src_h`.
pub fn transformed_to_source(frame: &Frame, p: Point) -> Point {
    let q = Point::new(p.x * frame.ow, p.y * frame.oh);
    let g = match &frame.warp {
        Some(w) => w.to_source(q, 1),
        None => q,
    };
    let (x, y) = frame.orient.inverse().map(g.x, g.y, frame.ow, frame.oh);
    Point::new(x, y)
}

/// Output pixel `(x, y)` of a `w × h` render of `frame` in the source (normalized), or `None` on
/// blank canvas.
fn mapper(frame: &Frame, w: usize, h: usize) -> impl Fn(usize, usize) -> Option<SourcePos> + '_ {
    let o2t = frame.out_to_oriented(w, h);
    let inv = frame.orient.inverse();
    let (sw, sh) = (frame.src_w.max(1) as f64, frame.src_h.max(1) as f64);
    move |x, y| {
        let (g, gain) = match &frame.warp {
            Some(wp) => {
                let (_, g) = wp.frame(&o2t, x, y)?;
                (g, if wp.has_gain() { wp.gain(g) } else { 1.0 })
            }
            None => (o2t.apply(Point::new(x as f64 + 0.5, y as f64 + 0.5)), 1.0),
        };
        let (sx, sy) = inv.map(g.x, g.y, frame.ow, frame.oh);
        let (nx, ny) = (sx / sw, sy / sh);
        (nx.is_finite() && ny.is_finite()).then_some(SourcePos { nx, ny, gain })
    }
}

/// Size of the coarse grid that finds the output pixels near a patch.
const BLOCK: usize = 8;

/// Composite `layer` into `img` (a `w × h` render of `frame`), then give it matching noise.
/// `seed` makes the noise of one generation its own.
pub fn apply_layer(img: &mut Rgb32f, layer: &PatchLayer, frame: &Frame, seed: u64) {
    let (w, h) = (img.width, img.height);
    if w == 0 || h == 0 {
        return;
    }
    let map = mapper(frame, w, h);
    // source px per output px (the crop and window scale; the warp changes it only locally)
    let o2t = frame.out_to_oriented(w, h);
    let m = o2t.0;
    let k_out = (m[0] * m[3] - m[1] * m[2]).abs().sqrt();
    let [x0, y0, x1, y1] = layer.info.rect;
    let (sw, sh) = (frame.src_w.max(1) as f64, frame.src_h.max(1) as f64);
    let footprint = k_out * layer.rgb.width as f64 / ((x1 - x0) * sw).max(1e-9);
    // blocks whose corners come near the rectangle (padded by two blocks' worth of source)
    let pad = 2.0 * BLOCK as f64 * k_out / sw.min(sh);
    let near = |p: Option<SourcePos>| p.is_some_and(|p| p.nx >= x0 - pad && p.nx <= x1 + pad && p.ny >= y0 - pad && p.ny <= y1 + pad);
    let (bw, bh) = (w.div_ceil(BLOCK), h.div_ceil(BLOCK));
    let mut cands: Vec<usize> = Vec::new();
    for by in 0..bh {
        for bx in 0..bw {
            let (xa, ya) = (bx * BLOCK, by * BLOCK);
            let (xb, yb) = ((xa + BLOCK).min(w) - 1, (ya + BLOCK).min(h) - 1);
            if [(xa, ya), (xb, ya), (xa, yb), (xb, yb), ((xa + xb) / 2, (ya + yb) / 2)].into_iter().any(|(x, y)| near(map(x, y))) {
                for y in ya..=yb {
                    cands.extend((xa..=xb).map(|x| y * w + x));
                }
            }
        }
    }
    if cands.is_empty() {
        return;
    }
    let done = composite(img, layer, cands.iter().filter_map(|&i| map(i % w, i / w).map(|p| (i, p))), footprint);
    if done.changed.is_empty() {
        return;
    }
    // the noise the photo has around the patch, and what the patch already has of its own
    let Some(photo) = lcc_genfill::noise::estimate(img, &done.ring) else { return };
    let inner: Vec<usize> = done.changed.iter().filter(|(_, a)| *a >= 0.99).map(|(i, _)| *i).collect();
    let own = lcc_genfill::noise::estimate(img, &inner);
    // noise positions follow the whole output, so a windowed render matches the full one
    let (vx, vy) = frame.view.map_or((0, 0), |v| (v.x.max(0.0) as u64, v.y.max(0.0) as u64));
    lcc_genfill::noise::add(img, &done.changed, &photo, own.as_ref(), seed, |i| ((i / w) as u64 + vy) << 32 | ((i % w) as u64 + vx));
}

/// Seed of a generation's noise.
pub fn seed_of(id: &str, variant: u32) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325 ^ u64::from(variant);
    for b in id.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// Boxes normalized to the EXIF-oriented source (faces) as boxes in output pixels of a `w × h`
/// render of `frame`, a little larger (lens corrections are left out: they move a face by far
/// less than the margin).
pub fn avoid_boxes(boxes: &[[f64; 4]], frame: &Frame, w: usize, h: usize) -> Vec<[f32; 4]> {
    let to_out = frame.norm_to_out(w, h);
    boxes
        .iter()
        .filter_map(|b| {
            let (mx, my) = ((b[2] - b[0]) * 0.1, (b[3] - b[1]) * 0.1);
            let corners = [(b[0] - mx, b[1] - my), (b[2] + mx, b[1] - my), (b[0] - mx, b[3] + my), (b[2] + mx, b[3] + my)];
            let pts: Vec<Point> = corners
                .iter()
                .map(|(x, y)| {
                    let (ox, oy) = frame.orient.map(*x, *y, 1.0, 1.0);
                    to_out.apply(Point::new(ox, oy))
                })
                .collect();
            let (x0, x1) = pts.iter().fold((f64::MAX, f64::MIN), |(a, b), p| (a.min(p.x), b.max(p.x)));
            let (y0, y1) = pts.iter().fold((f64::MAX, f64::MIN), |(a, b), p| (a.min(p.y), b.max(p.y)));
            [x0, y0, x1, y1].iter().all(|v| v.is_finite()).then_some([x0 as f32, y0 as f32, x1 as f32, y1 as f32])
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use lcc_develop::{DevelopSettings, GenFill, Spot, SpotMode};
    use lcc_genfill::PatchInfo;
    use lcc_raster::Plane;

    struct One(Arc<PatchLayer>);
    impl PatchSource for One {
        fn patch(&self, id: &str, _: u32) -> Option<Arc<PatchLayer>> {
            (id == "00000000000000aa").then(|| self.0.clone())
        }
    }

    /// A 400×300 source with a red "object" at 50 %, its patch (the grey that should be there).
    fn scene() -> (Rgb32f, GenStore, Spot) {
        let src = Rgb32f::from_fn(400, 300, |x, y| if (x as f32 - 200.0).hypot(y as f32 - 150.0) < 20.0 { [0.8, 0.05, 0.05] } else { [0.18; 3] });
        let rgb = Rgb32f::from_fn(80, 80, |_, _| [0.18; 3]);
        let alpha = Plane::from_fn(80, 80, |x, y| if (x as f32 - 40.0).hypot(y as f32 - 40.0) < 30.0 { 1.0 } else { 0.0 });
        let info =
            PatchInfo { rect: [160.0 / 400.0, 110.0 / 300.0, 240.0 / 400.0, 190.0 / 300.0], source_w: 400, source_h: 300, ..Default::default() };
        let layer = Arc::new(PatchLayer::new(info, rgb, alpha).unwrap());
        let mut spot = Spot { mode: SpotMode::Generate, points: vec![Point::new(0.5, 0.5)], size: 25.0 / 400.0, ..Default::default() };
        spot.generated = Some(GenFill { id: "00000000000000aa".into(), variants: 1, stroke: spot.stroke_key(), scale: 1.0, ..Default::default() });
        (src, GenStore(Arc::new(One(layer))), spot)
    }

    fn render(src: &Rgb32f, s: &DevelopSettings, store: Option<&GenStore>, w: usize) -> lcc_raster::Rgba8 {
        let info = crate::SourceInfo { generated: store.cloned(), ..Default::default() };
        let req = crate::RenderRequest::fit(w, w);
        crate::render(src, &info, s, &req).image
    }

    #[test]
    fn the_patch_replaces_the_object_at_every_size_and_rotation() {
        let (src, store, spot) = scene();
        for orientation in [lcc_geom::Orientation::Normal, lcc_geom::Orientation::Rotate90] {
            let s = DevelopSettings { spots: vec![spot.clone()], orientation, ..Default::default() };
            for w in [400, 120] {
                let img = render(&src, &s, Some(&store), w);
                let (cx, cy) = (img.width / 2, img.height / 2);
                let p = img.data[cy * img.width + cx];
                assert!(p[0].abs_diff(p[1]) < 12, "{orientation:?} {w}: the red object is gone ({p:?})");
                // without its patch file the spot changes nothing (no content-aware stand-in)
                let none = render(&src, &s, None, w);
                let q = none.data[cy * none.width + cx];
                assert!(q[0] > q[1] + 60, "the red object is still there: {q:?}");
            }
        }
    }

    #[test]
    fn white_balance_and_exposure_still_apply_to_the_patch() {
        let (src, store, spot) = scene();
        let base = DevelopSettings { spots: vec![spot], ..Default::default() };
        let mut warm = base.clone();
        warm.light.exposure = 1.0;
        warm.wb.temp = 9000.0;
        warm.wb.tint = 20.0;
        let (a, b) = (render(&src, &base, Some(&store), 400), render(&src, &warm, Some(&store), 400));
        // the patch (centre) and the untouched photo (corner) change alike
        let (pc, pc2) = (a.data[150 * 400 + 200], b.data[150 * 400 + 200]);
        let (qc, qc2) = (a.data[20 * 400 + 20], b.data[20 * 400 + 20]);
        for c in 0..3 {
            let (dp, dq) = (i32::from(pc2[c]) - i32::from(pc[c]), i32::from(qc2[c]) - i32::from(qc[c]));
            assert!((dp - dq).abs() <= 6, "channel {c}: patch moved {dp}, photo {dq}");
        }
        assert!(pc2[0] > pc[0] + 20, "brighter and warmer");
    }

    #[test]
    fn a_moved_spot_does_not_show_its_old_patch() {
        let (_, store, mut spot) = scene();
        assert!(shown(std::slice::from_ref(&spot), Some(&store))[0].is_some());
        spot.points[0].x += 0.01;
        assert!(shown(std::slice::from_ref(&spot), Some(&store))[0].is_none());
    }

    #[test]
    fn source_mapping_inverts_orientation() {
        let s = DevelopSettings { orientation: lcc_geom::Orientation::Rotate90, ..Default::default() };
        let f = Frame::new(400, 300, &s, false);
        // the top-left of the rotated view is the bottom-left of the stored image
        let p = transformed_to_source(&f, Point::new(0.0, 0.0));
        assert!((p.x - 0.0).abs() < 1e-6 && (p.y - 300.0).abs() < 1e-6, "{p:?}");
    }
}
