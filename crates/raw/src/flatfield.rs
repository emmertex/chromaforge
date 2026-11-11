//! Flat-field correction: a reference raw shot through a diffuser (same lens, focal length and aperture as the
//! photos to correct) records the lens's vignetting and colour cast on its own. [`fit`] measures it into one smooth
//! DNG `GainMap` opcode per CFA colour site (DNG 1.3, `OpcodeList2`: applied to black-subtracted, white-scaled
//! data over the active area), normalised so the centre of the frame keeps its level; [`apply`] adds them to a
//! target raw of the same sensor layout, which `write_dng` then stores.
//!
//! The fit: the active area's samples of each CFA phase are box-averaged into a coarse grid (clipped, non-finite
//! and non-positive samples left out), a least-squares polynomial surface (degree 4) is fitted through the cell
//! means so sensor noise and dust don't end up in the map and empty cells are filled, and each gain is `centre / level`,
//! clamped to [`MIN_GAIN`, `MAX_GAIN`]. The opcode layout follows the DNG specification (1.3+, chapter 6);
//! filmcraft's `crates/dng/src/opcode.rs` was not needed.

use crate::opcodes::{Area, Opcode};
use crate::{Cfa, RawImage};

/// Grid points along each axis of a fitted gain map.
pub const GRID: u32 = 17;
/// Gains are clamped to this range: a darker corner than 1/8 of the centre is not a lens to correct.
pub const MIN_GAIN: f32 = 0.25;
pub const MAX_GAIN: f32 = 8.0;
/// Samples at or above this normalised level count as clipped.
const CLIP: f32 = 0.98;

/// Why a flat field can't be fitted or applied.
#[derive(Debug, Clone, PartialEq)]
pub enum FlatFieldError {
    /// The reference is too small, too dark, clipped, or not a mosaic/linear layout the map can describe.
    Reference(String),
    /// The target's sensor layout differs from the reference's.
    Mismatch(String),
}

impl std::fmt::Display for FlatFieldError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Reference(m) | Self::Mismatch(m) => f.write_str(m),
        }
    }
}

/// The sensor layout a flat field is tied to: active-area size, samples per pixel and CFA (anchored at the active
/// area's corner).
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    pub width: usize,
    pub height: usize,
    pub cpp: usize,
    pub cfa: Option<Cfa>,
}

impl Layout {
    pub fn of(raw: &RawImage) -> Layout {
        let a = raw.active_area;
        Layout { width: a.width, height: a.height, cpp: raw.cpp, cfa: raw.cfa.as_ref().map(|c| c.shifted(a.x, a.y)) }
    }
}

/// A fitted flat field: the layout it was measured on and its `GainMap` opcodes.
#[derive(Debug, Clone, PartialEq)]
pub struct FlatField {
    pub layout: Layout,
    pub ops: Vec<Opcode>,
}

