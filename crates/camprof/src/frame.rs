//! The picture a chart is found and measured on: camera-native RGB at half the sensor's
//! resolution (one value per 2 × 2 colour-filter unit), black removed, 1.0 the white level, not
//! white balanced and not clipped.
//!
//! The engine never reads a raw file itself: the caller (the app's raw decoders) builds the
//! frame, so every format the app opens can be profiled.

use crate::linalg::Vec3;

/// The largest frame accepted, in pixels (a 400 MP sensor at half resolution is 100 MP).
pub const MAX_PIXELS: usize = 120_000_000;

/// Why a frame could not be built.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FrameError {
    /// Width or height is zero.
    Empty,
    /// The sample count does not match the size.
    Size { expected: usize, got: usize },
    /// Larger than [`MAX_PIXELS`].
    TooLarge,
}

impl core::fmt::Display for FrameError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Empty => write!(f, "the frame has no pixels"),
            Self::Size { expected, got } => write!(f, "the frame should have {expected} pixels but has {got}"),
            Self::TooLarge => write!(f, "the frame is larger than {MAX_PIXELS} pixels"),
        }
    }
}

impl std::error::Error for FrameError {}

/// Camera-native linear RGB, row by row from the top left.
#[derive(Clone, Debug, PartialEq)]
pub struct RawFrame {
    /// Pixels across.
    pub width: usize,
    /// Pixels down.
    pub height: usize,
    data: Vec<[f32; 3]>,
}

impl RawFrame {
    /// A frame from `width × height` camera RGB values.
    pub fn from_rgb(width: usize, height: usize, data: Vec<[f32; 3]>) -> Result<Self, FrameError> {
        if width == 0 || height == 0 {
            return Err(FrameError::Empty);
        }
        let expected = width.checked_mul(height).ok_or(FrameError::TooLarge)?;
        if expected > MAX_PIXELS {
            return Err(FrameError::TooLarge);
        }
        if data.len() != expected {
            return Err(FrameError::Size { expected, got: data.len() });
        }
        Ok(Self { width, height, data })
    }

    /// The pixels, row by row.
    pub fn pixels(&self) -> &[[f32; 3]] {
        &self.data
    }

    /// The pixel at `(x, y)`; black outside the frame.
    pub fn at(&self, x: usize, y: usize) -> Vec3 {
        if x >= self.width {
            return Vec3::default();
        }
        let p = y.checked_mul(self.width).and_then(|r| r.checked_add(x)).and_then(|i| self.data.get(i)).copied().unwrap_or([0.0; 3]);
        Vec3::new(f64::from(p[0]), f64::from(p[1]), f64::from(p[2]))
    }

    /// Camera RGB averaged over `factor` × `factor` blocks within `window` (`x0, y0, x1, y1`,
    /// clamped to the frame). Returns the blocks in reading order and how many there are across
    /// and down.
    pub fn block_means(&self, factor: usize, window: [usize; 4]) -> (Vec<Vec3>, usize, usize) {
        let factor = factor.max(1);
        let x0 = window[0].min(self.width);
        let y0 = window[1].min(self.height);
        let x1 = window[2].min(self.width).max(x0);
        let y1 = window[3].min(self.height).max(y0);
        let (across, down) = ((x1 - x0) / factor, (y1 - y0) / factor);
        let scale = 1.0 / (factor * factor) as f64;
        let mut out = Vec::with_capacity(across.saturating_mul(down));
        for by in 0..down {
            for bx in 0..across {
                let mut sums = [0.0f64; 3];
                for dy in 0..factor {
                    let y = y0 + by * factor + dy;
                    for dx in 0..factor {
                        let v = self.at(x0 + bx * factor + dx, y);
                        for c in 0..3 {
                            sums[c] += v.v[c];
                        }
                    }
                }
                out.push(Vec3::new(sums[0] * scale, sums[1] * scale, sums[2] * scale));
            }
        }
        (out, across, down)
    }

    /// The frame averaged down to `width × height`, for looking at (camera colours).
    pub fn preview(&self, width: usize, height: usize) -> Vec<Vec3> {
        let (pw, ph) = (self.width, self.height);
        let count = width.saturating_mul(height);
        if pw == 0 || ph == 0 || count == 0 || count > MAX_PIXELS {
            return vec![Vec3::default(); count.min(MAX_PIXELS)];
        }
        let mut sums = vec![[0.0f64; 3]; count];
        let mut counts = vec![0.0f64; count];
        for y in 0..ph {
            let ty = (y * height / ph).min(height - 1);
            for x in 0..pw {
                let tx = (x * width / pw).min(width - 1);
                let slot = ty * width + tx;
                let value = self.at(x, y);
                if let (Some(sum), Some(n)) = (sums.get_mut(slot), counts.get_mut(slot)) {
                    for c in 0..3 {
                        sum[c] += value.v[c];
                    }
                    *n += 1.0;
                }
            }
        }
        sums.iter()
            .zip(&counts)
            .map(|(sum, count)| if *count > 0.0 { Vec3::new(sum[0] / count, sum[1] / count, sum[2] / count) } else { Vec3::default() })
            .collect()
    }
}

/// A frame from a full-resolution RGGB mosaic (`width × height` samples, `white` the white
/// level), for tests that paint charts at sensor resolution.
#[cfg(test)]
pub(crate) fn rggb_test_frame(width: usize, height: usize, samples: &[u16], white: f32) -> RawFrame {
    let (hw, hh) = (width / 2, height / 2);
    let mut data = Vec::with_capacity(hw * hh);
    for y in 0..hh {
        for x in 0..hw {
            let s = |dx: usize, dy: usize| f32::from(samples[(2 * y + dy) * width + 2 * x + dx]) / white;
            data.push([s(0, 0), (s(1, 0) + s(0, 1)) / 2.0, s(1, 1)]);
        }
    }
    RawFrame::from_rgb(hw, hh, data).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bad_sizes_are_errors_not_panics() {
        assert_eq!(RawFrame::from_rgb(0, 4, vec![]), Err(FrameError::Empty));
        assert!(matches!(RawFrame::from_rgb(2, 2, vec![[0.0; 3]; 3]), Err(FrameError::Size { .. })));
        assert_eq!(RawFrame::from_rgb(usize::MAX, 2, vec![]), Err(FrameError::TooLarge));
    }

    #[test]
    fn block_means_clamp_their_window() {
        let f = RawFrame::from_rgb(4, 2, vec![[1.0, 0.5, 0.25]; 8]).unwrap();
        let (v, a, d) = f.block_means(2, [0, 0, 100, 100]);
        assert_eq!((a, d), (2, 1));
        assert!((v[0].v[1] - 0.5).abs() < 1e-9);
        let (v, _, _) = f.block_means(1, [10, 10, 2, 2]);
        assert!(v.is_empty());
    }
}
