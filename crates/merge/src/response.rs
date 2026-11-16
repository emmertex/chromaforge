//! Camera response recovery for brackets of display-referred files (JPEG, 8/16-bit TIFF, PNG).
//!
//! A camera writes a JPEG through its own tone curve, not the sRGB curve the decoder undoes, so the decoded values
//! are not proportional to scene radiance and an HDR merge of them bands and shifts its midtones. The curve is
//! recovered from the bracket itself, as Debevec & Malik describe ("Recovering High Dynamic Range Radiance Maps from
//! Photographs", SIGGRAPH 1997): for pixel `j` of frame `i` with code `Zᵢⱼ` and exposure time `Δtᵢ`,
//! `g(Zᵢⱼ) = ln Eⱼ + ln Δtᵢ`, solved in the least-squares sense for the 256 values of `g` and the `ln Eⱼ` with a
//! hat weighting of the codes and a second-difference smoothness term on `g`. Each channel gets its own curve,
//! normalised so the file's white stays 1.0; every frame's pixels are then mapped through `exp g`.
//!
//! The codes are the decoded values re-encoded with the sRGB curve (the decoder's inverse, so 8-bit files give back
//! their exact codes; deeper files give fractional ones, interpolated). The exposure times come from EXIF; a bracket
//! without them, a raw or a floating-point source is left as it is.
//!
//! Sources: Debevec & Malik 1997 (above); own design for the sampling and the normalisation.

use lcc_color::transfer::{linear_to_srgb, srgb_to_linear};

use crate::half::HalfImage;
use crate::hdr::HdrFrame;
use crate::linalg;

/// Number of code values of the response curve.
const CODES: usize = 256;
/// Pixel positions sampled per channel (Debevec & Malik need N·(P − 1) > 255).
const SAMPLES: usize = 160;
/// Weight of the smoothness term (per unit of data rows per code).
const LAMBDA: f64 = 6.0;

/// A recovered response: per channel, linear value (white = 1.0) for each code.
#[derive(Clone, Debug, PartialEq)]
pub struct Response {
    pub linear: [Vec<f32>; 3],
}

impl Response {
    /// The linear value of a decoded (sRGB-linearised) `v` on channel `c`.
    pub fn apply(&self, c: usize, v: f32) -> f32 {
        let Some(t) = self.linear.get(c) else { return v };
        let z = (linear_to_srgb(v.clamp(0.0, 1.0)) * (CODES - 1) as f32).clamp(0.0, (CODES - 1) as f32);
        let i = (z.floor() as usize).min(CODES - 2);
        let f = z - i as f32;
        let (a, b) = (t.get(i).copied().unwrap_or(0.0), t.get(i + 1).copied().unwrap_or(1.0));
        a + (b - a) * f
    }
}

/// The code (0..=255) of a decoded value.
fn code(v: f32) -> usize {
    ((linear_to_srgb(v.clamp(0.0, 1.0)) * (CODES - 1) as f32).round() as usize).min(CODES - 1)
}

fn weight(z: usize) -> f64 {
    (z.min(CODES - 1 - z) + 1) as f64
}

/// Linearise a bracket of display-referred frames in place (see the module docs). Does nothing unless every frame is
/// display-referred with an EXIF exposure, the exposures span at least half a stop, and the curve can be recovered.
pub fn linearise(frames: &mut [HdrFrame]) {
    if frames.len() < 2 || !frames.iter().all(|f| f.frame.display_referred) {
        return;
    }
    let Some(ln_t) =
        frames.iter().map(|f| f.frame.exposure.filter(|e| e.is_finite()).map(|e| e * std::f64::consts::LN_2)).collect::<Option<Vec<f64>>>()
    else {
        return;
    };
    let (lo, hi) = ln_t.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), &v| (a.min(v), b.max(v)));
    if hi - lo < 0.5 * std::f64::consts::LN_2 {
        return;
    }
    let images: Vec<&HalfImage> = frames.iter().map(|f| &f.pixels).collect();
    let Some(r) = recover(&images, &ln_t) else { return };
    for f in frames.iter_mut() {
        let clip = f.frame.clip;
        f.pixels = f.pixels.map(|p| [r.apply(0, p[0]), r.apply(1, p[1]), r.apply(2, p[2])]);
        f.frame.clip = (0..3).map(|c| r.apply(c, clip)).fold(f32::INFINITY, f32::min).max(1e-6);
        f.frame.display_referred = false;
    }
}

