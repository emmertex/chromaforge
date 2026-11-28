//! The AHD demosaic of a Bayer raw on the GPU (`wgsl/demosaic.wgsl`; issue #1377 step 3). The CPU's
//! (`lcc_raw::demosaic`, AHD) is the reference: the kernels are a pass-for-pass port of it, on strips of the mosaic
//! padded by mirror reflection as the CPU pads its tiles, so a pixel differs from the CPU's only by the rounding of
//! the device's arithmetic (and, where that rounding tips a homogeneity count, by which of the two candidates, both
//! good, it takes).
//!
//! Strips keep each buffer under the device's limit and the device memory used at a few hundred MB for any size of
//! sensor. Any failure (no device, out of memory, a device error, a panic in wgpu) is `None`: the caller demosaics on
//! the CPU. A device error also stops renders on the device, as a render's would.

use std::sync::Arc;

use lcc_raster::Rgb32f;

use crate::ctx::{self, FailKind, Gpu};

/// Rows of the padding on each side of a strip: as deep as AHD reads (`ahd::MARGIN`).
const MARGIN: usize = 6;
/// The most rows of output a strip has: about 100 MB of device buffers at 8256 columns (45 MP).
const MAX_ROWS: usize = 512;
/// The smallest strip worth a dispatch (narrower devices than this give up).
const MIN_ROWS: usize = 16;

/// `x` reflected into `0..n` as `lcc_raw::demosaic::reflect` does (keeps Bayer parity).
fn reflect(i: isize, n: usize) -> usize {
    if i >= 0 && (i as usize) < n {
        return i as usize;
    }
    let n = n as isize;
    if n <= 1 {
        return 0;
    }
    let period = 2 * (n - 1);
    let mut i = i.rem_euclid(period);
    if i >= n {
        i = period - i;
    }
    i as usize
}

/// AHD of the `w × h` Bayer mosaic `data` (one sample a pixel, normalised), whose colour at `(x, y)` is
/// `pattern[(y & 1) * 2 + (x & 1)]` (0 red, 1 green, 2 blue), on the renderer's GPU. `None`: no GPU, turned off, a
/// mosaic this does not do (smaller than 4 × 4, a pattern that is not Bayer), or the device failed.
pub fn ahd(w: usize, h: usize, data: &[f32], pattern: [u8; 4]) -> Option<Rgb32f> {
    ahd_in_strips(w, h, data, pattern, MAX_ROWS)
}

/// [`ahd`] in strips of at most `max_rows` rows.
fn ahd_in_strips(w: usize, h: usize, data: &[f32], pattern: [u8; 4], max_rows: usize) -> Option<Rgb32f> {
    if w < 4 || h < 4 || data.len() != w.checked_mul(h)? || pattern.iter().any(|&c| c > 2) {
        return None;
    }
    if !crate::allowed() || crate::gate::cpu_only() {
        return None;
    }
    let gpu = crate::device()?;
    let _work = crate::exit::enter()?;
    let _ = ctx::take_failure();
    let result = {
        let _scope = ctx::RenderScope::new(&gpu);
        let errors = ctx::ErrorScopes::push(&gpu);
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&gpu, w, h, data, pattern, max_rows)));
        let scoped = errors.pop();
        match r {
            Ok(img) => scoped.or_else(ctx::take_failure).map_or(img, Err),
            Err(_) => Err(ctx::Failure { kind: FailKind::Fatal, reason: "the demosaic panicked".into() }),
        }
    };
    match result {
        Ok(img) => Some(img),
        Err(f) => {
            log::warn!("gpu: demosaic on the CPU: {}", f.reason);
            if matches!(f.kind, FailKind::Fatal) {
                crate::mark_broken(gpu.generation, &f.reason);
            }
            None
        }
    }
}

/// Output rows a strip of `bw` padded columns can have on `gpu`, or `None` when not even [`MIN_ROWS`] fit.
fn strip_rows(gpu: &Gpu, bw: usize, h: usize, max_rows: usize) -> Option<usize> {
    let limit = gpu.limit() as usize / 4;
    // the largest buffer is the Lab: 6 values a padded pixel
    let rows = (limit / 6 / bw.max(1)).saturating_sub(2 * MARGIN).min(max_rows.max(1)).min(h);
    (rows >= MIN_ROWS.min(h).min(max_rows.max(1))).then_some(rows)
}

