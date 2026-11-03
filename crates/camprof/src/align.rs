// Ported from RADcamProfiles (github.com/OneSlips/RADcamProfiles, commit dd8003f, crates/engine/src/align.rs),
// Copyright (c) 2026 RAD Films - Roman Alurkoff, MIT; relicensed MIT OR Apache-2.0 by its author.
// Modified by the porting contributors (2026): no panics on bad input, the app's frame type.
//! Finding a chart's exact position and orientation.
//!
//! [`crate::detect`] gives a first guess at where a chart is. That is enough to
//! be useful but not enough to measure from: a grid that is off by a third of a
//! patch samples the gaps between patches, and the resulting profile is wrong in
//! a way that looks plausible.
//!
//! This module refines the guess by minimising something objective. A grid that
//! lines up with a chart puts the centre of every sampling rectangle on a patch,
//! so the signal inside each rectangle is nearly uniform. A grid that is offset
//! straddles the boundaries and the spread jumps. Total spread over the patches
//! is therefore a direct measure of placement, needs no colour information, and
//! works for any chart shape.
//!
//! Orientation is a separate problem: a square-ish grid scores equally well at
//! 0 and 2 quarter turns because the geometry is symmetric, so the placement
//! search cannot choose between them. Orientation is instead decided by which
//! one produces the better profile, which is the question that actually matters.

use crate::frame::RawFrame;
use crate::profile::{self, FitOptions};
use crate::reference::{self, Chart, ChartCorners, ChartMeasurement};

/// A chart's position and orientation in a frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    /// Corners of the patch grid, in the frame's own pixel coordinates.
    pub corners: ChartCorners,
    /// Quarter turns clockwise the chart lies turned in the frame.
    pub quarter_turns: u8,
    /// Mean spread within the sampling rectangles. Lower is better; a well
    /// aligned grid on a clean photograph is under about 0.05.
    pub variation: f64,
}

/// Mean relative spread of the signal inside each sampling rectangle.
///
/// This is the quantity alignment minimises. It is normalised by each patch's
/// own mean, so a dark patch and a bright one contribute comparably, and it does
/// not depend on which patch is which.
pub fn mean_variation(measurement: &ChartMeasurement) -> f64 {
    let finite: Vec<f64> = measurement.variation.iter().copied().filter(|v| v.is_finite()).collect();
    if finite.is_empty() {
        return f64::INFINITY;
    }
    // An empty sampling rectangle is a failure, not a good score.
    if finite.len() != measurement.variation.len() {
        return f64::INFINITY;
    }
    finite.iter().sum::<f64>() / finite.len() as f64
}

/// How well a candidate rectangle looks like a chart.
///
/// Alignment alone is not enough to identify a chart, and the reason is worth
/// stating because it is not obvious: a grid placed on a blank surface has
/// *perfectly* even interiors, so minimising spread alone drives the search to
/// the flattest region of the frame. That is the opposite of what is wanted, and
/// it is what an earlier version of this function did — it reported a variation
/// of zero and then fitted a profile at 26 DE.
///
/// A chart is even *within* its patches and different *between* them. The score
/// is therefore the within-patch spread divided by how much the patches differ
/// from each other, which is small only for a grid that is both aligned and on
/// something with patch-to-patch contrast. A blank surface has a near-zero
/// denominator and scores badly; a grid straddling boundaries has a large
/// numerator and scores badly; a grid on a chart scores well.
fn shape_score(measurement: &ChartMeasurement) -> f64 {
    let spread = mean_variation(measurement);
    if !spread.is_finite() {
        return f64::INFINITY;
    }
    // How much the patch means differ, relative to their own level.
    let levels: Vec<f64> =
        measurement.camera_rgb.iter().map(|v| 0.2126 * v.v[0] + 0.7152 * v.v[1] + 0.0722 * v.v[2]).filter(|v| v.is_finite()).collect();
    if levels.len() < 2 {
        return f64::INFINITY;
    }
    let mean = levels.iter().sum::<f64>() / levels.len() as f64;
    if mean <= 1e-9 {
        return f64::INFINITY;
    }
    let contrast = (levels.iter().map(|l| (l - mean).powi(2)).sum::<f64>() / levels.len() as f64).sqrt() / mean;
    // A chart's patches differ by tens of percent; a blank surface by almost
    // nothing. The floor keeps a near-flat candidate from dividing by zero and
    // looking perfect.
    spread / contrast.max(0.02)
}

