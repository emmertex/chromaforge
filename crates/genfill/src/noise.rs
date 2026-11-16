//! Matching sensor noise for a generated patch.
//!
//! A generated patch is clean: next to the grain of the photo it would look pasted on, and noise
//! reduction (which comes after it in the pipeline) would have nothing to remove there. So every
//! render measures the noise of the original pixels around the patch, in the image it is rendering
//! (so a small preview, a full-size export, a photo developed from its AI-denoised picture all get
//! the noise they actually have), and adds the missing part to the patch.
//!
//! The model is Poisson-Gaussian per channel, as for raw sensor data: variance = `a·L + b` at
//! signal level `L` (shot noise grows with the signal, read noise is the floor). It is fitted to
//! the high-pass residual (`v − 3×3 mean`, whose variance is 8/9 of white noise's) with robust
//! (MAD) estimates in signal-level bins. The noise added is white; demosaicked noise is slightly
//! correlated between neighbours, so the fine grain matches and the coarsest grain is a little
//! weaker than the photo's. It is deterministic per pixel position and seed.

use lcc_raster::Rgb32f;

/// Noise variance per channel: `a[c]·L + b[c]` at signal level `L` (linear).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct NoiseModel {
    pub a: [f32; 3],
    pub b: [f32; 3],
}

impl NoiseModel {
    pub fn sigma(&self, c: usize, level: f32) -> f32 {
        let (a, b) = (self.a.get(c).copied().unwrap_or(0.0), self.b.get(c).copied().unwrap_or(0.0));
        (a * level.max(0.0) + b).max(0.0).sqrt()
    }
}

/// Fewest residuals a fit is made from.
const MIN_SAMPLES: usize = 64;
/// Most residuals examined (an even subsample beyond).
const MAX_SAMPLES: usize = 200_000;

fn median(v: &mut [f32]) -> f32 {
    if v.is_empty() {
        return 0.0;
    }
    let k = v.len() / 2;
    let (_, m, _) = v.select_nth_unstable_by(k, |a, b| a.total_cmp(b));
    *m
}

/// Fit the noise of `img` at the pixels `at` (indices; pixels on the image border are skipped).
/// `None` when there are too few of them.
pub fn estimate(img: &Rgb32f, at: &[usize]) -> Option<NoiseModel> {
    let (w, h) = (img.width, img.height);
    if w < 3 || h < 3 {
        return None;
    }
    let step = (at.len() / MAX_SAMPLES).max(1);
    // (level, residual per channel)
    let mut samples: Vec<([f32; 3], [f32; 3])> = Vec::new();
    for &i in at.iter().step_by(step) {
        let (x, y) = (i % w, i / w);
        if x == 0 || y == 0 || x + 1 >= w || y + 1 >= h {
            continue;
        }
        let mut mean = [0.0f32; 3];
        for dy in 0..3 {
            for dx in 0..3 {
                let p = img.data.get((y + dy - 1) * w + x + dx - 1).copied().unwrap_or_default();
                mean = [mean[0] + p[0], mean[1] + p[1], mean[2] + p[2]];
            }
        }
        let mean = mean.map(|v| v / 9.0);
        let p = img.data.get(i).copied().unwrap_or_default();
        let r = [p[0] - mean[0], p[1] - mean[1], p[2] - mean[2]];
        if r.iter().chain(&mean).all(|v| v.is_finite()) {
            samples.push((mean, r));
        }
    }
    if samples.len() < MIN_SAMPLES {
        return None;
    }
    let mut model = NoiseModel::default();
    let white = (8.0f32 / 9.0).sqrt();
    for c in 0..3 {
        // signal-level bins (equal counts), robust σ per bin
        let mut s: Vec<(f32, f32)> = samples.iter().map(|(m, r)| (m[c].max(0.0), r[c])).collect();
        s.sort_by(|a, b| a.0.total_cmp(&b.0));
        let bins = (s.len() / MIN_SAMPLES).clamp(1, 8);
        let mut pts: Vec<(f32, f32)> = Vec::new();
        for chunk in s.chunks(s.len().div_ceil(bins).max(1)) {
            if chunk.len() < MIN_SAMPLES / 2 {
                continue;
            }
            let mut lv: Vec<f32> = chunk.iter().map(|p| p.0).collect();
            let mut res: Vec<f32> = chunk.iter().map(|p| p.1).collect();
            let med = median(&mut res);
            let mut dev: Vec<f32> = res.iter().map(|r| (r - med).abs()).collect();
            let sigma = 1.4826 * median(&mut dev) / white;
            pts.push((median(&mut lv), sigma * sigma));
        }
        // least squares var = a·L + b, both ≥ 0
        let n = pts.len() as f32;
        if n == 0.0 {
            continue;
        }
        let (sl, sv) = pts.iter().fold((0.0f32, 0.0f32), |(a, b), p| (a + p.0, b + p.1));
        let (ml, mv) = (sl / n, sv / n);
        let (cov, var) = pts.iter().fold((0.0f32, 0.0f32), |(c, v), p| (c + (p.0 - ml) * (p.1 - mv), v + (p.0 - ml) * (p.0 - ml)));
        let a = if var > 1e-12 { (cov / var).max(0.0) } else { 0.0 };
        let b = (mv - a * ml).max(0.0);
        let (a, b) = if b == 0.0 && a == 0.0 { (0.0, mv.max(0.0)) } else { (a, b) };
        model.a[c] = if a.is_finite() { a } else { 0.0 };
        model.b[c] = if b.is_finite() { b } else { 0.0 };
    }
    Some(model)
}

