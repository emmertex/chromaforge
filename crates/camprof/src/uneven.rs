// Ported from RADcamProfiles (github.com/OneSlips/RADcamProfiles, commit 4cb3c45, crates/engine/src/reference/uneven.rs),
// Copyright (c) 2026 RAD Films - Roman Alurkoff, MIT; relicensed MIT OR Apache-2.0 by its author.
// Modified by the porting contributors (2026): the app's half-resolution RGB frame.
//! Uneven light on a chart: falloff across the card and glare off its surface.
//!
//! A chart lit by a lamp close to one side is brighter at that side. Across a
//! card that spans 20 % in light, the patches at the dim end read darker than
//! they are, and a fit that reproduces the chart's lightness learns the falloff
//! as the camera's tone. The card's own surface between the patches is one
//! material, so its brightness maps the light: [`falloff`] fits a plane to it,
//! and [`flatten`] divides it out of every patch.
//!
//! A lamp at a shallow angle also reflects off the patches' surface. That adds
//! the same amount of the lamp's colour to every patch, so dark patches read
//! lighter and every patch less saturated. It is not lens flare: the darkest
//! parts of the scene stay dark. The chart's neutral patches show it: their
//! camera values should be proportional to their reference luminance, and an
//! offset common to all of them is the glare. [`glare`] measures that offset per
//! channel and [`remove_glare`] subtracts it.
//!
//! Both are measured from the frame and the chart's own reference values; a
//! chart lit evenly, without glare, comes out with a flat plane and no offset.

use crate::frame::RawFrame;
use crate::linalg::Vec3;
use crate::reference::{ChartCorners, ChartMeasurement, Layout, lerp};

/// The light over the chart as a gain `c0 + cu * u + cv * v`, where `u` runs
/// across the frame's grid and `v` down it, both from 0 to 1. Normalised so the
/// mean gain over the patch centres is 1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Falloff {
    /// `[c0, cu, cv]`.
    pub coefficients: [f64; 3],
    /// How many points of the card's surface the plane was fitted to.
    pub samples: usize,
    /// The largest gain over the grid divided by the smallest.
    pub range: f64,
}

impl Falloff {
    /// The gain at grid position `(u, v)`.
    pub fn gain(&self, u: f64, v: f64) -> f64 {
        let [c0, cu, cv] = self.coefficients;
        c0 + cu * u + cv * v
    }
}

/// The frame's grid for a chart lying `turns` quarter turns: an odd number
/// exchanges rows and columns, as in [`super::measure_rotated`].
pub fn frame_layout(chart_layout: Layout, turns: u8) -> Layout {
    if turns % 2 == 1 { Layout { columns: chart_layout.rows, rows: chart_layout.columns } } else { chart_layout }
}

fn point(corners: &ChartCorners, u: f64, v: f64) -> [f64; 2] {
    let top = lerp(corners.top_left, corners.top_right, u);
    let bottom = lerp(corners.bottom_left, corners.bottom_right, u);
    lerp(top, bottom, v)
}

