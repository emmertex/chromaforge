// Built on RADcamProfiles' DCP builder (crates/engine/src/dcp.rs, commit dd8003f), Copyright (c) 2026 RAD Films -
// Roman Alurkoff, MIT; relicensed MIT OR Apache-2.0 by its author. Writes the app's DCP model instead of its own codec.
//! A fitted [`Profile`] as a DNG Camera Profile, in the app's own DCP model
//! ([`lcc_raw::dcp::Dcp`], written by [`lcc_raw::dcp::write`]).
//!
//! A DNG renderer takes a raw file through a profile as follows (DNG specification 1.6, chapter 6):
//!
//! ```text
//! camera RGB --(neutral from the white balance)--> balanced camera RGB
//!   --ForwardMatrix--> XYZ D50 --> linear ProPhoto, clipped to [0, 1]
//!   --HueSatMap--> --exposure--> --LookTable--> --ProfileToneCurve--> output
//! ```
//!
//! This module fills in `ColorMatrix1`, `ForwardMatrix1` and a 2.5D `ProfileHueSatMap` (one value
//! division: the correction is exposure invariant) for the chart's illuminant. No tone curve is
//! written: the app keeps the photo's own contrast for a profile without one.

use lcc_raw::dcp::Dcp;
use lcc_raw::profile::{HsvTable, ProfileLook};

use crate::color::{bradford_map_white, prophoto_to_xyz};
use crate::illuminants::ExifLightSource;
use crate::linalg::{Mat3, Vec3};
use crate::profile::Profile;
use crate::target::dng_d50;

/// Default `HueSatMap` divisions: hue, saturation, value.
pub const HUE_SAT_DIMS: [usize; 3] = [90, 30, 1];

/// Why a DCP could not be built.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DcpBuildError {
    /// A matrix that must be inverted is singular, or a value is not finite.
    Degenerate(String),
}

