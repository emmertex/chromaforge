// Ported from RADcamProfiles (github.com/OneSlips/RADcamProfiles, commit dd8003f, crates/engine/src/reference/mod.rs),
// Copyright (c) 2026 RAD Films - Roman Alurkoff, MIT; relicensed MIT OR Apache-2.0 by its author.
// Modified by the porting contributors (2026): the ColorChecker 24 (before Nov 2014) only, the app's frame type.
//! Chart definitions and the step from a decoded frame to a measurement.
//!
//! A camera profile is fitted from pairs: what the camera recorded at each patch
//! of a chart, and what that patch actually is. This module supplies the second
//! half for the charts the application supports, and turns a frame plus a patch
//! grid into the first.
//!
//! Reference values are the published target measurements for each chart, which
//! are widely reproduced and are the values profiling software expects. They are
//! stored as CIELAB under D50, because that is how they are published; the
//! conversion to the fitting illuminant happens here so a caller only has to
//! name the light.
//!
//! No file from another program is needed for the built-in charts: their data
//! is in the crate. Other charts are read from a reference file ([`file`]).

use std::borrow::Cow;

use crate::color;
use crate::frame::RawFrame;
use crate::linalg::Vec3;
use crate::target::{Patch, PatchSet};

/// How many patches in each direction a chart is laid out in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    /// Patches across.
    pub columns: usize,
    /// Patches down.
    pub rows: usize,
}

impl Layout {
    /// Total number of patches.
    pub fn patches(self) -> usize {
        self.columns * self.rows
    }
}

/// A patch's identity on a chart.
#[derive(Debug, Clone, PartialEq)]
pub struct ChartPatch {
    /// The identifier written into a measurement, e.g. `"A01"`.
    pub id: Cow<'static, str>,
    /// A human readable name; the identifier when the chart gives none.
    pub name: Cow<'static, str>,
    /// Reference value, CIELAB under D50.
    pub lab: [f64; 3],
}

/// A chart the application can profile from: a built-in one, or one read from
/// a reference file ([`file`]).
#[derive(Debug, Clone, PartialEq)]
pub struct Chart {
    /// Short key, e.g. `"cc24"`.
    pub key: Cow<'static, str>,
    /// Display name.
    pub name: Cow<'static, str>,
    /// Where the reference values come from, for the interface.
    pub source: Cow<'static, str>,
    /// Grid shape.
    pub layout: Layout,
    /// The patches, in reading order: row by row from the top left.
    pub patches: Cow<'static, [ChartPatch]>,
    /// Whether the reference says which way its grid is read. A file that
    /// names its patches only by a letter and a number does not, and the chart
    /// search then tries both readings ([`Chart::transposed`]).
    pub reading_known: bool,
}

impl Chart {
    /// The same patches read the other way round: the rows as columns.
    ///
    /// A reference file that names its patches only by a letter and a number
    /// does not say which of the two counts along the rows. The two readings
    /// are mirror images of each other, and a photograph can only show a chart
    /// turned, never mirrored, so exactly one of them matches it; the chart
    /// search tries both ([`crate::detect::find_by_pattern`]).
    pub fn transposed(&self) -> Chart {
        let Layout { columns, rows } = self.layout;
        let mut patches = Vec::with_capacity(self.patches.len());
        for row in 0..columns {
            for column in 0..rows {
                if let Some(patch) = self.patches.get(column * columns + row) {
                    patches.push(patch.clone());
                }
            }
        }
        Chart {
            key: Cow::Owned(format!("{}-transposed", self.key)),
            name: self.name.clone(),
            source: self.source.clone(),
            layout: Layout { columns: rows, rows: columns },
            patches: Cow::Owned(patches),
            reading_known: self.reading_known,
        }
    }
}

macro_rules! patch {
    ($id:literal, $name:literal, $l:expr, $a:expr, $b:expr) => {
        ChartPatch { id: Cow::Borrowed($id), name: Cow::Borrowed($name), lab: [$l, $a, $b] }
    };
}