/// Fit the light's falloff from the card's surface between the patches.
///
/// `layout` is the frame's grid ([`frame_layout`]). The surface is sampled in
/// the middle of every gap between two neighbouring patches, in a small box
/// that stays clear of the patches. `None` when there are too few gaps, the
/// samples are empty, or the fit is implausible (a gain at or below 0.2 of the
/// mean anywhere on the grid).
pub fn falloff(frame: &RawFrame, corners: &ChartCorners, layout: Layout) -> Option<Falloff> {
    let columns = layout.columns.max(1);
    let rows = layout.rows.max(1);
    let cell = corners.patch_extent(layout, 1.0);
    // A tenth of a cell: inside the gap between patches on every chart here.
    let half = [(cell[0] * 0.1).max(1.0), (cell[1] * 0.1).max(1.0)];
    let mut points: Vec<(f64, f64)> = Vec::new();
    for row in 0..rows {
        for column in 1..columns {
            points.push((column as f64 / columns as f64, (row as f64 + 0.5) / rows as f64));
        }
    }
    for row in 1..rows {
        for column in 0..columns {
            points.push(((column as f64 + 0.5) / columns as f64, row as f64 / rows as f64));
        }
    }
    let mut samples: Vec<(f64, f64, f64)> = Vec::with_capacity(points.len());
    for (u, v) in points {
        let [x, y] = point(corners, u, v);
        if !(x.is_finite() && y.is_finite()) {
            continue;
        }
        let x0 = (x - half[0]).floor().max(0.0) as usize;
        let y0 = (y - half[1]).floor().max(0.0) as usize;
        let x1 = (x + half[0]).ceil().max(0.0) as usize;
        let y1 = (y + half[1]).ceil().max(0.0) as usize;
        let (values, _, _) = frame.block_means(1, [x0, y0, x1.max(x0 + 1), y1.max(y0 + 1)]);
        if values.is_empty() {
            continue;
        }
        let sum: f64 = values.iter().map(|p| p.v[0] + p.v[1] + p.v[2]).sum::<f64>() / values.len() as f64;
        if sum.is_finite() && sum > 0.0 {
            samples.push((u, v, sum));
        }
    }
    if samples.len() < 6 {
        return None;
    }
    let mut c = fit_plane(&samples)?;
    // One pass of outlier rejection: a gap with a speck of dust or a patch
    // edge in it should not tilt the plane.
    let residuals: Vec<f64> = samples.iter().map(|(u, v, s)| (s - (c[0] + c[1] * u + c[2] * v)).abs()).collect();
    let mad = median(&residuals)?;
    let limit = 3.0 * 1.4826 * mad + 1e-12;
    let kept: Vec<(f64, f64, f64)> = samples.iter().zip(&residuals).filter(|(_, r)| **r <= limit).map(|(s, _)| *s).collect();
    if kept.len() >= 6 {
        c = fit_plane(&kept)?;
    }
    // Normalise to the mean over the patch centres.
    let mut mean = 0.0;
    for row in 0..rows {
        for column in 0..columns {
            let u = (column as f64 + 0.5) / columns as f64;
            let v = (row as f64 + 0.5) / rows as f64;
            mean += c[0] + c[1] * u + c[2] * v;
        }
    }
    mean /= (rows * columns) as f64;
    if !(mean.is_finite() && mean > 0.0) {
        return None;
    }
    let coefficients = [c[0] / mean, c[1] / mean, c[2] / mean];
    let f = Falloff { coefficients, samples: kept.len().max(6), range: 1.0 };
    let corner_gains = [f.gain(0.0, 0.0), f.gain(1.0, 0.0), f.gain(0.0, 1.0), f.gain(1.0, 1.0)];
    let lo = corner_gains.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = corner_gains.iter().copied().fold(0.0f64, f64::max);
    if !(lo.is_finite() && lo > 0.2 && hi.is_finite()) {
        return None;
    }
    Some(Falloff { range: hi / lo, ..f })
}

/// Divide the falloff out of every patch. `layout` is the frame's grid the
/// measurement was taken on, so patch `index` sits in frame cell `index`.
pub fn flatten(measurement: &mut ChartMeasurement, falloff: &Falloff, layout: Layout) {
    let columns = layout.columns.max(1);
    let rows = layout.rows.max(1);
    for (index, rgb) in measurement.camera_rgb.iter_mut().enumerate() {
        let u = ((index % columns) as f64 + 0.5) / columns as f64;
        let v = ((index / columns) as f64 + 0.5) / rows as f64;
        let gain = falloff.gain(u, v);
        if gain.is_finite() && gain > 0.0 {
            *rgb = Vec3::new(rgb.v[0] / gain, rgb.v[1] / gain, rgb.v[2] / gain);
        }
    }
}

/// The glare: per channel, the offset common to all neutral patches.
///
/// Each channel of the unclipped neutral patches (|a*|, |b*| under 4, below
/// 95 % of the white level in every channel) is fitted as `slope * Y + offset`
/// against the patches' reference luminance. `None` when fewer than four such
/// patches remain. Negative offsets are returned as zero: a dark patch reading
/// darker than its reference is noise or a fit artefact, not glare.
pub fn glare(measurement: &ChartMeasurement) -> Option<Vec3> {
    let mut points: Vec<(f64, [f64; 3])> = Vec::new();
    for (index, rgb) in measurement.camera_rgb.iter().enumerate() {
        let lab = measurement.reference_lab(index);
        if lab.v[1].abs() >= 4.0 || lab.v[2].abs() >= 4.0 {
            continue;
        }
        if rgb.v.iter().any(|c| !c.is_finite() || *c >= 0.95 || *c <= 0.0) {
            continue;
        }
        let y = ((lab.v[0] + 16.0) / 116.0).powi(3);
        points.push((y, rgb.v));
    }
    if points.len() < 4 {
        return None;
    }
    let n = points.len() as f64;
    let sy: f64 = points.iter().map(|p| p.0).sum();
    let syy: f64 = points.iter().map(|p| p.0 * p.0).sum();
    let det = n * syy - sy * sy;
    if !(det.is_finite() && det.abs() > 1e-12) {
        return None;
    }
    let mut offset = [0.0f64; 3];
    for (c, o) in offset.iter_mut().enumerate() {
        let sc: f64 = points.iter().map(|p| p.1[c]).sum();
        let syc: f64 = points.iter().map(|p| p.0 * p.1[c]).sum();
        let value = (syy * sc - sy * syc) / det;
        *o = if value.is_finite() { value.max(0.0) } else { 0.0 };
    }
    Some(Vec3::new(offset[0], offset[1], offset[2]))
}

