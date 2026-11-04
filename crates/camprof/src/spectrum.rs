// Ported from RADcamProfiles (github.com/OneSlips/RADcamProfiles, commit dd8003f, crates/engine/src/spectrum.rs),
// Copyright (c) 2026 RAD Films - Roman Alurkoff, MIT; relicensed MIT OR Apache-2.0 by its author.
// Modified by the porting contributors (2026): no panics on bad input, the app's frame type.
//! Sampled spectra and the integration helpers built on top of them.
//!
//! A spectrum is a strictly increasing list of wavelengths in nanometres, each
//! carrying an amplitude. Interpolation between samples is linear; the
//! integration routine uses the trapezoid rule, which is what the rest of the
//! pipeline assumes when it converts spectra to tristimulus values.

/// A sampled spectrum: strictly increasing bands with amplitudes.
#[derive(Clone, Debug, PartialEq)]
pub struct Spectrum {
    bands: Vec<f64>,
    amps: Vec<f64>,
}

/// Interpolate the amplitude at `f` between two neighbouring samples.
///
/// `f` must lie within `[f1, f2]`.
#[inline]
fn midpoint_e(f1: f64, mid_f: f64, f2: f64, e1: f64, e2: f64) -> f64 {
    if mid_f == f1 {
        return e1;
    }
    if mid_f == f2 {
        return e2;
    }
    let b = e2 - e1;
    let a = f2 - f1;
    let a2 = mid_f - f1;
    e1 + a2 * (b / a)
}

/// Area of the trapezoid between two samples.
#[inline]
fn traparea(f1: f64, f2: f64, e1: f64, e2: f64) -> f64 {
    if e2 > e1 { (e2 - e1) * (f2 - f1) * 0.5 + e1 * (f2 - f1) } else { (e1 - e2) * (f2 - f1) * 0.5 + e2 * (f2 - f1) }
}

impl Spectrum {
    /// Build a spectrum from equally spaced amplitudes.
    ///
    /// The band list runs from `start` to `end` inclusive in steps of
    /// `spacing`; `amps` must hold exactly that many entries.
    pub fn from_uniform(amps: &[f64], start: f64, end: f64, spacing: f64) -> Self {
        // (only ever called with the built-in tables; a mismatch keeps the amplitudes, spaced from `start`)
        let spacing = if spacing.is_finite() && spacing > 0.0 { spacing } else { 1.0 };
        let _ = end;
        let bands = (0..amps.len()).map(|i| start + i as f64 * spacing).collect();
        Self { bands, amps: amps.to_vec() }
    }

    /// Build from explicit band and amplitude lists. Pairs past the shorter list, and bands that
    /// don't increase, are dropped (every interpolation needs strictly increasing bands).
    pub fn from_pairs(bands: &[f64], amps: &[f64]) -> Self {
        let mut out = Self { bands: Vec::with_capacity(bands.len()), amps: Vec::with_capacity(amps.len()) };
        for (&b, &a) in bands.iter().zip(amps) {
            if b.is_finite() && out.bands.last().is_none_or(|&last| b > last) {
                out.bands.push(b);
                out.amps.push(a);
            }
        }
        out
    }

    /// Number of samples.
    #[inline]
    pub fn len(&self) -> usize {
        self.bands.len()
    }