/// Refine a starting guess at a chart's position.
///
/// The grid is parameterised by its centre and its width, with the height fixed
/// by the chart's own aspect ratio. Letting all four edges move independently
/// lets the search shrink the grid until it covers fewer rows, which is
/// internally consistent and completely wrong.
///
/// Returns `None` when the starting guess is too far out for the local search to
/// recover, which is reported rather than papered over.
pub fn align(frame: &RawFrame, chart: &Chart, start: &ChartCorners, sample_fraction: f64) -> Option<Placement> {
    // The pins' shape says whether the chart lies upright or on its side, and
    // with it which grid shape the frame is sampled on.
    let candidates = start.plausible_turns(chart);
    let (columns, rows) = (chart.layout.columns.max(1) as f64, chart.layout.rows as f64);
    let aspect = if candidates[0] % 2 == 1 { columns / rows.max(1.0) } else { rows / columns };
    let centre_of = |c: &ChartCorners| {
        [
            (c.top_left[0] + c.top_right[0] + c.bottom_left[0] + c.bottom_right[0]) / 4.0,
            (c.top_left[1] + c.bottom_left[1] + c.top_right[1] + c.bottom_right[1]) / 4.0,
        ]
    };
    let width_of = |c: &ChartCorners| {
        let top = ((c.top_right[0] - c.top_left[0]).powi(2) + (c.top_right[1] - c.top_left[1]).powi(2)).sqrt();
        let bottom = ((c.bottom_right[0] - c.bottom_left[0]).powi(2) + (c.bottom_right[1] - c.bottom_left[1]).powi(2)).sqrt();
        (top + bottom) / 2.0
    };

    let mut centre = centre_of(start);
    let mut width = width_of(start);
    if width < 8.0 {
        return None;
    }

    // The frame's half-resolution extent, which is where the patches have to
    // lie. A candidate that leaves it is rejected: without this the refinement
    // walks off the edge, and a grid hanging outside the frame has evenly
    // sampled nothing, which scores deceptively well.
    let extent = [frame.width as f64, frame.height as f64];
    let inside = |corners: &ChartCorners| {
        corners.is_usable() && corners.points().iter().all(|p| p[0] >= 0.0 && p[1] >= 0.0 && p[0] <= extent[0] && p[1] <= extent[1])
    };

    let rect = |centre: [f64; 2], width: f64| {
        let height = width * aspect;
        ChartCorners::from_rect(centre[0] - width / 2.0, centre[1] - height / 2.0, width, height)
    };

    // The search only needs a relative comparison, so try the two orientations
    // the geometry cannot distinguish and keep whichever is more even. Which one
    // is *correct* is decided later by the profile fit.
    let mut turns = candidates[0];
    let mut best = f64::INFINITY;
    let mut best_corners = rect(centre, width);
    for candidate in candidates {
        let corners = rect(centre, width);
        if !inside(&corners) {
            continue;
        }
        let measurement = reference::measure_rotated(frame, chart, &corners, sample_fraction, candidate);
        let variation = shape_score(&measurement);
        if variation < best {
            best = variation;
            best_corners = corners;
            turns = candidate;
        }
    }
    if !best.is_finite() {
        return None;
    }

    let mut step = (width * 0.06).max(1.0);
    let mut scale_step = 0.05f64;
    let mut stalled = 0;
    for _ in 0..60 {
        let mut improved = false;
        // Move the centre.
        for (dx, dy) in [(-step, 0.0), (step, 0.0), (0.0, -step), (0.0, step), (step, step), (-step, -step)] {
            let candidate_centre = [centre[0] + dx, centre[1] + dy];
            let corners = rect(candidate_centre, width);
            if !inside(&corners) {
                continue;
            }
            let measurement = reference::measure_rotated(frame, chart, &corners, sample_fraction, turns);
            let variation = shape_score(&measurement);
            if variation < best {
                best = variation;
                best_corners = corners;
                centre = candidate_centre;
                improved = true;
            }
        }
        // Resize about the centre.
        for factor in [1.0 - scale_step, 1.0 + scale_step] {
            let candidate_width = width * factor;
            if candidate_width < 8.0 {
                continue;
            }
            let corners = rect(centre, candidate_width);
            if !inside(&corners) {
                continue;
            }
            let measurement = reference::measure_rotated(frame, chart, &corners, sample_fraction, turns);
            let variation = shape_score(&measurement);
            if variation < best {
                best = variation;
                best_corners = corners;
                width = candidate_width;
                improved = true;
            }
        }
        if improved {
            stalled = 0;
        } else {
            stalled += 1;
            step *= 0.5;
            scale_step *= 0.5;
            if step < 0.15 || stalled > 24 {
                break;
            }
        }
    }

    Some(Placement { corners: best_corners, quarter_turns: turns, variation: best })
}