/// Subtract a glare offset from every patch, keeping each channel above zero.
pub fn remove_glare(measurement: &mut ChartMeasurement, offset: Vec3) {
    for rgb in measurement.camera_rgb.iter_mut() {
        let floor = |value: f64, o: f64| (value - o).max(value * 0.01);
        *rgb = Vec3::new(floor(rgb.v[0], offset.v[0]), floor(rgb.v[1], offset.v[1]), floor(rgb.v[2], offset.v[2]));
    }
}

/// How far the neutral patches are from proportional to their reference
/// luminance: the largest camera-green-over-Y ratio divided by the smallest,
/// over unclipped neutrals. 1.0 is a perfectly linear ramp. Use it to check a
/// correction: [`falloff`] and [`glare`] assume a plane of light and an even
/// glare, and a lamp whose reflection moves across the card breaks both, so a
/// correction that does not bring this closer to 1 should be discarded.
pub fn ramp_spread(measurement: &ChartMeasurement) -> Option<f64> {
    let mut lo = f64::INFINITY;
    let mut hi = 0.0f64;
    let mut n = 0;
    for (index, rgb) in measurement.camera_rgb.iter().enumerate() {
        let lab = measurement.reference_lab(index);
        if lab.v[1].abs() >= 4.0 || lab.v[2].abs() >= 4.0 {
            continue;
        }
        if rgb.v.iter().any(|c| !c.is_finite() || *c >= 0.95 || *c <= 0.0) {
            continue;
        }
        let y = ((lab.v[0] + 16.0) / 116.0).powi(3);
        let r = rgb.v[1] / y;
        lo = lo.min(r);
        hi = hi.max(r);
        n += 1;
    }
    (n >= 4 && lo > 0.0 && hi.is_finite()).then(|| hi / lo)
}

/// A corrected ramp must end within this of linear (see [`ramp_spread`]) for
/// [`even_out`] to keep the correction.
pub const KEEP_SPREAD: f64 = 1.5;

/// What [`even_out`] measured and whether it kept the correction.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EvenOut {
    /// The light's falloff over the card, when the card could be measured.
    pub falloff: Option<Falloff>,
    /// The glare offset found on the neutral patches, as a fraction of the white level.
    pub glare: Option<Vec3>,
    /// [`ramp_spread`] before and after the correction.
    pub before: Option<f64>,
    /// See `before`.
    pub after: Option<f64>,
    /// Whether the measurement now holds the corrected values.
    pub kept: bool,
}

impl EvenOut {
    /// One line per step, for a log.
    pub fn describe(&self) -> Vec<String> {
        let mut lines = Vec::new();
        lines.push(match &self.falloff {
            Some(f) => format!("light over the chart varies {:.0}% (from {} points of the card)", (f.range - 1.0) * 100.0, f.samples),
            None => "falloff: the card between the patches could not be measured".into(),
        });
        lines.push(match &self.glare {
            Some(g) => format!("glare: {:.4} {:.4} {:.4} of the white level", g.v[0], g.v[1], g.v[2]),
            None => "glare: too few unclipped neutral patches".into(),
        });
        let show = |v: Option<f64>| v.map_or("?".into(), |v| format!("{v:.2}"));
        lines.push(if self.kept {
            format!("neutral ramp {} -> {} of linear: correction kept", show(self.before), show(self.after))
        } else {
            format!("neutral ramp {} -> {}: the light is not a plane with even glare here; correction discarded", show(self.before), show(self.after))
        });
        lines
    }
}

