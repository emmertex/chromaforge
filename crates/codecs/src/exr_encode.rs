//! OpenEXR export: scene-linear RGB as 16-bit half or 32-bit float, lossless ZIP compression, the
//! file's primaries and white point in the standard `chromaticities` header attribute (so readers
//! such as Nuke, Resolve or Blender know whether it is linear Rec. 2020 or ACEScg). Written with the
//! pure-Rust `exr` crate (BSD-3-Clause, no unsafe code). Own code, no port; the ACEScg primaries are
//! the published ACES AP1 numbers.

use crate::{Error, Result, checked_samples};
use exr::meta::attribute::Chromaticities;
use exr::prelude::{Encoding, Image, ImageAttributes, Layer, LayerAttributes, SpecificChannels, Text, Vec2, WritableImage, f16};
use lcc_color::{RgbSpace, Xy};

/// The ACES AP1 primaries with the ACES white point (SMPTE ST 2065-1 / Academy S-2014-004): the
/// working space called ACEScg.
pub const ACESCG: RgbSpace =
    RgbSpace { name: "ACEScg", r: Xy::new(0.713, 0.293), g: Xy::new(0.165, 0.830), b: Xy::new(0.128, 0.044), white: Xy::new(0.32168, 0.33767) };

/// How [`encode_exr`] writes its samples.
#[derive(Clone, Copy, Debug)]
pub struct ExrOptions {
    /// 16-bit half floats (`true`) or 32-bit floats.
    pub half: bool,
    /// The primaries and white of the (linear) samples, written as the `chromaticities` attribute.
    pub primaries: RgbSpace,
}

/// A sample the file can hold: NaN becomes 0 (it would poison compositing downstream), and in
/// half precision anything past the largest finite half is clamped to it rather than becoming inf.
fn sanitize(v: f32, half: bool) -> f32 {
    if v.is_nan() {
        0.0
    } else if half {
        v.clamp(-65504.0, 65504.0)
    } else {
        v.clamp(f32::MIN, f32::MAX)
    }
}