/// The X-Rite ColorChecker Classic, its Mini, and the 24-patch page of the
/// ColorChecker Passport, as made before November 2014.
///
/// Reference values are X-Rite's (GretagMacbeth's) published measurements,
/// which are also ArgyllCMS's `ColorChecker.cie`. A chart's code says when it
/// was made: a Passport marked `MSCCPPCC0613` dates from June 2013.
pub static COLORCHECKER_CLASSIC: Chart = Chart {
    key: Cow::Borrowed("cc24"),
    name: Cow::Borrowed("ColorChecker 24, made before Nov 2014"),
    source: Cow::Borrowed("X-Rite's published values for the ColorChecker Classic, Mini and Passport made before November 2014"),
    layout: Layout { columns: 6, rows: 4 },
    patches: Cow::Borrowed(&[
        patch!("A01", "Dark Skin", 37.99, 13.56, 14.06),
        patch!("A02", "Light Skin", 65.71, 18.13, 17.81),
        patch!("A03", "Blue Sky", 49.93, -4.88, -21.93),
        patch!("A04", "Foliage", 43.14, -13.10, 21.91),
        patch!("A05", "Blue Flower", 55.11, 8.84, -25.40),
        patch!("A06", "Bluish Green", 70.72, -33.40, -0.20),
        patch!("B01", "Orange", 62.66, 36.07, 57.10),
        patch!("B02", "Purplish Blue", 40.02, 10.41, -45.96),
        patch!("B03", "Moderate Red", 51.12, 48.24, 16.25),
        patch!("B04", "Purple", 30.33, 22.98, -21.59),
        patch!("B05", "Yellow Green", 72.53, -23.71, 57.26),
        patch!("B06", "Orange Yellow", 71.94, 19.36, 67.86),
        patch!("C01", "Blue", 28.78, 14.18, -50.30),
        patch!("C02", "Green", 55.26, -38.34, 31.37),
        patch!("C03", "Red", 42.10, 53.38, 28.19),
        patch!("C04", "Yellow", 81.73, 4.04, 79.82),
        patch!("C05", "Magenta", 51.94, 49.99, -14.57),
        patch!("C06", "Cyan", 51.04, -28.63, -28.64),
        patch!("D01", "White", 96.54, -0.43, 1.19),
        patch!("D02", "Neutral 8", 81.26, -0.64, -0.34),
        patch!("D03", "Neutral 6.5", 66.77, -0.73, -0.50),
        patch!("D04", "Neutral 5", 50.87, -0.15, -0.27),
        patch!("D05", "Neutral 3.5", 35.66, -0.42, -1.23),
        patch!("D06", "Black", 20.46, -0.08, -0.97),
    ]),
    reading_known: true,
};

pub static CHARTS: &[&Chart] = &[&COLORCHECKER_CLASSIC];

/// Look a built-in chart up by its key.
pub fn by_key(key: &str) -> Option<&'static Chart> {
    CHARTS.iter().copied().find(|chart| chart.key == key)
}

/// The D50 white point the reference values are published under.
fn d50() -> Vec3 {
    crate::color::D50
}

/// A chart position expressed as the four corners of its patch area, in frame
/// pixels, going clockwise from the top left of the first patch.
///
/// The corners frame the *patch grid*, not the physical card: a chart has a
/// margin and gaps between patches, and including those in the grid would shift
/// every patch. A caller drawing pins on a preview should place them on the
/// outer edge of the outermost patches.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChartCorners {
    /// Top left.
    pub top_left: [f64; 2],
    /// Top right.
    pub top_right: [f64; 2],
    /// Bottom right.
    pub bottom_right: [f64; 2],
    /// Bottom left.
    pub bottom_left: [f64; 2],
}

impl ChartCorners {
    /// A rectangle, for the common case where the chart is square to the frame.
    pub fn from_rect(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self { top_left: [x, y], top_right: [x + width, y], bottom_right: [x + width, y + height], bottom_left: [x, y + height] }
    }

    /// The frame's full extent.
    pub fn whole_frame(frame: &RawFrame) -> Self {
        let pw = frame.width as f64;
        let ph = frame.height as f64;
        Self::from_rect(0.0, 0.0, pw, ph)
    }

    /// Relabel the corner ring a quarter turn clockwise.
    ///
    /// The pins stay where they are; only which one is called the top left
    /// changes. Orientation is *not* expressed this way any more — corners are
    /// kept in screen order and [`measure_rotated`]'s `quarter_turns` says how
    /// the chart lies — so this is only useful for code that genuinely needs the
    /// same quadrilateral read from another corner.
    pub fn rotated_quarter(self, turns: u8) -> Self {
        let mut corners = self;
        for _ in 0..(turns % 4) {
            corners = Self {
                top_left: corners.bottom_left,
                top_right: corners.top_left,
                bottom_right: corners.top_right,
                bottom_left: corners.bottom_right,
            };
        }
        corners
    }

