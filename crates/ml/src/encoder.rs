//! Vision + text [`Encoder`] trait and image preprocessing (`RgbImage` → normalized planar `CHW` tensor).

use crate::manifest::{InputSpec, ModelManifest};
use crate::queue::{Ctx, JobError};

/// Maximum source image width or height accepted by [`RgbImage`] (16384 px).
pub const MAX_IMAGE_DIM: u32 = 16_384;

/// Interleaved 8-bit sRGB image (`width * height * 3` bytes, row-major `RGBRGB…`) passed to an [`Encoder`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RgbImage {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
}

impl RgbImage {
    /// Construct a validated `RgbImage`.
    pub fn new(width: u32, height: u32, rgb: Vec<u8>) -> Result<Self, String> {
        if width == 0 || height == 0 || width > MAX_IMAGE_DIM || height > MAX_IMAGE_DIM {
            return Err(format!("invalid image dimensions {width}x{height}"));
        }
        let expected = (width as usize)
            .checked_mul(height as usize)
            .and_then(|px| px.checked_mul(3))
            .ok_or_else(|| format!("image dimensions {width}x{height} overflow"))?;
        if rgb.len() != expected {
            return Err(format!("RGB buffer length {} does not match {width}x{height}x3 ({expected})", rgb.len()));
        }
        Ok(Self { width, height, rgb })
    }
}

/// Resize the shortest edge of `img` to cover `(spec.width, spec.height)`, center-crop, and normalize to planar
/// `CHW` (`3 * spec.height * spec.width` `f32` values).
pub fn preprocess_image(img: &RgbImage, spec: &InputSpec) -> Result<Vec<f32>, JobError> {
    if img.width == 0 || img.height == 0 || img.width > MAX_IMAGE_DIM || img.height > MAX_IMAGE_DIM {
        return Err(JobError::Failed(format!("invalid source image dimensions {}x{}", img.width, img.height)));
    }
    let expected_src = (img.width as usize)
        .checked_mul(img.height as usize)
        .and_then(|n| n.checked_mul(3))
        .ok_or_else(|| JobError::Failed("source image size overflow".into()))?;
    if img.rgb.len() != expected_src {
        return Err(JobError::Failed(format!("source RGB buffer length {} does not match {}x{}x3", img.rgb.len(), img.width, img.height)));
    }
    let dst_w = spec.width as usize;
    let dst_h = spec.height as usize;
    if dst_w == 0 || dst_h == 0 || dst_w > 2048 || dst_h > 2048 {
        return Err(JobError::Failed(format!("invalid model input dimensions {}x{}", spec.width, spec.height)));
    }
    for ch in 0..3 {
        if !spec.mean[ch].is_finite() || !spec.std[ch].is_finite() || spec.std[ch] <= 0.0 {
            return Err(JobError::Failed("invalid normalization mean/std".into()));
        }
    }

    let src_w = img.width as f32;
    let src_h = img.height as f32;
    // Scale so that the shorter side matches the target crop while covering both dimensions.
    let scale = (dst_w as f32 / src_w).max(dst_h as f32 / src_h);
    let crop_src_w = (dst_w as f32 / scale).min(src_w);
    let crop_src_h = (dst_h as f32 / scale).min(src_h);
    let offset_x = ((src_w - crop_src_w) * 0.5).max(0.0);
    let offset_y = ((src_h - crop_src_h) * 0.5).max(0.0);

    let plane = dst_w * dst_h;
    let mut out = vec![0.0f32; plane * 3];
    let src_stride = (img.width as usize) * 3;
    let max_x = img.width.saturating_sub(1) as usize;
    let max_y = img.height.saturating_sub(1) as usize;

    for dy in 0..dst_h {
        let sy = offset_y + ((dy as f32 + 0.5) / dst_h as f32) * crop_src_h - 0.5;
        let sy_clamped = sy.clamp(0.0, max_y as f32);
        let y0 = (sy_clamped.floor() as usize).min(max_y);
        let y1 = (y0 + 1).min(max_y);
        let wy = sy_clamped - (y0 as f32);

        for dx in 0..dst_w {
            let sx = offset_x + ((dx as f32 + 0.5) / dst_w as f32) * crop_src_w - 0.5;
            let sx_clamped = sx.clamp(0.0, max_x as f32);
            let x0 = (sx_clamped.floor() as usize).min(max_x);
            let x1 = (x0 + 1).min(max_x);
            let wx = sx_clamped - (x0 as f32);

            let i00 = y0 * src_stride + x0 * 3;
            let i01 = y0 * src_stride + x1 * 3;
            let i10 = y1 * src_stride + x0 * 3;
            let i11 = y1 * src_stride + x1 * 3;
            let dst_idx = dy * dst_w + dx;

            for ch in 0..3 {
                let p00 = *img.rgb.get(i00 + ch).unwrap_or(&0) as f32;
                let p01 = *img.rgb.get(i01 + ch).unwrap_or(&0) as f32;
                let p10 = *img.rgb.get(i10 + ch).unwrap_or(&0) as f32;
                let p11 = *img.rgb.get(i11 + ch).unwrap_or(&0) as f32;
                let top = p00 + (p01 - p00) * wx;
                let bot = p10 + (p11 - p10) * wx;
                let val = (top + (bot - top) * wy) * (1.0 / 255.0);
                let norm = (val - spec.mean[ch]) / spec.std[ch];
                if let Some(slot) = out.get_mut(ch * plane + dst_idx) {
                    *slot = norm;
                }
            }
        }
    }
    Ok(out)
}

/// Shared interface for a vision + text embedding model run on [`crate::Queue`].
pub trait Encoder: Send + Sync {
    /// Model manifest describing the encoder's id, dimension, normalization and licence.
    fn manifest(&self) -> &ModelManifest;

    /// Versioned cache tag (`<id>@<sha256-prefix>`).
    fn tag(&self) -> String {
        self.manifest().tag()
    }

    /// Encode a batch of RGB images into unit-norm embeddings of length `manifest().embed_dim`.
    fn encode_images(&self, batch: &[RgbImage], ctx: &Ctx) -> Result<Vec<Vec<f32>>, JobError>;

    /// Encode a natural-language query into a unit-norm embedding in the same space as [`Self::encode_images`].
    fn encode_text(&self, text: &str, ctx: &Ctx) -> Result<Vec<f32>, JobError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preprocesses_and_normalizes_rgb_image() {
        let spec = InputSpec { width: 16, height: 16, mean: [0.5, 0.5, 0.5], std: [0.5, 0.5, 0.5] };
        let img = RgbImage::new(32, 24, vec![255u8; 32 * 24 * 3]).unwrap();
        let chw = preprocess_image(&img, &spec).unwrap();
        assert_eq!(chw.len(), 3 * 16 * 16);
        for v in chw {
            assert!((v - 1.0).abs() < 1e-5);
        }
    }

    #[test]
    fn rejects_corrupt_image_buffers() {
        let spec = InputSpec { width: 16, height: 16, mean: [0.5, 0.5, 0.5], std: [0.5, 0.5, 0.5] };
        let bad = RgbImage { width: 10, height: 10, rgb: vec![0u8; 12] };
        assert!(preprocess_image(&bad, &spec).is_err());
    }
}
