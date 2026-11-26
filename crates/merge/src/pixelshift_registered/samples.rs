//! The raw samples of a series with their colours: what the gains and the reconstruction read.
//!
//! [`Mosaics`] keeps the frames' raw mosaics (the sample vectors of the decoded frames, moved in, not copied) and the
//! geometry of the active area. A sample is read by active-area site `(x, y)`; the colour follows from the Bayer layout.

use super::series::GreenSeries;
use lcc_raw::{RawData, RawImage};

/// Samples by frame and active-area site.
pub trait Samples: Sync {
    /// Colour (0 red, 1 green, 2 blue) and raw value of frame `k` at site `(x, y)`; `None` outside the active area or
    /// for a missing frame.
    fn sample(&self, k: usize, x: i64, y: i64) -> Option<(u8, f32)>;
    fn frame_count(&self) -> usize;
    /// Active area (sites).
    fn size(&self) -> (usize, usize);
    fn black(&self) -> f64;
    fn white(&self) -> f64;
}

#[derive(Debug, Clone)]
pub struct Mosaics {
    frames: Vec<Vec<u16>>,
    stride: usize,
    height: usize,
    x_origin: usize,
    y_origin: usize,
    aw: usize,
    ah: usize,
    /// Colour at `(x & 1, y & 1)` of the active area, index `(y & 1) * 2 + (x & 1)`.
    table: [u8; 4],
    black: f64,
    white: f64,
}

impl Mosaics {
    /// Geometry from the first frame of a series (its samples are not taken). `Err` for anything but a 16-bit Bayer
    /// mosaic with one black level and a valid active area (the same limits as PR A's placement).
    pub fn new(first: &RawImage) -> Result<Mosaics, String> {
        let a = first.active_area;
        if first.cpp != 1 || first.cfa.as_ref().is_none_or(|c| !c.is_bayer()) || !matches!(first.data, RawData::U16(_)) {
            return Err("pixel shift needs raw frames with a Bayer mosaic of 16-bit samples".into());
        }
        if a.width == 0 || a.height == 0 || a.x.saturating_add(a.width) > first.width || a.y.saturating_add(a.height) > first.height {
            return Err("the raw frames have no valid active area".into());
        }
        let b = &first.black;
        if !b.delta_h.is_empty() || !b.delta_v.is_empty() || b.values.windows(2).any(|v| v[0] != v[1]) {
            return Err("the black level of these frames varies by position".into());
        }
        let cfa = first.cfa.as_ref().ok_or("no colour filter array")?;
        let mut table = [1u8; 4];
        for (i, t) in table.iter_mut().enumerate() {
            *t = cfa.color_at(a.x + (i & 1), a.y + (i >> 1));
        }
        Ok(Mosaics {
            frames: Vec::new(),
            stride: first.width,
            height: first.height,
            x_origin: a.x,
            y_origin: a.y,
            aw: a.width,
            ah: a.height,
            table,
            black: f64::from(b.mean()),
            white: f64::from(first.white.first().copied().unwrap_or(16383.0)),
        })
    }

    /// Takes the samples of a frame (moved, not copied); `Err` if it does not match the first frame.
    pub fn push(&mut self, frame: RawImage) -> Result<(), String> {
        let a = frame.active_area;
        if (frame.width, frame.height) != (self.stride, self.height)
            || (a.x, a.y, a.width, a.height) != (self.x_origin, self.y_origin, self.aw, self.ah)
        {
            return Err("a frame has a different size from the first".into());
        }
        let RawData::U16(d) = frame.data else { return Err("pixel shift needs 16-bit mosaic frames".into()) };
        if d.len() < self.stride.saturating_mul(self.height) {
            return Err("a frame holds fewer samples than its size says".into());
        }
        if self.frames.len() >= super::series::MAX_FRAMES {
            return Err("too many frames".into());
        }
        self.frames.push(d);
        Ok(())
    }

    /// A mosaic set from plain parts (for tests and tools): `stride` samples per row, the active area at
    /// `(x_origin, y_origin)`, colours by `table`.
    #[allow(clippy::too_many_arguments)]
    pub fn from_parts(
        frames: Vec<Vec<u16>>,
        stride: usize,
        height: usize,
        x_origin: usize,
        y_origin: usize,
        aw: usize,
        ah: usize,
        table: [u8; 4],
        black: f64,
        white: f64,
    ) -> Result<Mosaics, String> {
        if x_origin.checked_add(aw).is_none_or(|x| x > stride) || y_origin.checked_add(ah).is_none_or(|y| y > height) || aw == 0 || ah == 0 {
            return Err("active area outside the mosaic".into());
        }
        if frames.iter().any(|f| f.len() < stride * height) || frames.len() > super::series::MAX_FRAMES {
            return Err("frames do not fit the mosaic".into());
        }
        Ok(Mosaics { frames, stride, height, x_origin, y_origin, aw, ah, table, black, white })
    }