/// Correct uneven light on the chart: divide out the falloff measured on the
/// card between the patches ([`falloff`], [`flatten`]), then subtract the
/// glare the neutral patches show ([`glare`], [`remove_glare`]). The result
/// replaces `measurement` only when it brings the neutral ramp closer to
/// linear and within [`KEEP_SPREAD`] of it; a light that is not a plane, or
/// glare that moves across the card, leaves the measurement as it was.
pub fn even_out(frame: &RawFrame, corners: &ChartCorners, layout: Layout, measurement: &mut ChartMeasurement) -> EvenOut {
    let before = ramp_spread(measurement);
    let mut corrected = measurement.clone();
    let falloff = falloff(frame, corners, layout);
    if let Some(f) = &falloff {
        flatten(&mut corrected, f, layout);
    }
    let glare = glare(&corrected);
    if let Some(g) = glare {
        remove_glare(&mut corrected, g);
    }
    let after = ramp_spread(&corrected);
    let kept = matches!((before, after), (Some(b), Some(a)) if a < b && a <= KEEP_SPREAD);
    if kept {
        *measurement = corrected;
    }
    EvenOut { falloff, glare, before, after, kept }
}

fn fit_plane(samples: &[(f64, f64, f64)]) -> Option<[f64; 3]> {
    // Normal equations for s = c0 + cu u + cv v.
    let mut a = [[0.0f64; 3]; 3];
    let mut b = [0.0f64; 3];
    for (u, v, s) in samples {
        let x = [1.0, *u, *v];
        for i in 0..3 {
            for j in 0..3 {
                a[i][j] += x[i] * x[j];
            }
            b[i] += x[i] * s;
        }
    }
    let det = det3(&a);
    if !(det.is_finite() && det.abs() > 1e-12) {
        return None;
    }
    let mut out = [0.0f64; 3];
    for (k, o) in out.iter_mut().enumerate() {
        let mut m = a;
        for (row, value) in m.iter_mut().zip(b) {
            row[k] = value;
        }
        *o = det3(&m) / det;
    }
    out.iter().all(|c| c.is_finite()).then_some(out)
}

fn det3(m: &[[f64; 3]; 3]) -> f64 {
    m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
}

