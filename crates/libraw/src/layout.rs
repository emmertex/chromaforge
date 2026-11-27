//! LibRaw's description of a mosaic turned into the app's terms: the colour filter pattern anchored at the data's
//! first sample, the black level as one repeating grid anchored at the image area, the levels and colour numbers
//! checked. Plain arithmetic on numbers LibRaw read from an untrusted file: every one is checked, nothing panics.

/// The numbers LibRaw reports after `unpack()` (see `shim/shim.cpp`).
#[derive(Clone, Debug, PartialEq)]
pub struct Facts {
    pub raw_width: usize,
    pub raw_height: usize,
    pub left_margin: usize,
    pub top_margin: usize,
    pub width: usize,
    pub height: usize,
    pub filters: u32,
    pub colors: i32,
    /// `raw_image` holds the mosaic (one sample per pixel).
    pub is_mosaic: bool,
    /// Fujifilm SuperCCD's rotated layout (not read through LibRaw).
    pub fuji_width: usize,
    /// LibRaw `COLOR(row, col)` at image-area coordinates, rows then columns.
    pub pattern: [[u8; 48]; 48],
    pub black: u32,
    pub cblack: Vec<u32>,
    pub maximum: u32,
    pub raw_bps: u32,
    pub cam_mul: [f32; 4],
    pub cam_xyz: [[f32; 3]; 4],
}

/// A colour filter pattern: `width × height` colour indices (0 red, 1 green, 2 blue), row-major, anchored at the
/// data's sample (0, 0).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pattern {
    pub width: usize,
    pub height: usize,
    pub colors: Vec<u8>,
}

/// The black level as a `rows × cols` grid repeating from the image area's top-left sample.
#[derive(Clone, Debug, PartialEq)]
pub struct BlackGrid {
    pub rows: usize,
    pub cols: usize,
    pub values: Vec<f32>,
}

/// A raw file's mosaic as LibRaw unpacked it.
#[derive(Clone, Debug, PartialEq)]
pub struct Unpacked {
    /// The whole data, masked borders included.
    pub width: usize,
    pub height: usize,
    /// `width × height` samples.
    pub data: Vec<u16>,
    /// The image area: x, y, width, height.
    pub image_area: [usize; 4],
    /// `None`: a monochrome sensor.
    pub pattern: Option<Pattern>,
    pub black: BlackGrid,
    pub white: f32,
    pub bits: u32,
    /// As-shot multipliers, green = 1.
    pub as_shot: Option<[f32; 3]>,
    /// XYZ (D65) → camera.
    pub xyz_to_camera: Option<[[f64; 3]; 3]>,
    /// LibRaw's version.
    pub decoder: String,
}

/// The most samples read from LibRaw: 600 MP, above any sensor made.
pub const MAX_SAMPLES: usize = 600_000_000;
/// The largest repeat (rows or columns) of a pattern or black grid.
const MAX_REPEAT: usize = 16;

/// Checks `f` and derives the pattern, black grid and levels; `data` comes later (see [`Facts::samples`]).
pub fn describe(f: &Facts) -> Result<(Option<Pattern>, BlackGrid), String> {
    if f.fuji_width != 0 {
        return Err("LibRaw reads this SuperCCD file in its rotated layout, which the app doesn't take from LibRaw".into());
    }
    if !f.is_mosaic {
        return Err("LibRaw reads this file as full-colour pixels, not a mosaic; the app takes only mosaics from LibRaw".into());
    }
    f.samples()?;
    let fits = |start: usize, len: usize, total: usize| len > 0 && start.checked_add(len).is_some_and(|e| e <= total);
    if !fits(f.left_margin, f.width, f.raw_width) || !fits(f.top_margin, f.height, f.raw_height) {
        return Err("LibRaw's image area lies outside the raw data".into());
    }
    let mono = f.filters == 0;
    if !mono && f.colors != 3 {
        return Err(format!("LibRaw reads a {}-colour sensor; the app develops three-colour and monochrome sensors only", f.colors));
    }
    // colour per image-area pixel: LibRaw writes the second green of a Bayer layout as 3
    let colour = |r: usize, c: usize| f.pattern.get(r).and_then(|row| row.get(c)).copied().unwrap_or(255);
    let pattern = if mono {
        None
    } else {
        let (ph, pw) = (period(colour, true)?, period(colour, false)?);
        let mut colors = Vec::with_capacity(ph * pw);
        for y in 0..ph {
            for x in 0..pw {
                // the colour at data sample (x, y) is the image area's at (x - left, y - top)
                let r = (y as i64 - f.top_margin as i64).rem_euclid(ph as i64) as usize;
                let c = (x as i64 - f.left_margin as i64).rem_euclid(pw as i64) as usize;
                colors.push(match colour(r, c) {
                    0 => 0,
                    1 | 3 => 1,
                    2 => 2,
                    other => return Err(format!("LibRaw reports colour {other} in a three-colour pattern")),
                });
            }
        }
        if !(0..3u8).all(|k| colors.contains(&k)) {
            return Err("LibRaw's colour pattern lacks one of red, green and blue".into());
        }
        Some(Pattern { width: pw, height: ph, colors })
    };
    Ok((pattern, black_grid(f, mono, &colour)?))
}

