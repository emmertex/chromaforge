//! Half-float pixel storage for the HDR merge's frames.
//!
//! A bracket's frames are held for the whole merge; at 32-bit float a 67 MP frame took 0.8 GB (plus 0.27 GB for its
//! saturation plane). IEEE half floats keep 11 significant bits (a relative step of 2⁻¹¹ ≈ 0.05 %, below the shot
//! noise of any photosite) and, with subnormals, values down to 6·10⁻⁸ — far below one step of a 16-bit raw — so a
//! frame held this way merges to the same radiance in half the memory. The merged result itself stays 32-bit.

use half::f16;
use lcc_raster::Rgb32f;
use rayon::prelude::*;

/// An RGB image stored as half floats (6 bytes a pixel instead of 12).
#[derive(Clone, Debug, Default)]
pub struct HalfImage {
    pub width: usize,
    pub height: usize,
    data: Vec<[f16; 3]>,
}

impl HalfImage {
    /// `img` rounded to half floats. Values past the half range (65504) saturate at it, NaN becomes 0.
    pub fn from_rgb32f(img: &Rgb32f) -> HalfImage {
        let data = img.data.par_iter().map(|p| p.map(to_half)).collect();
        HalfImage { width: img.width, height: img.height, data }
    }

    /// Pixel `i` (row-major) as 32-bit floats; black outside the image.
    #[inline]
    pub fn get(&self, i: usize) -> [f32; 3] {
        self.data.get(i).map_or([0.0; 3], |p| p.map(f16::to_f32))
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// The image at 32-bit float again (for the steps that work on one frame at a time).
    pub fn to_rgb32f(&self) -> Rgb32f {
        let mut out = Rgb32f::new(self.width, self.height);
        out.data.par_iter_mut().zip(self.data.par_iter()).for_each(|(o, p)| *o = p.map(f16::to_f32));
        out
    }

    /// Each pixel through `f` (at 32-bit precision), stored as half floats again.
    pub fn map(&self, f: impl Fn([f32; 3]) -> [f32; 3] + Sync) -> HalfImage {
        let data = self.data.par_iter().map(|p| f(p.map(f16::to_f32)).map(to_half)).collect();
        HalfImage { width: self.width, height: self.height, data }
    }

    /// Bytes the pixels take.
    pub fn bytes(&self) -> usize {
        self.data.len() * std::mem::size_of::<[f16; 3]>()
    }
}

/// A plane of half floats (the merge's per-frame saturation map).
#[derive(Clone, Debug, Default)]
pub struct HalfPlane(Vec<f16>);

impl HalfPlane {
    pub fn from_f32(v: &[f32]) -> HalfPlane {
        HalfPlane(v.par_iter().map(|&x| to_half(x)).collect())
    }

    #[inline]
    pub fn get(&self, i: usize) -> f32 {
        self.0.get(i).map_or(0.0, |v| v.to_f32())
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn to_f32(&self) -> Vec<f32> {
        self.0.par_iter().map(|v| v.to_f32()).collect()
    }
}

#[inline]
fn to_half(v: f32) -> f16 {
    if v.is_nan() { f16::ZERO } else { f16::from_f32(v.clamp(-65504.0, 65504.0)) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn half_storage_keeps_raw_precision_and_halves_the_bytes() {
        let img = Rgb32f::from_fn(64, 64, |x, y| [(x * 64 + y) as f32 / 4096.0, 1.0 / 16383.0, (x as f32 + 1.0) * 3.7]);
        let h = HalfImage::from_rgb32f(&img);
        assert_eq!(h.bytes() * 2, img.data.len() * std::mem::size_of::<[f32; 3]>());
        for (i, p) in img.data.iter().enumerate() {
            let q = h.get(i);
            for c in 0..3 {
                assert!((q[c] - p[c]).abs() <= p[c].abs() * 5e-4 + 1e-7, "{} vs {}", q[c], p[c]);
            }
        }
        // hostile values: no panic, NaN to 0, out of range saturates, out of bounds is black
        let bad = Rgb32f::from_fn(2, 1, |x, _| if x == 0 { [f32::NAN, f32::INFINITY, 1e9] } else { [-1e9, 0.0, 0.5] });
        let h = HalfImage::from_rgb32f(&bad);
        assert_eq!(h.get(0), [0.0, 65504.0, 65504.0]);
        assert_eq!(h.get(1)[0], -65504.0);
        assert_eq!(h.get(99), [0.0; 3]);
        assert_eq!(HalfPlane::from_f32(&[f32::NAN]).get(0), 0.0);
    }
}