    /// True when the quadrilateral is taller than it is wide.
    ///
    /// A ColorChecker is half again as wide as it is tall, so the shape of the
    /// pins says whether the chart lies upright or on its side: an upright chart
    /// is read with an even number of quarter turns and one on its side with an
    /// odd number. Only the remaining half-turn ambiguity needs the colours to
    /// resolve.
    pub fn is_portrait(&self) -> bool {
        let width = (distance(self.top_left, self.top_right) + distance(self.bottom_left, self.bottom_right)) / 2.0;
        let height = (distance(self.top_left, self.bottom_left) + distance(self.top_right, self.bottom_right)) / 2.0;
        height > width
    }

    /// The quarter turns consistent with the corners' shape for a chart that is
    /// wider than tall: `[0, 2]` for a landscape quadrilateral, `[1, 3]` for a
    /// portrait one.
    pub fn plausible_turns(&self, chart: &Chart) -> [u8; 2] {
        let chart_portrait = chart.layout.rows > chart.layout.columns;
        if self.is_portrait() != chart_portrait { [1, 3] } else { [0, 2] }
    }

    /// The four corners in reading order, clockwise from the top left.
    pub fn points(&self) -> [[f64; 2]; 4] {
        [self.top_left, self.top_right, self.bottom_right, self.bottom_left]
    }

    /// Whether the corners describe a usable quadrilateral.
    ///
    /// Self-intersecting or collapsed shapes are rejected rather than being
    /// sampled, because they would produce patch positions that look plausible
    /// on a preview and are wrong.
    pub fn is_usable(&self) -> bool {
        let p = self.points();
        // Each edge must have a sane length.
        for i in 0..4 {
            let a = p[i];
            let b = p[(i + 1) % 4];
            let length = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
            if length < 8.0 {
                return false;
            }
        }
        // The four corners must form a convex ring. A quadrilateral drawn in an
        // order that crosses over itself turns the opposite way at some corner,
        // so requiring every turn to have the same sign rejects it. The earlier
        // diagonal test let a bow tie through.
        let cross = |o: [f64; 2], a: [f64; 2], b: [f64; 2]| (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0]);
        let turns: Vec<f64> = (0..4).map(|i| cross(p[i], p[(i + 1) % 4], p[(i + 2) % 4])).collect();
        turns.iter().all(|t| *t > 0.0) || turns.iter().all(|t| *t < 0.0)
    }

    /// The centre of patch `index` within the grid.
    ///
    /// The grid is taken to be a perspective projection of a regular array, so
    /// a chart photographed at an angle still samples correctly. The position is
    /// found by bilinear interpolation between the corners, which is exact for a
    /// rectangle and a good approximation for the modest keystone of a hand-held
    /// photograph.
    pub fn patch_centre(&self, layout: Layout, index: usize) -> [f64; 2] {
        let columns = layout.columns.max(1);
        let rows = layout.rows.max(1);
        let column = (index % columns) as f64;
        let row = (index / columns) as f64;
        // Parameter within the grid: the centre of a cell, not its corner.
        let u = (column + 0.5) / columns as f64;
        let v = (row + 0.5) / rows as f64;

        let top = lerp(self.top_left, self.top_right, u);
        let bottom = lerp(self.bottom_left, self.bottom_right, u);
        lerp(top, bottom, v)
    }

    /// Half the width and height of a patch's sampling area, in frame pixels.
    ///
    /// `fraction` is the share of a cell that is sampled. Taking less than the
    /// whole cell keeps the sample clear of the gap between patches, which is
    /// what makes a measurement repeatable. The value returned is a
    /// *half*-extent, so a caller spans `centre - extent .. centre + extent`.
    pub fn patch_extent(&self, layout: Layout, fraction: f64) -> [f64; 2] {
        let columns = layout.columns.max(1) as f64;
        let rows = layout.rows.max(1) as f64;
        // The fraction applies to the patch cell, not to the corner separation:
        // scaling the separation would turn a perspective quadrilateral into
        // something that is not a scaled copy of itself.
        let top_width = distance(self.top_left, self.top_right) / columns;
        let bottom_width = distance(self.bottom_left, self.bottom_right) / columns;
        let left_height = distance(self.top_left, self.bottom_left) / rows;
        let right_height = distance(self.top_right, self.bottom_right) / rows;
        let cell_width = top_width.max(bottom_width) * fraction;
        let cell_height = left_height.max(right_height) * fraction;
        // Half-extents, because callers use them either side of a centre.
        [cell_width / 2.0, cell_height / 2.0]
    }
}