/// Measure `reference` (a raw of an evenly lit diffuser) into a [`FlatField`].
pub fn fit(reference: &RawImage) -> Result<FlatField, FlatFieldError> {
    let layout = Layout::of(reference);
    let n = reference.normalized().map_err(|e| FlatFieldError::Reference(format!("the flat-field reference doesn't decode: {e}")))?;
    let (w, h, cpp) = (n.width, n.height, n.cpp);
    // (phase x, phase y, pitch x, pitch y, plane)
    let phases: Vec<(usize, usize, usize, usize, usize)> = match (&n.cfa, cpp) {
        (Some(c), 1) if c.width <= 2 && c.height <= 2 && c.width > 0 && c.height > 0 => {
            (0..c.height).flat_map(|y| (0..c.width).map(move |x| (x, y, c.width, c.height, 0))).collect()
        }
        (Some(_), _) => return Err(FlatFieldError::Reference("flat-field correction supports Bayer (2×2) mosaics, not this CFA layout".into())),
        (None, 1 | 3) => (0..cpp).map(|p| (0, 0, 1, 1, p)).collect(),
        (None, _) => return Err(FlatFieldError::Reference(format!("flat-field correction doesn't support {cpp} samples per pixel"))),
    };
    let g = GRID as usize;
    if w < 4 * g || h < 4 * g {
        return Err(FlatFieldError::Reference(format!("the flat-field reference is too small ({w}×{h})")));
    }
    let mut ops = Vec::with_capacity(phases.len());
    for &(px, py, sx, sy, plane) in &phases {
        // per grid cell: sum of levels, of x and y (normalised to -1..1), count
        let mut cells = vec![(0f64, 0f64, 0f64, 0u32); g * g];
        let mut y = py;
        while y < h {
            // nearest grid cell: grid point i sits at i/(g-1) of the frame
            let gy = ((y as f64 / h as f64) * (g - 1) as f64).round() as usize;
            let ny = 2.0 * y as f64 / h as f64 - 1.0;
            let mut x = px;
            while x < w {
                let gx = ((x as f64 / w as f64) * (g - 1) as f64).round() as usize;
                if let Some(&v) = n.data.get((y * w + x) * cpp + plane)
                    && v.is_finite()
                    && v > 0.0
                    && v < CLIP
                    && let Some(c) = cells.get_mut(gy * g + gx)
                {
                    *c = (c.0 + v as f64, c.1 + 2.0 * x as f64 / w as f64 - 1.0, c.2 + ny, c.3 + 1);
                }
                x += sx;
            }
            y += sy;
        }
        // a low-order surface through the cell means (at their samples' centroids) is the smooth map: it ignores
        // noise and dust, and fills cells without usable samples
        let pts: Vec<(f64, f64, f64)> = cells.iter().filter(|c| c.3 > 0).map(|c| (c.1 / c.3 as f64, c.2 / c.3 as f64, c.0 / c.3 as f64)).collect();
        let coef = fit_surface(&pts)
            .ok_or_else(|| FlatFieldError::Reference("the flat-field reference has too few usable samples (black or clipped)".into()))?;
        let grid: Vec<f64> = (0..g * g)
            .map(|i| {
                let t = |k: usize| 2.0 * k as f64 / (g - 1) as f64 - 1.0;
                eval_surface(&coef, t(i % g), t(i / g))
            })
            .collect();
        let centre = grid.get((g / 2) * g + g / 2).copied().unwrap_or(0.0);
        if !(centre.is_finite() && centre > 1e-4) {
            return Err(FlatFieldError::Reference("the flat-field reference's centre is black".into()));
        }
        let gains = grid.iter().map(|&l| if l.is_finite() && l > 0.0 { ((centre / l) as f32).clamp(MIN_GAIN, MAX_GAIN) } else { 1.0 }).collect();
        let as_u32 = |v: usize| u32::try_from(v).unwrap_or(u32::MAX);
        ops.push(Opcode::GainMap {
            area: Area {
                top: as_u32(py),
                left: as_u32(px),
                bottom: as_u32(h),
                right: as_u32(w),
                plane: as_u32(plane),
                planes: 1,
                row_pitch: as_u32(sy),
                col_pitch: as_u32(sx),
            },
            points_v: GRID,
            points_h: GRID,
            spacing: [1.0 / (GRID - 1) as f64, 1.0 / (GRID - 1) as f64],
            origin: [0.0, 0.0],
            map_planes: 1,
            gains,
        });
    }
    Ok(FlatField { layout, ops })
}

/// Monomials x^i y^j with i + j <= [`DEGREE`].
const DEGREE: usize = 4;
const TERMS: usize = (DEGREE + 1) * (DEGREE + 2) / 2;

fn monomials(x: f64, y: f64) -> [f64; TERMS] {
    let mut m = [0.0; TERMS];
    let mut k = 0;
    for d in 0..=DEGREE {
        for j in 0..=d {
            if let Some(v) = m.get_mut(k) {
                *v = x.powi((d - j) as i32) * y.powi(j as i32);
            }
            k += 1;
        }
    }
    m
}

