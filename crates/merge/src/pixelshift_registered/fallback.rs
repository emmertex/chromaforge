//! The plain merge as the fallback of the registered merge: PR A's placement, with the frames' brightness gains applied
//! first when (and only when) they vary by more than noise. Without gains this is PR A's merge itself, bit for bit.

use super::gain::Gains;
use crate::Result;
use crate::pixelshift::{self, Merged, Pattern};
use lcc_raw::{RawData, RawImage};
use rayon::prelude::*;

/// Puts the samples of `img` (frame `k`) on the common brightness level: `black + (v - black) / gain` per colour,
/// rounded and limited to the sample range; clipped samples (at or above white) are left as they are.
pub fn apply_gains(img: &mut RawImage, gains: &Gains, k: usize) {
    let Some(g) = gains.per_frame.get(k).copied() else { return };
    let black = f64::from(img.black.mean());
    let white = img.white.first().copied().map_or(f64::INFINITY, f64::from);
    let (w, cfa) = (img.width, img.cfa.clone());
    let (RawData::U16(d), Some(cfa)) = (&mut img.data, cfa) else { return };
    if w == 0 {
        return;
    }
    d.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, v) in row.iter_mut().enumerate() {
            let f = f64::from(*v);
            if f >= white {
                continue;
            }
            let gc = g.get(cfa.color_at(x, y) as usize % 3).copied().unwrap_or(1.0);
            *v = (black + (f - black) / gc).round().clamp(0.0, 65535.0) as u16;
        }
    });
}

/// PR A's merge of a series; with `gains` each frame is put on the common level before it is placed. `gains = None`
/// calls [`pixelshift::merge_series`] unchanged.
pub fn merge_plain(
    pattern: &'static Pattern,
    frames: std::ops::Range<usize>,
    load: &mut dyn FnMut(usize) -> std::result::Result<RawImage, String>,
    progress: &dyn Fn(f32) -> bool,
    gains: Option<&Gains>,
) -> Result<Merged> {
    let Some(gains) = gains else {
        return pixelshift::merge_series(pattern, frames, load, progress);
    };
    let first = frames.start;
    let mut with_gains = |i: usize| -> std::result::Result<RawImage, String> {
        let mut img = load(i)?;
        apply_gains(&mut img, gains, i - first);
        Ok(img)
    };
    pixelshift::merge_series(pattern, frames, &mut with_gains, progress)
}

#[cfg(test)]
mod tests {
    use super::super::synth::{BLACK, Scene, Spec, WHITE, mosaic};
    use super::*;
    use lcc_geom::Orientation;
    use lcc_raw::{BlackLevel, Cfa, ColorData, RawFormat, Rect};

    const W: usize = 96;
    const H: usize = 80;

    fn raw(spec: &Spec, scene: &Scene, k: usize) -> RawImage {
        RawImage {
            format: RawFormat::Nef,
            width: W,
            height: H,
            cpp: 1,
            data: RawData::U16(mosaic(spec, scene, k)),
            cfa: Cfa::bayer("RGGB"),
            bits: 14,
            black: BlackLevel::uniform(BLACK as f32),
            white: vec![WHITE as f32],
            active_area: Rect::new(0, 0, W, H),
            crop: Rect::new(2, 2, W - 4, H - 4),
            orientation: Orientation::Normal,
            color: ColorData::default(),
            wb_multipliers: None,
            linearized: false,
            pixel_aspect: 1.0,
            opcodes: Default::default(),
            metadata: Default::default(),
            lens_metadata: Default::default(),
        }
    }

    fn spec(gains: Vec<f64>) -> Spec {
        Spec { aw: W, ah: H, frames: 16, gpar: 1, noise_free: true, gains, ..Spec::default() }
    }

    fn merge(spec: &Spec, gains: Option<&Gains>) -> Merged {
        let scene = Scene::new(spec.seed, spec.level, spec.contrast);
        let mut load = |i: usize| Ok(raw(spec, &scene, i));
        merge_plain(&pixelshift::SHOT_16, 0..16, &mut load, &|_| true, gains).unwrap()
    }

    fn pr_a(spec: &Spec) -> Merged {
        let scene = Scene::new(spec.seed, spec.level, spec.contrast);
        let mut load = |i: usize| Ok(raw(spec, &scene, i));
        pixelshift::merge_series(&pixelshift::SHOT_16, 0..16, &mut load, &|_| true).unwrap()
    }

    fn same(a: &Merged, b: &Merged) -> bool {
        (a.width, a.height, a.scale, a.crop, a.filled) == (b.width, b.height, b.scale, b.crop, b.filled)
            && a.black == b.black
            && a.white == b.white
            && a.data == b.data
    }

    #[test]
    fn without_gains_the_fallback_is_pr_as_merge_bit_for_bit() {
        for s in [spec(Vec::new()), spec((0..16).map(|k| 1.0 + 0.1 * (k % 3) as f64).collect())] {
            // with or without brightness differences in the data: no gains given, nothing changes
            assert!(same(&merge(&s, None), &pr_a(&s)));
        }
    }

    #[test]
    fn identity_gains_change_nothing_and_real_gains_undo_the_flicker() {
        let flicker: Vec<f64> = (0..16).map(|k| if k % 2 == 0 { 0.92 } else { 1.08 }).collect();
        let flat = spec(Vec::new());
        let s = spec(flicker.clone());
        assert!(same(&merge(&flat, Some(&Gains::identity(16))), &pr_a(&flat)), "gains of 1 leave the data as it is");
        // the gains of the flicker put every frame back on the level of the flat series (to the rounding of the samples)
        let mean = flicker.iter().sum::<f64>() / 16.0;
        let g = Gains { per_frame: flicker.iter().map(|f| [f / mean; 3]).collect() };
        let (a, b) = (merge(&s, Some(&g)), pr_a(&flat));
        let (mut worst, mut worse_without) = (0.0f64, 0.0f64);
        let raw_flicker = pr_a(&s);
        for i in (0..a.data.len()).step_by(7) {
            // the corrected series sits at the mean level, the flat one at level 1: compare after removing the mean factor
            let exp = BLACK + (f64::from(b.data[i]) - BLACK) * mean;
            worst = worst.max((f64::from(a.data[i]) - exp).abs());
            worse_without = worse_without.max((f64::from(raw_flicker.data[i]) - exp).abs());
        }
        assert!(worst <= 4.0, "worst difference {worst}");
        assert!(worse_without > 10.0 * worst, "uncorrected {worse_without} against corrected {worst}");
    }

    #[test]
    fn clipped_samples_and_unknown_frames_are_left_alone() {
        let s = spec(Vec::new());
        let scene = Scene::new(s.seed, s.level, s.contrast);
        let mut img = raw(&s, &scene, 0);
        if let RawData::U16(d) = &mut img.data {
            d[10] = WHITE as u16;
        }
        let before: Vec<u16> = if let RawData::U16(d) = &img.data { d.clone() } else { Vec::new() };
        let g = Gains { per_frame: vec![[1.1; 3]; 4] };
        apply_gains(&mut img, &g, 9);
        let after = |img: &RawImage| if let RawData::U16(d) = &img.data { d.clone() } else { Vec::new() };
        assert_eq!(after(&img), before, "frame 9 has no gain");
        apply_gains(&mut img, &g, 1);
        let a = after(&img);
        assert_eq!(a[10], before[10], "a clipped sample stays");
        assert_ne!(a[11], before[11]);
    }
}
