//! The correction models and their conversion to the DNG-style polynomials the pipeline applies.
//!
//! All models are functions of a normalized radius `r`. Following the lensfun database documentation, distortion and
//! TCA use `r = 1` at half the shorter side of the calibration frame, vignetting `r = 1` at half its diagonal (the
//! corner). Distortion and TCA map a corrected (undistorted) radius to the radius where the light actually landed,
//! which is the inverse map a resampler needs.

/// Radial distortion: `r_d = r_u · ratio(r_u)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Distortion {
    /// `r_d = r_u · (1 − k1 + k1·r_u²)`.
    Poly3 { k1: f64 },
    /// `r_d = r_u · (1 + k1·r_u² + k2·r_u⁴)`.
    Poly5 { k1: f64, k2: f64 },
    /// Panorama Tools: `r_d = r_u · (a·r_u³ + b·r_u² + c·r_u + 1 − a − b − c)`.
    PtLens { a: f64, b: f64, c: f64 },
}

impl Distortion {
    /// `r_d / r_u` at `r_u`.
    pub fn ratio(&self, r: f64) -> f64 {
        match *self {
            Distortion::Poly3 { k1 } => 1.0 - k1 + k1 * r * r,
            Distortion::Poly5 { k1, k2 } => {
                let r2 = r * r;
                1.0 + k1 * r2 + k2 * r2 * r2
            }
            Distortion::PtLens { a, b, c } => ((a * r + b) * r + c) * r + 1.0 - a - b - c,
        }
    }
}

/// Transverse (lateral) chromatic aberration of red and blue relative to green: `r_ch = r_g · ratio(ch, r_g)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Tca {
    /// `r_ch = r_g · k`.
    Linear { kr: f64, kb: f64 },
    /// `r_ch = r_g · (b·r_g² + c·r_g + v)`, per channel.
    Poly3 { vr: f64, vb: f64, cr: f64, cb: f64, br: f64, bb: f64 },
}

impl Tca {
    /// `r_ch / r_g` for `ch` 0 (red), 2 (blue); 1 for green.
    pub fn ratio(&self, ch: usize, r: f64) -> f64 {
        match (*self, ch) {
            (_, 1) => 1.0,
            (Tca::Linear { kr, .. }, 0) => kr,
            (Tca::Linear { kb, .. }, _) => kb,
            (Tca::Poly3 { vr, cr, br, .. }, 0) => (br * r + cr) * r + vr,
            (Tca::Poly3 { vb, cb, bb, .. }, _) => (bb * r + cb) * r + vb,
        }
    }
}

/// Vignetting, `pa` model: the recorded brightness is the true brightness times `1 + k1·r² + k2·r⁴ + k3·r⁶`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vignetting {
    pub k1: f64,
    pub k2: f64,
    pub k3: f64,
}

impl Vignetting {
    pub fn attenuation(&self, r: f64) -> f64 {
        let r2 = r * r;
        1.0 + r2 * (self.k1 + r2 * (self.k2 + r2 * self.k3))
    }
}

/// Least squares `Σ w (Σ_j c_j·basis_j(x) − y)²` for small dense systems (normal equations, partial pivoting).
/// `None` when the system is singular or a value isn't finite.
pub(crate) fn least_squares<const N: usize>(samples: &[([f64; N], f64)]) -> Option<[f64; N]> {
    let mut a = [[0.0; N]; N];
    let mut b = [0.0; N];
    for (basis, y) in samples {
        for i in 0..N {
            b[i] += basis[i] * y;
            for j in 0..N {
                a[i][j] += basis[i] * basis[j];
            }
        }
    }
    for col in 0..N {
        let pivot = (col..N).max_by(|&x, &y| a[x][col].abs().total_cmp(&a[y][col].abs()))?;
        if !a[pivot][col].is_finite() || a[pivot][col].abs() < 1e-14 {
            return None;
        }
        a.swap(col, pivot);
        b.swap(col, pivot);
        for row in 0..N {
            if row != col {
                let f = a[row][col] / a[col][col];
                for k in col..N {
                    a[row][k] -= f * a[col][k];
                }
                b[row] -= f * b[col];
            }
        }
    }
    let mut x = [0.0; N];
    for i in 0..N {
        x[i] = b[i] / a[i][i];
    }
    x.iter().all(|v| v.is_finite()).then_some(x)
}