/// Recover the response of `images` (same size, decoded values) shot at `ln_t` (ln of relative exposure).
pub fn recover(images: &[&HalfImage], ln_t: &[f64]) -> Option<Response> {
    let first = images.first()?;
    let (w, h) = (first.width, first.height);
    if images.len() != ln_t.len() || images.len() < 2 || w < 3 || h < 3 || images.iter().any(|i| i.width != w || i.height != h) {
        return None;
    }
    // the middle exposure picks the positions
    let mut order: Vec<usize> = (0..images.len()).collect();
    order.sort_by(|&a, &b| ln_t[a].total_cmp(&ln_t[b]));
    let mid = images.get(*order.get(order.len() / 2)?)?;
    let mut linear: [Vec<f32>; 3] = Default::default();
    for (c, out) in linear.iter_mut().enumerate() {
        let pos = positions(mid, c, w, h);
        if pos.len() < 20 {
            return None;
        }
        let g = solve_channel(images, ln_t, &pos, c)?;
        let top = *g.last()?;
        *out = g.iter().map(|v| ((v - top).exp() as f32).max(0.0)).collect();
        // code 0 is black
        if let Some(z) = out.first_mut() {
            *z = 0.0;
        }
    }
    Some(Response { linear })
}

/// Pixel positions (indices) to sample on channel `c`: smooth spots (the 3 × 3 neighbourhood within a few codes, so a
/// small misalignment doesn't matter), spread over the codes the middle exposure shows.
fn positions(mid: &HalfImage, c: usize, w: usize, h: usize) -> Vec<usize> {
    let grid = 96usize;
    let mut by_code: Vec<Vec<usize>> = vec![Vec::new(); CODES];
    for gy in 0..grid {
        for gx in 0..grid {
            let x = 1 + gx * (w - 2) / grid;
            let y = 1 + gy * (h - 2) / grid;
            let z = |dx: isize, dy: isize| {
                let (xx, yy) = ((x as isize + dx) as usize, (y as isize + dy) as usize);
                code(mid.get(yy * w + xx).get(c).copied().unwrap_or(0.0))
            };
            let z0 = z(0, 0);
            let smooth = [(-1, -1), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)].iter().all(|&(dx, dy)| z(dx, dy).abs_diff(z0) <= 3);
            if smooth && let Some(b) = by_code.get_mut(z0) {
                b.push(y * w + x);
            }
        }
    }
    // round-robin over the codes, so every part of the curve is seen
    let mut out = Vec::with_capacity(SAMPLES);
    let mut k = 0;
    while out.len() < SAMPLES {
        let mut any = false;
        for b in &by_code {
            if let Some(&p) = b.get(k * 7 % b.len().max(1)).filter(|_| k < b.len()) {
                out.push(p);
                any = true;
                if out.len() == SAMPLES {
                    break;
                }
            }
        }
        if !any {
            break;
        }
        k += 1;
    }
    out
}

/// Debevec & Malik's system for one channel, solved through its normal equations: unknowns `g(0..256)` then `ln E` of
/// each position. Returns `g`, made non-decreasing.
fn solve_channel(images: &[&HalfImage], ln_t: &[f64], pos: &[usize], c: usize) -> Option<Vec<f64>> {
    let n = CODES + pos.len();
    let mut ata = vec![0f64; n * n];
    let mut atb = vec![0f64; n];
    let mut add = |row: &[(usize, f64)], b: f64| {
        for &(i, a) in row {
            for &(j, bj) in row {
                if let Some(v) = ata.get_mut(i * n + j) {
                    *v += a * bj;
                }
            }
            if let Some(v) = atb.get_mut(i) {
                *v += a * b;
            }
        }
    };
    let mut rows = 0usize;
    for (j, &p) in pos.iter().enumerate() {
        for (img, &t) in images.iter().zip(ln_t) {
            let z = code(img.get(p).get(c).copied().unwrap_or(0.0));
            let wz = weight(z);
            add(&[(z, wz), (CODES + j, -wz)], wz * t);
            rows += 1;
        }
    }
    // g(128) = 0 fixes the scale of the unknowns
    add(&[(CODES / 2, 1e3)], 0.0);
    let lambda = LAMBDA * (rows as f64 / CODES as f64).sqrt();
    for z in 1..CODES - 1 {
        let l = lambda * weight(z);
        add(&[(z - 1, l), (z, -2.0 * l), (z + 1, l)], 0.0);
    }
    let x = linalg::solve(ata, atb, n)?;
    let mut g: Vec<f64> = x.get(..CODES)?.to_vec();
    // a response is monotonic: noise at the ends of the range must not fold it back
    for i in 1..g.len() {
        let prev = g.get(i - 1).copied().unwrap_or(f64::NEG_INFINITY);
        if let Some(v) = g.get_mut(i) {
            *v = v.max(prev + 1e-6);
        }
    }
    g.iter().all(|v| v.is_finite()).then_some(g)
}

