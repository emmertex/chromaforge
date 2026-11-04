//! The fixed, invertible transform a crop is sent to an image model with, and its inverse.
//!
//! Image models read and write display-referred 8-bit RGB. The develop pipeline's source is
//! scene-linear Rec.2020 developed for the camera's as-shot white (the app's raw decode: demosaic,
//! camera matrix at the as-shot white). A crop is sent as:
//!
//! 1. linear as-shot Rec.2020 (no user white balance, exposure, tone or colour edit),
//! 2. times one gain per generation ([`Neutral::gain`], set so the crop's bright end sits near
//!    white: dark crops are not sent black),
//! 3. converted to linear sRGB primaries (negative, out-of-gamut values clip to 0: such colours
//!    come back desaturated and are pulled back by [`crate::matching`]),
//! 4. through a flat logarithmic curve with two stops of headroom above that white
//!    (`E(x) = log2(1 + M·x) / log2(1 + M·H)`, `H` = 4), so highlights up to 4× the crop's
//!    white keep their gradation; above that they clip (and [`crate::matching`] cannot invent it),
//! 5. quantised to 8 bits.
//!
//! [`Neutral::decode`] runs the same steps backwards. Only the quantisation (8 bits over a log
//! curve: about 0.4 % steps in the mid-tones) and anything the model changes is lost. The look is
//! flat (like a camera's log profile); models continue such a picture in the same flat look, which
//! is what is wanted: it is turned back into scene-linear light.

use lcc_color::{Mat3, REC2020, SRGB};
use lcc_raster::Rgb32f;
use serde::{Deserialize, Serialize};

/// Linear values up to this multiple of the crop's white are encoded below 1.0 (two stops).
pub const HEADROOM: f32 = 4.0;
/// Curvature of the log curve: mid grey (0.18) encodes near 0.42, the crop's white near 0.74.
pub const CURVE: f32 = 45.0;

/// The transform of one generation (see the module docs).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Neutral {
    /// Linear gain applied before encoding.
    pub gain: f32,
}

impl Default for Neutral {
    fn default() -> Self {
        Neutral { gain: 1.0 }
    }
}

fn matrices() -> (Mat3, Mat3) {
    let to = REC2020.to_space(&SRGB);
    let from = to.inverse().unwrap_or(Mat3::IDENTITY);
    (to, from)
}

impl Neutral {
    /// The transform for a crop: its 99th-percentile luminance maps to 0.9 of the white (so a
    /// crop with a few specular highlights keeps them in the headroom). Gains stay within
    /// 1/64…256; an empty or black crop gets 1.
    pub fn for_crop(img: &Rgb32f) -> Neutral {
        let luma = lcc_color::LUMA_2020;
        let mut y: Vec<f32> =
            img.data.iter().map(|p| p[0] * luma[0] + p[1] * luma[1] + p[2] * luma[2]).filter(|v| v.is_finite() && *v > 0.0).collect();
        if y.is_empty() {
            return Neutral::default();
        }
        let k = (y.len() * 99 / 100).min(y.len() - 1);
        let (_, p99, _) = y.select_nth_unstable_by(k, |a, b| a.total_cmp(b));
        let gain = if *p99 > 1e-6 { 0.9 / *p99 } else { 1.0 };
        Neutral { gain: gain.clamp(1.0 / 64.0, 256.0) }
    }

    fn safe_gain(&self) -> f32 {
        if self.gain.is_finite() && self.gain > 0.0 { self.gain } else { 1.0 }
    }

    /// Encode one linear Rec.2020 pixel to 0..1 per channel (before quantisation).
    pub fn encode_f(&self, rgb: [f32; 3], to_srgb: &Mat3) -> [f32; 3] {
        let g = self.safe_gain();
        let v = to_srgb.apply_f32(rgb.map(|c| if c.is_finite() { c * g } else { 0.0 }));
        let norm = (1.0 + CURVE * HEADROOM).log2();
        v.map(|c| ((1.0 + CURVE * c.max(0.0)).log2() / norm).clamp(0.0, 1.0))
    }

