//! The green samples of a series, as the registration reads them.
//!
//! The measurement uses green samples only (twice as many as red or blue, and the camera-filled blue rows stay out
//! of it). Each frame keeps its green sites in a half-width array: the sites `(x, y)` of the active area with
//! `(x + y) & 1 == gpar`, row by row. Every access is checked; a coordinate outside the area or of the wrong parity
//! gives `None`.

/// Rows whose raw index is `MASK_ROW` modulo 12 hold camera-filled values in the blue sites; the measurement
/// replaces the green sites of those rows by the mean of the sites two rows above and below.
pub const MASK_PERIOD: usize = 12;
pub const MASK_ROW: usize = 5;

/// Most frames a series may hold (16 for Pixel Shift; the cap keeps hostile input from growing memory).
pub const MAX_FRAMES: usize = 32;

#[derive(Debug, Clone)]
pub struct GreenSeries {
    /// Width and height of the active area in sites (the width is even).
    pub aw: usize,
    pub ah: usize,
    /// Raw row index of the first active-area row (for the camera-filled rows).
    pub ay: usize,
    /// Parity of `x + y` at green sites (0 or 1).
    pub gpar: usize,
    pub black: f64,
    pub white: f64,
    /// Noise variance of a sample is `noise.0 * (level - black) + noise.1` (DN²).
    pub noise: (f64, f64),
    frames: Vec<Vec<u16>>,
}

