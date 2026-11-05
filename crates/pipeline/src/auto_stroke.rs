//! Auto Mask strokes on the CPU: each stroke's coverage (its dabs stamped, then snapped to the
//! photo's edges with [`super::guided_cross`]) kept for the next render, and the edge snap run only
//! on the part of the plane the stroke can reach.
//!
//! The GPU renderer refines over the same rectangle and keeps its strokes on the device the same way
//! (`lcc_gpu`'s `strokes`). On a 45 MP photo at 100 % every window render refined every Auto Mask
//! stroke over the whole window: twelve big strokes took 13-14 s per window on the CPU, again after
//! Fit → 100 % with nothing edited, and again for every stroke added. Now an unchanged stroke on the
//! same pixels comes from [`CACHE`], and a new one is refined within its own reach.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use lcc_raster::{Plane, Rgb32f};

use super::{BrushDabs, auto_refine, guided_cross, stamp_dabs_with};

/// The most bytes of stroke planes [`CACHE`] keeps: a dozen strokes of a 490-pixel brush over a
/// 4K window at 100 % of a 45 MP photo (each one reaching 5736 × 2900 pixels, 33 MB in 16 bits)
/// and the same strokes at Fit.
const CACHE_BYTES: usize = 512 << 20;

/// Finished Auto Mask strokes by [`stroke_key`] with the [`pass`] that last used them, most recent
/// last.
static CACHE: Mutex<VecDeque<(u64, u64, Arc<StrokePlane>)>> = Mutex::new(VecDeque::new());

static PASSES: AtomicU64 = AtomicU64::new(0);

/// A new brush render's pass number. A pass never evicts the strokes it used itself: when its
/// strokes outgrow [`CACHE_BYTES`] (a dozen big strokes over a 4K window), the ones that fit stay
/// and the rest aren't kept, where evicting the oldest would push out each stroke just before the
/// next render asks for it (no hits at all).
pub(super) fn pass() -> u64 {
    PASSES.fetch_add(1, Ordering::Relaxed).wrapping_add(1)
}

/// One finished stroke: its coverage over `rect` (x0, y0, x1, y1 of the `w × h` plane; exclusive
/// ends), zero everywhere else. Only the part it reaches is kept, so a dozen strokes over a 100 %
/// window fit [`CACHE_BYTES`].
pub(super) struct StrokePlane {
    pub w: usize,
    pub h: usize,
    pub rect: (usize, usize, usize, usize),
    /// The coverage over `rect`, row by row, in 16 bits (half the memory of `f32`; within
    /// 1 / 131070 of it).
    pub alpha: Vec<u16>,
}

impl StrokePlane {
    /// The stroke's coverage at pixel (x, y) of the plane.
    #[cfg(test)]
    pub fn at(&self, x: usize, y: usize) -> f32 {
        let (x0, y0, x1, y1) = self.rect;
        if x < x0 || x >= x1 || y < y0 || y >= y1 {
            return 0.0;
        }
        self.alpha.get((y - y0) * (x1 - x0) + x - x0).map_or(0.0, |v| unq(*v))
    }

    /// `out` (the `w × h` plane) after this stroke paints there with `combine` (identity where the
    /// stroke paints nothing: [`super::combine_stroke`] of 0).
    pub fn combine_into(&self, out: &mut Plane, combine: impl Fn(f32, f32) -> f32) {
        let (x0, y0, x1, y1) = self.rect;
        for (y, arow) in (y0..y1).zip(self.alpha.chunks(x1.saturating_sub(x0).max(1))) {
            if let Some(row) = out.data.get_mut(y * out.width + x0..y * out.width + x1) {
                for (o, a) in row.iter_mut().zip(arow) {
                    *o = combine(*o, unq(*a));
                }
            }
        }
    }
}