    /// Decode one 0..1 pixel back to linear Rec.2020.
    pub fn decode_f(&self, e: [f32; 3], from_srgb: &Mat3) -> [f32; 3] {
        let norm = (1.0 + CURVE * HEADROOM).log2();
        let lin = e.map(|c| {
            let c = if c.is_finite() { c.clamp(0.0, 1.0) } else { 0.0 };
            ((c * norm).exp2() - 1.0) / CURVE
        });
        let g = self.safe_gain();
        from_srgb.apply_f32(lin).map(|c| (c / g).max(0.0))
    }

    /// The crop as interleaved 8-bit RGB (`w × h × 3`), ready for a PNG.
    pub fn encode(&self, img: &Rgb32f) -> Vec<u8> {
        let (to, _) = matrices();
        img.data.iter().flat_map(|p| self.encode_f(*p, &to).map(|c| (c * 255.0 + 0.5) as u8)).collect()
    }

    /// Interleaved 8-bit RGB (`w × h × 3`) back to linear Rec.2020; `None` when the length is not
    /// `w × h × 3`.
    pub fn decode(&self, w: usize, h: usize, rgb8: &[u8]) -> Option<Rgb32f> {
        if Some(rgb8.len()) != w.checked_mul(h).and_then(|n| n.checked_mul(3)) {
            return None;
        }
        let (_, from) = matrices();
        let (px, _) = rgb8.as_chunks::<3>();
        Some(Rgb32f { width: w, height: h, data: px.iter().map(|p| self.decode_f(p.map(|c| f32::from(c) / 255.0), &from)).collect() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_transform_round_trips_within_quantisation() {
        let n = Neutral { gain: 2.0 };
        let (to, from) = matrices();
        // neutral greys and mild colours from deep shadow to two stops over the crop's white (0.5 after the gain)
        for v in [0.001f32, 0.01, 0.05, 0.09, 0.2, 0.45, 1.0, 1.75] {
            for tint in [[1.0, 1.0, 1.0], [1.1, 1.0, 0.85], [0.8, 0.95, 1.1]] {
                let p = [v * tint[0], v * tint[1], v * tint[2]];
                let e = n.encode_f(p, &to).map(|c| (c * 255.0 + 0.5).floor() / 255.0);
                let back = n.decode_f(e, &from);
                for c in 0..3 {
                    let rel = (back[c] - p[c]).abs() / p[c].max(0.002);
                    assert!(rel < 0.05, "{p:?} → {back:?}");
                }
            }
        }
        // mid grey sits near 0.42 after the gain, the white near 0.74, the headroom ends at 1
        let grey = n.encode_f([0.09; 3], &to)[1];
        assert!((grey - 0.42).abs() < 0.02, "{grey}");
        assert!((n.encode_f([2.0; 3], &to)[1] - 1.0).abs() < 1e-3);
        assert!(n.encode_f([100.0; 3], &to)[1] <= 1.0);
    }

    #[test]
    fn hostile_values_do_not_escape() {
        let (to, from) = matrices();
        for g in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            let n = Neutral { gain: g };
            assert!(n.encode_f([f32::NAN, -1.0, f32::INFINITY], &to).iter().all(|c| (0.0..=1.0).contains(c)));
            assert!(n.decode_f([f32::NAN, 2.0, -1.0], &from).iter().all(|c| c.is_finite() && *c >= 0.0));
        }
        assert!(Neutral::default().decode(2, 2, &[0; 11]).is_none());
        assert_eq!(Neutral::for_crop(&Rgb32f::new(4, 4)), Neutral::default());
    }

    #[test]
    fn a_crop_gets_a_gain_that_puts_its_bright_end_near_white() {
        let img = Rgb32f::from_fn(100, 10, |x, _| [x as f32 / 1000.0; 3]);
        let n = Neutral::for_crop(&img);
        assert!((n.gain * 0.099 - 0.9).abs() < 0.05, "{n:?}");
        let enc = n.encode(&img);
        assert_eq!(enc.len(), 3000);
        let back = n.decode(100, 10, &enc).unwrap();
        assert!((back.get(50, 0)[1] - 0.05).abs() < 0.002);
    }
}
