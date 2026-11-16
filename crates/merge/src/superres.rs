//! Enhance ▸ Super Resolution without a learned model: 2× per side by iterative back-projection.
//!
//! Sources: Irani & Peleg, "Improving resolution by image registration" (CVGIP 1991) — iterative back-projection
//! for a single frame; the ringing guard (each output pixel held to the range of the 3×3 input pixels around it) is
//! own design.
//!
//! The model is that each input pixel integrates a 2×2 block of the wanted picture (a box filter, as a sensor pixel
//! does). Starting from a Lanczos enlargement, the error between the input and the box-reduced estimate is enlarged
//! and added back until the estimate reduces to the input; edges come back sharper than any fixed interpolation
//! filter gives, while flat areas stay flat.

use lcc_raster::Rgb32f;
use lcc_raster::resample::{Filter, half, resize};
use rayon::prelude::*;

use crate::{MergeError, Result};

/// Largest output (megapixels) a Super Resolution run makes.
pub const MAX_OUTPUT_MEGAPIXELS: f64 = 400.0;
/// Back-projection passes.
const ITERATIONS: usize = 12;

/// `img` enlarged 2× per side. `progress(fraction)` returns false to cancel.
pub fn super_resolve(img: &Rgb32f, progress: &(dyn Fn(f32) -> bool + Sync)) -> Result<Rgb32f> {
    let (w, h) = (img.width, img.height);
    if w == 0 || h == 0 {
        return Err(MergeError::Output("empty image".into()));
    }
    let (ow, oh) = (w.checked_mul(2), h.checked_mul(2));
    let (Some(ow), Some(oh)) = (ow, oh) else { return Err(MergeError::Output("image too large".into())) };
    if ow as f64 * oh as f64 / 1e6 > MAX_OUTPUT_MEGAPIXELS {
        return Err(MergeError::Output(format!("Super Resolution makes at most {MAX_OUTPUT_MEGAPIXELS} MP; this photo would be larger")));
    }
    // the estimate plus one enlarged error image are the largest buffers: refuse rather than abort on allocation
    for _ in 0..2 {
        drop(Rgb32f::try_new(ow, oh).map_err(|_| MergeError::Output("not enough memory for Super Resolution".into()))?);
    }
    let src = Rgb32f { width: w, height: h, data: img.data.iter().map(|p| p.map(|v| if v.is_finite() { v } else { 0.0 })).collect() };
    // per input pixel, the range of its 3×3 neighbourhood
    let range: Vec<([f32; 3], [f32; 3])> = (0..w * h)
        .into_par_iter()
        .map(|i| {
            let (x, y) = (i % w, i / w);
            let (mut lo, mut hi) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
            for ny in y.saturating_sub(1)..=(y + 1).min(h - 1) {
                for nx in x.saturating_sub(1)..=(x + 1).min(w - 1) {
                    let p = src.get(nx, ny);
                    for c in 0..3 {
                        lo[c] = lo[c].min(p[c]);
                        hi[c] = hi[c].max(p[c]);
                    }
                }
            }
            (lo, hi)
        })
        .collect();
    let mut est = resize(&src, ow, oh, Filter::Lanczos3);
    for k in 0..ITERATIONS {
        if !progress(k as f32 / ITERATIONS as f32) {
            return Err(MergeError::Cancelled);
        }
        let down = half(&est);
        if down.width != w || down.height != h {
            return Err(MergeError::Output("unexpected reduced size".into()));
        }
        let err =
            Rgb32f { width: w, height: h, data: src.data.par_iter().zip(&down.data).map(|(a, b)| [a[0] - b[0], a[1] - b[1], a[2] - b[2]]).collect() };
        let up = resize(&err, ow, oh, Filter::Lanczos3);
        est.data.par_chunks_mut(ow).zip(up.data.par_chunks(ow)).enumerate().for_each(|(y, (row, erow))| {
            for (x, (p, e)) in row.iter_mut().zip(erow).enumerate() {
                let (lo, hi) = range.get((y / 2) * w + x / 2).copied().unwrap_or(([0.0; 3], [0.0; 3]));
                for c in 0..3 {
                    let v = p[c] + e[c];
                    p[c] = if lo[c] <= hi[c] { v.clamp(lo[c], hi[c]) } else { v };
                }
            }
        });
    }
    progress(1.0);
    Ok(est)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sharp-edged detail: discs, stripes and a diagonal edge on a smooth colour field.
    fn scene(w: usize, h: usize) -> Rgb32f {
        Rgb32f::from_fn(w, h, |x, y| {
            let (fx, fy) = (x as f32, y as f32);
            let d = ((fx - w as f32 * 0.3).powi(2) + (fy - h as f32 * 0.45).powi(2)).sqrt();
            let lum = if d < w as f32 * 0.2 {
                0.8
            } else if x > w * 2 / 3 && (y / 5) % 2 == 0 {
                0.1
            } else if fx * 0.8 + fy > w as f32 * 0.9 {
                0.6
            } else {
                0.3
            };
            [lum * (0.8 + 0.2 * fx / w as f32), lum * 0.9, lum * (0.6 + 0.4 * fy / h as f32)]
        })
    }

    fn psnr(a: &Rgb32f, b: &Rgb32f) -> f64 {
        let se: f64 = a.data.iter().zip(&b.data).flat_map(|(p, q)| (0..3).map(move |c| ((p[c] - q[c]) as f64).powi(2))).sum();
        10.0 * (1.0 / (se / (a.data.len() * 3) as f64)).log10()
    }

    #[test]
    fn doubles_each_side_and_recovers_more_detail_than_bicubic() {
        let truth = scene(160, 128);
        let small = half(&truth);
        let sr = super_resolve(&small, &|_| true).unwrap();
        assert_eq!((sr.width, sr.height), (truth.width, truth.height));
        let bicubic = resize(&small, truth.width, truth.height, Filter::Mitchell);
        let lanczos = resize(&small, truth.width, truth.height, Filter::Lanczos3);
        let (ps, pb, pl) = (psnr(&sr, &truth), psnr(&bicubic, &truth), psnr(&lanczos, &truth));
        assert!(ps > pb + 2.0, "super resolution {ps:.2} dB vs bicubic {pb:.2} dB");
        assert!(ps > pl, "super resolution {ps:.2} dB vs Lanczos {pl:.2} dB");
    }

    #[test]
    fn flat_stays_flat_and_hostile_input_is_an_error_or_finite() {
        let flat = Rgb32f::filled(9, 7, [0.25, 0.5, 0.75]);
        let out = super_resolve(&flat, &|_| true).unwrap();
        assert!(out.data.iter().all(|p| (p[0] - 0.25).abs() < 1e-5 && (p[2] - 0.75).abs() < 1e-5));
        assert!(super_resolve(&Rgb32f::new(0, 5), &|_| true).is_err());
        let mut bad = scene(12, 10);
        bad.data[3] = [f32::NAN, f32::INFINITY, -1e30];
        let out = super_resolve(&bad, &|_| true).unwrap();
        assert!(out.data.iter().all(|p| p.iter().all(|v| v.is_finite())));
        assert!(matches!(super_resolve(&flat, &|_| false), Err(MergeError::Cancelled)));
        let one = super_resolve(&Rgb32f::filled(1, 1, [0.5; 3]), &|_| true).unwrap();
        assert_eq!((one.width, one.height), (2, 2));
    }
}