fn lerp(a: [f64; 2], b: [f64; 2], t: f64) -> [f64; 2] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
}

fn distance(a: [f64; 2], b: [f64; 2]) -> f64 {
    ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt()
}

/// What a chart measurement produced.
#[derive(Debug, Clone)]
pub struct ChartMeasurement {
    /// Camera RGB per patch, in reading order, as recorded.
    pub camera_rgb: Vec<Vec3>,
    /// Reference XYZ per patch under the fitting illuminant.
    pub reference_xyz: Vec<Vec3>,
    /// Patch identifiers.
    pub ids: Vec<String>,
    /// Per patch, how much the sampled area varied relative to its mean. A
    /// high value means the patch was misplaced, so it is reported rather than
    /// hidden.
    pub variation: Vec<f64>,
    /// How much of each patch's area was sampled, for diagnostics.
    pub sample_counts: Vec<usize>,
}

impl ChartMeasurement {
    /// Build the patch set a profile can be fitted from.
    ///
    /// The reference is adapted from the published D50 values to `illuminant`
    /// white, which is what a fit expects: the numbers then describe the target
    /// as it would appear under the light it was photographed under.
    pub fn to_patch_set(&self, illuminant_white: Vec3) -> PatchSet {
        let mut set = PatchSet::new();
        let adapt = color::white_map(color::Cat::Bradford, d50(), illuminant_white);
        for index in 0..self.ids.len() {
            let lab = self.reference_lab(index);
            let xyz_d50 = color::lab_to_xyz(lab, d50());
            let xyz = adapt * xyz_d50;
            let mut patch = Patch::new(self.ids[index].clone());
            patch.xyz = xyz;
            patch.cam = self.camera_rgb[index];
            set.push(patch, None);
        }
        set
    }

    /// The published reference value of a patch, in CIELAB under D50.
    ///
    /// This is kept so a caller can show it; the measurement itself is stored
    /// as XYZ under the fitting illuminant.
    pub fn reference_lab(&self, index: usize) -> Vec3 {
        // The reference XYZ under D50 is recovered from what was stored, then
        // converted back. Storing both would be redundant, so this derives it.
        let xyz = self.reference_xyz[index];
        let adapt = color::white_map(color::Cat::Bradford, xyz_white(), d50());
        color::xyz_to_lab(adapt * xyz, d50())
    }
}

/// Placeholder white used when a caller has not stated one.
fn xyz_white() -> Vec3 {
    d50()
}

/// The chart cell `(row, column)` that a cell of the frame's grid shows when the
/// chart lies `turns` quarter turns clockwise.
///
/// The frame grid is laid out in screen order: `frame_row` counts down the
/// picture and `frame_column` across it, over the transposed layout when
/// `turns` is odd. Each case is the inverse of a true rotation — not a
/// reflection, which would also be a permutation of the cells and therefore
/// pass a test that only checks every patch is visited once.
pub fn chart_cell(turns: u8, frame_row: usize, frame_column: usize, chart_rows: usize, chart_columns: usize) -> (usize, usize) {
    let last_row = chart_rows.saturating_sub(1);
    let last_column = chart_columns.saturating_sub(1);
    match turns % 4 {
        // Clockwise: the chart's top row becomes the right-hand column, read
        // downwards, so A01 is at the top right.
        1 => (last_row.saturating_sub(frame_column), frame_row),
        // Half turn: A01 at the bottom right.
        2 => (last_row.saturating_sub(frame_row), last_column.saturating_sub(frame_column)),
        // Anticlockwise: the chart's top row becomes the left-hand column, read
        // upwards, so A01 is at the bottom left.
        3 => (frame_column, last_column.saturating_sub(frame_row)),
        _ => (frame_row, frame_column),
    }
}

/// Sample a chart from a decoded frame.
///
/// Every patch is measured from its own colour-filter sites, so no colour is
/// interpolated: the mean of the red sites gives the red value, and so on. That
/// is what makes the result a measurement rather than a rendering.
pub fn measure(frame: &RawFrame, chart: &Chart, corners: &ChartCorners, sample_fraction: f64) -> ChartMeasurement {
    measure_rotated(frame, chart, corners, sample_fraction, 0)
}