impl GreenSeries {
    /// `Err` for an empty or odd-width area, a parity other than 0 or 1, or levels that are not finite and ordered.
    pub fn new(aw: usize, ah: usize, ay: usize, gpar: usize, black: f64, white: f64, noise: (f64, f64)) -> Result<GreenSeries, String> {
        if aw < 2 || ah < 2 || !aw.is_multiple_of(2) || aw.checked_mul(ah).is_none_or(|n| n > 1 << 28) {
            return Err(format!("active area {aw} x {ah} is not usable"));
        }
        if gpar > 1 {
            return Err("green parity must be 0 or 1".into());
        }
        if !(black.is_finite() && white.is_finite() && white > black && noise.0.is_finite() && noise.1.is_finite()) {
            return Err("black level, white level or noise model not usable".into());
        }
        Ok(GreenSeries { aw, ah, ay, gpar, black, white, noise, frames: Vec::new() })
    }

    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }

    /// Adds the green sites of one frame from its raw mosaic. The active area starts at `(x_origin, y_origin)` of the
    /// mosaic (`stride` samples per row); `Err` if the mosaic does not hold the whole area or the cap is reached.
    pub fn push_mosaic(&mut self, mosaic: &[u16], stride: usize, x_origin: usize, y_origin: usize) -> Result<(), String> {
        if self.frames.len() >= MAX_FRAMES {
            return Err(format!("more than {MAX_FRAMES} frames"));
        }
        let last = y_origin.checked_add(self.ah).and_then(|y| y.checked_mul(stride)).ok_or("mosaic size overflows")?;
        if x_origin.checked_add(self.aw).is_none_or(|x| x > stride) || mosaic.len() < last {
            return Err("the mosaic does not cover the active area".into());
        }
        let half = self.aw / 2;
        let mut f = vec![0u16; half * self.ah];
        for y in 0..self.ah {
            let x0 = (self.gpar + y) & 1;
            let row = mosaic.get((y_origin + y) * stride + x_origin..(y_origin + y) * stride + x_origin + self.aw).ok_or("row outside the mosaic")?;
            for (i, o) in f[y * half..(y + 1) * half].iter_mut().enumerate() {
                *o = row.get(x0 + 2 * i).copied().unwrap_or(0);
            }
        }
        self.frames.push(f);
        Ok(())
    }

    /// Adds a frame that is already in the half-width green layout (`aw / 2` samples per row, `ah` rows); `Err` for any other length or the frame cap.
    pub fn push_green_frame(&mut self, green: Vec<u16>) -> Result<(), String> {
        if self.frames.len() >= MAX_FRAMES {
            return Err(format!("more than {MAX_FRAMES} frames"));
        }
        if green.len() != self.aw / 2 * self.ah {
            return Err(format!("a green frame of {} samples does not fit {} x {}", green.len(), self.aw, self.ah));
        }
        self.frames.push(green);
        Ok(())
    }

    /// Replaces every sample of every frame by `f(frame, sample)` (in place).
    pub fn map_frames(&mut self, f: impl Fn(usize, u16) -> u16 + Sync) {
        use rayon::prelude::*;
        self.frames.par_iter_mut().enumerate().for_each(|(k, fr)| {
            for v in fr.iter_mut() {
                *v = f(k, *v);
            }
        });
    }

    /// Green sample of frame `k` at site `(x, y)`, `None` outside the area, on a non-green site or for a missing frame.
    #[inline]
    pub fn g(&self, k: usize, x: i64, y: i64) -> Option<f64> {
        if x < 0 || y < 0 || x >= self.aw as i64 || y >= self.ah as i64 || ((x + y) as usize & 1) != self.gpar {
            return None;
        }
        let x0 = (self.gpar + y as usize) & 1;
        let half = self.aw / 2;
        self.frames.get(k)?.get(y as usize * half + (x as usize - x0) / 2).map(|v| f64::from(*v))
    }

    /// True if the raw row of active-area row `y` is a camera-filled row.
    #[inline]
    pub fn masked_row(&self, y: i64) -> bool {
        (self.ay as i64 + y).rem_euclid(MASK_PERIOD as i64) as usize == MASK_ROW
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mosaic(w: usize, h: usize) -> Vec<u16> {
        (0..w * h).map(|i| (i % 65_000) as u16).collect()
    }

    #[test]
    fn green_sites_are_taken_by_parity_and_read_back() {
        let (w, h) = (20, 8);
        for gpar in 0..2 {
            let mut s = GreenSeries::new(16, 6, 0, gpar, 100.0, 4000.0, (0.3, 0.0)).unwrap();
            let m = mosaic(w, h);
            s.push_mosaic(&m, w, 2, 1).unwrap(); // the active area starts at (2, 1) of the mosaic
            assert_eq!(s.frame_count(), 1);
            for y in 0..6i64 {
                for x in 0..16i64 {
                    let got = s.g(0, x, y);
                    if ((x + y) as usize & 1) == gpar {
                        assert_eq!(got, Some(f64::from(m[(1 + y as usize) * w + 2 + x as usize])), "gpar {gpar} at ({x}, {y})");
                    } else {
                        assert_eq!(got, None);
                    }
                }
            }
        }
    }

    #[test]
    fn a_green_frame_in_the_half_width_layout_is_accepted_only_at_the_right_length() {
        let mut s = GreenSeries::new(8, 4, 0, 1, 0.0, 100.0, (1.0, 0.0)).unwrap();
        assert!(s.push_green_frame(vec![0; 15]).is_err());
        assert!(s.push_green_frame(vec![7; 16]).is_ok());
        assert_eq!(s.g(0, 1, 0), Some(7.0));
        assert_eq!(s.g(0, 0, 0), None);
    }

    #[test]
    fn reads_outside_the_area_or_the_frames_are_none() {
        let mut s = GreenSeries::new(8, 4, 0, 1, 0.0, 100.0, (1.0, 0.0)).unwrap();
        s.push_mosaic(&mosaic(8, 4), 8, 0, 0).unwrap();
        assert_eq!(s.g(1, 1, 0), None);
        for (x, y) in [(-1, 1), (1, -1), (8, 1), (1, 4), (i64::MIN, 0), (0, i64::MAX)] {
            assert_eq!(s.g(0, x, y), None, "({x}, {y})");
        }
    }

    #[test]
    fn construction_and_pushing_refuse_bad_input() {
        assert!(GreenSeries::new(0, 4, 0, 0, 0.0, 1.0, (0.0, 0.0)).is_err());
        assert!(GreenSeries::new(7, 4, 0, 0, 0.0, 1.0, (0.0, 0.0)).is_err());
        assert!(GreenSeries::new(8, 4, 0, 2, 0.0, 1.0, (0.0, 0.0)).is_err());
        assert!(GreenSeries::new(8, 4, 0, 0, 5.0, 5.0, (0.0, 0.0)).is_err());
        assert!(GreenSeries::new(8, 4, 0, 0, f64::NAN, 5.0, (0.0, 0.0)).is_err());
        assert!(GreenSeries::new(usize::MAX - 1, usize::MAX, 0, 0, 0.0, 1.0, (0.0, 0.0)).is_err());
        let mut s = GreenSeries::new(8, 4, 0, 0, 0.0, 100.0, (1.0, 0.0)).unwrap();
        assert!(s.push_mosaic(&mosaic(8, 3), 8, 0, 0).is_err(), "too few rows");
        assert!(s.push_mosaic(&mosaic(8, 4), 8, 1, 0).is_err(), "area runs past the row");
        assert!(s.push_mosaic(&mosaic(8, 4), 8, 0, usize::MAX).is_err(), "origin overflows");
        for _ in 0..MAX_FRAMES {
            s.push_mosaic(&mosaic(8, 4), 8, 0, 0).unwrap();
        }
        assert!(s.push_mosaic(&mosaic(8, 4), 8, 0, 0).is_err(), "frame cap");
    }

    #[test]
    fn camera_filled_rows_follow_the_raw_row_index() {
        let s = GreenSeries::new(8, 40, 7, 0, 0.0, 100.0, (1.0, 0.0)).unwrap();
        let rows: Vec<i64> = (0..40).filter(|y| s.masked_row(*y)).collect();
        assert_eq!(rows, vec![10, 22, 34], "raw rows 17, 29, 41 are = 5 mod 12");
    }
}
