//! DNG raw-image digests: Library ▸ Validate DNG Files.
//!
//! A DNG can carry an MD5 of its raw pixels (`NewRawImageDigest`, DNG 1.4; `RawImageDigest`, 1.0). Recomputing it from the
//! decoded pixels and comparing tells whether the raw data survived a copy, a sync or a disk: a damaged tile decodes to
//! different samples, and the digest no longer matches. The digest is a property of the samples, not of the compressed
//! bytes, so it holds across lossless recompression too.
//!
//! - `RawImageDigest` (tag 50972; DNG specification): the MD5 of all samples in row-column order, each zero-padded to 16
//!   bits (8- and 16-bit data), little-endian, the samples of a pixel interleaved as stored.
//! - `NewRawImageDigest` (tag 51111): the specification only says the algorithm is "documented in the DNG SDK source", which
//!   was not read. It was found by black-box comparison against a camera-written DNG (a Leica M, lossless JPEG,
//!   16-bit CFA, 256 x 256 tiles), whose stored digest equals the MD5 of the concatenated MD5 digests of the image's
//!   256 x 256 tiles, taken left to right, top to bottom, each tile (clipped at the right and bottom edges) hashed as
//!   its rows of 16-bit little-endian samples. Only that one file could be compared, so the tile size is held as 256, with
//!   the file's own tiling also accepted when it differs, and a mismatch counts as damage only for the layout the
//!   comparison covered (one sample per pixel, more than 8 bits); any other layout is reported as unverifiable.
//!
//! Never panics on malformed input: a file that cannot be parsed or decoded is reported, not unwound.

use lcc_hash::{Md5, Md5Digest};
use lcc_tiff::Tiff;
use lcc_tiff::tags as t;

use crate::RawData;
use crate::tiffraw::{Packing, read_image};

/// `RawImageDigest` (DNG 1.0).
pub const RAW_IMAGE_DIGEST: u16 = 50972;
/// `NewRawImageDigest` (DNG 1.4).
pub const NEW_RAW_IMAGE_DIGEST: u16 = 51111;
/// The tile size of [`new_raw_image_digest`] the one real file compared against used.
const DIGEST_TILE: usize = 256;

/// Which stored digest a file was checked against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DigestKind {
    NewRawImage,
    RawImage,
}

/// The outcome for one file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DngCheck {
    /// The raw data hashes to a stored digest.
    Intact(DigestKind),
    /// The stored digest does not match the raw data, or the raw data (or the file structure) cannot be read at all.
    Damaged(String),
    /// Nothing to compare against, or a layout this check does not cover: not a verdict either way.
    Unverifiable(String),
}

fn stored(ifd: &lcc_tiff::Ifd, tag: u16) -> Option<[u8; 16]> {
    <[u8; 16]>::try_from(ifd.bytes(tag)?).ok()
}

fn feed(samples: &[u16], md5: &mut Md5) {
    let mut buf = Vec::with_capacity(samples.len().min(1 << 15) * 2);
    for chunk in samples.chunks(1 << 15) {
        buf.clear();
        for s in chunk {
            buf.extend_from_slice(&s.to_le_bytes());
        }
        md5.update(&buf);
    }
}

/// `RawImageDigest` of decoded samples. Floating-point data is hashed as 32-bit little-endian floats.
pub fn raw_image_digest(data: &RawData) -> Md5Digest {
    let mut md5 = Md5::new();
    match data {
        RawData::U16(v) => feed(v, &mut md5),
        RawData::F32(v) => {
            for chunk in v.chunks(1 << 14) {
                let buf: Vec<u8> = chunk.iter().flat_map(|s| s.to_le_bytes()).collect();
                md5.update(&buf);
            }
        }
    }
    md5.finish()
}

/// `NewRawImageDigest` of `width x height` pixels of `cpp` samples each: the MD5 of the MD5s of the `tile x tile` tiles
/// (see the module docs). `None` when the dimensions do not fit `samples` or a tile is empty.
pub fn new_raw_image_digest(samples: &[u16], width: usize, height: usize, cpp: usize, tile_w: usize, tile_h: usize) -> Option<Md5Digest> {
    if width == 0 || height == 0 || cpp == 0 || tile_w == 0 || tile_h == 0 || width.checked_mul(height)?.checked_mul(cpp)? != samples.len() {
        return None;
    }
    let mut outer = Md5::new();
    for ty in (0..height).step_by(tile_h) {
        for tx in (0..width).step_by(tile_w) {
            let mut tile = Md5::new();
            for y in ty..(ty + tile_h).min(height) {
                let from = (y * width + tx) * cpp;
                let to = (y * width + (tx + tile_w).min(width)) * cpp;
                feed(samples.get(from..to)?, &mut tile);
            }
            outer.update(&tile.finish().0);
        }
    }
    Some(outer.finish())
}