/// The decoded value a display-referred file stores for code `z` (for tests and callers building brackets).
pub fn decoded(z: f32) -> f32 {
    srgb_to_linear(z.clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lcc_raster::Rgb32f;

    /// A bracket of a smooth radiance ramp written through `curve` (linear exposure → 0..1 code), 8-bit, decoded
    /// as the codecs do (sRGB curve undone).
    fn bracket(curve: impl Fn(f32) -> f32 + Sync + Send, stops: &[f64]) -> (Vec<HalfImage>, Vec<f64>, Rgb32f) {
        let (w, h) = (200, 150);
        let e = Rgb32f::from_fn(w, h, |x, y| {
            let t = (x as f32 + 0.5) / w as f32;
            let v = 0.004 * (2f32).powf(t * 9.0) * (1.0 + 0.1 * (y as f32 / h as f32));
            [v, v * 0.8, v * 1.1]
        });
        let imgs = stops
            .iter()
            .map(|&s| {
                let k = 2f32.powf(s as f32);
                HalfImage::from_rgb32f(&e.map(|p| p.map(|v| decoded((curve((v * k).min(1.0)) * 255.0).round() / 255.0))))
            })
            .collect();
        (imgs, stops.iter().map(|s| s * std::f64::consts::LN_2).collect(), e)
    }

    #[test]
    fn a_known_gamma_is_recovered_as_linear_radiance() {
        let gamma = 2.6f32;
        let stops = [-2.0, 0.0, 2.0];
        let (imgs, ln_t, e) = bracket(|v| v.powf(1.0 / gamma), &stops);
        let refs: Vec<&HalfImage> = imgs.iter().collect();
        let r = recover(&refs, &ln_t).unwrap();
        // the middle frame through the recovered curve is proportional to the scene: ratios between pixels match
        let mid = &imgs[1];
        let (mut sx, mut sy, mut sxx, mut sxy, mut n) = (0f64, 0f64, 0f64, 0f64, 0f64);
        for (i, p) in e.data.iter().enumerate() {
            let v = mid.get(i)[1];
            let code = linear_to_srgb(v) * 255.0;
            if !(20.0..235.0).contains(&code) {
                continue;
            }
            let (x, y) = ((p[1] as f64).ln(), (r.apply(1, v) as f64).ln());
            sx += x;
            sy += y;
            sxx += x * x;
            sxy += x * y;
            n += 1.0;
        }
        let slope = (n * sxy - sx * sy) / (n * sxx - sx * sx);
        assert!((slope - 1.0).abs() < 0.05, "log-log slope {slope}");
        // without it, the decoded values are far from linear (slope 2.2 / 2.6)
        let mut raw_slope = (0f64, 0f64, 0f64, 0f64, 0f64);
        for (i, p) in e.data.iter().enumerate() {
            let v = mid.get(i)[1];
            let code = linear_to_srgb(v) * 255.0;
            if (20.0..235.0).contains(&code) {
                let (x, y) = ((p[1] as f64).ln(), (v as f64).ln());
                raw_slope = (raw_slope.0 + x, raw_slope.1 + y, raw_slope.2 + x * x, raw_slope.3 + x * y, raw_slope.4 + 1.0);
            }
        }
        let (sx, sy, sxx, sxy, n) = raw_slope;
        let s0 = (n * sxy - sx * sy) / (n * sxx - sx * sx);
        assert!((s0 - 1.0).abs() > 0.1, "the test bracket should be non-linear: {s0}");
        // white stays white, black black
        for c in 0..3 {
            assert!((r.apply(c, 1.0) - 1.0).abs() < 1e-3, "{}", r.apply(c, 1.0));
            assert_eq!(r.apply(c, 0.0), 0.0);
        }
    }

    #[test]
    fn an_srgb_camera_comes_back_as_the_identity() {
        let (imgs, ln_t, _) = bracket(linear_to_srgb, &[-1.0, 0.0, 1.0, 2.0]);
        let refs: Vec<&HalfImage> = imgs.iter().collect();
        let r = recover(&refs, &ln_t).unwrap();
        for v in [0.02f32, 0.05, 0.1, 0.2, 0.4] {
            let ratio = r.apply(1, v) / v;
            let ratio2 = r.apply(1, v * 2.0) / (v * 2.0);
            assert!((ratio / ratio2 - 1.0).abs() < 0.08, "{v}: {ratio} vs {ratio2}");
        }
    }

    #[test]
    fn hostile_brackets_give_no_curve_and_no_panic() {
        let flat = HalfImage::from_rgb32f(&Rgb32f::from_fn(8, 8, |_, _| [0.5; 3]));
        let _ = recover(&[&flat, &flat], &[0.0, 1.0]);
        assert!(recover(&[&flat], &[0.0]).is_none());
        assert!(recover(&[&flat, &flat], &[0.0]).is_none());
        let tiny = HalfImage::from_rgb32f(&Rgb32f::new(1, 1));
        assert!(recover(&[&tiny, &tiny], &[0.0, 1.0]).is_none());
        let nan = HalfImage::from_rgb32f(&Rgb32f::from_fn(40, 40, |x, _| [f32::NAN, x as f32, -1.0]));
        let _ = recover(&[&nan, &nan, &nan], &[f64::NAN, 0.0, f64::INFINITY]);
    }
}
