//! WebP decode via `image-webp` (lossy + lossless, alpha, ICCP/EXIF/XMP chunks; first frame of
//! animations).

use crate::convert::{Buf, Meta, Model, Raw, check_size, finish};
use crate::{DecodeOptions, Decoded, Error, Format, Result};

const F: Format = Format::WebP;

fn err(e: impl std::fmt::Display) -> Error {
    Error::Malformed(F, e.to_string())
}

pub(crate) fn decode(bytes: &[u8], opts: &DecodeOptions) -> Result<Decoded> {
    let mut d = image_webp::WebPDecoder::new(std::io::Cursor::new(bytes)).map_err(err)?;
    // RIFF chunk lengths are untrusted. Metadata cannot exceed the input;
    // otherwise a tiny truncated file can request gigabytes before read_exact fails.
    d.set_memory_limit(bytes.len());
    let (w, h) = d.dimensions();
    check_size(F, w as u64, h as u64, opts)?;
    let alpha = d.has_alpha();
    let size = d.output_buffer_size().ok_or_else(|| err("buffer size overflow"))?;
    let mut buf = vec![0u8; size];
    d.read_image(&mut buf).map_err(err)?;
    let icc = d.icc_profile().ok().flatten();
    let exif = d.exif_metadata().ok().flatten().map(|e| if e.starts_with(b"Exif\0\0") { e[6..].to_vec() } else { e });
    let xmp = d.xmp_metadata().ok().flatten().map(|x| String::from_utf8_lossy(&x).trim_end_matches('\0').to_string());
    let raw = Raw { width: w as usize, height: h as usize, model: Model::Rgb, alpha, premultiplied: false, buf: Buf::U8(buf), bit_depth: 8 };
    finish(F, raw, Meta { icc, exif, xmp, ..Default::default() }, (w, h), opts)
}

/// Stored dimensions and EXIF orientation from the RIFF chunks and the bitstream header, without
/// decoding the image. Refused, as a decode would refuse it: no image chunk (`VP8 `, `VP8L` or an
/// animation frame), or one that runs past the end of the file (truncated). The compressed data
/// itself is not checked.
pub(crate) fn header(bytes: &[u8]) -> Result<(u32, u32, u16)> {
    let mut p = 12usize;
    loop {
        let Some(h) = bytes.get(p..p.saturating_add(8)) else { return Err(err("no image data")) };
        let size = u32::from_le_bytes([h[4], h[5], h[6], h[7]]) as usize;
        let end = p.saturating_add(8).saturating_add(size);
        if matches!(&h[0..4], b"VP8 " | b"VP8L" | b"ANMF") {
            if end > bytes.len() {
                return Err(err("truncated image data"));
            }
            break;
        }
        p = end.saturating_add(size & 1);
    }
    let mut d = image_webp::WebPDecoder::new(std::io::Cursor::new(bytes)).map_err(err)?;
    d.set_memory_limit(bytes.len());
    let (w, h) = d.dimensions();
    check_size(F, w as u64, h as u64, &DecodeOptions::default())?;
    let exif = d.exif_metadata().ok().flatten();
    let exif = exif.as_deref().map(|e| e.strip_prefix(b"Exif\0\0").unwrap_or(e));
    Ok((w, h, exif.map(crate::exif::summarize).unwrap_or_default().orientation.unwrap_or(1)))
}
