//! Lightroom Classic's brush, measured (TEST-NOTES 2026-10-28 notes 18 and 22, "Brush model, exact"): what a
//! stroke's Size and Feather mean, and the shape of one dab.
//!
//! - **Size** is the solid core: a dab's radius `R0` at Feather 0 (stored in [`crate::BrushStroke::size`], a
//!   fraction of the photo's long side).
//! - **Feather** adds a soft halo *outside* the core. The dab is cut to 0 at `Radius = R0 · M(Feather)`
//!   ([`outer_ratio`]; 1 at Feather 0, about 2.5 at 100). Lightroom stores `Radius` and `CenterWeight`
//!   ([`center_weight`]) per stroke; both follow Feather through the measured tables below.
//! - One dab paints `m(r) = exp(−(r / a)^p)` at distance `r` (in `R0`) from its centre, up to `Radius`
//!   ([`profile`]): a near-hard disc at Feather 0, a Gaussian from Feather 60 up. Its peak is always 1.
//! - Dabs along a stroke add up as `1 − ∏(1 − flow · m_i)` (up to the density), and strokes add up the same
//!   way; Lightroom saves a dab every [`DAB_SPACING`] × `Radius` along the path.
//!
//! The tables are measured on Lightroom Classic's own exports of procedural charts (no Adobe file is read): the
//! values between the measured Feathers are interpolated with a monotone cubic, so the inverse ([`feather_of`])
//! is exact.

/// Feather values the tables were measured at.
const FEATHERS: [f64; 6] = [0.0, 20.0, 40.0, 60.0, 80.0, 100.0];
/// `Radius / R0` at [`FEATHERS`].
const OUTER: [f64; 6] = [1.0, 1.1, 1.414, 1.919, 2.29, 2.492];
/// `CenterWeight` at [`FEATHERS`] (as Lightroom writes it).
const CENTER_WEIGHT: [f64; 6] = [1.0, 0.501304, 0.240684, 0.105656, 0.035858, 0.0];
/// The dab profile `exp(−(r/a)^p)` fitted at [`FEATHERS`] (`a` in `R0`) to the single clicks and to the strokes
/// those clicks predict (within 0.03, 0.045 at Feather 20 and 40 where the measurements scatter that much).
const PROFILE_A: [f64; 6] = [0.945, 0.915, 0.87, 0.875, 0.855, 0.845];
const PROFILE_P: [f64; 6] = [23.8, 5.6, 2.5, 2.0, 1.9, 1.9];

/// Distance between the dabs Lightroom saves along a stroke, as a fraction of the brush's `Radius` (the outer
/// edge).
pub const DAB_SPACING: f64 = 0.30;

/// Lightroom's Size slider (1..100) → the core radius `R0` as a fraction of the photo's long side, painted at
/// Fit on the screen it was measured on. Only the feel of a Size slider: strokes store the photo-unit radius.
pub fn size_to_core(size: f64) -> f64 {
    let s = if size.is_finite() { size.clamp(0.0, 100.0) } else { 0.0 };
    0.002322 * s + 0.0000218 * s * s
}

/// [`size_to_core`]'s inverse: the Size (1..100) whose core is `core` (a fraction of the photo's long side at
/// Fit). Brings a brush size saved as a photo fraction, before Size was a size on the screen, to the slider.
pub fn core_to_size(core: f64) -> f64 {
    let (a, b) = (0.0000218, 0.002322);
    let c = if core.is_finite() { core.max(0.0) } else { 0.0 };
    ((-b + (b * b + 4.0 * a * c).sqrt()) / (2.0 * a)).clamp(1.0, 100.0)
}

/// Monotone cubic (Fritsch–Carlson) through `(xs, ys)` at `x`, clamped to the ends. `xs` increasing.
fn pchip(xs: &[f64; 6], ys: &[f64; 6], x: f64) -> f64 {
    let x = if x.is_finite() { x.clamp(xs[0], xs[5]) } else { xs[0] };
    let mut k = 0;
    while k < 4 && x > xs[k + 1] {
        k += 1;
    }
    let slope = |i: usize| (ys[i + 1] - ys[i]) / (xs[i + 1] - xs[i]);
    let tangent = |i: usize| -> f64 {
        if i == 0 {
            return slope(0);
        }
        if i == 5 {
            return slope(4);
        }
        let (a, b) = (slope(i - 1), slope(i));
        if a * b <= 0.0 { 0.0 } else { 2.0 / (1.0 / a + 1.0 / b) }
    };
    let h = xs[k + 1] - xs[k];
    let t = (x - xs[k]) / h;
    let (t2, t3) = (t * t, t * t * t);
    let (h00, h10, h01, h11) = (2.0 * t3 - 3.0 * t2 + 1.0, t3 - 2.0 * t2 + t, -2.0 * t3 + 3.0 * t2, t3 - t2);
    h00 * ys[k] + h10 * h * tangent(k) + h01 * ys[k + 1] + h11 * h * tangent(k + 1)
}

fn feather01(feather: f64) -> f64 {
    if feather.is_finite() { feather.clamp(0.0, 100.0) } else { 0.0 }
}

/// `Radius / R0` at `feather` (0..100): how far the soft halo reaches outside the core.
pub fn outer_ratio(feather: f64) -> f64 {
    pchip(&FEATHERS, &OUTER, feather01(feather)).max(1.0)
}