/// A fingerprint of the pixels an Auto Mask stroke reads: the image (its chromaticity) and its log
/// luminance, every value.
pub(super) fn pixels_key(img: &Rgb32f, log_l: &Plane) -> u64 {
    use rayon::prelude::*;
    const CHUNK: usize = 1 << 16;
    fn mix(h: u64, v: u32) -> u64 {
        (h ^ u64::from(v)).wrapping_mul(0x0000_0100_0000_01b3)
    }
    let a: Vec<u64> =
        img.data.par_chunks(CHUNK).map(|c| c.iter().fold(0xcbf2_9ce4_8422_2325, |h, p| p.iter().fold(h, |h, v| mix(h, v.to_bits())))).collect();
    let b: Vec<u64> = log_l.data.par_chunks(CHUNK).map(|c| c.iter().fold(0x8422_2325_cbf2_9ce4, |h, v| mix(h, v.to_bits()))).collect();
    hash((img.width, img.height, log_l.width, log_l.height, a, b))
}

fn hash<T: std::hash::Hash>(v: T) -> u64 {
    use std::hash::{BuildHasher, BuildHasherDefault, DefaultHasher};
    BuildHasherDefault::<DefaultHasher>::default().hash_one(v)
}

/// What one stroke's plane depends on: its dabs as placed in this `w × h` plane (so the frame,
/// crop and window), their brush, and the pixels ([`pixels_key`]).
pub(super) fn stroke_key(bd: &BrushDabs, w: usize, h: usize, pixels: u64) -> u64 {
    let dabs: Vec<(u64, u64)> = bd.dabs.iter().map(|d| (d.x.to_bits(), d.y.to_bits())).collect();
    let pressure: Vec<u32> = bd.pressure.iter().map(|p| p.to_bits()).collect();
    let lrc = bd.lrc.map(|(a, p)| (a.to_bits(), p.to_bits()));
    let brush = (bd.r.to_bits(), bd.hard.to_bits(), bd.flow.to_bits(), bd.density.to_bits(), bd.auto_scale.to_bits());
    hash((pixels, w, h, dabs, pressure, lrc, brush, bd.erase, bd.auto))
}

/// The pixels (x0, y0, x1, y1; exclusive ends) of a `w × h` plane an Auto Mask stroke's refine can
/// change: its dabs' extent plus six sigmas (the guided filter blurs twice, each blur reaching
/// three), clipped to the plane; `None` when it reaches none.
pub(super) fn refine_reach(bd: &BrushDabs, origin: (f64, f64), w: usize, h: usize, sigma: f32) -> Option<(usize, usize, usize, usize)> {
    let (mut x0, mut y0, mut x1, mut y1) = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    for (k, d) in bd.dabs.iter().enumerate() {
        let r = bd.dab(k).0;
        if !(d.x.is_finite() && d.y.is_finite() && r.is_finite()) {
            continue;
        }
        let (x, y) = (d.x + origin.0, d.y + origin.1);
        x0 = x0.min(x - r);
        y0 = y0.min(y - r);
        x1 = x1.max(x + r);
        y1 = y1.max(y + r);
    }
    let m = 6.0 * f64::from(sigma) + 2.0;
    let clip = |v: f64, n: usize| v.clamp(0.0, n as f64) as usize;
    let (x0, y0, x1, y1) = (clip((x0 - m).floor(), w), clip((y0 - m).floor(), h), clip((x1 + m).ceil(), w), clip((y1 + m).ceil(), h));
    (x0 < x1 && y0 < y1).then_some((x0, y0, x1, y1))
}

fn crop(p: &Plane, (x0, y0, x1, y1): (usize, usize, usize, usize)) -> Plane {
    let mut out = Plane::new(x1.saturating_sub(x0), y1.saturating_sub(y0));
    for (y, row) in (y0..y1).zip(out.data.chunks_mut(x1.saturating_sub(x0).max(1))) {
        if let Some(src) = p.data.get(y * p.width + x0..y * p.width + x1) {
            row.copy_from_slice(src);
        }
    }
    out
}