fn eval_surface(c: &[f64; TERMS], x: f64, y: f64) -> f64 {
    monomials(x, y).iter().zip(c).map(|(m, c)| m * c).sum()
}

/// Least-squares polynomial surface through `(x, y, z)` points (normal equations, Gaussian elimination with partial
/// pivoting); `None` when there are too few points or the system is singular.
fn fit_surface(pts: &[(f64, f64, f64)]) -> Option<[f64; TERMS]> {
    if pts.len() < 2 * TERMS {
        return None;
    }
    let mut a = [[0f64; TERMS + 1]; TERMS];
    for &(x, y, z) in pts {
        let m = monomials(x, y);
        for (r, row) in a.iter_mut().enumerate() {
            let mr = m.get(r).copied().unwrap_or(0.0);
            for (c, v) in row.iter_mut().enumerate().take(TERMS) {
                *v += mr * m.get(c).copied().unwrap_or(0.0);
            }
            if let Some(v) = row.get_mut(TERMS) {
                *v += mr * z;
            }
        }
    }
    for col in 0..TERMS {
        let piv = (col..TERMS).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))?;
        a.swap(col, piv);
        let d = a[col][col];
        if d.is_nan() || d.abs() <= 1e-12 {
            return None;
        }
        for r in 0..TERMS {
            if r != col {
                let f = a[r][col] / d;
                for c in col..=TERMS {
                    a[r][c] -= f * a[col][c];
                }
            }
        }
    }
    let mut out = [0f64; TERMS];
    for (i, o) in out.iter_mut().enumerate() {
        *o = a[i][TERMS] / a[i][i];
        if !o.is_finite() {
            return None;
        }
    }
    Some(out)
}