/// Lightroom's `CenterWeight` at `feather` (0..100): 1 at Feather 0, 0 at 100.
pub fn center_weight(feather: f64) -> f64 {
    pchip(&FEATHERS, &CENTER_WEIGHT, feather01(feather)).clamp(0.0, 1.0)
}

/// The Feather (0..100) whose [`center_weight`] is `cw`; the inverse of it (a hostile value is clamped, NaN is
/// Feather 0).
pub fn feather_of(cw: f64) -> f64 {
    let cw = if cw.is_finite() { cw.clamp(0.0, 1.0) } else { 1.0 };
    // center_weight falls monotonically: bisect
    let (mut lo, mut hi) = (0.0f64, 100.0f64);
    for _ in 0..50 {
        let mid = 0.5 * (lo + hi);
        if center_weight(mid) > cw {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    0.5 * (lo + hi)
}

/// The dab profile at `feather` (0..100): `(a, p)` of `m(r) = exp(−(r/a)^p)`, `a` in `R0`.
pub fn profile(feather: f64) -> (f64, f64) {
    let f = feather01(feather);
    // p spans 2..18: interpolated on a log scale, so the hard end doesn't dominate the soft one
    let ln_p = PROFILE_P.map(f64::ln);
    (pchip(&FEATHERS, &PROFILE_A, f).max(0.05), pchip(&FEATHERS, &ln_p, f).exp().clamp(1.0, 64.0))
}

/// One dab's coverage at distance `r` from its centre, in units of `R0`, at `feather`: `exp(−(r/a)^p)` up to
/// [`outer_ratio`], 0 beyond.
pub fn dab_value(r: f64, feather: f64) -> f64 {
    if !(r.is_finite() && r >= 0.0) || r > outer_ratio(feather) {
        return 0.0;
    }
    let (a, p) = profile(feather);
    (-(r / a).powf(p)).exp()
}

#[cfg(test)]
mod tests_core_to_size {
    use super::*;

    #[test]
    fn core_to_size_undoes_size_to_core() {
        for size in [1.0, 5.0, 12.0, 40.0, 100.0] {
            assert!((core_to_size(size_to_core(size)) - size).abs() < 1e-6, "{size}");
        }
        for bad in [f64::NAN, f64::INFINITY, -1.0, 9.0] {
            let s = core_to_size(bad);
            assert!((1.0..=100.0).contains(&s), "{bad} → {s}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The tables come back exactly at the measured Feathers, and the inverse finds the Feather again
    #[test]
    fn tables_hit_the_measured_points_and_invert() {
        for (i, f) in FEATHERS.iter().enumerate() {
            assert!((outer_ratio(*f) - OUTER[i]).abs() < 1e-9);
            assert!((center_weight(*f) - CENTER_WEIGHT[i]).abs() < 1e-9);
        }
        for f in [0.0, 5.0, 17.0, 33.3, 50.0, 64.0, 91.0, 99.0] {
            assert!((feather_of(center_weight(f)) - f).abs() < 0.05, "{f}");
        }
        assert!(outer_ratio(f64::NAN) == 1.0 && feather_of(f64::NAN) < 1e-9 && center_weight(-5.0) == 1.0);
        // monotone between the points
        let mut prev = (0.0, 2.0);
        for k in 0..=1000 {
            let f = k as f64 / 10.0;
            let (o, c) = (outer_ratio(f), center_weight(f));
            assert!(o >= prev.0 - 1e-12 && c <= prev.1 + 1e-12, "{f}");
            prev = (o, c);
        }
    }

    // The single-click table measured on Lightroom's export (r in R0 → m), within 0.03 (0.045 at
    // Feather 20 and 40, which have ~0.035 of measurement scatter)
    #[test]
    fn a_dab_matches_lightrooms_single_click() {
        let r = [0.0, 0.2, 0.4, 0.6, 0.8, 0.9, 1.0, 1.1, 1.2, 1.4, 1.6];
        let table: [(f64, &[f64]); 6] = [
            (0.0, &[1.0, 1.0, 1.0, 1.0, 0.96, 0.73, 0.0]),
            (20.0, &[1.0, 0.99, 0.95, 0.88, 0.65, 0.44, 0.19, 0.0]),
            (40.0, &[1.0, 0.96, 0.84, 0.66, 0.45, 0.36, 0.26, 0.18, 0.11, 0.0]),
            (60.0, &[1.0, 0.95, 0.81, 0.62, 0.42, 0.34, 0.27, 0.20, 0.16, 0.09, 0.04]),
            (80.0, &[1.0, 0.95, 0.81, 0.61, 0.42, 0.33, 0.26, 0.20, 0.15, 0.09, 0.05]),
            (100.0, &[1.0, 0.95, 0.81, 0.61, 0.41, 0.33, 0.25, 0.19, 0.15, 0.08, 0.05]),
        ];
        for (f, want) in table {
            let tol = if f == 20.0 || f == 40.0 { 0.045 } else { 0.03 };
            for (x, m) in r.iter().zip(want) {
                // a sample exactly on the cut (Radius = 1.0 R0 at Feather 0, 1.1 at 20) reads the outside
                let x = if (*x - outer_ratio(f)).abs() < 1e-6 { x + 1e-6 } else { *x };
                let got = dab_value(x, f);
                assert!((got - m).abs() <= tol, "Feather {f}, r {x}: {got:.3} vs {m}");
            }
        }
    }
}