/// One Auto Mask stroke's plane (`w × h`): stamped, then (a legacy stroke) refined within its
/// [`refine_reach`]; from [`CACHE`] when this stroke was finished on these pixels before.
/// `computed` says whether it was worked out now.
pub(super) fn auto_stroke(
    bd: &BrushDabs,
    w: usize,
    h: usize,
    log_l: &Plane,
    chroma: &[[f32; 3]],
    pixels: u64,
    pass: u64,
) -> (Arc<StrokePlane>, bool) {
    let key = stroke_key(bd, w, h, pixels);
    {
        let mut cache = CACHE.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(i) = cache.iter().position(|(k, _, p)| *k == key && p.w == w && p.h == h)
            && let Some(hit) = cache.remove(i)
        {
            let plane = hit.2.clone();
            cache.push_back((hit.0, pass, hit.2));
            return (plane, false);
        }
    }
    let mut alpha = Plane::new(w, h);
    stamp_dabs_with(bd, w, h, log_l, chroma, &mut alpha);
    // (a Lightroom-model stroke keeps its pixel-sharp cut, note 24)
    let (sigma, eps) = auto_refine(bd.r);
    let refine = bd.lrc.is_none();
    let reach = super::auto_stroke_reach(bd, w, h);
    if refine && let Some(rect) = reach {
        let q = guided_cross(&crop(log_l, rect), &crop(&alpha, rect), sigma, eps);
        let (x0, y0, x1, y1) = rect;
        for (y, qrow) in (y0..y1).zip(q.data.chunks(x1.saturating_sub(x0).max(1))) {
            if let Some(row) = alpha.data.get_mut(y * w + x0..y * w + x1) {
                // the refinement fills gaps along edges; it never takes paint away
                for (a, q) in row.iter_mut().zip(qrow) {
                    *a = a.max(*q);
                }
            }
        }
    }
    let rect = reach.unwrap_or((0, 0, 0, 0));
    let plane =
        Arc::new(StrokePlane { w, h, rect, alpha: crop(&alpha, rect).data.iter().map(|v| (v.clamp(0.0, 1.0) * 65535.0).round() as u16).collect() });
    keep(&mut CACHE.lock().unwrap_or_else(PoisonError::into_inner), (key, pass, plane.clone()), CACHE_BYTES);
    (plane, true)
}

type Entry = (u64, u64, Arc<StrokePlane>);

fn unq(v: u16) -> f32 {
    f32::from(v) / 65535.0
}

/// Add `entry` to `cache` within `cap` bytes: the least recently used go first, never one its pass
/// used (then `entry` isn't kept).
fn keep(cache: &mut VecDeque<Entry>, entry: Entry, cap: usize) {
    let bytes = |p: &StrokePlane| p.alpha.len().saturating_mul(2);
    let mut total = cache.iter().map(|(_, _, p)| bytes(p)).fold(bytes(&entry.2), usize::saturating_add);
    while total > cap && cache.front().is_some_and(|(_, p, _)| *p != entry.1) {
        let Some((_, _, old)) = cache.pop_front() else { break };
        total = total.saturating_sub(bytes(&old));
    }
    if total <= cap {
        cache.push_back(entry);
    }
}

#[cfg(test)]
mod tests {
    use super::super::{BrushStroke, Frame, brush_dabs, chroma_plane};
    use super::*;
    use lcc_develop::DevelopSettings;
    use lcc_geom::Point;

    /// More strokes than fit, rendered again and again: the ones that fit stay (evicting the
    /// oldest each time kept none of them for the next render).
    #[test]
    fn strokes_that_outgrow_the_cache_still_hit_on_the_next_render() {
        let plane = |n| Arc::new(StrokePlane { w: 0, h: 0, rect: (0, 0, 0, 0), alpha: vec![0; n] });
        let mut cache = VecDeque::new();
        // a cap of 3 strokes, 4 strokes per render
        let mut hits = 0;
        for pass in 1..=3u64 {
            for key in 0..4u64 {
                if let Some(i) = cache.iter().position(|(k, _, _): &Entry| *k == key)
                    && let Some(e) = cache.remove(i)
                {
                    hits += 1;
                    cache.push_back((e.0, pass, e.2));
                } else {
                    keep(&mut cache, (key, pass, plane(10)), 3 * 20);
                }
            }
        }
        assert_eq!(hits, 6, "three of four strokes hit on each later render");
        // an old pass's strokes make way
        keep(&mut cache, (9, 4, plane(10)), 3 * 20);
        assert!(cache.iter().any(|e| e.0 == 9));
    }

