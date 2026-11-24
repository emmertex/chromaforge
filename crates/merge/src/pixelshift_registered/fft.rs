//! FFT helpers of the registered Pixel Shift merge: square 2-D transforms on top of `rustfft`, a Hann window and
//! phase correlation.
//!
//! Sources: Cooley and Tukey, "An algorithm for the machine calculation of complex Fourier series", Math.
//! Computation 19 (1965), through the `rustfft` library; Kuglin and Hines, "The phase correlation image alignment
//! method", Proc. IEEE Int. Conf. on Cybernetics and Society (1975), with a parabolic fit of the correlation peak
//! for the sub-sample part (own design).

use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};
use std::sync::Arc;

pub type C64 = Complex<f64>;

/// Largest plane side accepted (a plane of this size already holds 16.7 million samples).
pub const MAX_SIDE: usize = 4096;

/// Planned forward and inverse 2-D transforms of an `n` × `n` plane stored row by row.
pub struct Fft2 {
    n: usize,
    fwd: Arc<dyn Fft<f64>>,
    inv: Arc<dyn Fft<f64>>,
}

impl Fft2 {
    /// `None` when `n` is below 2 or above [`MAX_SIDE`].
    pub fn new(n: usize) -> Option<Fft2> {
        if !(2..=MAX_SIDE).contains(&n) {
            return None;
        }
        let mut planner = FftPlanner::<f64>::new();
        Some(Fft2 { n, fwd: planner.plan_fft_forward(n), inv: planner.plan_fft_inverse(n) })
    }

    pub fn side(&self) -> usize {
        self.n
    }

    /// Forward transform in place; `false` (data untouched) when `data` is not `n * n` long.
    pub fn forward(&self, data: &mut [C64]) -> bool {
        self.run(data, true)
    }

    /// Inverse transform in place, scaled by `1 / (n * n)`; `false` when `data` is not `n * n` long.
    pub fn inverse(&self, data: &mut [C64]) -> bool {
        if !self.run(data, false) {
            return false;
        }
        let s = 1.0 / (self.n * self.n) as f64;
        for c in data.iter_mut() {
            *c *= s;
        }
        true
    }

    fn run(&self, data: &mut [C64], forward: bool) -> bool {
        if data.len() != self.n * self.n {
            return false;
        }
        let f = if forward { &self.fwd } else { &self.inv };
        f.process(data);
        transpose(data, self.n);
        f.process(data);
        transpose(data, self.n);
        true
    }
}

fn transpose(data: &mut [C64], n: usize) {
    for j in 0..n {
        for i in j + 1..n {
            data.swap(j * n + i, i * n + j);
        }
    }
}

/// Periodic Hann window of `n` samples.
pub fn hann(n: usize) -> Vec<f64> {
    (0..n).map(|i| 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / n.max(1) as f64).cos()).collect()
}

/// Wrapped index: `i` in `0..n` becomes `i` for `i < n / 2` and `i - n` above.
fn wrap(i: usize, n: usize) -> f64 {
    if i < n / 2 { i as f64 } else { i as f64 - n as f64 }
}

/// Result of a phase correlation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PhaseShift {
    /// Displacement `s` with `b(x) ≈ a(x + s)` in plane samples (x right, y down).
    pub shift: [f64; 2],
    /// Height of the correlation peak: about 1 for a pure shift of identical content, near 0 for unrelated planes.
    pub peak: f64,
}

/// Windowed spectrum of a plane with its mean removed; `None` when the plane has the wrong length or non-finite values.
fn spectrum(fft: &Fft2, win: &[f64], a: &[f64]) -> Option<Vec<C64>> {
    let n = fft.side();
    if a.len() != n * n || a.iter().any(|v| !v.is_finite()) {
        return None;
    }
    let m = a.iter().sum::<f64>() / a.len() as f64;
    let mut f: Vec<C64> = a
        .iter()
        .enumerate()
        .map(|(i, v)| C64::new((v - m) * win.get(i % n).copied().unwrap_or(0.0) * win.get(i / n).copied().unwrap_or(0.0), 0.0))
        .collect();
    fft.forward(&mut f).then_some(f)
}