    /// Parity of `x + y` at green sites of the active area.
    pub fn green_parity(&self) -> usize {
        (0..4).find(|&i| self.table[i] == 1).map_or(0, |i| ((i & 1) + (i >> 1)) & 1)
    }

    /// Raw row index of the first active-area row.
    pub fn row_origin(&self) -> usize {
        self.y_origin
    }

    /// The green series of these frames (the half-width layout of [`GreenSeries`]).
    pub fn green_series(&self, noise: (f64, f64)) -> Result<GreenSeries, String> {
        let mut s = GreenSeries::new(self.aw, self.ah, self.y_origin, self.green_parity(), self.black, self.white, noise)?;
        for f in &self.frames {
            s.push_mosaic(f, self.stride, self.x_origin, self.y_origin)?;
        }
        Ok(s)
    }
}

impl Samples for Mosaics {
    #[inline]
    fn sample(&self, k: usize, x: i64, y: i64) -> Option<(u8, f32)> {
        if x < 0 || y < 0 || x >= self.aw as i64 || y >= self.ah as i64 {
            return None;
        }
        let c = self.table[((y & 1) * 2 + (x & 1)) as usize];
        let v = *self.frames.get(k)?.get((self.y_origin + y as usize) * self.stride + self.x_origin + x as usize)?;
        Some((c, f32::from(v)))
    }
    fn frame_count(&self) -> usize {
        self.frames.len()
    }
    fn size(&self) -> (usize, usize) {
        (self.aw, self.ah)
    }
    fn black(&self) -> f64 {
        self.black
    }
    fn white(&self) -> f64 {
        self.white
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parts(gpar_table: [u8; 4]) -> Mosaics {
        let f = |k: usize| (0..12 * 8).map(|i| (i + 1000 * k) as u16).collect::<Vec<u16>>();
        Mosaics::from_parts(vec![f(0), f(1)], 12, 8, 2, 1, 8, 6, gpar_table, 100.0, 4000.0).unwrap()
    }

    #[test]
    fn samples_are_read_by_active_area_site_with_their_colour() {
        let m = parts([0, 1, 1, 2]); // RGGB
        assert_eq!(m.frame_count(), 2);
        assert_eq!(m.size(), (8, 6));
        assert_eq!(m.sample(0, 0, 0), Some((0, (12 + 2) as f32)));
        assert_eq!(m.sample(1, 1, 0), Some((1, (1000 + 12 + 3) as f32)));
        assert_eq!(m.sample(0, 1, 1), Some((2, (2 * 12 + 3) as f32)));
        assert_eq!(m.green_parity(), 1);
        assert_eq!(parts([1, 0, 2, 1]).green_parity(), 0, "GRBG: green at (0, 0)");
        for (x, y) in [(-1, 0), (0, -1), (8, 0), (0, 6), (i64::MAX, 0)] {
            assert_eq!(m.sample(0, x, y), None);
        }
        assert_eq!(m.sample(2, 0, 0), None, "missing frame");
    }

    #[test]
    fn the_green_series_holds_the_green_sites_of_every_frame() {
        let m = parts([0, 1, 1, 2]);
        let g = m.green_series((0.3, 0.0)).unwrap();
        assert_eq!((g.aw, g.ah, g.ay, g.gpar, g.frame_count()), (8, 6, 1, 1, 2));
        for k in 0..2 {
            for y in 0..6i64 {
                for x in 0..8i64 {
                    match (m.sample(k, x, y), g.g(k, x, y)) {
                        (Some((1, v)), Some(w)) => assert_eq!(f64::from(v), w),
                        (Some((c, _)), None) => assert_ne!(c, 1),
                        other => panic!("({x}, {y}) frame {k}: {other:?}"),
                    }
                }
            }
        }
    }

    #[test]
    fn bad_geometry_is_refused() {
        assert!(Mosaics::from_parts(vec![], 4, 4, 2, 0, 4, 4, [0, 1, 1, 2], 0.0, 1.0).is_err(), "area past the row");
        assert!(Mosaics::from_parts(vec![vec![0; 3]], 4, 4, 0, 0, 4, 4, [0, 1, 1, 2], 0.0, 1.0).is_err(), "short frame");
        assert!(Mosaics::from_parts(vec![], 4, 4, 0, 0, 0, 4, [0, 1, 1, 2], 0.0, 1.0).is_err(), "empty area");
    }
}
