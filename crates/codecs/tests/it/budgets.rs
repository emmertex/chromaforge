//! Decoder memory budgets (adapted from upstream #2182): a few-KB file must not make the header read or a decode
//! allocate gigabytes from its header alone.

use lcc_codecs::*;
use lcc_raster::Rgba8;

/// A TIFF with `spp` samples a pixel of 8 bits, one uncompressed strip of a few bytes.
fn tiff_multiband(w: u32, h: u32, spp: u16) -> Vec<u8> {
    let entries: [(u16, u16, u32, u32); 9] = [
        (256, 4, 1, w),
        (257, 4, 1, h),
        (258, 3, u32::from(spp), 0),
        (259, 3, 1, 1),
        (262, 3, 1, 1),
        (273, 4, 1, 0),
        (277, 3, 1, u32::from(spp)),
        (278, 4, 1, h),
        (279, 4, 1, 16),
    ];
    let ifd_len = 2 + entries.len() * 12 + 4;
    let bps_at = 8 + ifd_len as u32;
    let strip_at = bps_at + 2 * u32::from(spp);
    let mut b = b"II*\0\x08\0\0\0".to_vec();
    b.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    for (tag, kind, count, value) in entries {
        let value = match tag {
            258 if spp > 2 => bps_at,
            258 => 8,
            273 => strip_at,
            _ => value,
        };
        b.extend_from_slice(&tag.to_le_bytes());
        b.extend_from_slice(&kind.to_le_bytes());
        b.extend_from_slice(&count.to_le_bytes());
        b.extend_from_slice(&value.to_le_bytes());
    }
    b.extend_from_slice(&[0; 4]);
    if spp > 2 {
        for _ in 0..spp {
            b.extend_from_slice(&8u16.to_le_bytes());
        }
    }
    b.extend_from_slice(&[0x80; 16]);
    b
}

/// 4000 samples a pixel at 1 MP is a 4 GB result buffer from a file of a few KB: refused by the header read and the
/// decode before it is allocated.
#[test]
fn multiband_tiff_sample_bomb_is_refused() {
    let bomb = tiff_multiband(1024, 1024, 4000);
    assert!(bomb.len() < 10_000);
    assert!(read_header(&bomb).is_err(), "{:?}", read_header(&bomb));
    assert!(decode(&bomb, DecodeOptions::default()).is_err());
    // a byte budget below the samples refuses an ordinary layout too
    let pixels = Rgba8::from_fn(64, 64, |x, y| [(x * 4) as u8, (y * 4) as u8, 80, 255]);
    let rgb = encode_tiff(&EncodeImage::rgba8(&pixels), TiffCompression::None, &EncodeMeta::default()).unwrap();
    assert!(decode(&rgb, DecodeOptions::default()).is_ok());
    assert!(matches!(decode(&rgb, DecodeOptions { max_bytes: 64 * 64 * 4 - 1, ..Default::default() }), Err(Error::TooLarge(64, 64))));
}

/// Options written before `max_bytes` existed still deserialize, with the default budget.
#[test]
fn decode_options_without_a_byte_budget_deserialize() {
    let o: DecodeOptions = serde_json::from_str(r#"{"max_size":null,"max_pixels":5}"#).unwrap();
    assert_eq!((o.max_pixels, o.max_bytes), (5, DEFAULT_MAX_BYTES));
}

/// A JPEG XL's size comes from its codestream, agreeing with a decode; a truncated one is refused, never a panic.
#[cfg(feature = "jxl")]
#[test]
fn jxl_header_agrees_with_decode_and_refuses_truncation() {
    let pixels = Rgba8::from_fn(40, 24, |x, y| [(x * 6) as u8, (y * 9) as u8, 80, 255]);
    let bytes = encode_jxl(&EncodeImage::rgba8(&pixels), &JxlOptions { lossless: true, ..Default::default() }, &EncodeMeta::default()).unwrap();
    let h = read_header(&bytes).unwrap();
    let d = decode(&bytes, DecodeOptions::default()).unwrap();
    assert_eq!((h.width, h.height), (d.source_width, d.source_height));
    assert_eq!((h.width, h.height), (40, 24));
    for cut in [0, 1, 4, 12, bytes.len() / 2, bytes.len() - 1] {
        let _ = read_header(bytes.get(..cut).unwrap());
    }
    assert!(read_header(bytes.get(..bytes.len() / 2).unwrap()).is_err());
}