/// The outcome of trying every orientation.
#[derive(Debug, Clone, PartialEq)]
pub struct OrientedPlacement {
    /// The placement chosen.
    pub placement: Placement,
    /// Mean correction-table error for each of the four orientations, in order.
    pub errors: [Option<f64>; 4],
}

/// Choose the orientation by fitting a profile for each one.
///
/// The pins' shape fixes whether the chart lies upright or on its side, which
/// leaves a half turn the geometry cannot decide: a grid and its half turn
/// sample exactly the same rectangles. Reading the chart the wrong way round
/// scrambles patch order, which does not change how even the sampling
/// rectangles are but does wreck the fit — so the fit is what decides.
///
/// The matrix error is used rather than the correction table's, because a table
/// can absorb part of a scrambled assignment and so blur the distinction.
pub fn resolve_orientation(frame: &RawFrame, chart: &Chart, placement: &Placement, options: &FitOptions, sample_fraction: f64) -> OrientedPlacement {
    let candidates = placement.corners.plausible_turns(chart);

    let mut errors: [Option<f64>; 4] = [None; 4];
    let mut best_turns = placement.quarter_turns;
    let mut best_error = f64::INFINITY;
    let options = FitOptions { make_lut: false, ..options.clone() };

    for turns in candidates {
        let measurement = reference::measure_rotated(frame, chart, &placement.corners, sample_fraction, turns);
        let target = measurement.to_patch_set(crate::color::D50);
        match profile::fit(&target, &options, &mut |_| {}) {
            Ok((_, report)) => {
                let error = report.matrix_mean_de;
                errors[turns as usize] = Some(error);
                if error < best_error {
                    best_error = error;
                    best_turns = turns;
                }
            }
            Err(_) => continue,
        }
    }

    OrientedPlacement { placement: Placement { quarter_turns: best_turns, ..*placement }, errors }
}