/// Fit `ratio(r) ≈ k0 + k1·r² + k2·r⁴ + k3·r⁶` over `r ∈ [0, 1]` (the DNG `WarpRectilinear` radial form).
/// Returns the coefficients and the largest radial error `max |r·(fit − ratio)|`, in the same units as `r`.
pub fn fit_even4(ratio: impl Fn(f64) -> f64) -> Option<([f64; 4], f64)> {
    let pts: Vec<f64> = (0..=SAMPLES).map(|i| i as f64 / SAMPLES as f64).collect();
    let samples: Vec<([f64; 4], f64)> = pts
        .iter()
        .map(|&r| {
            let r2 = r * r;
            // weight by r: the error that matters is the displacement r·Δratio
            ([r, r * r2, r * r2 * r2, r * r2 * r2 * r2], r * ratio(r))
        })
        .collect();
    let k = least_squares(&samples)?;
    let eval = |r: f64| {
        let r2 = r * r;
        k[0] + r2 * (k[1] + r2 * (k[2] + r2 * k[3]))
    };
    let err = pts.iter().map(|&r| (r * (eval(r) - ratio(r))).abs()).fold(0.0, f64::max);
    // the warp must stay monotonic (no folding): r·fit(r) increasing
    let mono = pts.windows(2).all(|w| w[1] * eval(w[1]) > w[0] * eval(w[0]));
    (err.is_finite() && mono).then_some((k, err))
}

/// Fit `gain(r) ≈ 1 + k0·r² + … + k4·r¹⁰` over `r ∈ [0, 1]` (the DNG `FixVignetteRadial` form).
/// Returns the coefficients and the largest relative gain error.
pub fn fit_gain5(gain: impl Fn(f64) -> f64) -> Option<([f64; 5], f64)> {
    let pts: Vec<f64> = (0..=SAMPLES).map(|i| i as f64 / SAMPLES as f64).collect();
    let samples: Vec<([f64; 5], f64)> = pts
        .iter()
        .map(|&r| {
            let r2 = r * r;
            ([r2, r2 * r2, r2.powi(3), r2.powi(4), r2.powi(5)], gain(r) - 1.0)
        })
        .collect();
    let k = least_squares(&samples)?;
    let eval = |r: f64| {
        let r2 = r * r;
        1.0 + r2 * (k[0] + r2 * (k[1] + r2 * (k[2] + r2 * (k[3] + r2 * k[4]))))
    };
    let err = pts.iter().map(|&r| ((eval(r) - gain(r)) / gain(r).abs().max(1e-9)).abs()).fold(0.0, f64::max);
    err.is_finite().then_some((k, err))
}

const SAMPLES: usize = 64;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn models_are_identity_at_centre_and_documented_at_one() {
        assert_eq!(Distortion::Poly3 { k1: 0.1 }.ratio(1.0), 1.0, "poly3 keeps r = 1");
        assert!((Distortion::Poly5 { k1: 0.1, k2: 0.01 }.ratio(1.0) - 1.11).abs() < 1e-12);
        assert!((Distortion::PtLens { a: 0.01, b: -0.02, c: 0.03 }.ratio(1.0) - 1.0).abs() < 1e-12, "ptlens keeps r = 1");
        assert_eq!(Tca::Linear { kr: 1.001, kb: 0.999 }.ratio(0, 0.5), 1.001);
        assert_eq!(Tca::Poly3 { vr: 1.0, vb: 1.0, cr: 0.0, cb: 0.0, br: 0.001, bb: 0.0 }.ratio(1, 0.7), 1.0);
        assert!((Vignetting { k1: -0.3, k2: 0.0, k3: 0.0 }.attenuation(1.0) - 0.7).abs() < 1e-12);
    }

    #[test]
    fn even_fit_is_exact_for_even_models_and_close_for_ptlens() {
        let (k, err) = fit_even4(|r| Distortion::Poly5 { k1: -0.05, k2: 0.01 }.ratio(r)).unwrap();
        assert!(err < 1e-9, "{err}");
        assert!((k[0] - 1.0).abs() < 1e-9 && (k[1] + 0.05).abs() < 1e-9 && (k[2] - 0.01).abs() < 1e-9);
        let (_, err) = fit_even4(|r| Distortion::PtLens { a: 0.01, b: -0.03, c: 0.0 }.ratio(r)).unwrap();
        assert!(err < 2e-4, "{err}");
    }

    #[test]
    fn folding_maps_are_refused() {
        assert!(fit_even4(|r| 1.0 - 2.0 * r * r).is_none());
    }

    #[test]
    fn gain_fit_matches_inverse_attenuation() {
        let v = Vignetting { k1: -0.4, k2: 0.1, k3: -0.02 };
        let (_, err) = fit_gain5(|r| 1.0 / v.attenuation(r)).unwrap();
        assert!(err < 1e-3, "{err}");
    }

    #[test]
    fn singular_systems_are_none() {
        assert!(least_squares::<2>(&[([1.0, 1.0], 1.0), ([2.0, 2.0], 2.0)]).is_none());
        assert!(least_squares::<1>(&[]).is_none());
    }
}
