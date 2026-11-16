//! Colour and level matching of a generated result to the photo, in linear light.
//!
//! What comes back from a model, decoded with [`crate::Neutral::decode`], is close to the photo but
//! not equal to it: models shift colour and brightness a little, and every colour the 8-bit sRGB
//! encoding clipped comes back desaturated. Around the hole both are the same scene, so the ratio
//! of original to generated pixels there is the error. Its logarithm (per channel) is spread over
//! the hole by normalized convolution (the same idea as Heal's tone correction) and the generated
//! pixels are multiplied by it: a smooth correction that keeps the model's detail and takes the
//! surrounding photo's tone and colour. The correction is limited to ±2 stops.

use lcc_raster::{Plane, Rgb32f};

/// Grow `mask` (w×h) by `r` pixels (square neighbourhood; two passes of a running max).
pub fn dilate(w: usize, h: usize, mask: &[bool], r: usize) -> Vec<bool> {
    if r == 0 || mask.len() != w * h {
        return mask.to_vec();
    }
    let mut tmp = vec![false; w * h];
    for y in 0..h {
        // distance to the last set pixel, forwards and backwards
        let row = mask.get(y * w..(y + 1) * w).unwrap_or_default();
        let mut last: Option<usize> = None;
        for x in 0..w {
            if row.get(x).copied().unwrap_or(false) {
                last = Some(x);
            }
            if last.is_some_and(|l| x - l <= r)
                && let Some(t) = tmp.get_mut(y * w + x)
            {
                *t = true;
            }
        }
        let mut next: Option<usize> = None;
        for x in (0..w).rev() {
            if row.get(x).copied().unwrap_or(false) {
                next = Some(x);
            }
            if next.is_some_and(|n| n - x <= r)
                && let Some(t) = tmp.get_mut(y * w + x)
            {
                *t = true;
            }
        }
    }
    let mut out = vec![false; w * h];
    for x in 0..w {
        let mut last: Option<usize> = None;
        for y in 0..h {
            if tmp.get(y * w + x).copied().unwrap_or(false) {
                last = Some(y);
            }
            if last.is_some_and(|l| y - l <= r)
                && let Some(o) = out.get_mut(y * w + x)
            {
                *o = true;
            }
        }
        let mut next: Option<usize> = None;
        for y in (0..h).rev() {
            if tmp.get(y * w + x).copied().unwrap_or(false) {
                next = Some(y);
            }
            if next.is_some_and(|n| n - y <= r)
                && let Some(o) = out.get_mut(y * w + x)
            {
                *o = true;
            }
        }
    }
    out
}