/// Find a chart, refine its position and decide its orientation.
///
/// # Limits
///
/// The refinement is local, so this only succeeds when the initial search lands
/// on the right chart. On a frame holding more than one target — a ColorChecker
/// Passport holds two — the initial search can settle on the wrong one, and the
/// refinement then converges on that target instead of the intended chart.
///
/// # What was tried, and why it did not work
///
/// The quantity used to judge a placement is how evenly the sampling rectangles
/// sit inside patches. Swept over positions at a fixed size on a real photograph
/// whose correct placement is known, it has **no usable peak**: the correct
/// placement scores 0.062 while the best any other position reaches is 0.356, and
/// the values between are close to noise. Making the search global therefore did
/// not help; the correct placement needed size and position searched together.
///
/// Two traps in the criterion were found and are fixed. A grid on a blank surface
/// has perfectly even interiors, so evenness alone drives the search to the
/// flattest region of the frame — the shape score below divides by how much the
/// patches differ, which a blank surface cannot satisfy. And a grid may not leave
/// the frame; one that does samples nothing and scores deceptively well.
///
/// Until a criterion with a usable peak is found, the intended route is the one
/// the original application used: the user places the pins and this refines them.
pub fn locate(frame: &RawFrame, chart: &Chart, options: &FitOptions, sample_fraction: f64) -> Option<OrientedPlacement> {
    // A chart with reference colours is found by them, orientation included;
    // see `detect::find_by_pattern`. The search below is for grids it cannot
    // recognise that way.
    if let Some(found) = crate::detect::find_by_pattern(frame, chart) {
        let measurement = reference::measure_rotated(frame, chart, &found.corners, sample_fraction, found.quarter_turns);
        return Some(OrientedPlacement {
            placement: Placement { corners: found.corners, quarter_turns: found.quarter_turns, variation: mean_variation(&measurement) },
            errors: [None; 4],
        });
    }
    if crate::detect::find_by_pattern_possible(chart) {
        // The chart could have been recognised and was not there.
        return None;
    }
    let guess = crate::detect::detect_in_frame(frame, chart)?;
    // The detector works in preview pixels; the rest of the engine works on the
    // half-resolution grid.
    let half_width = frame.width as f64;
    let preview_width = 700.0;
    let scale = half_width / preview_width;
    let scale_point = |p: [f64; 2]| [p[0] * scale, p[1] * scale];
    let start = ChartCorners {
        top_left: scale_point(guess.corners.top_left),
        top_right: scale_point(guess.corners.top_right),
        bottom_right: scale_point(guess.corners.bottom_right),
        bottom_left: scale_point(guess.corners.bottom_left),
    };
    let placement = align(frame, chart, &start, sample_fraction)?;
    Some(resolve_orientation(frame, chart, &placement, options, sample_fraction))
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::linalg::Vec3;

    /// A frame with a chart painted into it: each patch gets its own flat level,
    /// so a correctly placed grid has zero variation inside every rectangle.
    ///
    /// `corners` is in the **half-resolution grid**, which is the system
    /// [`reference::measure_rotated`] works in. The frame is sized from the
    /// corners with a margin, and the painted rectangle is doubled on the way
    /// into the samples, because the samples live at the full sensor size. Doing
    /// it that way keeps a fixture from silently painting outside its own frame,
    /// which shows up as an empty sampling rectangle rather than as an error.
    fn painted_frame(corners: &ChartCorners, chart: &Chart) -> RawFrame {
        // Half-grid extent, with a margin, then doubled for the sample buffer.
        let max_x = corners.points().iter().map(|p| p[0]).fold(0.0f64, f64::max);
        let max_y = corners.points().iter().map(|p| p[1]).fold(0.0f64, f64::max);
        let half_width = (max_x + 40.0).ceil() as usize;
        let half_height = (max_y + 40.0).ceil() as usize;
        let width = half_width * 2;
        let height = half_height * 2;

        let mut samples = vec![0u16; width * height];
        for s in samples.iter_mut() {
            *s = 1000;
        }
        let layout = chart.layout;
        for index in 0..layout.patches() {
            let centre = corners.patch_centre(layout, index);
            // Leave the gaps between patches at the background level so a grid
            // that is offset picks up the transition.
            let extent = corners.patch_extent(layout, 0.86);
            let level = 1200 + (index as u16 * 97) % 14000;
            let x0 = ((centre[0] - extent[0]) * 2.0).max(0.0) as usize;
            let x1 = (((centre[0] + extent[0]) * 2.0) as usize).min(width);
            let y0 = ((centre[1] - extent[1]) * 2.0).max(0.0) as usize;
            let y1 = (((centre[1] + extent[1]) * 2.0) as usize).min(height);
            for y in y0..y1 {
                for x in x0..x1 {
                    samples[y * width + x] = level;
                }
            }
        }
        crate::frame::rggb_test_frame(width, height, &samples, 16383.0)
    }
    #[test]
    fn a_well_placed_grid_has_little_variation() {
        let chart = &reference::COLORCHECKER_CLASSIC;
        let truth = ChartCorners::from_rect(60.0, 40.0, 240.0, 160.0);
        let frame = painted_frame(&truth, chart);
        let measurement = reference::measure_rotated(&frame, chart, &truth, 0.6, 0);
        assert!(mean_variation(&measurement) < 0.02, "a grid on the patches should be even, got {}", mean_variation(&measurement));
    }

    #[test]
    fn a_shifted_grid_has_more_variation_than_an_aligned_one() {
        let chart = &reference::COLORCHECKER_CLASSIC;
        let truth = ChartCorners::from_rect(60.0, 40.0, 240.0, 160.0);
        let frame = painted_frame(&truth, chart);
        let aligned = mean_variation(&reference::measure_rotated(&frame, chart, &truth, 0.6, 0));
        // Half a patch along in each direction.
        let shifted_corners = ChartCorners::from_rect(60.0 + 20.0, 40.0 + 13.3, 240.0, 160.0);
        let shifted = mean_variation(&reference::measure_rotated(&frame, chart, &shifted_corners, 0.6, 0));
        assert!(shifted > aligned * 3.0, "shifted {shifted} should be much worse than aligned {aligned}");
    }

    #[test]
    fn aligning_recovers_a_shifted_grid() {
        let chart = &reference::COLORCHECKER_CLASSIC;
        let truth = ChartCorners::from_rect(60.0, 40.0, 240.0, 160.0);
        let frame = painted_frame(&truth, chart);
        // Start a fifth of a patch out in each direction, which is the kind of
        // error the initial detector makes.
        let start = ChartCorners::from_rect(74.0, 50.0, 240.0, 160.0);
        let found = align(&frame, chart, &start, 0.6).expect("alignment should succeed");
        assert!(found.variation < 0.05, "alignment left variation at {}", found.variation);
        // The corners should be close to the truth.
        for (a, b) in found.corners.points().iter().zip(truth.points().iter()) {
            let d = ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt();
            assert!(d < 12.0, "corner {a:?} is {d:.1} from the truth {b:?}");
        }
    }

    #[test]
    fn aligning_keeps_the_charts_aspect_ratio() {
        let chart = &reference::COLORCHECKER_CLASSIC;
        let truth = ChartCorners::from_rect(60.0, 40.0, 240.0, 160.0);
        let frame = painted_frame(&truth, chart);
        let start = ChartCorners::from_rect(60.0, 40.0, 240.0, 160.0);
        let found = align(&frame, chart, &start, 0.6).unwrap();
        let width = found.corners.top_right[0] - found.corners.top_left[0];
        let height = found.corners.bottom_left[1] - found.corners.top_left[1];
        let aspect = height / width;
        let expected = chart.layout.rows as f64 / chart.layout.columns as f64;
        assert!((aspect - expected).abs() < 0.02, "aspect {aspect:.3} should stay near {expected:.3}");
    }

    #[test]
    fn a_degenerate_start_is_rejected() {
        let chart = &reference::COLORCHECKER_CLASSIC;
        let frame = painted_frame(&ChartCorners::from_rect(60.0, 40.0, 240.0, 160.0), chart);
        let tiny = ChartCorners::from_rect(100.0, 100.0, 2.0, 1.0);
        assert!(align(&frame, chart, &tiny, 0.6).is_none());
    }

    #[test]
    fn the_painted_frame_is_not_uniform() {
        // Guards the test fixture itself: if painting failed, every alignment
        // test above would pass for the wrong reason.
        let chart = &reference::COLORCHECKER_CLASSIC;
        let truth = ChartCorners::from_rect(60.0, 40.0, 240.0, 160.0);
        let frame = painted_frame(&truth, chart);
        let levels: std::collections::HashSet<u32> = frame.pixels().iter().map(|p| p[0].to_bits()).collect();
        assert!(levels.len() > 20, "only {} levels painted", levels.len());
        // And the painted frame has real colour variation between patches.
        let measurement = reference::measure_rotated(&frame, chart, &truth, 0.6, 0);
        let first = measurement.camera_rgb[0];
        let others: Vec<Vec3> = measurement.camera_rgb[1..].to_vec();
        assert!(others.iter().any(|p| (p.v[0] - first.v[0]).abs() > 0.01), "all patches read the same");
    }
}
