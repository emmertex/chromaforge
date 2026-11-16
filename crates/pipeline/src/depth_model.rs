//! An optional monocular depth model for Depth Range masks (`MaskShape::DepthRange`).
//!
//! The engine installs one ([`set_depth_model`]) when its weights are in place (Depth Anything
//! V2 Small); without it a Depth Range selects nothing. This layer only calls the model: given
//! the whole uncropped, oriented photo as 8-bit sRGB, it returns a `0..1` distance grid over the
//! same frame (0 = nearest, 1 = farthest). Results are cached per photo (pixels and framing;
//! white balance left out, so a WB drag never re-runs the model), and the model runs only for
//! photos that have a Depth Range mask.
//!
//! Sources: own design (the same hook as the scene model's, [`crate::scene_model`]).

use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, PoisonError, RwLock};

use lcc_develop::{DevelopSettings, MaskShape};
use lcc_raster::{Plane, Rgb32f};

use crate::SourceInfo;

/// The model: `(rgb, w, h)` → a `0..1` distance grid of any size, or `None` when it can't.
pub type DepthModel = dyn Fn(&[u8], usize, usize) -> Option<Plane> + Send + Sync;

static MODEL: RwLock<Option<Arc<DepthModel>>> = RwLock::new(None);
static CACHE: Mutex<Vec<(u64, Plane)>> = Mutex::new(Vec::new());
/// Held while the model runs: a second render of the same photo waits and takes the cached result.
static BUSY: Mutex<()> = Mutex::new(());
const CACHE_LEN: usize = 8;

/// Long edge of the image the depth model is given (it works at ≈ 518 px on the short side).
pub const MODEL_EDGE: usize = 1036;

/// Install (or with `None` remove) the depth model; cached depth maps are dropped.
pub fn set_depth_model(m: Option<Arc<DepthModel>>) {
    *MODEL.write().unwrap_or_else(PoisonError::into_inner) = m;
    CACHE.lock().unwrap_or_else(PoisonError::into_inner).clear();
    crate::auto_mask::clear_cache();
}

/// Whether a depth model is installed.
pub fn has_depth_model() -> bool {
    MODEL.read().unwrap_or_else(PoisonError::into_inner).is_some()
}

/// Whether any visible mask of `s` has a Depth Range component (only those photos run the model).
pub fn wanted(s: &DevelopSettings) -> bool {
    s.masks.iter().any(|m| m.components.iter().any(|c| matches!(c.shape, MaskShape::DepthRange { .. })))
}

pub(crate) fn usable(p: &Plane) -> bool {
    p.width > 0 && p.height > 0 && p.data.len() == p.width.saturating_mul(p.height) && p.data.iter().all(|v| v.is_finite())
}

/// The installed model's depth grid of `src` under `s` (cached), if there is a model and it ran.
pub(crate) fn depth(src: &Rgb32f, info: &SourceInfo, s: &DevelopSettings) -> Option<Plane> {
    let model = MODEL.read().unwrap_or_else(PoisonError::into_inner).clone()?;
    let mut frame = crate::frame_for(src, info, s, false);
    // stored unflipped: sampling applies the render's flips (as the other automatic grids)
    frame.flip_h = false;
    frame.flip_v = false;
    let mut hs = std::collections::hash_map::DefaultHasher::new();
    (src.width, src.height, format!("{frame:?}")).hash(&mut hs);
    let step = (src.data.len() / 4096).max(1);
    for c in src.data.iter().step_by(step) {
        c.map(f32::to_bits).hash(&mut hs);
    }
    let key = hs.finish();
    let cached = || CACHE.lock().unwrap_or_else(PoisonError::into_inner).iter().find(|(k, _)| *k == key).map(|(_, p)| p.clone());
    if let Some(p) = cached() {
        return Some(p);
    }
    let _busy = BUSY.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(p) = cached() {
        return Some(p);
    }
    let (w, h) = frame.fit(MODEL_EDGE, MODEL_EDGE);
    let (w, h) = (w.max(1), h.max(1));
    let img = frame.sample(src, w, h);
    let rgb = crate::auto_mask::model_input(&img);
    let mut p = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| model(&rgb, w, h))).ok().flatten()?;
    if !usable(&p) {
        return None;
    }
    p.data.iter_mut().for_each(|v| *v = v.clamp(0.0, 1.0));
    let mut c = CACHE.lock().unwrap_or_else(PoisonError::into_inner);
    if c.len() >= CACHE_LEN {
        c.remove(0);
    }
    c.push((key, p.clone()));
    Some(p)
}

/// The Depth Range coverage of distance `d` (0 = nearest, 1 = farthest): full inside
/// `lo..=hi`, fading to none over `feather` beyond each end.
pub fn range_alpha(d: f32, lo: f32, hi: f32, feather: f32) -> f32 {
    if !d.is_finite() {
        return 0.0;
    }
    let (lo, hi) = (lo.min(hi), lo.max(hi));
    let f = if feather.is_finite() { feather.clamp(1e-3, 1.0) } else { 1e-3 };
    let rise = ((d - (lo - f)) / f).clamp(0.0, 1.0);
    let fall = (((hi + f) - d) / f).clamp(0.0, 1.0);
    let s = |t: f32| t * t * (3.0 - 2.0 * t);
    s(rise) * s(fall)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_range_selects_its_band_and_feathers_out() {
        assert!(range_alpha(0.5, 0.3, 0.7, 0.1) > 0.99);
        assert!(range_alpha(0.1, 0.3, 0.7, 0.1) < 0.01);
        assert!(range_alpha(0.95, 0.3, 0.7, 0.1) < 0.01);
        let edge = range_alpha(0.25, 0.3, 0.7, 0.1);
        assert!(edge > 0.05 && edge < 0.95, "{edge}");
        // swapped bounds and hostile numbers never panic or leave 0..1
        for (d, lo, hi, f) in [(0.5, 0.7, 0.3, 0.1), (f32::NAN, 0.0, 1.0, 0.1), (0.5, 0.0, 1.0, f32::INFINITY), (0.5, 0.0, 1.0, -3.0)] {
            let a = range_alpha(d, lo, hi, f);
            assert!((0.0..=1.0).contains(&a), "{a}");
        }
    }

    #[test]
    fn malformed_depth_grids_are_not_used() {
        assert!(!usable(&Plane { width: 4, height: 3, data: vec![0.0; 5] }));
        assert!(!usable(&Plane { width: 2, height: 2, data: vec![f32::NAN; 4] }));
        assert!(usable(&Plane { width: 2, height: 2, data: vec![0.5; 4] }));
    }
}