fn run(gpu: &Arc<Gpu>, w: usize, h: usize, data: &[f32], pattern: [u8; 4], max_rows: usize) -> Result<Rgb32f, ctx::Failure> {
    let failure = |reason: String| ctx::Failure { kind: FailKind::Limit, reason };
    let bw = w + 2 * MARGIN;
    let rows = strip_rows(gpu, bw, h, max_rows).ok_or_else(|| failure(format!("a {w}-pixel-wide mosaic does not fit the device's buffers")))?;
    let mut out = Rgb32f::try_new(w, h).map_err(|_| ctx::Failure { kind: FailKind::OutOfMemory, reason: "no memory for the picture".into() })?;
    let mut y0 = 0;
    let mut padded = Vec::new();
    while y0 < h {
        let th = rows.min(h - y0);
        let bh = th + 2 * MARGIN;
        padded.clear();
        padded.reserve(bw * bh);
        for by in 0..bh {
            let ry = reflect(y0 as isize + by as isize - MARGIN as isize, h);
            let row = data.get(ry * w..ry * w + w).ok_or_else(|| failure("the mosaic is shorter than its size".into()))?;
            padded.extend((0..MARGIN).map(|k| row.get(reflect(k as isize - MARGIN as isize, w)).copied().unwrap_or(0.0)));
            padded.extend_from_slice(row);
            padded.extend((0..MARGIN).map(|k| row.get(reflect((w + k) as isize, w)).copied().unwrap_or(0.0)));
        }
        // the padded strip's colour at (bx, by) is the image's at (bx - MARGIN, y0 + by - MARGIN): the same parity
        // in x (MARGIN is even), shifted by y0's in y
        let mut pat = 0u32;
        for py in 0..2 {
            for px in 0..2 {
                let c = pattern.get(((py + y0) & 1) * 2 + px).copied().unwrap_or(1);
                pat |= u32::from(c & 3) << ((py * 2 + px) * 2);
            }
        }
        let n = bw * bh;
        let mos = gpu.upload(&padded);
        let (gr, lab, homo, rgb) = (gpu.buffer(2 * n), gpu.buffer(6 * n), gpu.buffer(n), gpu.buffer(3 * w * th));
        let p = [bw as u32, bh as u32, pat];
        let mut enc = gpu.encoder();
        let bufs = [Some(&mos), Some(&gr), Some(&lab), Some(&homo), Some(&rgb)];
        for kernel in ["ahd_green", "ahd_to_lab", "ahd_homogeneity", "ahd_choose"] {
            gpu.run(&mut enc, kernel, &p, &bufs, ctx::groups1(n));
        }
        let back: Vec<f32> = gpu.finish_and_read(enc, &rgb, 3 * w * th);
        if let Some(f) = ctx::take_failure() {
            return Err(f);
        }
        let dst = out.data.get_mut(y0 * w..(y0 + th) * w).ok_or_else(|| failure("a strip outside the picture".into()))?;
        for (d, s) in dst.iter_mut().zip(back.as_chunks::<3>().0) {
            *d = *s;
        }
        y0 += th;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::{ahd_in_strips, reflect};
    use lcc_raster::Rgb32f;
    use lcc_raw::{Cfa, Method};

    #[test]
    fn reflection_keeps_the_bayer_parity() {
        for n in [4usize, 5, 9] {
            for i in -6isize..(n as isize + 6) {
                let r = reflect(i, n);
                assert!(r < n);
                assert_eq!(r % 2, i.rem_euclid(2) as usize, "{i} in {n}");
            }
        }
    }

    /// A photo-like scene: luminance edges (a disc, fine stripes, a diagonal, texture) over a slow colour change.
    fn scene(w: usize, h: usize) -> Rgb32f {
        Rgb32f::from_fn(w, h, |x, y| {
            let (fx, fy) = (x as f32, y as f32);
            let d = ((fx - w as f32 * 0.35).powi(2) + (fy - h as f32 * 0.4).powi(2)).sqrt();
            let lum = if d < w as f32 * 0.18 {
                0.85
            } else if x > w * 2 / 3 && (y / 3) % 2 == 0 {
                0.15
            } else if fx + fy * 0.5 > w as f32 * 1.1 {
                0.7
            } else {
                0.35 + 0.1 * ((fx * 0.7).sin() * (fy * 0.9).cos())
            };
            [lum * (0.9 + 0.1 * fx / w as f32), lum * 0.8, lum * (0.55 + 0.3 * fy / h as f32)]
        })
    }

    /// The GPU's AHD against the CPU's, the reference, for every Bayer phase, odd sizes and strips that split the
    /// picture (their seams must not show): the same picture but for the device's rounding. A pixel may take the
    /// other direction's candidate where rounding tips a homogeneity count, so the bound is on how many differ by
    /// more than rounding, and on the mean.
    #[test]
    fn the_gpu_demosaic_matches_the_cpu() {
        if crate::device().is_none() {
            lcc_testkit::skip!(Gpu, "no GPU device");
            return;
        }
        for (pattern, w, h, rows) in [("RGGB", 160, 120, 512), ("BGGR", 157, 97, 16), ("GRBG", 64, 81, 20), ("GBRG", 33, 50, 17)] {
            let cfa = Cfa::bayer(pattern).unwrap();
            let n = lcc_raw::demosaic::mosaic_from_rgb(&scene(w, h), &cfa);
            let cpu = lcc_raw::demosaic(&n, Method::Ahd);
            let colours = [cfa.color_at(0, 0), cfa.color_at(1, 0), cfa.color_at(0, 1), cfa.color_at(1, 1)];
            let Some(gpu) = ahd_in_strips(w, h, &n.data, colours, rows) else { panic!("{pattern}: the GPU demosaic failed") };
            assert_eq!((gpu.width, gpu.height), (w, h));
            let (mut off, mut sum) = (0usize, 0f64);
            for (a, b) in gpu.data.iter().zip(&cpu.data) {
                let d = (0..3).map(|c| (a[c] - b[c]).abs()).fold(0f32, f32::max);
                assert!(d.is_finite() && d < 0.2, "{pattern}: {a:?} vs {b:?}");
                off += usize::from(d > 1e-4);
                sum += f64::from(d);
            }
            let px = (w * h) as f64;
            assert!((off as f64) < px * 0.005, "{pattern} {w}x{h}: {off} of {px} pixels differ");
            assert!(sum / px < 1e-4, "{pattern}: mean difference {}", sum / px);
        }
    }

    /// What it does not do is `None`, not a panic or a wrong picture: too small, a short buffer, a colour that is not
    /// red, green or blue.
    #[test]
    fn mosaics_it_does_not_do_are_left_to_the_cpu() {
        let data = vec![0.5f32; 64];
        assert!(ahd_in_strips(3, 3, &data, [0, 1, 1, 2], 512).is_none());
        assert!(ahd_in_strips(8, 9, &data, [0, 1, 1, 2], 512).is_none());
        assert!(ahd_in_strips(8, 8, &data, [0, 1, 1, 3], 512).is_none());
    }
}