/// Sample a chart that is lying rotated in the frame.
///
/// `corners` are the pins in screen order: `top_left` is the pin nearest the
/// top left of the picture, whichever patch it sits on. `quarter_turns` is how
/// many quarter turns clockwise the chart appears to have been turned:
///
/// | turns | where patch A01 (dark skin) is |
/// | --- | --- |
/// | 0 | top left (the chart upright) |
/// | 1 | top right |
/// | 2 | bottom right |
/// | 3 | bottom left |
///
/// A chart turned an odd number of times has its rows and columns exchanged in
/// the picture, so the grid the frame is sampled on is the transpose of the
/// chart's own layout; the logical patch order is unchanged, which is what keeps
/// the reference values aligned with the right patches.
///
/// Patches are measured on camera-native values ([`RawFrame::camera_rgb_half`]),
/// with no white balance applied.
pub fn measure_rotated(frame: &RawFrame, chart: &Chart, corners: &ChartCorners, sample_fraction: f64, quarter_turns: u8) -> ChartMeasurement {
    let turns = quarter_turns % 4;
    // In the frame, an odd number of turns exchanges the two counts.
    let layout = if turns % 2 == 1 { Layout { columns: chart.layout.rows, rows: chart.layout.columns } } else { chart.layout };
    let half_width = frame.width;
    let half_height = frame.height;
    let count = layout.patches();

    let mut camera_rgb = Vec::with_capacity(count);
    let mut variation = Vec::with_capacity(count);
    let mut sample_counts = Vec::with_capacity(count);
    let mut ids = Vec::with_capacity(count);
    let mut reference_xyz = Vec::with_capacity(count);

    // The frame is always walked in raster order over the transposed grid; what
    // changes with rotation is which chart patch each frame cell holds.
    let extent = corners.patch_extent(layout, sample_fraction);
    let chart_columns = chart.layout.columns.max(1);

    for index in 0..count {
        // Frame-space cell.
        let frame_row = index / layout.columns.max(1);
        let frame_column = index % layout.columns.max(1);
        // The frame is sampled on `layout`, which is the transpose of the chart
        // when the chart lies turned. Working out which patch a frame cell holds
        // is done in the chart's own coordinates, so the counts used for the
        // reflection are always the chart's.
        // The mapping is an inverse rotation: it takes a cell of the frame's
        // grid and finds the chart cell that ended up there.
        let chart_rows = chart.layout.rows;
        let (chart_row, chart_column) = chart_cell(turns, frame_row, frame_column, chart_rows, chart_columns);
        let chart_index = (chart_row * chart_columns + chart_column).min(count - 1);
        let centre = corners.patch_centre(layout, index);
        let x0 = (centre[0] - extent[0]).floor().max(0.0) as usize;
        let x1 = ((centre[0] + extent[0]).ceil() as usize).min(half_width);
        let y0 = (centre[1] - extent[1]).floor().max(0.0) as usize;
        let y1 = ((centre[1] + extent[1]).ceil() as usize).min(half_height);

        // Only the patch's own window is read, straight from the samples, so
        // measuring never builds the whole half-resolution image.
        let (values, _, _) = frame.block_means(1, [x0, y0, x1.max(x0 + 1), y1.max(y0 + 1)]);
        let mut sums = [0.0f64; 3];
        let mut sums_sq = [0.0f64; 3];
        let n = values.len();
        for value in &values {
            for c in 0..3 {
                sums[c] += value.v[c];
                sums_sq[c] += value.v[c] * value.v[c];
            }
        }

        let mut mean = [0.0f64; 3];
        let mut spread = 0.0f64;
        if n > 0 {
            for c in 0..3 {
                mean[c] = sums[c] / n as f64;
                let var = (sums_sq[c] / n as f64 - mean[c] * mean[c]).max(0.0);
                if mean[c] > 0.0 {
                    spread = spread.max(var.sqrt() / mean[c]);
                }
            }
        } else {
            spread = f64::INFINITY;
        }

        camera_rgb.push(Vec3::new(mean[0], mean[1], mean[2]));
        variation.push(spread);
        sample_counts.push(n);
        let patch = &chart.patches[chart_index];
        ids.push(patch.id.to_string());
        reference_xyz.push(color::lab_to_xyz(Vec3::new(patch.lab[0], patch.lab[1], patch.lab[2]), d50()));
    }

    // The channel map is part of the frame's identity, so a caller that has
    // mismatched the two would otherwise get silently wrong colours. It is
    // reported through the counts instead: the median sample count is what a
    // correctly sized grid produces.

    ChartMeasurement { camera_rgb, reference_xyz, ids, variation, sample_counts }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_classic_chart_has_its_patches() {
        assert_eq!(COLORCHECKER_CLASSIC.layout.patches(), 24);
        assert_eq!(COLORCHECKER_CLASSIC.patches.len(), 24);
        // The identifiers run A01..D06 in the order the grid reads.
        assert_eq!(COLORCHECKER_CLASSIC.patches[0].id, "A01");
        assert_eq!(COLORCHECKER_CLASSIC.patches[5].id, "A06");
        assert_eq!(COLORCHECKER_CLASSIC.patches[6].id, "B01");
        assert_eq!(COLORCHECKER_CLASSIC.patches[23].id, "D06");
        // The last row is the neutral ramp, which is what a tone curve is
        // derived from, so it must stay neutral.
        for patch in &COLORCHECKER_CLASSIC.patches[18..] {
            assert!(patch.lab[1].abs() < 1.5, "{} is not neutral", patch.id);
            assert!(patch.lab[2].abs() < 1.5, "{} is not neutral", patch.id);
        }
        // And the ramp descends in lightness.
        for pair in COLORCHECKER_CLASSIC.patches[18..].windows(2) {
            assert!(pair[0].lab[0] > pair[1].lab[0], "{:?}", pair);
        }
    }

    #[test]
    fn every_rotation_visits_every_patch_once() {
        // A rotation that is not a permutation makes several frame cells resolve
        // to the same patch, which silently produces a measurement made of
        // repeated values. This checks the four orientations directly.
        let chart = &COLORCHECKER_CLASSIC;
        let chart_columns = chart.layout.columns;
        let chart_rows = chart.layout.rows;
        for turns in 0..4u8 {
            let layout = if turns % 2 == 1 { Layout { columns: chart_rows, rows: chart_columns } } else { chart.layout };
            let mut seen = vec![false; chart.layout.patches()];
            for index in 0..layout.patches() {
                let frame_row = index / layout.columns;
                let frame_column = index % layout.columns;
                let (chart_row, chart_column) = chart_cell(turns, frame_row, frame_column, chart_rows, chart_columns);
                assert!(chart_row < chart_rows && chart_column < chart_columns, "turn {turns} cell {index} gave ({chart_row},{chart_column})");
                let chart_index = chart_row * chart_columns + chart_column;
                assert!(!seen[chart_index], "turn {turns}: patch {} visited twice", chart.patches[chart_index].id);
                seen[chart_index] = true;
            }
            assert!(seen.iter().all(|s| *s), "turn {turns} missed a patch");
        }
    }

    #[test]
    fn the_cell_mapping_inverts_a_true_rotation() {
        // Put each patch centre of an upright 6x4 chart in chart coordinates,
        // turn the coordinates clockwise `turns` times as a photograph would,
        // and check that the frame cell they land in maps back to that patch.
        // A reflection passes the "visits every patch once" test but fails this.
        let (rows, columns) = (4usize, 6usize);
        for turns in 0..4u8 {
            for r in 0..rows {
                for c in 0..columns {
                    let (mut x, mut y) = (c as f64 + 0.5, r as f64 + 0.5);
                    let (mut w, mut h) = (columns as f64, rows as f64);
                    for _ in 0..turns {
                        // A quarter turn clockwise in screen coordinates (y down).
                        let (nx, ny) = (h - y, x);
                        x = nx;
                        y = ny;
                        std::mem::swap(&mut w, &mut h);
                    }
                    let (frame_column, frame_row) = (x.floor() as usize, y.floor() as usize);
                    assert_eq!(
                        chart_cell(turns, frame_row, frame_column, rows, columns),
                        (r, c),
                        "turn {turns}: patch ({r},{c}) landed in frame cell ({frame_row},{frame_column})"
                    );
                }
            }
        }
        // And the documented landmarks: where A01 appears for each turn.
        let a01 = |turns: u8| -> (usize, usize) {
            let (fr, fc) = if turns % 2 == 1 { (6, 4) } else { (4, 6) };
            for row in 0..fr {
                for col in 0..fc {
                    if chart_cell(turns, row, col, rows, columns) == (0, 0) {
                        return (row, col);
                    }
                }
            }
            panic!("A01 is nowhere")
        };
        assert_eq!(a01(0), (0, 0), "upright: top left");
        assert_eq!(a01(1), (0, 3), "clockwise: top right");
        assert_eq!(a01(2), (3, 5), "half turn: bottom right");
        assert_eq!(a01(3), (5, 0), "anticlockwise: bottom left");
    }

    #[test]
    fn the_pin_shape_decides_the_parity() {
        let landscape = ChartCorners::from_rect(0.0, 0.0, 600.0, 400.0);
        let portrait = ChartCorners::from_rect(0.0, 0.0, 400.0, 600.0);
        assert_eq!(landscape.plausible_turns(&COLORCHECKER_CLASSIC), [0, 2]);
        assert_eq!(portrait.plausible_turns(&COLORCHECKER_CLASSIC), [1, 3]);
    }

    #[test]
    fn rotating_the_corners_moves_the_same_points_round_the_ring() {
        let corners = ChartCorners::from_rect(0.0, 0.0, 60.0, 40.0);
        let once = corners.rotated_quarter(1);
        // The point that was the top left becomes the top right.
        assert_eq!(once.top_right, corners.top_left);
        assert_eq!(once.top_left, corners.bottom_left);
        assert_eq!(once.bottom_left, corners.bottom_right);
        // Four turns return to the start.
        assert_eq!(corners.rotated_quarter(4), corners);
        // A rotation relabels the corners, so the old top edge becomes the new
        // right edge and its length follows the original height.
        assert_eq!(distance(once.top_left, once.top_right), distance(corners.top_left, corners.bottom_left));
        assert!(once.is_usable());
    }

    #[test]
    fn rotating_swaps_which_way_the_grid_is_read() {
        // A grid twice as wide as it is tall: after one turn the cell centres
        // step along the other axis, which is what a transposed photograph
        // needs.
        let layout = Layout { columns: 6, rows: 4 };
        let swapped = Layout { columns: 4, rows: 6 };
        let corners = ChartCorners::from_rect(0.0, 0.0, 120.0, 80.0);
        let rotated = corners.rotated_quarter(1);
        // Turning the ring clockwise sends its top-left corner to the top-right,
        // so the grid's long axis runs vertically afterwards. That is exactly
        // what a chart photographed the other way up needs.
        assert_eq!(rotated.top_right, corners.top_left);
        assert_eq!(rotated.top_left, corners.bottom_left);
        // The cells are therefore laid out on the other axis: with the swapped
        // layout, stepping one index moves down the turned ring.
        let first = rotated.patch_centre(swapped, 0);
        let second = rotated.patch_centre(swapped, 1);
        assert!((second[1] - first[1]).abs() > (second[0] - first[0]).abs(), "the turned grid should step along y: {first:?} then {second:?}");
        // And the unturned grid steps along x.
        let first = corners.patch_centre(layout, 0);
        let second = corners.patch_centre(layout, 1);
        assert!((second[0] - first[0]).abs() > (second[1] - first[1]).abs(), "the unturned grid should step along x: {first:?} then {second:?}");
        // Everything stays inside the area the corners describe.
        for p in [corners.patch_centre(layout, 23), rotated.patch_centre(swapped, 23)] {
            assert!(p[0] >= 0.0 && p[0] <= 120.0, "{p:?}");
            assert!(p[1] >= 0.0 && p[1] <= 80.0, "{p:?}");
        }
    }

    #[test]
    fn charts_are_found_by_key() {
        assert!(by_key("cc24").is_some());
        assert!(by_key("nope").is_none());
        assert_eq!(by_key("cc24").unwrap().name, "ColorChecker 24, made before Nov 2014");
        for chart in CHARTS {
            assert_eq!(by_key(&chart.key), Some(*chart));
            assert_eq!(chart.patches.len(), chart.layout.patches(), "{}", chart.name);
            assert!(chart.reading_known);
        }
    }

    #[test]
    fn transposing_twice_is_the_identity_and_swaps_the_counts() {
        let turned = COLORCHECKER_CLASSIC.transposed();
        assert_eq!(turned.layout, Layout { columns: 4, rows: 6 });
        // Down the first column of the chart becomes along the first row.
        assert_eq!(turned.patches[1].id, "B01");
        assert_eq!(turned.patches[4].id, "A02");
        let back = turned.transposed();
        assert_eq!(back.patches, COLORCHECKER_CLASSIC.patches);
        assert_eq!(back.layout, COLORCHECKER_CLASSIC.layout);
    }

    #[test]
    fn a_rectangle_places_the_first_and_last_patch_centres() {
        // A 600x400 grid of 6x4 cells: cells are 100x100.
        let corners = ChartCorners::from_rect(0.0, 0.0, 600.0, 400.0);
        let first = corners.patch_centre(COLORCHECKER_CLASSIC.layout, 0);
        assert!((first[0] - 50.0).abs() < 1e-9, "{first:?}");
        assert!((first[1] - 50.0).abs() < 1e-9, "{first:?}");
        // The sixth patch of the first row is five cells along.
        let sixth = corners.patch_centre(COLORCHECKER_CLASSIC.layout, 5);
        assert!((sixth[0] - 550.0).abs() < 1e-9, "{sixth:?}");
        // The last patch is the bottom right cell.
        let last = corners.patch_centre(COLORCHECKER_CLASSIC.layout, 23);
        assert!((last[0] - 550.0).abs() < 1e-9, "{last:?}");
        assert!((last[1] - 350.0).abs() < 1e-9, "{last:?}");
    }

    #[test]
    fn a_tilted_quadrilateral_still_places_patches_inside_itself() {
        // A chart photographed at an angle: the right edge is shorter and lower.
        let corners = ChartCorners { top_left: [100.0, 100.0], top_right: [500.0, 140.0], bottom_right: [470.0, 480.0], bottom_left: [120.0, 500.0] };
        assert!(corners.is_usable());
        for index in 0..24 {
            let p = corners.patch_centre(COLORCHECKER_CLASSIC.layout, index);
            assert!(p[0] > 100.0 && p[0] < 500.0, "patch {index} at {p:?}");
            assert!(p[1] > 100.0 && p[1] < 500.0, "patch {index} at {p:?}");
        }
        // Adjacent patches stay in order along each row.
        let a = corners.patch_centre(COLORCHECKER_CLASSIC.layout, 0);
        let b = corners.patch_centre(COLORCHECKER_CLASSIC.layout, 1);
        assert!(b[0] > a[0], "{a:?} then {b:?}");
    }

    #[test]
    fn a_collapsed_or_crossed_quadrilateral_is_rejected() {
        let collapsed = ChartCorners::from_rect(0.0, 0.0, 4.0, 400.0);
        assert!(!collapsed.is_usable());
        // Corners whose edges cross: the third point is on the wrong side.
        let crossed = ChartCorners { top_left: [0.0, 0.0], top_right: [100.0, 0.0], bottom_right: [0.0, 100.0], bottom_left: [100.0, 100.0] };
        assert!(!crossed.is_usable());
    }

    #[test]
    fn the_sample_extent_shrinks_with_the_fraction() {
        let corners = ChartCorners::from_rect(0.0, 0.0, 600.0, 400.0);
        let half = corners.patch_extent(COLORCHECKER_CLASSIC.layout, 0.5);
        // A 600x400 grid of 6x4 cells has 100 pixel cells. The returned value
        // is a half-extent, so a half-size sample spans 50 pixels: a half of 50.
        assert!((half[0] - 25.0).abs() < 1e-9, "{half:?}");
        assert!((half[1] - 25.0).abs() < 1e-9, "{half:?}");
        let full = corners.patch_extent(COLORCHECKER_CLASSIC.layout, 1.0);
        // Doubling the fraction doubles the extent: a full cell spans 100.
        assert!((full[0] - 50.0).abs() < 1e-9, "{full:?}");
        assert!((full[1] - 50.0).abs() < 1e-9, "{full:?}");
    }

    #[test]
    fn the_reference_values_convert_to_the_published_lab() {
        // Round trip through the stored XYZ must return the published values.
        let xyz = color::lab_to_xyz(Vec3::new(96.54, -0.43, 1.19), d50());
        let back = color::xyz_to_lab(xyz, d50());
        assert!((back.v[0] - 96.54).abs() < 1e-9, "{back:?}");
        assert!((back.v[1] + 0.43).abs() < 1e-9, "{back:?}");
        assert!((back.v[2] - 1.19).abs() < 1e-9, "{back:?}");
    }
}