fn median(values: &[f64]) -> Option<f64> {
    let mut v: Vec<f64> = values.iter().copied().filter(|x| x.is_finite()).collect();
    if v.is_empty() {
        return None;
    }
    v.sort_by(|a, b| a.total_cmp(b));
    v.get(v.len() / 2).copied()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color;
    use crate::reference::{self, Chart};

    /// A chart painted under a light that falls off from left to right, with
    /// glare: patch = (0.45 * Y + glare) * gain, card surface = 0.08 * gain.
    fn lit_frame(corners: &ChartCorners, chart: &Chart, falloff: f64, glare: f64) -> RawFrame {
        let max_x = corners.points().iter().map(|p| p[0]).fold(0.0f64, f64::max);
        let max_y = corners.points().iter().map(|p| p[1]).fold(0.0f64, f64::max);
        let (width, height) = ((max_x + 40.0).ceil() as usize, (max_y + 40.0).ceil() as usize);
        let left = corners.top_left[0];
        let span = corners.top_right[0] - left;
        let gain = |x: usize| 1.0 - falloff * ((x as f64 - left) / span).clamp(0.0, 1.0);
        let mut data = vec![[0.0f32; 3]; width * height];
        for y in 0..height {
            for x in 0..width {
                data[y * width + x] = [(0.08 * gain(x)) as f32; 3];
            }
        }
        let layout = chart.layout;
        let extent = corners.patch_extent(layout, 0.8);
        for index in 0..layout.patches() {
            let lab = chart.patches[index].lab;
            let y_ref = color::lab_to_xyz(Vec3::new(lab[0], lab[1], lab[2]), Vec3::new(0.96422, 1.0, 0.82521)).v[1];
            let centre = corners.patch_centre(layout, index);
            let x0 = (centre[0] - extent[0]) as usize;
            let x1 = (centre[0] + extent[0]) as usize;
            let y0 = (centre[1] - extent[1]) as usize;
            let y1 = (centre[1] + extent[1]) as usize;
            for y in y0..y1 {
                for x in x0..x1 {
                    data[y * width + x] = [((0.45 * y_ref + glare) * gain(x)) as f32; 3];
                }
            }
        }
        RawFrame::from_rgb(width, height, data).expect("the painted frame is well formed")
    }

    fn blank() -> RawFrame {
        RawFrame::from_rgb(400, 450, vec![[0.0f32; 3]; 400 * 450]).expect("well formed")
    }

    fn ramp_spread(m: &ChartMeasurement) -> f64 {
        // Camera green over reference Y across the neutral patches: 1.0 when linear.
        let ratios: Vec<f64> = (0..m.ids.len())
            .filter(|i| {
                let lab = m.reference_lab(*i);
                lab.v[1].abs() < 4.0 && lab.v[2].abs() < 4.0
            })
            .map(|i| m.camera_rgb[i].v[1] / ((m.reference_lab(i).v[0] + 16.0) / 116.0).powi(3))
            .collect();
        let hi = ratios.iter().copied().fold(0.0f64, f64::max);
        let lo = ratios.iter().copied().fold(f64::INFINITY, f64::min);
        hi / lo
    }

    #[test]
    fn falloff_and_glare_are_measured_and_removed() {
        let chart = &reference::SPYDERCHECKR_24;
        let corners = ChartCorners::from_rect(60.0, 40.0, 240.0, 360.0);
        let frame = lit_frame(&corners, chart, 0.2, 0.015);
        let mut m = reference::measure_rotated(&frame, chart, &corners, 0.56, 0);
        assert!(ramp_spread(&m) > 1.5, "the painted chart should be uneven");
        let layout = frame_layout(chart.layout, 0);
        let f = falloff(&frame, &corners, layout).expect("the card's surface is sampled");
        assert!((f.range - 1.0 / 0.8).abs() < 0.05, "range {}", f.range);
        flatten(&mut m, &f, layout);
        let g = glare(&m).expect("the neutral patches give the glare");
        // Glare is 0.015 of white per channel, scaled by the mean gain (about 0.9).
        for c in 0..3 {
            assert!((g.v[c] - 0.015 * 0.9).abs() < 0.003, "glare {:?}", g.v);
        }
        remove_glare(&mut m, g);
        assert!(ramp_spread(&m) < 1.04, "after correction the ramp is linear, spread {}", ramp_spread(&m));
    }

    #[test]
    fn the_ramp_spread_shows_whether_a_correction_helped() {
        let chart = &reference::SPYDERCHECKR_24;
        let corners = ChartCorners::from_rect(60.0, 40.0, 240.0, 360.0);
        let even = lit_frame(&corners, chart, 0.0, 0.0);
        let m = reference::measure_rotated(&even, chart, &corners, 0.56, 0);
        let s = super::ramp_spread(&m).expect("neutrals");
        assert!(s < 1.03, "an even chart is linear, got {s}");
        let uneven = lit_frame(&corners, chart, 0.2, 0.015);
        let m = reference::measure_rotated(&uneven, chart, &corners, 0.56, 0);
        assert!(super::ramp_spread(&m).expect("neutrals") > 1.5);
    }

    #[test]
    fn even_out_keeps_a_correction_that_helps_and_drops_one_that_does_not() {
        let chart = &reference::SPYDERCHECKR_24;
        let corners = ChartCorners::from_rect(60.0, 40.0, 240.0, 360.0);
        let layout = frame_layout(chart.layout, 0);
        let uneven = lit_frame(&corners, chart, 0.2, 0.015);
        let mut m = reference::measure_rotated(&uneven, chart, &corners, 0.56, 0);
        let out = even_out(&uneven, &corners, layout, &mut m);
        assert!(out.kept, "{:?}", out.describe());
        assert!(super::ramp_spread(&m).expect("neutrals") < 1.1);
        // A frame without a chart: nothing measurable, the measurement stays.
        let blank = blank();
        let mut m = reference::measure_rotated(&blank, chart, &corners, 0.56, 0);
        let copy = m.clone();
        let out = even_out(&blank, &corners, layout, &mut m);
        assert!(!out.kept);
        assert_eq!(m.camera_rgb.len(), copy.camera_rgb.len());
        assert_eq!(out.describe().len(), 3);
    }

    #[test]
    fn an_even_chart_without_glare_is_left_alone() {
        let chart = &reference::SPYDERCHECKR_24;
        let corners = ChartCorners::from_rect(60.0, 40.0, 240.0, 360.0);
        let frame = lit_frame(&corners, chart, 0.0, 0.0);
        let m = reference::measure_rotated(&frame, chart, &corners, 0.56, 0);
        let f = falloff(&frame, &corners, frame_layout(chart.layout, 0)).expect("sampled");
        assert!((f.range - 1.0).abs() < 0.01, "range {}", f.range);
        let g = glare(&m).expect("neutrals");
        assert!(g.v.iter().all(|c| *c < 0.002), "glare {:?}", g.v);
    }

    #[test]
    fn a_frame_without_a_chart_gives_no_falloff_rather_than_a_panic() {
        let chart = &reference::SPYDERCHECKR_24;
        let corners = ChartCorners::from_rect(60.0, 40.0, 240.0, 360.0);
        let frame = blank();
        assert!(falloff(&frame, &corners, chart.layout).is_none());
        let m = reference::measure_rotated(&frame, chart, &corners, 0.56, 0);
        assert!(glare(&m).is_none());
    }
}
