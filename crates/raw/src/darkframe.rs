//! Dark frames: a raw shot with the lens cap on (or the body cap), at the settings a photographer uses. It shows
//! the sensor alone: its black offset, read noise, row/column banding, and every hot or stuck pixel at that
//! exposure and ISO, at once — no photos of different scenes are needed to tell them from detail.
//!
//! [`analyze`] measures one frame into a [`DarkFrame`], a small JSON-serialisable record (format
//! [`FORMAT_VERSION`], documented in `docs/sensor-defects.md`). Camera bodies' pixel maps take its `hot` list as
//! confirmed defects; dark-frame noise calibration (for denoise) can take the noise figures from the same record.
//! Values are normalised: 0 = the file's black level, 1 = its white level.

use crate::RawImage;
use crate::defects::{Defect, DefectKind, View};
use serde::{Deserialize, Serialize};

/// Version of the [`DarkFrame`] record.
pub const FORMAT_VERSION: u32 = 1;
/// At most this many hot pixels are listed (strongest first); a frame with more is not dark.
pub const MAX_HOT: usize = 100_000;

/// What one dark frame says about its sensor.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DarkFrame {
    pub version: u32,
    pub make: Option<String>,
    pub model: Option<String>,
    /// Body serial number, when the file has one.
    pub serial: Option<String>,
    /// Full data size (the hot list's coordinates are in it) and the active area `[x, y, width, height]`.
    pub sensor: [usize; 2],
    pub active: [usize; 4],
    /// CFA layout name (`RGGB`, …; 36 letters for X-Trans), `None` for linear or monochrome data.
    pub cfa: Option<String>,
    pub iso: Option<u32>,
    /// Seconds.
    pub exposure: Option<f64>,
    /// ISO 8601.
    pub captured: Option<String>,
    /// White − black in stored units, to turn the normalised figures back into raw numbers.
    pub range: f32,
    /// Median level per colour (red, green, blue) above the file's black level (its true black offset).
    pub offset: [f32; 3],
    /// Read noise sigma per colour.
    pub noise: [f32; 3],
    /// Sigma of the row means and of the column means (green samples): horizontal and vertical banding.
    pub row_noise: f32,
    pub column_noise: f32,
    /// Hot and stuck pixels: above `offset + max(10 sigma, 0.004)` of their colour.
    pub hot: Vec<Defect>,
}

/// Robust centre and sigma (median, 1.4826 × median absolute deviation) of `v` (reordered).
fn robust(v: &mut [f32]) -> (f32, f32) {
    if v.is_empty() {
        return (0.0, 0.0);
    }
    let mid = v.len() / 2;
    let med = *v.select_nth_unstable_by(mid, f32::total_cmp).1;
    for x in v.iter_mut() {
        *x = (*x - med).abs();
    }
    let mad = *v.select_nth_unstable_by(mid, f32::total_cmp).1;
    (med, mad * 1.4826)
}

fn std_dev(v: &[f32]) -> f32 {
    if v.len() < 2 {
        return 0.0;
    }
    let m = v.iter().sum::<f32>() / v.len() as f32;
    (v.iter().map(|x| (x - m) * (x - m)).sum::<f32>() / (v.len() - 1) as f32).sqrt()
}

/// Whether `raw` looks like a dark frame: its typical sample sits at the black level and almost nothing is lit.
pub fn is_dark(raw: &RawImage) -> bool {
    let Some(view) = View::new(raw) else { return false };
    let a = view.area;
    let mut v = Vec::new();
    let step = ((a.width * a.height) / 200_000).max(1);
    let mut i = 0usize;
    while i < a.width * a.height {
        let (x, y) = (a.x + i % a.width, a.y + i / a.width);
        v.push(view.val(x, y, 0));
        i += step;
    }
    if v.is_empty() {
        return false;
    }
    v.sort_by(f32::total_cmp);
    let q = |p: f64| v.get(((v.len() - 1) as f64 * p) as usize).copied().unwrap_or(1.0);
    q(0.5) < 0.01 && q(0.99) < 0.03
}

/// Measure a dark frame. `Err` when the data can't be read or the frame isn't dark ([`is_dark`]).
pub fn analyze(raw: &RawImage) -> Result<DarkFrame, String> {
    if !is_dark(raw) {
        return Err("not a dark frame: shoot with the lens cap on (and the viewfinder covered)".into());
    }
    let view = View::new(raw).ok_or("the raw data is too small or damaged")?;
    let a = view.area;
    // per colour: robust level and sigma from two rows in eight (both rows of a Bayer pair)
    let mut per: [Vec<f32>; 3] = Default::default();
    let (mut rows, mut cols) = (Vec::new(), vec![(0.0f64, 0usize); a.width]);
    for y in (a.y..a.y + a.height).filter(|y| y % 8 < 2) {
        let (mut sum, mut n) = (0.0f64, 0usize);
        for x in a.x..a.x + a.width {
            for s in 0..view.cpp {
                let c = view.colour(x, y, s);
                let v = view.val(x, y, s);
                if let Some(p) = per.get_mut(c) {
                    p.push(v);
                }
                if c == 1 {
                    sum += f64::from(v);
                    n += 1;
                    if let Some(col) = cols.get_mut(x - a.x) {
                        col.0 += f64::from(v);
                        col.1 += 1;
                    }
                }
            }
        }
        if n > 0 {
            rows.push((sum / n as f64) as f32);
        }
    }
    let mut offset = [0.0f32; 3];
    let mut noise = [0.0f32; 3];
    for (c, p) in per.iter_mut().enumerate() {
        let (m, s) = robust(p);
        offset[c] = m;
        noise[c] = s;
    }
    let cols: Vec<f32> = cols.iter().filter(|c| c.1 > 0).map(|c| (c.0 / c.1 as f64) as f32).collect();
    let mut hot = Vec::new();
    for y in a.y..a.y + a.height {
        for x in a.x..a.x + a.width {
            for s in 0..view.cpp {
                let c = view.colour(x, y, s);
                let v = view.val(x, y, s);
                let (o, n) = (offset.get(c).copied().unwrap_or(0.0), noise.get(c).copied().unwrap_or(0.0));
                if v > o + (10.0 * n).max(0.004) {
                    let kind = if v >= 0.97 { DefectKind::Stuck } else { DefectKind::Hot };
                    hot.push(Defect { x: x as u32, y: y as u32, s: s as u8, kind, level: v - o });
                    if hot.len() > MAX_HOT {
                        return Err(format!("more than {MAX_HOT} lit pixels: not a dark frame"));
                    }
                }
            }
        }
    }
    hot.sort_by(|p, q| q.level.total_cmp(&p.level));
    let m = &raw.metadata;
    Ok(DarkFrame {
        version: FORMAT_VERSION,
        make: m.make.clone(),
        model: m.model.clone(),
        serial: m.serial_number.clone().filter(|s| !s.trim().is_empty()),
        sensor: [raw.width, raw.height],
        active: [a.x, a.y, a.width, a.height],
        cfa: raw.cfa.as_ref().map(|c| c.name()),
        iso: m.iso,
        exposure: m.exposure_time,
        captured: m.capture_time.map(|t| t.to_iso()),
        range: view.scale.first().map_or(1.0, |s| 1.0 / s),
        offset,
        noise,
        row_noise: std_dev(&rows),
        column_noise: std_dev(&cols),
        hot,
    })
}