/// Correct `generated` (same size as `original`) to `original`'s tone and colour: `area` marks the
/// pixels to correct (the hole and its blend margin), `ring` the original pixels that measure the
/// error (around the hole, outside `area`). `None` when sizes differ or the ring is empty.
pub fn match_to_surroundings(generated: &mut Rgb32f, original: &Rgb32f, area: &[bool], ring: &[bool]) -> Option<()> {
    let (w, h) = (generated.width, generated.height);
    let n = w * h;
    if original.width != w || original.height != h || area.len() != n || ring.len() != n || generated.data.len() != n || original.data.len() != n {
        return None;
    }
    if !ring.iter().any(|r| *r) {
        return None;
    }
    // a floor under the ratio: a hundredth of the ring's mean level (noise in deep shadow)
    let (mut sum, mut cnt) = (0.0f64, 0usize);
    for (i, _) in ring.iter().enumerate().filter(|(_, r)| **r) {
        if let Some(p) = original.data.get(i) {
            sum += f64::from(p[0] + p[1] + p[2]) / 3.0;
            cnt += 1;
        }
    }
    let eps = ((sum / cnt.max(1) as f64) as f32 * 0.01).max(1e-5);
    // work on a grid at most 256 px across (the correction is smooth)
    let f = (w.max(h).div_ceil(256)).max(1);
    let (gw, gh) = (w.div_ceil(f), h.div_ceil(f));
    let mut acc = Rgb32f::new(gw, gh);
    let mut wts = Plane::new(gw, gh);
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            if !ring.get(i).copied().unwrap_or(false) {
                continue;
            }
            let (Some(o), Some(g)) = (original.data.get(i), generated.data.get(i)) else { continue };
            let lr: [f32; 3] = std::array::from_fn(|c| ((o[c].max(0.0) + eps) / (g[c].max(0.0) + eps)).ln().clamp(-1.4, 1.4));
            if !lr.iter().all(|v| v.is_finite()) {
                continue;
            }
            let gi = (y / f) * gw + x / f;
            if let (Some(a), Some(wt)) = (acc.data.get_mut(gi), wts.data.get_mut(gi)) {
                *a = [a[0] + lr[0], a[1] + lr[1], a[2] + lr[2]];
                *wt += 1.0;
            }
        }
    }
    // extent of the area on the grid sets the smoothing: wide enough to bridge the hole
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
    for (i, _) in area.iter().enumerate().filter(|(_, a)| **a) {
        let (x, y) = ((i % w) / f, (i / w) / f);
        (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
    }
    let extent = if x1 >= x0 && y1 >= y0 && x0 != usize::MAX { (x1 - x0).max(y1 - y0) + 1 } else { 1 };
    let sigma = (extent as f32 / 3.0).max(2.0);
    let (ba, bw) = (lcc_raster::blur::gaussian(&acc, sigma), lcc_raster::blur::gaussian(&wts, sigma));
    // where the blur is too thin (far inside a big hole), fall back to the ring's overall mean
    let total: f32 = wts.data.iter().sum();
    let mean: [f32; 3] = if total > 0.0 {
        let s = acc.data.iter().fold([0.0f32; 3], |m, a| [m[0] + a[0], m[1] + a[1], m[2] + a[2]]);
        s.map(|v| v / total)
    } else {
        [0.0; 3]
    };
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            if !area.get(i).copied().unwrap_or(false) {
                continue;
            }
            let gi = (y / f).min(gh - 1) * gw + (x / f).min(gw - 1);
            let (a, wt) = (ba.data.get(gi).copied().unwrap_or_default(), bw.data.get(gi).copied().unwrap_or(0.0));
            // blend towards the global mean where the local estimate is weak
            let k = (wt / 0.05).min(1.0);
            let corr: [f32; 3] = std::array::from_fn(|c| {
                let local = if wt > 1e-6 { a[c] / wt } else { mean[c] };
                (local * k + mean[c] * (1.0 - k)).clamp(-1.4, 1.4).exp()
            });
            if let Some(g) = generated.data.get_mut(i) {
                *g = std::array::from_fn(|c| ((g[c].max(0.0) + eps) * corr[c] - eps).max(0.0));
            }
        }
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dilation_grows_by_the_radius() {
        let (w, h) = (20, 10);
        let mut m = vec![false; w * h];
        m[5 * w + 10] = true;
        let d = dilate(w, h, &m, 2);
        assert_eq!(d.iter().filter(|v| **v).count(), 25);
        assert!(d[3 * w + 8] && d[7 * w + 12] && !d[2 * w + 10]);
        assert_eq!(dilate(w, h, &m, 0), m);
        assert_eq!(dilate(w, h, &m[..5], 3).len(), 5);
    }

    #[test]
    fn a_tinted_darker_result_is_pulled_to_the_photo() {
        let (w, h) = (120, 100);
        // the photo: a gentle gradient; the "model" returned it 30 % darker and greener
        let original = Rgb32f::from_fn(w, h, |x, _| [0.2 + x as f32 * 0.001, 0.25, 0.15]);
        let mut generated = Rgb32f::from_fn(w, h, |x, y| {
            let o = original.get(x, y);
            [o[0] * 0.7, o[1] * 0.8, o[2] * 0.7]
        });
        let hole: Vec<bool> = (0..w * h).map(|i| ((i % w) as f32 - 60.0).hypot((i / w) as f32 - 50.0) < 20.0).collect();
        let area = dilate(w, h, &hole, 4);
        let grown = dilate(w, h, &area, 12);
        let ring: Vec<bool> = grown.iter().zip(&area).map(|(g, a)| *g && !*a).collect();
        match_to_surroundings(&mut generated, &original, &area, &ring).unwrap();
        for (x, y) in [(60, 50), (45, 50), (60, 35)] {
            let (g, o) = (generated.get(x, y), original.get(x, y));
            for c in 0..3 {
                assert!((g[c] - o[c]).abs() / o[c] < 0.03, "({x},{y}) {g:?} vs {o:?}");
            }
        }
        // outside the area nothing changes
        assert!((generated.get(2, 2)[0] - original.get(2, 2)[0] * 0.7).abs() < 1e-6);
        assert!(match_to_surroundings(&mut generated, &original, &area, &vec![false; w * h]).is_none());
        assert!(match_to_surroundings(&mut generated, &Rgb32f::new(3, 3), &area, &ring).is_none());
    }
}
