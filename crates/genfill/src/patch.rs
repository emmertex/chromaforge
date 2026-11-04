//! A generated patch in the photo's own pixels, its file format, and compositing it into a render.
//!
//! The patch covers a rectangle of the EXIF-oriented source in normalized coordinates (so it is
//! the same at every resolution), at the source's native pixel size. It holds scene-linear
//! as-shot Rec.2020 RGB (already matched to the photo, see [`crate::matching`]) and a blend weight
//! (1 over the hole, feathered to 0 at its edge and 0 in an outer ring of original pixels that
//! renders measure sensor noise in, see [`crate::noise`]).
//!
//! # File format (`.lcpatch`)
//!
//! `LCGPATCH` (8 bytes), a little-endian `u32` version (1), a `u32` header length, the header
//! as JSON ([`PatchInfo`]), then a zlib stream of `width × height` little-endian `f32` triples (RGB)
//! followed by `width × height` `f32` weights. Readers check every size against the header and
//! the caps below before allocating; a damaged file is an error, never a panic.

use std::sync::OnceLock;

use lcc_raster::{Plane, Rgb32f};
use serde::{Deserialize, Serialize};

pub const MAGIC: [u8; 8] = *b"LCGPATCH";
pub const VERSION: u32 = 1;
/// Longest patch side accepted.
pub const MAX_SIDE: u32 = 16_384;
/// Most pixels in one patch (a 200 MP photo's worth of hole is not a patch).
pub const MAX_PIXELS: u64 = 64 * 1024 * 1024;
const MAX_HEADER: u32 = 64 * 1024;

/// Why a patch could not be made or read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatchError {
    /// Not a patch file, a newer version, or damaged.
    Format(String),
    /// Sizes that don't fit together or exceed the caps.
    Size(String),
}

impl std::fmt::Display for PatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PatchError::Format(e) => write!(f, "not a usable generated patch: {e}"),
            PatchError::Size(e) => write!(f, "generated patch has an unusable size: {e}"),
        }
    }
}

impl std::error::Error for PatchError {}

/// What a patch file says about its pixels.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PatchInfo {
    pub width: u32,
    pub height: u32,
    /// `[x0, y0, x1, y1]`: where the patch sits, normalized to the EXIF-oriented source.
    pub rect: [f64; 4],
    /// The source's size (px) the patch was cut from: its native pixels.
    pub source_w: u32,
    pub source_h: u32,
    /// Generation resolution relative to native (1 = native; below 1 the model saw a downscaled
    /// crop and the patch is softer than the photo).
    pub scale: f64,
    /// Who generated it (provider id) and the transform the crop was sent with.
    pub provider: String,
    pub neutral: crate::Neutral,
}

impl Default for PatchInfo {
    fn default() -> Self {
        PatchInfo {
            width: 0,
            height: 0,
            rect: [0.0, 0.0, 1.0, 1.0],
            source_w: 0,
            source_h: 0,
            scale: 1.0,
            provider: String::new(),
            neutral: Default::default(),
        }
    }
}

impl PatchInfo {
    fn check(&self) -> Result<(), PatchError> {
        let (w, h) = (self.width, self.height);
        if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE || u64::from(w) * u64::from(h) > MAX_PIXELS {
            return Err(PatchError::Size(format!("{w}×{h}")));
        }
        let [x0, y0, x1, y1] = self.rect;
        if !self.rect.iter().all(|v| v.is_finite() && (-1.0..=2.0).contains(v)) || x1 <= x0 || y1 <= y0 {
            return Err(PatchError::Size(format!("rectangle {:?}", self.rect)));
        }
        Ok(())
    }
}

/// One level of a patch's pyramid (level 0 = the patch itself).
struct Level {
    rgb: Rgb32f,
    alpha: Plane,
}

/// A generated patch (see the module docs).
pub struct PatchLayer {
    pub info: PatchInfo,
    pub rgb: Rgb32f,
    pub alpha: Plane,
    /// Box-filtered halvings for renders far below native size, made on first use.
    mips: OnceLock<Vec<Level>>,
}

impl std::fmt::Debug for PatchLayer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PatchLayer").field("info", &self.info).finish_non_exhaustive()
    }
}