/// Check one file's raw data against the digest it carries.
pub fn check_dng(bytes: &[u8]) -> DngCheck {
    let tiff = match Tiff::parse(bytes) {
        Ok(t) => t,
        Err(e) => return DngCheck::Damaged(format!("not a readable TIFF/DNG: {e}")),
    };
    let Some(ifd0) = tiff.ifds.first() else { return DngCheck::Damaged("no image directory".into()) };
    if !ifd0.contains(t::DNG_VERSION) {
        return DngCheck::Unverifiable("not a DNG (no DNGVersion)".into());
    }
    let Some(raw) = crate::dng::raw_ifd(&tiff) else { return DngCheck::Damaged("no full-resolution raw image".into()) };
    // the digests live in IFD 0 (also the raw IFD, for a DNG whose raw data is the main image)
    let new = stored(ifd0, NEW_RAW_IMAGE_DIGEST).or_else(|| stored(raw, NEW_RAW_IMAGE_DIGEST));
    let old = stored(ifd0, RAW_IMAGE_DIGEST).or_else(|| stored(raw, RAW_IMAGE_DIGEST));
    if new.is_none() && old.is_none() {
        return DngCheck::Unverifiable("the file carries no raw-image digest".into());
    }
    let info = match raw.image() {
        Ok(i) => i,
        Err(e) => return DngCheck::Damaged(format!("raw image description unreadable: {e}")),
    };
    // layouts whose stored samples are not what the digest was taken over: lossy compression keeps a decoded
    // approximation, floating-point data may be half precision, and an interleaved raster is a permutation
    if matches!(info.compression, t::compression::LOSSY_JPEG | t::compression::JPEG_XL) {
        return DngCheck::Unverifiable("lossy compression: the original samples are not stored".into());
    }
    if info.sample_format == 3 {
        return DngCheck::Unverifiable("floating-point raw data".into());
    }
    if raw.u16(t::ROW_INTERLEAVE_FACTOR).unwrap_or(1) > 1 || raw.u16(t::COLUMN_INTERLEAVE_FACTOR).unwrap_or(1) > 1 {
        return DngCheck::Unverifiable("interleaved raw data".into());
    }
    let data = match read_image(bytes, &info, tiff.order, Packing::Msb) {
        Ok(d) => d,
        Err(e) => return DngCheck::Damaged(format!("raw data unreadable: {e}")),
    };
    let RawData::U16(samples) = &data else { return DngCheck::Unverifiable("floating-point raw data".into()) };
    let (w, h, cpp) = (info.width as usize, info.height as usize, info.samples_per_pixel.max(1) as usize);

    let mut damaged = None;
    let mut unsure = None;
    if let Some(want) = new {
        let (tw, th) = match info.layout {
            lcc_tiff::image::Layout::Tiles { tile_width, tile_height } => (tile_width as usize, tile_height as usize),
            _ => (DIGEST_TILE, DIGEST_TILE),
        };
        let mut tilings = vec![(DIGEST_TILE, DIGEST_TILE)];
        if (tw, th) != (DIGEST_TILE, DIGEST_TILE) {
            tilings.push((tw, th));
        }
        let mut got = None;
        for (a, b) in tilings {
            let d = new_raw_image_digest(samples, w, h, cpp, a, b);
            if d.is_some_and(|d| d.0 == want) {
                return DngCheck::Intact(DigestKind::NewRawImage);
            }
            got = got.or(d);
        }
        // only the layout compared against real files is trusted to call a mismatch damage
        if cpp == 1 && info.bits() > 8 {
            damaged = Some(format!(
                "raw data does not match its NewRawImageDigest (stored {}, computed {})",
                Md5Digest(want),
                got.unwrap_or(Md5Digest([0; 16]))
            ));
        } else {
            unsure = Some("the NewRawImageDigest of this layout (several samples per pixel or 8-bit) is not checked".to_string());
        }
    }
    if let Some(want) = old {
        let got = raw_image_digest(&data);
        if got.0 == want {
            return DngCheck::Intact(DigestKind::RawImage);
        }
        damaged = damaged.or_else(|| Some(format!("raw data does not match its RawImageDigest (stored {}, computed {got})", Md5Digest(want))));
    }
    match (damaged, unsure) {
        (Some(why), _) => DngCheck::Damaged(why),
        (None, why) => DngCheck::Unverifiable(why.unwrap_or_else(|| "no digest could be compared".into())),
    }
}