/// Encode `data` (`width × height` pixels, `channels` = 3 RGB or 4 RGBA interleaved, linear light
/// in `opts.primaries`; alpha is premultiplied, as EXR expects) as an OpenEXR file.
pub fn encode_exr(width: u32, height: u32, channels: usize, data: &[f32], opts: &ExrOptions) -> Result<Vec<u8>> {
    let (w, h) = (width as usize, height as usize);
    if w == 0 || h == 0 {
        return Err(Error::Encode(format!("EXR: empty image ({width}×{height})")));
    }
    if !matches!(channels, 3 | 4) {
        return Err(Error::Encode(format!("EXR: {channels} channels (needs 3 or 4)")));
    }
    // EXR data windows are i32 bounds
    if width > i32::MAX as u32 || height > i32::MAX as u32 {
        return Err(Error::TooLarge(u64::from(width), u64::from(height)));
    }
    if checked_samples(w, h, channels)? != data.len() {
        return Err(Error::Encode(format!("EXR: {} samples for a {width}×{height}×{channels} image", data.len())));
    }
    let px = move |p: Vec2<usize>| -> [f32; 4] {
        let i = p.y().saturating_mul(w).saturating_add(p.x()).saturating_mul(channels);
        let s = |c: usize| data.get(i.saturating_add(c)).copied().map_or(0.0, |v| sanitize(v, opts.half));
        [s(0), s(1), s(2), if channels == 4 { s(3).clamp(0.0, 1.0) } else { 1.0 }]
    };
    let p = &opts.primaries;
    let xy = |c: Xy| Vec2(c.x as f32, c.y as f32);
    let chroma = Chromaticities { red: xy(p.r), green: xy(p.g), blue: xy(p.b), white: xy(p.white) };
    let mut attrs = ImageAttributes::new(exr::prelude::IntegerBounds::from_dimensions((w, h)));
    attrs.chromaticities = Some(chroma);
    let layer_attrs = LayerAttributes { software_name: Some(Text::from(lcc_brand::DISPLAY_NAME)), ..Default::default() };
    let mut out = std::io::Cursor::new(Vec::new());
    let write = |e: exr::error::Error| Error::Encode(format!("EXR: {e}"));
    macro_rules! write_with {
        ($channels:expr) => {{
            let layer = Layer::new((w, h), layer_attrs, Encoding::SMALL_LOSSLESS, $channels);
            let mut image = Image::from_layer(layer);
            image.attributes = attrs;
            image.write().non_parallel().to_buffered(&mut out).map_err(write)?;
        }};
    }
    match (opts.half, channels == 4) {
        (true, true) => write_with!(SpecificChannels::rgba(|p| {
            let v = px(p);
            (f16::from_f32(v[0]), f16::from_f32(v[1]), f16::from_f32(v[2]), f16::from_f32(v[3]))
        })),
        (true, false) => write_with!(SpecificChannels::rgb(|p| {
            let v = px(p);
            (f16::from_f32(v[0]), f16::from_f32(v[1]), f16::from_f32(v[2]))
        })),
        (false, true) => write_with!(SpecificChannels::rgba(|p| {
            let v = px(p);
            (v[0], v[1], v[2], v[3])
        })),
        (false, false) => write_with!(SpecificChannels::rgb(|p| {
            let v = px(p);
            (v[0], v[1], v[2])
        })),
    }
    Ok(out.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Read an EXR back: (width, height, RGBA samples, chromaticities, half?).
    fn read(bytes: &[u8]) -> (usize, usize, Vec<[f32; 4]>, Option<Chromaticities>, bool) {
        use exr::prelude::*;
        let img = read()
            .no_deep_data()
            .largest_resolution_level()
            .rgba_channels(
                |res, ch: &RgbaChannels| (res.width(), vec![[0.0f32; 4]; res.area()], ch.0.sample_type == SampleType::F16),
                |(w, v, _), p, (r, g, b, a): (f32, f32, f32, f32)| v[p.y() * *w + p.x()] = [r, g, b, a],
            )
            .first_valid_layer()
            .all_attributes()
            .from_buffered(std::io::Cursor::new(bytes))
            .expect("readable EXR");
        let size = img.layer_data.size;
        let (_, px, half) = img.layer_data.channel_data.pixels;
        (size.width(), size.height(), px, img.attributes.chromaticities, half)
    }

    fn gradient(w: usize, h: usize, c: usize) -> Vec<f32> {
        (0..w * h * c).map(|i| (i % 97) as f32 / 13.0).collect()
    }

    #[test]
    fn round_trips_float_and_half_with_chromaticities() {
        for (half, channels, prim) in [(false, 3, lcc_color::REC2020), (true, 3, ACESCG), (true, 4, lcc_color::REC2020), (false, 4, ACESCG)] {
            let (w, h) = (37, 11);
            let mut data = gradient(w, h, channels);
            if channels == 4 {
                data.chunks_mut(4).for_each(|p| p[3] = 0.5);
            }
            let bytes = encode_exr(w as u32, h as u32, channels, &data, &ExrOptions { half, primaries: prim }).unwrap();
            assert_eq!(&bytes[..4], &[0x76, 0x2f, 0x31, 0x01], "the OpenEXR magic number");
            let (rw, rh, px, chroma, was_half) = read(&bytes);
            assert_eq!((rw, rh, was_half), (w, h, half));
            for (i, p) in px.iter().enumerate() {
                for c in 0..3 {
                    let want = data[i * channels + c];
                    let tol = if half { want.abs() * 1e-3 + 1e-4 } else { 0.0 };
                    assert!((p[c] - want).abs() <= tol, "pixel {i} channel {c}: {} vs {want}", p[c]);
                }
                assert!((p[3] - if channels == 4 { 0.5 } else { 1.0 }).abs() < 1e-6);
            }
            let chroma = chroma.expect("chromaticities written");
            assert!((chroma.red.x() - prim.r.x as f32).abs() < 1e-6 && (chroma.white.y() - prim.white.y as f32).abs() < 1e-6);
        }
    }

    #[test]
    fn values_above_one_survive_scene_linear() {
        let data = vec![12.5f32, 0.0, 3000.0];
        let (_, _, px, _, _) = read(&encode_exr(1, 1, 3, &data, &ExrOptions { half: false, primaries: ACESCG }).unwrap());
        assert_eq!([px[0][0], px[0][2]], [12.5, 3000.0]);
    }

    #[test]
    fn hostile_inputs_are_errors_or_sanitized_never_panics() {
        let o = ExrOptions { half: true, primaries: lcc_color::REC2020 };
        assert!(encode_exr(0, 5, 3, &[], &o).is_err());
        assert!(encode_exr(5, 0, 3, &[], &o).is_err());
        assert!(encode_exr(2, 2, 2, &[0.0; 8], &o).is_err());
        assert!(encode_exr(2, 2, 5, &[0.0; 20], &o).is_err());
        assert!(encode_exr(2, 2, 3, &[0.0; 11], &o).is_err(), "too few samples");
        assert!(encode_exr(2, 2, 3, &[0.0; 13], &o).is_err(), "too many samples");
        assert!(encode_exr(u32::MAX, u32::MAX, 3, &[0.0; 3], &o).is_err());
        // NaN → 0, ±inf and huge values clamp to the largest finite half
        let data = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 1.0e9, -1.0e9, f32::MIN_POSITIVE];
        let (_, _, px, _, _) = read(&encode_exr(2, 1, 3, &data, &o).unwrap());
        let flat: Vec<f32> = px.iter().flat_map(|p| p[..3].to_vec()).collect();
        assert!(flat.iter().all(|v| v.is_finite()), "{flat:?}");
        assert_eq!(flat[0], 0.0);
        assert_eq!(flat[1], 65504.0);
        assert_eq!(flat[2], -65504.0);
        // degenerate primaries still write (the attribute is just numbers)
        let flat_space = RgbSpace { name: "x", r: Xy::new(0.0, 0.0), g: Xy::new(0.0, 0.0), b: Xy::new(0.0, 0.0), white: Xy::new(0.0, 0.0) };
        assert!(encode_exr(1, 1, 3, &[0.1, 0.2, 0.3], &ExrOptions { half: false, primaries: flat_space }).is_ok());
    }
}