impl PatchLayer {
    /// A patch of `rgb` blended by `alpha` (same size as `info` says). Non-finite values become 0,
    /// weights are clamped to 0..1.
    pub fn new(mut info: PatchInfo, mut rgb: Rgb32f, mut alpha: Plane) -> Result<PatchLayer, PatchError> {
        info.width = u32::try_from(rgb.width).map_err(|_| PatchError::Size("too wide".into()))?;
        info.height = u32::try_from(rgb.height).map_err(|_| PatchError::Size("too tall".into()))?;
        info.check()?;
        if alpha.width != rgb.width || alpha.height != rgb.height || rgb.data.len() != rgb.width * rgb.height || alpha.data.len() != rgb.data.len() {
            return Err(PatchError::Size("pixels and weights differ in size".into()));
        }
        rgb.data.iter_mut().flatten().for_each(|v| *v = if v.is_finite() { v.max(0.0) } else { 0.0 });
        alpha.data.iter_mut().for_each(|a| *a = if a.is_finite() { a.clamp(0.0, 1.0) } else { 0.0 });
        Ok(PatchLayer { info, rgb, alpha, mips: OnceLock::new() })
    }

    /// Bytes held in memory (for cache accounting).
    pub fn bytes(&self) -> usize {
        self.rgb.data.len() * 16 * 4 / 3
    }

    /// The file's bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        let header = serde_json::to_vec(&self.info).unwrap_or_default();
        let mut raw = Vec::with_capacity(self.rgb.data.len() * 16);
        for p in &self.rgb.data {
            for c in p {
                raw.extend_from_slice(&c.to_le_bytes());
            }
        }
        for a in &self.alpha.data {
            raw.extend_from_slice(&a.to_le_bytes());
        }
        let body = miniz_oxide::deflate::compress_to_vec_zlib(&raw, 6);
        let mut out = Vec::with_capacity(16 + header.len() + body.len());
        out.extend_from_slice(&MAGIC);
        out.extend_from_slice(&VERSION.to_le_bytes());
        out.extend_from_slice(&(header.len() as u32).to_le_bytes());
        out.extend_from_slice(&header);
        out.extend_from_slice(&body);
        out
    }

    /// Read a patch file's bytes (see the module docs).
    pub fn from_bytes(b: &[u8]) -> Result<PatchLayer, PatchError> {
        let bad = |e: &str| PatchError::Format(e.to_string());
        let u32_at = |i: usize| b.get(i..i + 4).and_then(|s| s.try_into().ok()).map(u32::from_le_bytes);
        if b.get(..8) != Some(&MAGIC[..]) {
            return Err(bad("not a generative-fill patch file"));
        }
        let version = u32_at(8).ok_or_else(|| bad("truncated"))?;
        if version != VERSION {
            return Err(bad(&format!("version {version} (this build reads {VERSION})")));
        }
        let hlen = u32_at(12).ok_or_else(|| bad("truncated"))?;
        if hlen > MAX_HEADER {
            return Err(bad("header too large"));
        }
        let hend = 16usize.saturating_add(hlen as usize);
        let header = b.get(16..hend).ok_or_else(|| bad("truncated header"))?;
        let info: PatchInfo = serde_json::from_slice(header).map_err(|e| bad(&format!("header: {e}")))?;
        info.check()?;
        let n = info.width as usize * info.height as usize;
        let want = n * 16;
        let raw = miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(b.get(hend..).unwrap_or_default(), want)
            .map_err(|_| bad("pixels damaged or larger than the header says"))?;
        if raw.len() != want {
            return Err(bad("pixels cut short"));
        }
        let (floats, _) = raw.as_chunks::<4>();
        let f = |i: usize| floats.get(i).map_or(0.0, |c| f32::from_le_bytes(*c));
        let rgb = Rgb32f {
            width: info.width as usize,
            height: info.height as usize,
            data: (0..n).map(|i| [f(3 * i), f(3 * i + 1), f(3 * i + 2)]).collect(),
        };
        let alpha = Plane { width: info.width as usize, height: info.height as usize, data: (0..n).map(|i| f(3 * n + i)).collect() };
        PatchLayer::new(info, rgb, alpha)
    }

    fn levels(&self) -> &[Level] {
        self.mips.get_or_init(|| {
            let mut out: Vec<Level> = Vec::new();
            loop {
                let (rgb, alpha) = match out.last() {
                    Some(l) => (&l.rgb, &l.alpha),
                    None => (&self.rgb, &self.alpha),
                };
                if rgb.width < 8 || rgb.height < 8 || out.len() >= 12 {
                    break;
                }
                let (w, h) = (rgb.width.div_ceil(2), rgb.height.div_ceil(2));
                let mut nr = Rgb32f::new(w, h);
                let mut na = Plane::new(w, h);
                for y in 0..h {
                    for x in 0..w {
                        let (mut s, mut a, mut k) = ([0.0f32; 3], 0.0f32, 0.0f32);
                        for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                            let (sx, sy) = (2 * x + dx, 2 * y + dy);
                            if sx < rgb.width && sy < rgb.height {
                                let p = rgb.get(sx, sy);
                                s = [s[0] + p[0], s[1] + p[1], s[2] + p[2]];
                                a += alpha.get(sx, sy);
                                k += 1.0;
                            }
                        }
                        let k = k.max(1.0);
                        nr.set(x, y, s.map(|v| v / k));
                        na.set(x, y, a / k);
                    }
                }
                out.push(Level { rgb: nr, alpha: na });
            }
            out
        })
    }

    /// The patch's colour and weight at normalized source position `(nx, ny)`, read from the level
    /// whose pixels are about `footprint` patch pixels wide (bilinear; outside: weight 0).
    pub fn sample(&self, nx: f64, ny: f64, footprint: f64) -> ([f32; 3], f32) {
        let [x0, y0, x1, y1] = self.info.rect;
        let (u, v) = ((nx - x0) / (x1 - x0), (ny - y0) / (y1 - y0));
        if !(0.0..1.0).contains(&u) || !(0.0..1.0).contains(&v) {
            return ([0.0; 3], 0.0);
        }
        let level = if footprint.is_finite() && footprint > 1.5 { (footprint.log2().floor() as usize).min(12) } else { 0 };
        let (rgb, alpha) = match level.checked_sub(1).and_then(|i| self.levels().get(i).or(self.levels().last())) {
            Some(l) => (&l.rgb, &l.alpha),
            None => (&self.rgb, &self.alpha),
        };
        let (fx, fy) = ((u * rgb.width as f64) as f32, (v * rgb.height as f64) as f32);
        (rgb.sample_bilinear(fx, fy), sample_plane(alpha, fx, fy))
    }
}