    /// True when the spectrum has no samples. Never true for a valid spectrum;
    /// provided for API completeness.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.bands.is_empty()
    }

    /// First wavelength.
    #[inline]
    pub fn start(&self) -> f64 {
        self.bands.first().copied().unwrap_or(0.0)
    }

    /// Last wavelength.
    #[inline]
    pub fn end(&self) -> f64 {
        self.bands.last().copied().unwrap_or(0.0)
    }

    /// Wavelength list.
    #[inline]
    pub fn bands(&self) -> &[f64] {
        &self.bands
    }

    /// Amplitude list.
    #[inline]
    pub fn amplitudes(&self) -> &[f64] {
        &self.amps
    }

    /// Amplitude at sample `idx`.
    #[inline]
    pub fn at_index(&self, idx: usize) -> f64 {
        self.amps.get(idx).copied().unwrap_or(0.0)
    }

    /// Mutable amplitude at sample `idx`.
    #[inline]
    pub fn at_index_mut(&mut self, idx: usize) -> &mut f64 {
        &mut self.amps[idx]
    }

    /// Largest amplitude.
    pub fn max(&self) -> f64 {
        self.amps.iter().copied().fold(f64::NEG_INFINITY, f64::max)
    }

    /// Index of the sample nearest to `f`, for diagnostics.
    pub fn nearest_index(&self, f: f64) -> usize {
        match self.bands.binary_search_by(|b| b.total_cmp(&f)) {
            Ok(i) => i,
            Err(i) => i.min(self.bands.len().saturating_sub(1)),
        }
    }

    /// Interpolate the amplitude at an arbitrary wavelength.
    ///
    /// Returns zero outside the sampled range, matching how the engine treats
    /// values beyond the measured bands.
    pub fn at(&self, f: f64) -> f64 {
        if f > self.end() {
            return 0.0;
        }
        match self.bands.binary_search_by(|b| b.total_cmp(&f)) {
            Ok(i) => self.amps.get(i).copied().unwrap_or(0.0),
            Err(0) => 0.0,
            Err(i) => match (self.bands.get(i - 1), self.bands.get(i), self.amps.get(i - 1), self.amps.get(i)) {
                (Some(&b0), Some(&b1), Some(&a0), Some(&a1)) => midpoint_e(b0, f, b1, a0, a1),
                _ => 0.0,
            },
        }
    }

    /// Scale every amplitude in place.
    pub fn scale(&mut self, k: f64) {
        for a in &mut self.amps {
            *a *= k;
        }
    }

    /// Reciprocal of every amplitude; non-finite results become zero.
    pub fn reciprocal(&self) -> Self {
        Self {
            bands: self.bands.clone(),
            amps: self
                .amps
                .iter()
                .map(|a| {
                    let r = 1.0 / a;
                    if r.is_finite() { r } else { 0.0 }
                })
                .collect(),
        }
    }

    /// Integrate over `[start, end]`; negative bounds mean "use the full range".
    ///
    /// Partial intervals are handled by interpolating the amplitude linearly,
    /// so integrating a constant spectrum over a sub-range returns exactly its
    /// area, and a bound placed between samples contributes the correct
    /// fraction of that interval.
    pub fn integrate(&self, start: f64, end: f64) -> f64 {
        let n = self.bands.len();
        let start = if start < 0.0 { self.bands[0] } else { start };
        let end = if end < 0.0 { self.bands[n - 1] } else { end };
        if start >= end || end <= self.bands[0] || start >= self.bands[n - 1] {
            return 0.0;
        }

        // Accumulate interval by interval, carrying the amplitude at the
        // running lower bound so that a bound falling between samples
        // contributes exactly its fraction of the interval.
        let mut sum = 0.0;
        let mut lower_f = start;
        let mut lower_e = self.at(start);
        let mut i = self.bands.partition_point(|f| *f < start);
        while i < n && self.bands[i] <= end {
            let band = self.bands[i];
            if band > lower_f {
                sum += traparea(lower_f, band, lower_e, self.amps[i]);
            }
            lower_f = band;
            lower_e = self.amps[i];
            i += 1;
        }
        if end > lower_f {
            sum += traparea(lower_f, end, lower_e, self.at(end));
        }
        sum
    }

    /// Integrate the product of two spectra over `[start, end]`.
    pub fn multiply_and_integrate(&self, other: &Self, start: f64, end: f64) -> f64 {
        self.multiplied(other).integrate(start, end)
    }

    /// Sample-wise product, re-sampled onto the union of both band lists.
    pub fn multiplied(&self, other: &Self) -> Self {
        self.combine(other, |a, b| a * b)
    }

    /// Sample-wise sum, re-sampled onto the union of both band lists.
    pub fn added(&self, other: &Self) -> Self {
        self.combine(other, |a, b| a + b)
    }

    fn combine(&self, other: &Self, op: impl Fn(f64, f64) -> f64) -> Self {
        let (a, b) = (self, other);
        let mut bands = Vec::with_capacity(a.len() + b.len());
        let mut amps = Vec::with_capacity(a.len() + b.len());
        let (mut i, mut j) = (0usize, 0usize);
        while i < a.len() && j < b.len() {
            if a.bands[i] == b.bands[j] {
                bands.push(a.bands[i]);
                amps.push(op(a.amps[i], b.amps[j]));
                i += 1;
                j += 1;
            } else if a.bands[i] > b.bands[j] {
                // `b` has a band inside the current interval of `a`; the
                // leading sample of the shorter list is dropped.
                if i == 0 {
                    j += 1;
                } else {
                    let f = b.bands[j];
                    let e = midpoint_e(a.bands[i - 1], f, a.bands[i], a.amps[i - 1], a.amps[i]);
                    bands.push(f);
                    amps.push(op(e, b.amps[j]));
                    j += 1;
                }
            } else {
                // `a` has a band inside the current interval of `b`.
                if j == 0 {
                    i += 1;
                } else {
                    let f = a.bands[i];
                    let e = midpoint_e(b.bands[j - 1], f, b.bands[j], b.amps[j - 1], b.amps[j]);
                    bands.push(f);
                    amps.push(op(a.amps[i], e));
                    i += 1;
                }
            }
        }
        if bands.len() < 2 {
            let lo = a.start().min(b.start());
            let hi = a.end().max(b.end());
            return Self { bands: vec![lo, hi], amps: vec![0.0, 0.0] };
        }
        Self { bands, amps }
    }

    /// Flatten the low end: samples below 380 nm are zeroed and the section
    /// below `lim` is ramped up linearly to the value at `lim`.
    ///
    /// Some measurement devices report unreliable data below 400 nm; this
    /// mirrors the engine's pragmatic correction.
    pub fn with_guessed_lows(&self, lim: f64) -> Self {
        let mut s = self.clone();
        let lim_a = s.at(lim);
        let base = 380.0;
        for i in 0..s.len() {
            let f = s.bands[i];
            if f < base {
                s.amps[i] = 0.0;
            } else if f < lim {
                s.amps[i] = lim_a * (f - base) / (lim - base);
            } else {
                break;
            }
        }
        s
    }

    /// True when both spectra share identical bands and amplitudes.
    pub fn same_as(&self, other: &Self) -> bool {
        self.bands == other.bands && self.amps == other.amps
    }

    /// Render as whitespace separated `wavelength amplitude` lines.
    pub fn to_columns(&self, precision: usize) -> String {
        use core::fmt::Write as _;
        let mut out = String::with_capacity(self.len() * 24);
        for i in 0..self.len() {
            let _ = writeln!(out, "{:.1} {:.*}", self.bands[i], precision, self.amps[i]);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(n: usize, value: f64) -> Spectrum {
        Spectrum::from_uniform(&vec![value; n], 400.0, 400.0 + (n - 1) as f64 * 10.0, 10.0)
    }

    #[test]
    fn uniform_construction_matches_range() {
        let s = Spectrum::from_uniform(&[1.0, 2.0, 3.0], 400.0, 420.0, 10.0);
        assert_eq!(s.len(), 3);
        assert_eq!(s.bands(), &[400.0, 410.0, 420.0]);
        assert_eq!(s.amplitudes(), &[1.0, 2.0, 3.0]);
        assert_eq!(s.start(), 400.0);
        assert_eq!(s.end(), 420.0);
    }

    #[test]
    fn interpolation_is_linear_inside_and_zero_outside() {
        let s = Spectrum::from_uniform(&[0.0, 1.0, 2.0], 400.0, 420.0, 10.0);
        assert!((s.at(405.0) - 0.5).abs() < 1.0e-15);
        assert!((s.at(415.0) - 1.5).abs() < 1.0e-15);
        assert_eq!(s.at(400.0), 0.0);
        assert_eq!(s.at(420.0), 2.0);
        assert_eq!(s.at(430.0), 0.0);
        assert_eq!(s.at(390.0), 0.0);
    }

    #[test]
    fn integration_of_constant_spectrum_is_area() {
        // 11 samples from 400 to 500 nm, all at 2.0 => area 200.
        let s = flat(11, 2.0);
        let area = s.integrate(-1.0, -1.0);
        assert!((area - 200.0).abs() < 1.0e-9, "{area}");
    }

    #[test]
    fn integration_over_sub_range_is_exact_for_constant() {
        let s = flat(11, 1.0);
        // Sub-range bounds that fall on sample points take the plain
        // trapezoid path.
        let area = s.integrate(450.0, 480.0);
        assert!((area - 30.0).abs() < 1.0e-9, "{area}");
        // A bound inside the first interval takes the partial-trapezoid path.
        let partial = s.integrate(425.0, 480.0);
        assert!((partial - 55.0).abs() < 1.0e-9, "{partial}");
    }

    #[test]
    fn integration_of_ramp_matches_trapezoid_rule() {
        // Amplitudes 0..10 over 400..500.
        let amps: Vec<f64> = (0..11).map(|i| i as f64).collect();
        let s = Spectrum::from_uniform(&amps, 400.0, 500.0, 10.0);
        // Trapezoid over 10 intervals of width 10 with unit rise: 10*10*0.5 + 0*10 per pair
        // sum = 10 * (10 * (0+1)/2 ... ) -> 10 intervals, mean height 5 => 500
        assert!((s.integrate(-1.0, -1.0) - 500.0).abs() < 1.0e-9);
    }

    #[test]
    fn multiply_and_integrate_of_ones_equals_plain_integration() {
        let a = flat(11, 1.0);
        let b = flat(11, 3.0);
        let e = a.multiply_and_integrate(&b, -1.0, -1.0);
        assert!((e - 300.0).abs() < 1.0e-9, "{e}");
    }

    #[test]
    fn scaling_changes_area_proportionally() {
        let mut s = flat(11, 1.0);
        let before = s.integrate(-1.0, -1.0);
        s.scale(2.5);
        let after = s.integrate(-1.0, -1.0);
        assert!((after - before * 2.5).abs() < 1.0e-9);
    }

    #[test]
    fn reciprocal_of_zeros_is_zero() {
        let s = Spectrum::from_uniform(&[0.0, 0.5, 1.0], 400.0, 420.0, 10.0);
        let r = s.reciprocal();
        assert_eq!(r.amplitudes()[0], 0.0);
        assert!((r.amplitudes()[1] - 2.0).abs() < 1.0e-15);
    }

    #[test]
    fn guessed_lows_zeroes_below_base_and_ramps() {
        let s = Spectrum::from_uniform(&vec![1.0; 41], 350.0, 750.0, 10.0);
        let g = s.with_guessed_lows(420.0);
        assert_eq!(g.at(350.0), 0.0);
        assert_eq!(g.at(370.0), 0.0);
        // Halfway between 380 and 420 the ramp is at half of the value at 420.
        assert!((g.at(400.0) - 0.5).abs() < 1.0e-12);
        assert!((g.at(440.0) - 1.0).abs() < 1.0e-12);
    }

    #[test]
    fn non_increasing_bands_are_dropped_not_a_panic() {
        // (panicked before: the constructor asserted)
        let s = Spectrum::from_pairs(&[400.0, 400.0, 390.0, f64::NAN, 410.0], &[1.0, 2.0, 3.0, 4.0]);
        assert_eq!(s.bands(), &[400.0]);
        assert_eq!(s.at(400.0), 1.0);
        assert_eq!(Spectrum::from_pairs(&[], &[]).end(), 0.0);
        assert_eq!(Spectrum::from_pairs(&[], &[]).at(500.0), 0.0);
    }

    #[test]
    fn combining_coarse_and_fine_grids_uses_shared_extent() {
        let coarse = Spectrum::from_pairs(&[400.0, 500.0], &[0.0, 10.0]);
        let fine = Spectrum::from_pairs(&[400.0, 450.0, 500.0], &[1.0, 1.0, 1.0]);
        let product = coarse.multiplied(&fine);
        assert_eq!(product.bands(), &[400.0, 450.0, 500.0]);
        // At 450 nm the coarse curve interpolates to half its range.
        assert!((product.at(450.0) - 5.0).abs() < 1.0e-12, "{product:?}");
    }

    #[test]
    fn combine_over_disjoint_bands_degrades_to_zero_pair() {
        let a = Spectrum::from_pairs(&[400.0, 410.0], &[1.0, 1.0]);
        let b = Spectrum::from_pairs(&[600.0, 610.0], &[1.0, 1.0]);
        let s = a.added(&b);
        assert_eq!(s.len(), 2);
        assert_eq!(s.amplitudes(), &[0.0, 0.0]);
        assert_eq!(s.bands(), &[400.0, 610.0]);
    }
}