impl Facts {
    /// `raw_width × raw_height`, checked against [`MAX_SAMPLES`].
    pub fn samples(&self) -> Result<usize, String> {
        let n = self.raw_width.checked_mul(self.raw_height).ok_or("LibRaw's raw size overflows")?;
        if n == 0 || n > MAX_SAMPLES {
            return Err(format!("LibRaw's raw data is {} x {}, outside the sizes the app reads", self.raw_width, self.raw_height));
        }
        Ok(n)
    }

    /// The finished description, with the samples LibRaw copied out.
    pub fn unpacked(&self, data: Vec<u16>, decoder: String) -> Result<Unpacked, String> {
        let (pattern, black) = describe(self)?;
        if data.len() != self.samples()? {
            return Err("LibRaw's sample count doesn't match its size".into());
        }
        let black_max = black.values.iter().fold(0.0f32, |a, b| a.max(*b));
        let white = self.maximum as f32;
        if self.maximum == 0 || self.maximum > 65535 || white <= black_max {
            return Err(format!("LibRaw's levels are unusable (black up to {black_max}, white {})", self.maximum));
        }
        let m = self.cam_mul;
        let as_shot = (m[1] > 0.0 && [m[0], m[1], m[2]].iter().all(|v| v.is_finite() && *v > 0.0)).then(|| [m[0] / m[1], 1.0, m[2] / m[1]]);
        let x = self.cam_xyz;
        let rows = [x[0], x[1], x[2]].map(|r| r.map(f64::from));
        let xyz_to_camera =
            (rows.iter().flatten().all(|v| v.is_finite()) && rows.iter().flatten().any(|v| *v != 0.0) && pattern.is_some()).then_some(rows);
        let bits = if (1..=16).contains(&self.raw_bps) { self.raw_bps } else { 32 - self.maximum.leading_zeros() };
        Ok(Unpacked {
            width: self.raw_width,
            height: self.raw_height,
            data,
            image_area: [self.left_margin, self.top_margin, self.width, self.height],
            pattern,
            black,
            white,
            bits,
            as_shot,
            xyz_to_camera,
            decoder,
        })
    }
}

/// The smallest repeat (1 to 16) of `colour` over the 48 × 48 sample, down the rows (`rows`) or across.
fn period(colour: impl Fn(usize, usize) -> u8, rows: bool) -> Result<usize, String> {
    (1..=MAX_REPEAT)
        .find(|&p| (0..48 - p).all(|a| (0..48).all(|b| if rows { colour(a, b) == colour(a + p, b) } else { colour(b, a) == colour(b, a + p) })))
        .ok_or_else(|| "LibRaw's colour pattern repeats over more than 16 pixels".to_string())
}

fn gcd(a: usize, b: usize) -> usize {
    if b == 0 { a } else { gcd(b, a % b) }
}

fn lcm(a: usize, b: usize) -> usize {
    a / gcd(a, b).max(1) * b
}

/// The black level per image-area pixel: `black` + `cblack[colour]` + the `cblack[4] × cblack[5]` pattern from
/// `cblack[6]` (LibRaw's model), as one grid repeating over the colour pattern and the black pattern together.
fn black_grid(f: &Facts, mono: bool, colour: &impl Fn(usize, usize) -> u8) -> Result<BlackGrid, String> {
    let cb = |i: usize| f.cblack.get(i).copied().unwrap_or(0);
    let (br, bc) = (cb(4) as usize, cb(5) as usize);
    let (br, bc) = if br == 0 || bc == 0 { (1, 1) } else { (br, bc) };
    let patterned = cb(4) != 0 && cb(5) != 0;
    if br > MAX_REPEAT || bc > MAX_REPEAT {
        return Err(format!("LibRaw's black pattern is {br} x {bc}, larger than the app reads"));
    }
    let (ph, pw) = if mono { (1, 1) } else { (period(colour, true)?, period(colour, false)?) };
    let (rows, cols) = (lcm(ph, br), lcm(pw, bc));
    if rows > MAX_REPEAT || cols > MAX_REPEAT {
        return Err("LibRaw's black and colour patterns together repeat over more than 16 pixels".into());
    }
    let mut values = Vec::with_capacity(rows * cols);
    for r in 0..rows {
        for c in 0..cols {
            let per_colour = if mono { cb(0) } else { cb(colour(r, c) as usize & 3) };
            let pat = if patterned { cb(6 + (r % br) * bc + c % bc) } else { 0 };
            let v = u64::from(f.black) + u64::from(per_colour) + u64::from(pat);
            if v > 65535 {
                return Err(format!("LibRaw's black level {v} is above 16 bits"));
            }
            values.push(v as f32);
        }
    }
    Ok(BlackGrid { rows, cols, values })
}

#[cfg(test)]
mod tests;