impl core::fmt::Display for DcpBuildError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Degenerate(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for DcpBuildError {}

/// The DNG SDK's RGB to HSV: hue in `[0, 6)`, saturation and value in `[0, 1]` for in-gamut values.
pub fn dng_rgb_to_hsv(rgb: Vec3) -> (f64, f64, f64) {
    let (r, g, b) = (rgb[0], rgb[1], rgb[2]);
    let v = r.max(g).max(b);
    let gap = v - r.min(g).min(b);
    if gap > 0.0 && v > 0.0 {
        let mut h = if r == v {
            (g - b) / gap
        } else if g == v {
            2.0 + (b - r) / gap
        } else {
            4.0 + (r - g) / gap
        };
        if h < 0.0 {
            h += 6.0;
        }
        (h, gap / v, v)
    } else {
        (0.0, 0.0, v)
    }
}

/// The DNG SDK's HSV to RGB, the inverse of [`dng_rgb_to_hsv`].
pub fn dng_hsv_to_rgb(h: f64, s: f64, v: f64) -> Vec3 {
    if s <= 0.0 {
        return Vec3::new(v, v, v);
    }
    let h = h.rem_euclid(6.0);
    let i = (h.floor() as i32).clamp(0, 5);
    let f = h - f64::from(i);
    let p = v * (1.0 - s);
    let q = v * (1.0 - s * f);
    let t = v * (1.0 - s * (1.0 - f));
    match i {
        0 => Vec3::new(v, t, p),
        1 => Vec3::new(q, v, p),
        2 => Vec3::new(p, v, t),
        3 => Vec3::new(p, q, v),
        4 => Vec3::new(t, p, v),
        _ => Vec3::new(v, p, q),
    }
}

/// Any exposure-invariant colour mapping as a DNG hue/saturation map: `map` takes the colour the
/// forward matrix produces (XYZ relative to D50) to the colour the profile should produce instead.
/// Node `(h, s)` sits at hue `h × 360 / hues` degrees and saturation `s / (sats − 1)` of linear
/// ProPhoto HSV, where the DNG SDK reads it; grey is left alone, as the specification requires.
pub fn hue_sat_map_from_mapping(map: &dyn Fn(Vec3) -> Vec3, dims: [usize; 3]) -> Result<HsvTable, DcpBuildError> {
    let [hues, sats, vals] = dims;
    if hues == 0 || sats < 2 || vals == 0 || hues > 360 || sats > 256 || vals > 64 {
        return Err(DcpBuildError::Degenerate(format!("invalid hue/saturation map size {dims:?}")));
    }
    let to_xyz = prophoto_to_xyz();
    let from_xyz = to_xyz.inverted_or_identity();
    let mut data = Vec::with_capacity(hues * sats * vals);
    for v in 0..vals {
        // the correction does not depend on exposure: every value level gives the same entry
        let value = if vals == 1 { 1.0 } else { (v as f64 / (vals - 1) as f64).max(0.05) };
        for h in 0..hues {
            let hue = h as f64 * 6.0 / hues as f64;
            for s in 0..sats {
                if s == 0 {
                    data.push([0.0, 1.0, 1.0]);
                    continue;
                }
                let sat = s as f64 / (sats - 1) as f64;
                let rgb = dng_hsv_to_rgb(hue, sat, value);
                let corrected = from_xyz * map(to_xyz * rgb);
                let (h2, s2, v2) = dng_rgb_to_hsv(corrected);
                let mut shift = h2 - hue;
                if shift > 3.0 {
                    shift -= 6.0;
                } else if shift < -3.0 {
                    shift += 6.0;
                }
                let entry = [(shift * 60.0) as f32, (s2 / sat) as f32, (v2 / value) as f32];
                // a colour the mapping cannot place keeps its own (identity entry)
                data.push(if entry.iter().all(|x| x.is_finite()) { [entry[0], entry[1].max(0.0), entry[2].max(0.0)] } else { [0.0, 1.0, 1.0] });
            }
        }
    }
    Ok(HsvTable { hue_divisions: hues, sat_divisions: sats, val_divisions: vals, data, srgb_value: false })
}

fn to_lc(m: &Mat3) -> Result<lcc_color::Mat3, DcpBuildError> {
    let rows = [m[0], m[1], m[2]];
    if rows.iter().flatten().any(|v| !v.is_finite()) {
        return Err(DcpBuildError::Degenerate("the fitted matrix is not finite".into()));
    }
    Ok(lcc_color::Mat3(rows))
}

/// The DCP of `prof` for the camera `camera_model` (`UniqueCameraModel`), named `name`.
pub fn build(prof: &Profile, camera_model: &str, name: &str) -> Result<Dcp, DcpBuildError> {
    let illuminant = ExifLightSource::from_name(prof.illuminant_name).unwrap_or(ExifLightSource::D50);
    // With a hue/saturation map the map carries the profile's whole rendering, measured from the
    // written forward matrix, so that matrix is the one that keeps every camera colour inside
    // ProPhoto (`Profile::dcp_forward_matrix`). Without one the forward matrix is the rendering.
    let writes_map = prof.lut.as_ref().is_some_and(|t| !t.is_noop(1.0e-4) || prof.free.is_some());
    let (base, color_matrix) = match prof.dcp_forward_matrix {
        Some(base) if writes_map => (base, prof.color_matrix_for(&base)),
        _ => (prof.forward_matrix, prof.color_matrix),
    };
    // the forward matrix is white-locked to D50; the DNG SDK states its D50 as a chromaticity, so
    // adapt the matrix onto that exact white rather than let the renderer rescale its rows
    let white = base * Vec3::ONE;
    let adapt = bradford_map_white(white, dng_d50());
    let forward_matrix = adapt * base;
    let hue_sat = if writes_map {
        let to_camera = forward_matrix.inverted().ok_or_else(|| DcpBuildError::Degenerate("the forward matrix is singular".into()))?;
        let mapping = |xyz: Vec3| adapt * prof.render_balanced(to_camera * xyz);
        Some(hue_sat_map_from_mapping(&mapping, HUE_SAT_DIMS)?)
    } else {
        None
    };
    let mut color = lcc_raw::ColorData {
        illuminant: [u16::from(illuminant.tag()), 0],
        color_matrix: [Some(to_lc(&color_matrix)?), None],
        forward_matrix: [Some(to_lc(&forward_matrix)?), None],
        ..Default::default()
    };
    color.profile = ProfileLook { hue_sat_map: [hue_sat, None], ..Default::default() };
    Ok(Dcp { model: camera_model.to_string(), name: name.to_string(), copyright: String::new(), color })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hsv_round_trips() {
        for &(r, g, b) in &[(0.2, 0.5, 0.9), (0.9, 0.1, 0.3), (0.4, 0.4, 0.4), (0.0, 1.0, 0.5)] {
            let (h, s, v) = dng_rgb_to_hsv(Vec3::new(r, g, b));
            let back = dng_hsv_to_rgb(h, s, v);
            assert!((back[0] - r).abs() < 1e-9 && (back[1] - g).abs() < 1e-9 && (back[2] - b).abs() < 1e-9);
        }
    }

    #[test]
    fn the_identity_mapping_is_an_identity_map() {
        let map = hue_sat_map_from_mapping(&|xyz| xyz, [12, 5, 1]).unwrap();
        assert_eq!(map.data.len(), 60);
        for e in &map.data {
            assert!(e[0].abs() < 1e-3 && (e[1] - 1.0).abs() < 1e-4 && (e[2] - 1.0).abs() < 1e-4, "{e:?}");
        }
        assert!(hue_sat_map_from_mapping(&|xyz| xyz, [0, 5, 1]).is_err());
        // a mapping that returns nonsense gives identity entries, not NaN
        let map = hue_sat_map_from_mapping(&|_| Vec3::new(f64::NAN, 0.0, 0.0), [4, 3, 1]).unwrap();
        assert!(map.data.iter().flatten().all(|v| v.is_finite()));
    }
}