/// Add `ff`'s gain maps to `target`'s `OpcodeList2`, after checking it has the reference's sensor layout.
pub fn apply(ff: &FlatField, target: &mut RawImage) -> Result<(), FlatFieldError> {
    let t = Layout::of(target);
    let r = &ff.layout;
    if (t.width, t.height) != (r.width, r.height) {
        return Err(FlatFieldError::Mismatch(format!(
            "its sensor area is {}×{}, the flat-field reference's is {}×{}",
            t.width, t.height, r.width, r.height
        )));
    }
    if t.cpp != r.cpp || t.cfa != r.cfa {
        return Err(FlatFieldError::Mismatch("its colour filter layout differs from the flat-field reference's".into()));
    }
    target.opcodes.list2.extend(ff.ops.iter().cloned());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BlackLevel, OpcodeLists, RawData, RawFormat, Rect};

    /// A synthetic RGGB mosaic: `level(x, y, colour)` in [0, 1] over a 12-bit range with black 256.
    fn mosaic(w: usize, h: usize, level: impl Fn(usize, usize, u8) -> f64) -> RawImage {
        let cfa = Cfa::bayer("RGGB").unwrap();
        let data = (0..h)
            .flat_map(|y| (0..w).map(move |x| (x, y)))
            .map(|(x, y)| (256.0 + level(x, y, cfa.color_at(x, y)) * 3839.0).round() as u16)
            .collect();
        let full = Rect::new(0, 0, w, h);
        RawImage {
            format: RawFormat::Dng,
            width: w,
            height: h,
            cpp: 1,
            data: RawData::U16(data),
            cfa: Some(cfa.clone()),
            bits: 12,
            black: BlackLevel::uniform(256.0),
            white: vec![4095.0],
            active_area: full,
            crop: full,
            orientation: lcc_geom::Orientation::Normal,
            color: crate::ColorData::default(),
            wb_multipliers: None,
            linearized: false,
            pixel_aspect: 1.0,
            opcodes: OpcodeLists::default(),
            metadata: Default::default(),
            lens_metadata: Default::default(),
        }
    }

    /// Vignetting falling off to ~45 % in the corners, stronger in blue (a colour cast), with a little noise.
    fn vignette(w: usize, h: usize, x: usize, y: usize, c: u8) -> f64 {
        let (dx, dy) = (x as f64 / w as f64 - 0.5, y as f64 / h as f64 - 0.5);
        let r2 = (dx * dx + dy * dy) / 0.5;
        let k = [0.5, 0.55, 0.65][c as usize];
        1.0 - k * r2 + if (x * 7 + y * 13).is_multiple_of(5) { 0.004 } else { 0.0 }
    }

    #[test]
    fn written_gain_map_flattens_a_vignetted_frame_after_decode() {
        let (w, h) = (256, 192);
        let reference = mosaic(w, h, |x, y, c| 0.6 * vignette(w, h, x, y, c));
        let ff = fit(&reference).unwrap();
        assert_eq!(ff.ops.len(), 4, "one map per Bayer site");
        // a target of an even scene at a different exposure
        let mut target = mosaic(w, h, |x, y, c| 0.3 * vignette(w, h, x, y, c));
        let before = target.normalized().unwrap();
        apply(&ff, &mut target).unwrap();
        let dng = crate::write_dng(&target, &Default::default()).unwrap();
        let back = crate::decode(&dng).unwrap();
        let n = back.normalized().unwrap();
        let cfa = n.cfa.clone().unwrap();
        let spread = |d: &[f32]| {
            let mut worst = 0f32;
            for c in 0..3u8 {
                let v: Vec<f32> =
                    (0..h).flat_map(|y| (0..w).map(move |x| (x, y))).filter(|&(x, y)| cfa.color_at(x, y) == c).map(|(x, y)| d[y * w + x]).collect();
                let (lo, hi) = v.iter().fold((f32::MAX, f32::MIN), |(a, b), &x| (a.min(x), b.max(x)));
                worst = worst.max((hi - lo) / hi);
            }
            worst
        };
        assert!(spread(&before.data) > 0.3, "the frame was vignetted");
        assert!(spread(&n.data) < 0.06, "corrected to flat within tolerance: {}", spread(&n.data));
        // the centre keeps its level
        let c = n.data[(h / 2) * w + w / 2];
        assert!((c - before.data[(h / 2) * w + w / 2]).abs() < 0.01);
    }

    #[test]
    fn mismatched_layouts_are_refused() {
        let reference = mosaic(128, 96, |_, _, _| 0.5);
        let ff = fit(&reference).unwrap();
        let mut smaller = mosaic(64, 96, |_, _, _| 0.5);
        assert!(matches!(apply(&ff, &mut smaller), Err(FlatFieldError::Mismatch(_))));
        let mut other_cfa = mosaic(128, 96, |_, _, _| 0.5);
        other_cfa.cfa = Cfa::bayer("BGGR");
        assert!(matches!(apply(&ff, &mut other_cfa), Err(FlatFieldError::Mismatch(_))));
        assert!(other_cfa.opcodes.list2.is_empty(), "nothing added on refusal");
    }

    #[test]
    fn hostile_references_are_errors_not_panics() {
        assert!(fit(&mosaic(128, 96, |_, _, _| 0.0)).is_err(), "black");
        assert!(fit(&mosaic(128, 96, |_, _, _| 1.0)).is_err(), "clipped");
        assert!(fit(&mosaic(8, 8, |_, _, _| 0.5)).is_err(), "tiny");
        let mut xt = mosaic(128, 96, |_, _, _| 0.5);
        xt.cfa = Some(Cfa::xtrans());
        assert!(fit(&xt).is_err(), "X-Trans");
        // holes (a clipped half) are filled, gains stay finite and clamped
        let half = fit(&mosaic(128, 96, |x, _, _| if x < 64 { 1.0 } else { 0.5 })).unwrap();
        for op in &half.ops {
            let Opcode::GainMap { gains, .. } = op else { panic!() };
            assert!(gains.iter().all(|g| g.is_finite() && (MIN_GAIN..=MAX_GAIN).contains(g)));
        }
    }
}