/// A standard normal value for (`seed`, `i`, `c`): a hash to two uniforms, Box-Muller.
fn gauss(seed: u64, i: u64, c: u64) -> f32 {
    let mut z = seed ^ i.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ c.wrapping_mul(0xD1B5_4A32_D192_ED03);
    let mut next = || {
        z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut x = z;
        x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        x ^ (x >> 31)
    };
    let u1 = ((next() >> 11) as f64 + 0.5) / (1u64 << 53) as f64;
    let u2 = ((next() >> 11) as f64) / (1u64 << 53) as f64;
    ((-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()) as f32
}

/// Add noise to the patch pixels `changed` (index, blend weight) of `img` so they carry the noise
/// of `photo` (measured around them) minus the noise they already have (`own`). `key` positions
/// the noise (use the pixel's place in the whole output so a windowed render matches the full one).
pub fn add(img: &mut Rgb32f, changed: &[(usize, f32)], photo: &NoiseModel, own: Option<&NoiseModel>, seed: u64, key: impl Fn(usize) -> u64) {
    for &(i, a) in changed {
        let Some(p) = img.data.get_mut(i) else { continue };
        let k = key(i);
        for c in 0..3 {
            let level = p[c].max(0.0);
            let want = photo.sigma(c, level).powi(2);
            let have = own.map_or(0.0, |o| o.sigma(c, level).powi(2));
            let s = (want - have).max(0.0).sqrt() * a;
            if s > 0.0 && s.is_finite() {
                p[c] = (p[c] + s * gauss(seed, k, c as u64)).max(0.0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A flat-ish image with Poisson-Gaussian noise of known parameters.
    fn noisy(w: usize, h: usize, a: f32, b: f32) -> Rgb32f {
        Rgb32f::from_fn(w, h, |x, y| {
            let base = 0.05 + 0.4 * x as f32 / w as f32;
            let i = (y * w + x) as u64;
            std::array::from_fn(|c| base + (a * base + b).sqrt() * gauss(99, i, c as u64))
        })
    }

    #[test]
    fn the_fit_recovers_known_noise() {
        let (a, b) = (0.0004, 0.00001);
        let img = noisy(300, 200, a, b);
        let all: Vec<usize> = (0..300 * 200).collect();
        let m = estimate(&img, &all).unwrap();
        for c in 0..3 {
            for level in [0.1f32, 0.3] {
                let want = (a * level + b).sqrt();
                let got = m.sigma(c, level);
                assert!((got - want).abs() / want < 0.25, "c{c} at {level}: {got} vs {want} ({m:?})");
            }
        }
    }

    #[test]
    fn added_noise_matches_the_photo() {
        let (a, b) = (0.0004, 0.00001);
        let photo = noisy(200, 200, a, b);
        let model = estimate(&photo, &(0..200 * 200).collect::<Vec<_>>()).unwrap();
        // a clean patch the same tones, made noisy
        let mut clean = Rgb32f::from_fn(200, 200, |x, _| [0.05 + 0.4 * x as f32 / 200.0; 3]);
        let changed: Vec<(usize, f32)> = (0..200 * 200).map(|i| (i, 1.0)).collect();
        add(&mut clean, &changed, &model, None, 7, |i| i as u64);
        let got = estimate(&clean, &(0..200 * 200).collect::<Vec<_>>()).unwrap();
        for c in 0..3 {
            let (s0, s1) = (model.sigma(c, 0.25), got.sigma(c, 0.25));
            assert!((s0 - s1).abs() / s0 < 0.2, "{s0} vs {s1}");
        }
        // deterministic
        let mut again = Rgb32f::from_fn(200, 200, |x, _| [0.05 + 0.4 * x as f32 / 200.0; 3]);
        add(&mut again, &changed, &model, None, 7, |i| i as u64);
        assert_eq!(clean.data, again.data);
    }

    #[test]
    fn too_few_or_hostile_samples() {
        let img = noisy(20, 20, 0.0004, 0.0);
        assert!(estimate(&img, &[1, 2, 3]).is_none());
        assert!(estimate(&Rgb32f::new(2, 2), &[0, 1, 2, 3]).is_none());
        let mut nan = noisy(50, 50, 0.0004, 0.0);
        nan.data.iter_mut().step_by(3).for_each(|p| p[0] = f32::NAN);
        let m = estimate(&nan, &(0..2500).chain([usize::MAX]).collect::<Vec<_>>());
        assert!(m.is_none_or(|m| m.a.iter().chain(&m.b).all(|v| v.is_finite())));
        let mut img = noisy(10, 10, 0.0004, 0.0);
        add(&mut img, &[(usize::MAX, 1.0), (5, f32::NAN)], &NoiseModel { a: [f32::NAN; 3], b: [1.0; 3] }, None, 1, |i| i as u64);
        assert!(img.data.iter().flatten().all(|v| v.is_finite()));
    }
}