/// Bilinear sample of a plane (pixel centres at `i + 0.5`, edges extended).
pub fn sample_plane(p: &Plane, x: f32, y: f32) -> f32 {
    if p.width == 0 || p.height == 0 {
        return 0.0;
    }
    let (fx, fy) = (x - 0.5, y - 0.5);
    let (x0, y0) = (fx.floor(), fy.floor());
    let (tx, ty) = (fx - x0, fy - y0);
    let (x0, y0) = (x0 as isize, y0 as isize);
    let g = |x: isize, y: isize| p.get_clamped(x, y);
    let top = g(x0, y0) + (g(x0 + 1, y0) - g(x0, y0)) * tx;
    let bot = g(x0, y0 + 1) + (g(x0 + 1, y0 + 1) - g(x0, y0 + 1)) * tx;
    top + (bot - top) * ty
}

/// Where an output pixel's centre falls in the source, for [`composite`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SourcePos {
    /// Normalized EXIF-oriented source coordinates.
    pub nx: f64,
    pub ny: f64,
    /// The render's own gain at that position (lens vignetting correction): the patch, being source
    /// pixels, gets it too.
    pub gain: f32,
}

/// What [`composite`] did: the output pixels it changed (index, weight) and the original pixels
/// around them inside the patch rectangle (the ring renders measure noise in).
#[derive(Debug, Default)]
pub struct Composited {
    pub changed: Vec<(usize, f32)>,
    pub ring: Vec<usize>,
}