/// Phase correlation of two `n` × `n` planes (a Hann window, the mean removed, the cross spectrum normalised to unit
/// magnitude, the peak refined by a parabola in x and y). `None` for planes of the wrong length, non-finite values or
/// no energy.
pub fn phase_correlation(fft: &Fft2, a: &[f64], b: &[f64]) -> Option<PhaseShift> {
    let n = fft.side();
    let win = hann(n);
    let fa = spectrum(fft, &win, a)?;
    let fb = spectrum(fft, &win, b)?;
    let mut p: Vec<C64> = fb
        .iter()
        .zip(&fa)
        .map(|(b, a)| {
            let c = b * a.conj();
            let m = c.norm();
            if m > 1e-30 { c / m } else { C64::new(0.0, 0.0) }
        })
        .collect();
    if p.iter().all(|c| c.re == 0.0 && c.im == 0.0) {
        return None;
    }
    if !fft.inverse(&mut p) {
        return None;
    }
    let (mut pk, mut pv) = (0usize, f64::MIN);
    for (i, c) in p.iter().enumerate() {
        if c.re > pv {
            pv = c.re;
            pk = i;
        }
    }
    let (tx, ty) = (pk % n, pk / n);
    let at = |x: usize, y: usize| p.get((y % n) * n + (x % n)).map_or(0.0, |c| c.re);
    let parab = |m: f64, c: f64, pl: f64| {
        let d = m - 2.0 * c + pl;
        if d.abs() < 1e-30 { 0.0 } else { 0.5 * (m - pl) / d }
    };
    let fx = parab(at(tx + n - 1, ty), at(tx, ty), at(tx + 1, ty));
    let fy = parab(at(tx, ty + n - 1), at(tx, ty), at(tx, ty + 1));
    // the correlation peak lies at tau = -s for b(x) = a(x + s)
    Some(PhaseShift { shift: [-(wrap(tx, n) + fx), -(wrap(ty, n) + fy)], peak: pv })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A broadband random texture on an `n` × `n` grid (periodic), with a closed form at any sub-sample position.
    fn texture(n: usize) -> impl Fn(f64, f64) -> f64 {
        let waves: Vec<(f64, f64, f64, f64)> = (0..300)
            .map(|i| {
                let h = (i as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ ((i as u64 + 7) << 29);
                let h = (h ^ (h >> 31)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
                let fx = ((h >> 8) % 41) as f64 - 20.0;
                let fy = ((h >> 20) % 41) as f64 - 20.0;
                // broadband content: a sparse spectrum would make the normalised cross spectrum mostly leakage
                (fx, fy, ((h >> 40) % 628) as f64 / 100.0, 1.0 / (1.0 + fx.hypot(fy) / 8.0))
            })
            .collect();
        move |x: f64, y: f64| waves.iter().map(|(fx, fy, ph, a)| a * (2.0 * std::f64::consts::PI * (fx * x + fy * y) / n as f64 + ph).cos()).sum()
    }

    fn plane(n: usize, f: &dyn Fn(f64, f64) -> f64, dx: f64, dy: f64) -> Vec<f64> {
        (0..n * n).map(|i| f((i % n) as f64 + dx, (i / n) as f64 + dy)).collect()
    }

    #[test]
    fn two_dimensional_transform_round_trips() {
        let fft = Fft2::new(32).unwrap();
        let src: Vec<C64> = (0..32 * 32).map(|i| C64::new(((i * 7919) % 101) as f64, ((i * 31) % 17) as f64)).collect();
        let mut d = src.clone();
        assert!(fft.forward(&mut d));
        assert!(fft.inverse(&mut d));
        let err = d.iter().zip(&src).map(|(a, b)| (a - b).norm()).fold(0.0, f64::max);
        assert!(err < 1e-9, "round trip error {err}");
    }

    #[test]
    fn transform_of_a_single_wave_has_one_peak_pair() {
        let n = 16;
        let fft = Fft2::new(n).unwrap();
        let mut d: Vec<C64> = (0..n * n).map(|i| C64::new((2.0 * std::f64::consts::PI * 3.0 * (i % n) as f64 / n as f64).cos(), 0.0)).collect();
        assert!(fft.forward(&mut d));
        let big: Vec<usize> = d.iter().enumerate().filter(|(_, c)| c.norm() > 1.0).map(|(i, _)| i).collect();
        assert_eq!(big.len(), 2, "{big:?}");
    }

    #[test]
    fn whole_and_sub_sample_shifts_are_recovered() {
        let n = 128;
        let fft = Fft2::new(n).unwrap();
        let f = texture(n);
        let a = plane(n, &f, 0.0, 0.0);
        for (dx, dy) in [(0.0, 0.0), (3.0, -2.0), (-7.0, 5.0), (0.4, 0.0), (-1.3, 2.7), (10.55, -8.2)] {
            let b = plane(n, &f, dx, dy); // b(x) = a(x + s) with s = (dx, dy)
            let r = phase_correlation(&fft, &a, &b).unwrap();
            // whole-sample shifts are found to a tenth of a sample, fractional ones to 0.4 (the parabola fit is biased by up to about a third of a sample; the later stages refine, the coarse start only has to be good to about a pixel)
            let tol = if dx.fract() == 0.0 && dy.fract() == 0.0 { 0.1 } else { 0.4 };
            assert!((r.shift[0] - dx).abs() < tol && (r.shift[1] - dy).abs() < tol, "shift ({dx}, {dy}) found as {:?}", r.shift);
            assert!(r.peak > 0.05, "peak {}", r.peak);
        }
    }

    #[test]
    fn unrelated_planes_give_a_low_peak() {
        let n = 128;
        let fft = Fft2::new(n).unwrap();
        let a = plane(n, &texture(n), 0.0, 0.0);
        let b: Vec<f64> = (0..n * n).map(|i| (((i as u64).wrapping_mul(0x2545_F491_4F6C_DD1D) >> 33) % 1000) as f64).collect();
        let same = phase_correlation(&fft, &a, &plane(n, &texture(n), 2.0, 1.0)).unwrap().peak;
        let other = phase_correlation(&fft, &a, &b).unwrap().peak;
        assert!(other < 0.5 * same, "unrelated {other} against related {same}");
    }

    #[test]
    fn hostile_input_is_refused_not_panicked_on() {
        assert!(Fft2::new(0).is_none() && Fft2::new(1).is_none() && Fft2::new(MAX_SIDE + 1).is_none());
        let fft = Fft2::new(16).unwrap();
        let ok = vec![1.0; 256];
        let mut nan = ok.clone();
        nan[5] = f64::NAN;
        assert!(phase_correlation(&fft, &ok, &nan).is_none());
        assert!(phase_correlation(&fft, &ok, &ok[..100]).is_none());
        assert!(phase_correlation(&fft, &ok, &ok).is_none(), "a constant plane has no energy once the mean is removed");
        let mut short = vec![C64::new(0.0, 0.0); 10];
        assert!(!fft.forward(&mut short) && !fft.inverse(&mut short));
        assert!(hann(0).is_empty());
    }
}