    const W: usize = 640;
    const H: usize = 480;

    /// A photo with an edge down the middle, and a short Auto Mask stroke near it.
    fn scene(shade: f32) -> (Rgb32f, Plane, BrushDabs) {
        let mut img = Rgb32f::new(W, H);
        for (i, p) in img.data.iter_mut().enumerate() {
            *p = if i % W < W / 2 { [0.6 * shade, 0.3, 0.2] } else { [0.1, 0.2, 0.5] };
        }
        let l = Plane { width: W, height: H, data: img.data.iter().map(|p| (p[0] + p[1] + p[2] + 1e-4).log2()).collect() };
        let st = BrushStroke { points: vec![Point::new(0.40, 0.45), Point::new(0.46, 0.5)], size: 0.02, auto_mask: true, ..Default::default() };
        let bd = brush_dabs(&st, &Frame::new(W, H, &DevelopSettings::default(), true), W, H);
        (img, l, bd)
    }

    // Fit → 100 % → Fit → 100 % with nothing edited: an Auto Mask stroke already finished on these
    // pixels isn't stamped and refined again (seconds per 100 % window of a 45 MP photo, every time);
    // other pixels work it out anew
    #[test]
    fn an_unchanged_auto_mask_stroke_is_not_refined_again() {
        let (img, l, bd) = scene(1.0);
        let chroma = chroma_plane(&img);
        let px = pixels_key(&img, &l);
        let (first, computed) = auto_stroke(&bd, W, H, &l, &chroma, px, pass());
        assert!(computed);
        let (again, computed) = auto_stroke(&bd, W, H, &l, &chroma, px, pass());
        assert!(!computed, "the same stroke on the same pixels is kept");
        assert_eq!(first.alpha, again.alpha);
        let (img2, l2, _) = scene(0.9);
        let px2 = pixels_key(&img2, &l2);
        assert_ne!(px2, px);
        assert!(auto_stroke(&bd, W, H, &l2, &chroma_plane(&img2), px2, pass()).1, "other pixels: worked out again");
    }

    // The edge snap runs only where the stroke can reach, and gives what refining the whole plane did
    #[test]
    fn the_refine_is_bounded_by_the_strokes_reach() {
        let (img, l, bd) = scene(0.7);
        let (sigma, eps) = auto_refine(bd.r);
        let (x0, y0, x1, y1) = refine_reach(&bd, (0.0, 0.0), W, H, sigma).expect("the stroke is on the plane");
        assert!((x1 - x0) * (y1 - y0) < W * H / 2, "a short stroke refines a small part of the plane");
        let chroma = chroma_plane(&img);
        let mut whole = Plane::new(W, H);
        stamp_dabs_with(&bd, W, H, &l, &chroma, &mut whole);
        let q = guided_cross(&l, &whole, sigma, eps);
        let whole = whole.zip_map(&q, f32::max);
        let (ours, _) = auto_stroke(&bd, W, H, &l, &chroma, pixels_key(&img, &l), pass());
        let worst = (0..W * H).map(|i| (whole.data[i] - ours.at(i % W, i / W)).abs()).fold(0.0, f32::max);
        assert!(worst < 2e-3, "bounded refine differs from the whole-plane one by {worst}");
        let mut off = bd;
        off.dabs = vec![Point::new(-5000.0, -5000.0)];
        assert_eq!(refine_reach(&off, (0.0, 0.0), W, H, sigma), None, "a stroke off the plane refines nothing");
        let (gone, _) = auto_stroke(&off, W, H, &l, &chroma, 7, pass());
        let mut out = Plane::new(W, H);
        gone.combine_into(&mut out, f32::max);
        assert!(out.data.iter().all(|v| *v == 0.0), "and paints nothing, without panicking");
    }
}