/// Composite `layer` into `img` (an output image whose pixels map to the source through `map`;
/// `None` = canvas outside the image). `block` lists candidate pixel indices (the caller narrows
/// them to the patch's neighbourhood); `footprint` is the size of one output pixel in patch pixels.
pub fn composite(img: &mut Rgb32f, layer: &PatchLayer, candidates: impl Iterator<Item = (usize, SourcePos)>, footprint: f64) -> Composited {
    let mut out = Composited::default();
    for (i, pos) in candidates {
        let Some(px) = img.data.get_mut(i) else { continue };
        let [x0, y0, x1, y1] = layer.info.rect;
        if pos.nx < x0 || pos.nx >= x1 || pos.ny < y0 || pos.ny >= y1 {
            continue;
        }
        let (c, a) = layer.sample(pos.nx, pos.ny, footprint);
        if a <= 1e-4 {
            out.ring.push(i);
            continue;
        }
        let g = if pos.gain.is_finite() && pos.gain > 0.0 { pos.gain } else { 1.0 };
        *px = std::array::from_fn(|k| px[k] + (c[k] * g - px[k]) * a);
        out.changed.push((i, a));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layer(w: usize, h: usize) -> PatchLayer {
        let rgb = Rgb32f::from_fn(w, h, |x, y| [x as f32 / w as f32, y as f32 / h as f32, 0.25]);
        let alpha = Plane::from_fn(w, h, |x, _| if x < w / 2 { 1.0 } else { 0.0 });
        let info = PatchInfo { rect: [0.25, 0.25, 0.75, 0.75], source_w: 400, source_h: 300, provider: "mock".into(), ..Default::default() };
        PatchLayer::new(info, rgb, alpha).unwrap()
    }

    #[test]
    fn a_patch_survives_its_file() {
        let p = layer(40, 30);
        let b = p.to_bytes();
        let q = PatchLayer::from_bytes(&b).unwrap();
        assert_eq!((q.info.clone(), q.rgb.data.clone(), q.alpha.data.clone()), (p.info.clone(), p.rgb.data.clone(), p.alpha.data.clone()));
    }

    #[test]
    fn damaged_files_are_errors() {
        let b = layer(40, 30).to_bytes();
        for cut in [0, 7, 12, 15, 20, b.len() / 2, b.len() - 1] {
            assert!(PatchLayer::from_bytes(&b[..cut]).is_err(), "cut at {cut}");
        }
        let mut wrong = b.clone();
        wrong[8] = 9;
        assert!(PatchLayer::from_bytes(&wrong).is_err());
        // a header that claims a huge patch is refused before anything is allocated
        let info = PatchInfo { width: 100_000, height: 100_000, ..Default::default() };
        let h = serde_json::to_vec(&info).unwrap();
        let mut forged = MAGIC.to_vec();
        forged.extend_from_slice(&VERSION.to_le_bytes());
        forged.extend_from_slice(&(h.len() as u32).to_le_bytes());
        forged.extend_from_slice(&h);
        assert!(matches!(PatchLayer::from_bytes(&forged), Err(PatchError::Size(_))));
        // the pixels of a smaller patch under a larger header are cut short
        let mut lie = layer(40, 30).info.clone();
        lie.width = 41;
        let h = serde_json::to_vec(&lie).unwrap();
        let mut forged = MAGIC.to_vec();
        forged.extend_from_slice(&VERSION.to_le_bytes());
        forged.extend_from_slice(&(h.len() as u32).to_le_bytes());
        forged.extend_from_slice(&h);
        forged.extend_from_slice(&b[16 + serde_json::to_vec(&layer(40, 30).info).unwrap().len()..]);
        assert!(PatchLayer::from_bytes(&forged).is_err());
        assert!(PatchLayer::new(PatchInfo { rect: [f64::NAN, 0.0, 1.0, 1.0], ..Default::default() }, Rgb32f::new(4, 4), Plane::new(4, 4)).is_err());
        assert!(PatchLayer::new(PatchInfo::default(), Rgb32f::new(4, 4), Plane::new(3, 4)).is_err());
    }

    #[test]
    fn composite_blends_by_the_weight_and_reports_the_ring() {
        let p = layer(40, 30);
        let mut img = Rgb32f::from_fn(100, 100, |_, _| [1.0; 3]);
        let cands =
            (0..100 * 100usize).map(|i| (i, SourcePos { nx: (i % 100) as f64 / 100.0 + 0.005, ny: (i / 100) as f64 / 100.0 + 0.005, gain: 1.0 }));
        let c = composite(&mut img, &p, cands, 0.4);
        assert!(!c.changed.is_empty() && !c.ring.is_empty());
        // left half of the rectangle is the patch, right half untouched
        assert!((img.get(30, 50)[2] - 0.25).abs() < 1e-3);
        assert_eq!(img.get(70, 50), [1.0; 3]);
        assert_eq!(img.get(10, 10), [1.0; 3]);
        // a far smaller render reads a coarser level and stays finite
        let mut small = Rgb32f::from_fn(10, 10, |_, _| [1.0; 3]);
        let cands = (0..100usize).map(|i| (i, SourcePos { nx: (i % 10) as f64 / 10.0 + 0.05, ny: (i / 10) as f64 / 10.0 + 0.05, gain: 2.0 }));
        composite(&mut small, &p, cands, 4.0);
        assert!(small.data.iter().flatten().all(|v| v.is_finite()));
    }
}
