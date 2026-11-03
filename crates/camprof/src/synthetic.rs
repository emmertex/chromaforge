//! A ColorChecker photographed by a made-up camera, for tests and demonstrations: no photograph or
//! measurement of a real camera is involved.
//!
//! The camera sees XYZ under the chosen illuminant (see [`chart_frame`]) through [`TEST_CAMERA`] and then mixes its
//! channels a little non-linearly (but exposure-invariantly), so a matrix alone cannot reproduce it
//! and a correction table has work to do.

use crate::linalg::{Mat3, Vec3};
use crate::reference::{ChartCorners, Layout, chart_cell};

/// XYZ (under the scene illuminant) to the made-up camera's native RGB. Our own numbers: a broad,
/// overlapping response like a real sensor's, with a weak blue channel.
pub const TEST_CAMERA: [[f64; 3]; 3] = [[0.62, 0.42, -0.12], [-0.28, 1.12, 0.12], [0.06, -0.18, 0.62]];

/// The made-up camera's native signal for XYZ `xyz` (under the scene light).
pub fn camera_signal(xyz: Vec3) -> Vec3 {
    let m = Mat3::from_rows(TEST_CAMERA[0], TEST_CAMERA[1], TEST_CAMERA[2]);
    let c = m * xyz;
    let (r, g, b) = (c[0].max(0.0), c[1].max(0.0), c[2].max(0.0));
    let sum = r + g + b;
    // crosstalk that grows with saturation (degree one in exposure)
    let bleed = if sum > 0.0 { 0.12 * (r - b) * (r - b) / sum } else { 0.0 };
    Vec3::new(r, g + bleed, b + 0.4 * bleed)
}

/// Camera RGB of a `width × height` frame: a mid-grey textured background with the ColorChecker
/// 24 lit by `illuminant_white` (XYZ, Y = 1) at `exposure` (the white patch's Y), with the patch
/// grid's outer corners at `corners` and patch A01 `turns` quarter turns round.
pub fn chart_frame(width: usize, height: usize, corners: &ChartCorners, turns: u8, illuminant_white: Vec3, exposure: f64) -> Vec<[f32; 3]> {
    let chart = crate::chart();
    let count = width.saturating_mul(height).min(crate::frame::MAX_PIXELS);
    let mut out = vec![[0.0f32; 3]; count];
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let mut noise = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed >> 11) as f64 / (1u64 << 53) as f64
    };
    // background in blobs, so the search has edges at every scale
    let blob = 4;
    for by in (0..height).step_by(blob) {
        for bx in (0..width).step_by(blob) {
            let level = (0.04 + 0.2 * noise()) as f32;
            let tint = [(0.7 + 0.5 * noise()) as f32, 1.0, (0.6 + 0.4 * noise()) as f32];
            for y in by..(by + blob).min(height) {
                for x in bx..(bx + blob).min(width) {
                    if let Some(p) = out.get_mut(y * width + x) {
                        *p = [level * tint[0], level, level * tint[2]];
                    }
                }
            }
        }
    }
    let turns = turns % 4;
    let (rows, columns) = if turns.is_multiple_of(2) { (chart.layout.rows, chart.layout.columns) } else { (chart.layout.columns, chart.layout.rows) };
    let layout = Layout { columns, rows };
    // The scene's colours under the light: the chart's D50 values scaled channel by channel in XYZ
    // (a "wrong von Kries"), not the Bradford adaptation the fit assumes. Like a real camera's
    // departure from the observer, the difference grows as the light moves away from daylight, so a
    // profile fitted under one light reproduces another less well.
    let d50 = crate::color::D50;
    let adapt =
        Mat3::from_rows([illuminant_white[0] / d50[0], 0.0, 0.0], [0.0, illuminant_white[1] / d50[1], 0.0], [0.0, 0.0, illuminant_white[2] / d50[2]]);
    let extent = corners.patch_extent(layout, 0.84);
    let pitch = corners.patch_extent(layout, 1.0);
    // the card behind the patches: near black, a little beyond the grid
    let points = corners.points();
    let (min_x, max_x) = points.iter().fold((f64::MAX, f64::MIN), |(a, b), p| (a.min(p[0]), b.max(p[0])));
    let (min_y, max_y) = points.iter().fold((f64::MAX, f64::MIN), |(a, b), p| (a.min(p[1]), b.max(p[1])));
    for y in (min_y - pitch[1] * 0.4).max(0.0) as usize..((max_y + pitch[1] * 0.4).max(0.0) as usize).min(height) {
        for x in (min_x - pitch[0] * 0.4).max(0.0) as usize..((max_x + pitch[0] * 0.4).max(0.0) as usize).min(width) {
            if let Some(p) = out.get_mut(y * width + x) {
                *p = [0.012, 0.012, 0.012];
            }
        }
    }
    for fr in 0..rows {
        for fc in 0..columns {
            let (r, c) = chart_cell(turns, fr, fc, chart.layout.rows, chart.layout.columns);
            let Some(patch) = chart.patches.get(r * chart.layout.columns + c) else { continue };
            let xyz = adapt * crate::color::lab_to_xyz(Vec3::new(patch.lab[0], patch.lab[1], patch.lab[2]), crate::color::D50);
            let cam = camera_signal(xyz * exposure);
            let centre = corners.patch_centre(layout, fr * columns + fc);
            let (y0, y1) = ((centre[1] - extent[1]).round().max(0.0) as usize, (centre[1] + extent[1]).round().max(0.0) as usize);
            let (x0, x1) = ((centre[0] - extent[0]).round().max(0.0) as usize, (centre[0] + extent[0]).round().max(0.0) as usize);
            for y in y0..y1.min(height) {
                for x in x0..x1.min(width) {
                    if let Some(p) = out.get_mut(y * width + x) {
                        *p = [cam[0] as f32, cam[1] as f32, cam[2] as f32];
                    }
                }
            }
        }
    }
    out
}
