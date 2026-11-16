//! Updating the preview and the XMP of an existing DNG without touching its raw data
//! (Metadata ▸ Update DNG Preview & Metadata).
//!
//! The file is rewritten **by appending**: every byte of the original stays where it is (the raw
//! image's strips or tiles, its tables, the other IFDs, the maker note), and what is new goes after
//! it: a baseline JPEG preview, a preview IFD that points at it, the new XMP packet, a longer
//! `SubIFDs` array, and a copy of the first IFD (IFD0) that names them. Only the header's offset
//! of IFD0 changes (to the copy). The old IFD0, a previous preview of ours and an older XMP packet
//! stay in the file as unreferenced bytes: the cost of never moving the raw data (a replaced
//! preview is a few hundred kilobytes; a raw converter reads only what the directories reach).
//!
//! The entries of IFD0 are copied as the 12 bytes they are, so every value in them keeps its
//! meaning, including the ones stored out of line (their offsets still point at the unmoved
//! original). Only `XMP` (700) and `SubIFDs` (330) differ in the copy.
//!
//! A file this is not sure about is **refused**, never patched on a guess: a DNG written by Adobe
//! software (Lightroom and the DNG Converter keep their own preview and fast-load data consistent
//! with their own metadata), a BigTIFF, a file without a `DNGVersion`, directories that cannot be
//! read or are not in the order TIFF requires, and a file too big to address with 32-bit offsets.
//! The caller writes the result atomically (temp file, sync, rename); nothing here touches the disk.
//!
//! Sources: Adobe DNG Specification 1.7 (preview IFDs, `PreviewApplicationName`,
//! `PreviewColorSpace`, XMP in IFD0), TIFF 6.0 (IFD layout, `SubIFDs`, JPEG in strips),
//! TIFF Technical Note 2 (JPEG compression 7). Own design.

use lcc_tiff::tags as t;
use lcc_tiff::{ByteOrder, Ifd, Tiff};

use crate::{RawError, Result};

/// `PreviewApplicationName` (DNG 1.2): which program made a preview IFD. A preview carrying this app's
/// name is one a previous update made, and the next update drops it.
pub const PREVIEW_APPLICATION_NAME: u16 = 50966;
/// `PreviewApplicationVersion`.
pub const PREVIEW_APPLICATION_VERSION: u16 = 50967;

/// Largest directory read (real DNGs have well under 200 entries in IFD0).
const MAX_ENTRIES: usize = 1024;
/// Most `SubIFDs` entries read.
const MAX_SUB_IFDS: usize = 64;
/// A preview bigger than this is not a preview (and could not be attached to a 4 GiB-addressed file).
const MAX_PREVIEW: usize = 64 << 20;
/// Largest XMP packet embedded.
const MAX_XMP: usize = 16 << 20;
/// Most the JPEG header is scanned for its frame.
const MAX_JPEG_SEGMENTS: usize = 256;

/// What an update made.
#[derive(Clone, Debug)]
pub struct Updated {
    /// The whole new file.
    pub bytes: Vec<u8>,
    /// Previews this app made earlier that the new one replaces (they stay in the file, unreferenced).
    pub replaced_previews: usize,
    /// Width and height of the new preview.
    pub preview_size: (u32, u32),
}

fn unsupported(why: impl Into<String>) -> RawError {
    RawError::Unsupported(why.into())
}

/// The facts about a baseline JPEG that a preview IFD repeats.
struct JpegFrame {
    width: u32,
    height: u32,
    /// Horizontal and vertical sampling of the first component (the `YCbCrSubSampling`).
    subsampling: (u16, u16),
}

/// Read the frame header of a baseline, 8-bit, three-component JPEG: DNG previews are exactly that.
fn jpeg_frame(jpeg: &[u8]) -> Result<JpegFrame> {
    let bad = |why: &str| RawError::Corrupt(format!("the preview is not a usable JPEG: {why}"));
    if jpeg.get(..2) != Some(&[0xFF, 0xD8]) {
        return Err(bad("it does not start with an SOI marker"));
    }
    let mut pos = 2usize;
    for _ in 0..MAX_JPEG_SEGMENTS {
        // skip fill bytes up to the next marker
        while jpeg.get(pos) == Some(&0xFF) && jpeg.get(pos + 1) == Some(&0xFF) {
            pos += 1;
        }
        if jpeg.get(pos) != Some(&0xFF) {
            return Err(bad("a marker is missing"));
        }
        let marker = *jpeg.get(pos + 1).ok_or_else(|| bad("it ends inside a marker"))?;
        pos += 2;
        match marker {
            // standalone markers
            0x01 | 0xD0..=0xD7 => continue,
            // image data begins before any frame was seen
            0xD9 | 0xDA => return Err(bad("it has no frame header")),
            _ => {}
        }
        let len =
            usize::from(u16::from_be_bytes([*jpeg.get(pos).ok_or_else(|| bad("truncated"))?, *jpeg.get(pos + 1).ok_or_else(|| bad("truncated"))?]));
        let body = jpeg.get(pos + 2..pos.checked_add(len).ok_or_else(|| bad("truncated"))?).ok_or_else(|| bad("truncated"))?;
        match marker {
            // baseline and extended sequential DCT
            0xC0 | 0xC1 => {
                let (&precision, rest) = body.split_first().ok_or_else(|| bad("short frame header"))?;
                let height = u32::from(u16::from_be_bytes([
                    *rest.first().ok_or_else(|| bad("short frame header"))?,
                    *rest.get(1).ok_or_else(|| bad("short frame header"))?,
                ]));
                let width = u32::from(u16::from_be_bytes([
                    *rest.get(2).ok_or_else(|| bad("short frame header"))?,
                    *rest.get(3).ok_or_else(|| bad("short frame header"))?,
                ]));
                let components = *rest.get(4).ok_or_else(|| bad("short frame header"))?;
                if precision != 8 || components != 3 {
                    return Err(bad("a DNG preview is 8-bit with three components"));
                }
                if width == 0 || height == 0 {
                    return Err(bad("it has no size"));
                }
                let first = *rest.get(6).ok_or_else(|| bad("short frame header"))?;
                let (h, v) = (u16::from(first >> 4), u16::from(first & 15));
                if h == 0 || v == 0 {
                    return Err(bad("a zero sampling factor"));
                }
                return Ok(JpegFrame { width, height, subsampling: (h, v) });
            }
            // progressive, lossless, arithmetic-coded frames: not what a DNG preview may be
            0xC2..=0xC3 | 0xC5..=0xCF if marker != 0xC4 && marker != 0xC8 && marker != 0xCC => {
                return Err(unsupported("a DNG preview must be a baseline JPEG"));
            }
            _ => {}
        }
        pos = pos.checked_add(len).ok_or_else(|| bad("truncated"))?;
    }
    Err(bad("no frame header was found"))
}

/// One entry of IFD0 as the 12 bytes it is.
#[derive(Clone, Copy)]
struct RawEntry {
    tag: u16,
    bytes: [u8; 12],
}

/// The first IFD of a classic TIFF, as found in the file.
struct Head {
    order: ByteOrder,
    ifd0: usize,
    entries: Vec<RawEntry>,
    /// The offset after the entries: the next IFD of the chain, copied as it is.
    next: u32,
}

fn read_head(file: &[u8]) -> Result<Head> {
    let order = match file.get(..2) {
        Some(b"II") => ByteOrder::Little,
        Some(b"MM") => ByteOrder::Big,
        _ => return Err(unsupported("it is not a TIFF-structured file")),
    };
    match order.read_u16(file, 2) {
        Some(42) => {}
        Some(43) => return Err(unsupported("a BigTIFF DNG is not rewritten")),
        _ => return Err(unsupported("it is not a TIFF-structured file")),
    }
    let ifd0 = order.read_u32(file, 4).ok_or_else(|| RawError::Corrupt("the header is cut short".into()))? as usize;
    let n = usize::from(order.read_u16(file, ifd0).ok_or_else(|| RawError::Corrupt("the first directory is outside the file".into()))?);
    if n == 0 || n > MAX_ENTRIES {
        return Err(unsupported(format!("the first directory has {n} entries")));
    }
    let table = ifd0.checked_add(2).ok_or_else(|| unsupported("the first directory is outside the file"))?;
    let end = n.checked_mul(12).and_then(|b| b.checked_add(table)).ok_or_else(|| unsupported("the first directory is too large"))?;
    let next = order.read_u32(file, end).ok_or_else(|| RawError::Corrupt("the first directory is cut short".into()))?;
    let mut entries = Vec::with_capacity(n);
    let mut last: Option<u16> = None;
    for i in 0..n {
        let at = table + i * 12;
        let bytes: [u8; 12] =
            file.get(at..at + 12).and_then(|b| b.try_into().ok()).ok_or_else(|| RawError::Corrupt("the first directory is cut short".into()))?;
        let tag = order.read_u16(&bytes, 0).ok_or_else(|| RawError::Corrupt("the first directory is cut short".into()))?;
        // TIFF wants ascending tags; a directory that is not is a structure this does not know
        if last.is_some_and(|l| l >= tag) {
            return Err(unsupported("the first directory's tags are not in ascending order"));
        }
        last = Some(tag);
        entries.push(RawEntry { tag, bytes });
    }
    Ok(Head { order, ifd0, entries, next })
}

/// Whether a software name is one of Adobe's (Lightroom, Camera Raw, the DNG Converter, Photoshop).
fn adobe_software(s: &str) -> bool {
    s.to_ascii_lowercase().contains("adobe")
}

/// The structure checks shared by [`check`] and [`update`]: the file as parsed, its head.
fn inspect(file: &[u8]) -> Result<(Tiff, Head)> {
    let head = read_head(file)?;
    let tiff = Tiff::parse(file)?;
    let first = tiff.ifds.first().ok_or_else(|| unsupported("it has no image directory"))?;
    if first.offset != head.ifd0 as u64 {
        return Err(unsupported("the first directory could not be read the way it is stored"));
    }
    let version = first.bytes(t::DNG_VERSION).ok_or_else(|| unsupported("it has no DNGVersion: it is not a DNG"))?;
    if version.first() != Some(&1) {
        return Err(unsupported("its DNG version is not 1.x"));
    }
    for ifd in tiff.all_ifds() {
        if ifd.string(t::SOFTWARE).is_some_and(|s| adobe_software(&s)) {
            return Err(unsupported("it was written by Adobe software, which keeps its own preview and metadata consistent: update it there"));
        }
    }
    Ok((tiff, head))
}

/// `Ok` when [`update`] would take the file; otherwise why it is refused. Cheap: reads the
/// directories only.
pub fn check(file: &[u8]) -> Result<()> {
    inspect(file).map(|_| ())
}

/// The XMP packet embedded in IFD0, if any (the file is checked as [`check`] does).
pub fn embedded_xmp(file: &[u8]) -> Result<Option<Vec<u8>>> {
    let (tiff, _) = inspect(file)?;
    Ok(tiff.ifds.first().and_then(|i| i.bytes(t::XMP)).map(<[u8]>::to_vec))
}

/// Whether `ifd` is a preview a previous update of this app made.
fn is_ours(ifd: &Ifd) -> bool {
    ifd.u32(t::NEW_SUBFILE_TYPE) == Some(1)
        && ifd.string(PREVIEW_APPLICATION_NAME).is_some_and(|s| s.trim_end_matches('\0').trim() == lcc_brand::DISPLAY_NAME)
}

fn short(order: ByteOrder, v: u16) -> [u8; 4] {
    let mut b = Vec::with_capacity(4);
    order.put_u16(&mut b, v);
    b.resize(4, 0);
    [b.first().copied().unwrap_or(0), b.get(1).copied().unwrap_or(0), 0, 0]
}

fn long(order: ByteOrder, v: u32) -> [u8; 4] {
    let mut b = Vec::with_capacity(4);
    order.put_u32(&mut b, v);
    let mut out = [0u8; 4];
    for (o, i) in out.iter_mut().zip(b) {
        *o = i;
    }
    out
}

/// One directory entry: tag, field type, count and the 4 bytes of value field.
fn entry(order: ByteOrder, tag: u16, field_type: u16, count: u32, value: [u8; 4]) -> RawEntry {
    let mut b = Vec::with_capacity(12);
    order.put_u16(&mut b, tag);
    order.put_u16(&mut b, field_type);
    order.put_u32(&mut b, count);
    b.extend_from_slice(&value);
    let mut bytes = [0u8; 12];
    for (o, i) in bytes.iter_mut().zip(b) {
        *o = i;
    }
    RawEntry { tag, bytes }
}

const SHORT: u16 = 3;
const LONG: u16 = 4;
const BYTE: u16 = 1;
const ASCII: u16 = 2;

/// The append-only builder: the original file, then what is new, every block word-aligned.
struct Out {
    bytes: Vec<u8>,
    order: ByteOrder,
}

impl Out {
    fn align(&mut self) {
        if self.bytes.len() % 2 == 1 {
            self.bytes.push(0);
        }
    }
    /// Append `data` (word-aligned); its offset.
    fn block(&mut self, data: &[u8]) -> Result<u32> {
        self.align();
        let at = u32::try_from(self.bytes.len()).map_err(|_| unsupported("the file would be too large for 32-bit offsets"))?;
        self.bytes.extend_from_slice(data);
        if u32::try_from(self.bytes.len()).is_err() {
            return Err(unsupported("the file would be too large for 32-bit offsets"));
        }
        Ok(at)
    }
    /// An ASCII (NUL-terminated) value: inline when it fits the entry's 4 bytes.
    fn ascii(&mut self, tag: u16, text: &str) -> Result<RawEntry> {
        let mut data: Vec<u8> = text.bytes().filter(|b| *b != 0).collect();
        data.push(0);
        let count = u32::try_from(data.len()).map_err(|_| unsupported("text too long"))?;
        if data.len() <= 4 {
            data.resize(4, 0);
            let mut v = [0u8; 4];
            for (o, i) in v.iter_mut().zip(data) {
                *o = i;
            }
            return Ok(entry(self.order, tag, ASCII, count, v));
        }
        let at = self.block(&data)?;
        Ok(entry(self.order, tag, ASCII, count, long(self.order, at)))
    }
    /// A directory of `entries` (sorted by tag) followed by `next`; its offset.
    fn directory(&mut self, mut entries: Vec<RawEntry>, next: u32) -> Result<u32> {
        entries.sort_by_key(|e| e.tag);
        let n = u16::try_from(entries.len()).map_err(|_| unsupported("too many directory entries"))?;
        let mut data = Vec::with_capacity(6 + entries.len() * 12);
        self.order.put_u16(&mut data, n);
        for e in &entries {
            data.extend_from_slice(&e.bytes);
        }
        self.order.put_u32(&mut data, next);
        self.block(&data)
    }
}

/// Add (or replace) a baseline-JPEG preview IFD and the XMP packet of the DNG `file`, leaving the raw
/// data and everything else byte for byte as it was (see the module docs). `jpeg` is the preview
/// image (baseline, 8-bit, sRGB), `xmp` the XMP packet to embed.
///
/// A preview an earlier update of this app made is left out of the new `SubIFDs` list (its bytes stay
/// in the file); other previews, made by the camera or another program, are kept.
pub fn update(file: &[u8], jpeg: &[u8], xmp: &[u8]) -> Result<Updated> {
    if jpeg.is_empty() || jpeg.len() > MAX_PREVIEW {
        return Err(RawError::Corrupt("the preview image is empty or too large".into()));
    }
    if xmp.is_empty() || xmp.len() > MAX_XMP {
        return Err(RawError::Corrupt("the XMP packet is empty or too large".into()));
    }
    let frame = jpeg_frame(jpeg)?;
    let (tiff, head) = inspect(file)?;
    let order = head.order;
    let first = tiff.ifds.first().ok_or_else(|| unsupported("it has no image directory"))?;

    // the SubIFDs already there: which stay, which are ours from before
    let mut kept: Vec<u32> = Vec::new();
    let mut replaced = 0usize;
    if let Some(e) = head.entries.iter().find(|e| e.tag == t::SUB_IFDS) {
        let ty = order.read_u16(&e.bytes, 2).unwrap_or(0);
        let count = order.read_u32(&e.bytes, 4).unwrap_or(0) as usize;
        if !matches!(ty, 4 | 13) || count == 0 || count > MAX_SUB_IFDS {
            return Err(unsupported("its SubIFDs entry is not a list of offsets this understands"));
        }
        let at = if count == 1 { 8 } else { order.read_u32(&e.bytes, 8).unwrap_or(u32::MAX) as usize };
        let list: Vec<u32> = (0..count)
            .map(|i| if count == 1 { order.read_u32(&e.bytes, 8) } else { order.read_u32(file, at.saturating_add(i * 4)) })
            .collect::<Option<_>>()
            .ok_or_else(|| RawError::Corrupt("the SubIFDs list is outside the file".into()))?;
        if first.sub_ifds.len() != list.len() {
            return Err(unsupported("one of its SubIFDs could not be read"));
        }
        for (offset, sub) in list.into_iter().zip(&first.sub_ifds) {
            if is_ours(sub) {
                replaced += 1;
            } else {
                kept.push(offset);
            }
        }
    }

    let mut out = Out { bytes: Vec::with_capacity(file.len() + jpeg.len() + xmp.len() + 4096), order };
    out.bytes.extend_from_slice(file);

    // the preview: its pixels, then the directory that describes them
    let jpeg_at = out.block(jpeg)?;
    let mut bps = Vec::new();
    for _ in 0..3 {
        order.put_u16(&mut bps, 8);
    }
    let bps_at = out.block(&bps)?;
    let name = out.ascii(PREVIEW_APPLICATION_NAME, lcc_brand::DISPLAY_NAME)?;
    let version = out.ascii(PREVIEW_APPLICATION_VERSION, env!("CARGO_PKG_VERSION"))?;
    let jpeg_len = u32::try_from(jpeg.len()).map_err(|_| unsupported("the preview is too large"))?;
    let (sub_h, sub_v) = frame.subsampling;
    let mut subsampling = Vec::with_capacity(4);
    order.put_u16(&mut subsampling, sub_h);
    order.put_u16(&mut subsampling, sub_v);
    let mut sub_value = [0u8; 4];
    for (o, i) in sub_value.iter_mut().zip(subsampling) {
        *o = i;
    }
    let preview = vec![
        entry(order, t::NEW_SUBFILE_TYPE, LONG, 1, long(order, 1)),
        entry(order, t::IMAGE_WIDTH, LONG, 1, long(order, frame.width)),
        entry(order, t::IMAGE_LENGTH, LONG, 1, long(order, frame.height)),
        entry(order, t::BITS_PER_SAMPLE, SHORT, 3, long(order, bps_at)),
        entry(order, t::COMPRESSION, SHORT, 1, short(order, t::compression::JPEG)),
        entry(order, t::PHOTOMETRIC, SHORT, 1, short(order, t::photometric::YCBCR)),
        entry(order, t::STRIP_OFFSETS, LONG, 1, long(order, jpeg_at)),
        entry(order, t::SAMPLES_PER_PIXEL, SHORT, 1, short(order, 3)),
        entry(order, t::ROWS_PER_STRIP, LONG, 1, long(order, frame.height)),
        entry(order, t::STRIP_BYTE_COUNTS, LONG, 1, long(order, jpeg_len)),
        entry(order, t::PLANAR_CONFIGURATION, SHORT, 1, short(order, 1)),
        entry(order, t::YCBCR_SUBSAMPLING, SHORT, 2, sub_value),
        entry(order, 531, SHORT, 1, short(order, 1)),
        name,
        version,
        // 2 = sRGB
        entry(order, t::PREVIEW_COLOR_SPACE, LONG, 1, long(order, 2)),
    ];
    let preview_at = out.directory(preview, 0)?;

    // the SubIFDs list: what stays, then the new preview
    kept.push(preview_at);
    let count = u32::try_from(kept.len()).map_err(|_| unsupported("too many SubIFDs"))?;
    let sub_entry = if kept.len() == 1 {
        entry(order, t::SUB_IFDS, LONG, 1, long(order, preview_at))
    } else {
        let mut list = Vec::with_capacity(kept.len() * 4);
        for o in &kept {
            order.put_u32(&mut list, *o);
        }
        let at = out.block(&list)?;
        entry(order, t::SUB_IFDS, LONG, count, long(order, at))
    };

    // the XMP packet
    let xmp_len = u32::try_from(xmp.len()).map_err(|_| unsupported("the XMP packet is too large"))?;
    let xmp_entry = if xmp.len() <= 4 {
        let mut v = [0u8; 4];
        for (o, i) in v.iter_mut().zip(xmp) {
            *o = *i;
        }
        entry(order, t::XMP, BYTE, xmp_len, v)
    } else {
        let at = out.block(xmp)?;
        entry(order, t::XMP, BYTE, xmp_len, long(order, at))
    };

    // the copy of IFD0 that names both; every other entry as the 12 bytes it was
    let mut entries: Vec<RawEntry> = head.entries.iter().copied().filter(|e| e.tag != t::SUB_IFDS && e.tag != t::XMP).collect();
    entries.push(sub_entry);
    entries.push(xmp_entry);
    let ifd0_at = out.directory(entries, head.next)?;
    let mut header_offset = Vec::with_capacity(4);
    order.put_u32(&mut header_offset, ifd0_at);
    out.bytes.get_mut(4..8).ok_or_else(|| RawError::Corrupt("the header is cut short".into()))?.copy_from_slice(&header_offset);

    // the result must read back the way it was built
    let check = Tiff::parse(&out.bytes)?;
    let new_first = check.ifds.first().ok_or_else(|| RawError::Corrupt("the rewritten file has no image directory".into()))?;
    let ok = new_first.offset == u64::from(ifd0_at)
        && new_first.bytes(t::XMP) == Some(xmp)
        && new_first.sub_ifds.len() == kept.len()
        && new_first.sub_ifds.last().is_some_and(|s| s.offset == u64::from(preview_at) && s.u32(t::IMAGE_WIDTH) == Some(frame.width));
    if !ok {
        return Err(RawError::Corrupt("the rewritten file did not read back as intended".into()));
    }
    Ok(Updated { bytes: out.bytes, replaced_previews: replaced, preview_size: (frame.width, frame.height) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dngwrite::{DngCompression, DngWriteOptions, write_dng};
    use crate::{BlackLevel, OpcodeLists, RawData, RawFormat, RawImage, Rect};

    /// A baseline 4:2:0 JPEG of `w × h` made with the encoder this crate already uses.
    fn jpeg(w: u16, h: u16) -> Vec<u8> {
        let rgb: Vec<u8> = (0..usize::from(w) * usize::from(h) * 3).map(|i| (i % 251) as u8).collect();
        let mut out = Vec::new();
        jpeg_encoder::Encoder::new(&mut out, 85).encode(&rgb, w, h, jpeg_encoder::ColorType::Rgb).expect("encodes");
        out
    }

    fn raw() -> RawImage {
        let (w, h) = (16usize, 12usize);
        let full = Rect::new(0, 0, w, h);
        RawImage {
            format: RawFormat::Dng,
            width: w,
            height: h,
            cpp: 1,
            data: RawData::U16((0..w * h).map(|i| (i * 37 % 4000) as u16).collect()),
            cfa: None,
            bits: 12,
            black: BlackLevel::uniform(0.0),
            white: vec![4095.0],
            active_area: full,
            crop: full,
            orientation: lcc_geom::Orientation::Normal,
            color: crate::ColorData::default(),
            wb_multipliers: None,
            linearized: false,
            pixel_aspect: 1.0,
            opcodes: OpcodeLists::default(),
            metadata: Default::default(),
            lens_metadata: Default::default(),
        }
    }

    fn dng(order: ByteOrder) -> Vec<u8> {
        write_dng(&raw(), &DngWriteOptions { compression: DngCompression::Uncompressed, order, ..Default::default() }).expect("writes")
    }

    const XMP: &[u8] = b"<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"/></x:xmpmeta>";

    fn pixels(file: &[u8]) -> Vec<u16> {
        match crate::decode(file).expect("decodes").data {
            RawData::U16(v) => v,
            other => panic!("unexpected data {other:?}"),
        }
    }

    /// The point of the feature: the raw data and its decode are exactly what they were, the file
    /// gains a preview IFD that holds the JPEG, and the XMP is the new packet, in both byte orders.
    #[test]
    fn the_raw_is_untouched_and_the_preview_and_xmp_are_added() {
        for order in [ByteOrder::Little, ByteOrder::Big] {
            let original = dng(order);
            let preview = jpeg(32, 24);
            let done = update(&original, &preview, XMP).expect("updates");
            assert_eq!(done.preview_size, (32, 24));
            assert_eq!(done.replaced_previews, 0);
            // append-only: the whole original is still there, but for the header's IFD0 offset
            assert_eq!(done.bytes.get(..4), original.get(..4));
            assert_eq!(done.bytes.get(8..original.len()), original.get(8..));
            assert_eq!(pixels(&done.bytes), pixels(&original), "the raw decodes the same");
            let tiff = Tiff::parse(&done.bytes).expect("parses");
            let first = &tiff.ifds[0];
            assert_eq!(first.bytes(t::XMP), Some(XMP));
            let sub = first.sub_ifds.last().expect("a preview IFD");
            assert_eq!((sub.u32(t::NEW_SUBFILE_TYPE), sub.u16(t::COMPRESSION)), (Some(1), Some(7)));
            let strip = sub.u32(t::STRIP_OFFSETS).expect("strip") as usize;
            let len = sub.u32(t::STRIP_BYTE_COUNTS).expect("length") as usize;
            assert_eq!(done.bytes.get(strip..strip + len), Some(preview.as_slice()));
            assert_eq!(sub.string(PREVIEW_APPLICATION_NAME).as_deref().map(|s| s.trim_end_matches('\0')), Some(lcc_brand::DISPLAY_NAME));
            // everything else in IFD0 is as it was
            let before = Tiff::parse(&original).expect("parses");
            for e in &before.ifds[0].entries {
                if e.tag != t::XMP && e.tag != t::SUB_IFDS {
                    assert_eq!(first.get(e.tag).map(|x| &x.value), Some(&e.value), "tag {}", e.tag);
                }
            }
            assert_eq!(crate::probe(&done.bytes), Some(RawFormat::Dng));
        }
    }

    /// Updating again replaces our own preview (the list does not grow) and keeps a foreign one.
    #[test]
    fn a_second_update_replaces_our_preview_and_keeps_others() {
        let once = update(&dng(ByteOrder::Little), &jpeg(32, 24), XMP).expect("first").bytes;
        let twice = update(&once, &jpeg(48, 36), XMP).expect("second");
        assert_eq!(twice.replaced_previews, 1);
        let tiff = Tiff::parse(&twice.bytes).expect("parses");
        assert_eq!(tiff.ifds[0].sub_ifds.len(), 1, "one preview, not two");
        assert_eq!(tiff.ifds[0].sub_ifds[0].u32(t::IMAGE_WIDTH), Some(48));
        assert_eq!(pixels(&twice.bytes), pixels(&once));
    }

    /// Refusals, each with a message (no panic, nothing returned to write).
    #[test]
    fn files_that_are_not_safe_to_patch_are_refused() {
        let good = dng(ByteOrder::Little);
        let preview = jpeg(8, 8);
        let refused = |file: &[u8]| update(file, &preview, XMP).expect_err("refused").to_string();
        // not a TIFF at all, empty, a header alone
        for junk in [&b""[..], b"II", b"hello world, not a raw file", b"II*\0\x08\0\0\0"] {
            refused(junk);
        }
        // BigTIFF
        let mut big = good.clone();
        big[2..4].copy_from_slice(&43u16.to_le_bytes());
        assert!(refused(&big).contains("BigTIFF"));
        // Adobe software
        let mut adobe = good.clone();
        let sw = lcc_tiff::Tiff::parse(&good).expect("parses").ifds[0].get(t::SOFTWARE).expect("software").offset as usize;
        let name = crate::dngwrite::software();
        assert_eq!(&good[sw..sw + name.len()], name.as_bytes());
        adobe[sw..sw + 5].copy_from_slice(b"Adobe");
        assert!(refused(&adobe).contains("Adobe"), "{}", refused(&adobe));
        // a bad preview is refused before anything is built
        assert!(update(&good, b"not a jpeg", XMP).is_err());
        assert!(update(&good, &preview, b"").is_err());
        let mut progressive = preview.clone();
        if let Some(i) = progressive.windows(2).position(|w| w == [0xFF, 0xC0]) {
            progressive[i + 1] = 0xC2;
        }
        assert!(update(&good, &progressive, XMP).is_err());
    }

    /// Hostile input: every truncation of a valid DNG and a flip of each header / directory byte gives
    /// an error or a file that still parses, never a panic and never a result that fails its own check.
    #[test]
    fn damaged_files_never_panic() {
        let good = dng(ByteOrder::Little);
        let preview = jpeg(8, 8);
        let step = (good.len() / 400).max(1);
        for cut in (0..good.len()).step_by(step) {
            let _ = update(&good[..cut], &preview, XMP);
            let _ = check(&good[..cut]);
            let _ = embedded_xmp(&good[..cut]);
        }
        let ifd0 = u32::from_le_bytes([good[4], good[5], good[6], good[7]]) as usize;
        let mut spots: Vec<usize> = (0..16).collect();
        spots.extend(ifd0..(ifd0 + 2 + 12 * 40).min(good.len()));
        for at in spots {
            for flip in [0x01u8, 0x80, 0xFF] {
                let mut bad = good.clone();
                bad[at] ^= flip;
                if let Ok(done) = update(&bad, &preview, XMP) {
                    assert!(Tiff::parse(&done.bytes).is_ok(), "byte {at} ^ {flip:#x}");
                }
            }
        }
        // a SubIFDs loop and an offset list pointing outside
        let mut looped = good.clone();
        looped[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(update(&looped, &preview, XMP).is_err());
    }

    /// The JPEG reader only takes what a DNG preview may be, and never reads out of bounds.
    #[test]
    fn jpeg_frames_are_read_strictly() {
        let good = jpeg(40, 30);
        let f = jpeg_frame(&good).expect("frame");
        assert_eq!((f.width, f.height), (40, 30));
        assert!(f.subsampling.0 >= 1 && f.subsampling.1 >= 1);
        for cut in 0..good.len().min(300) {
            let _ = jpeg_frame(&good[..cut]);
        }
        assert!(jpeg_frame(&[0xFF, 0xD8, 0xFF]).is_err());
        assert!(jpeg_frame(&[0xFF, 0xD8, 0xFF, 0xC0, 0xFF, 0xFF]).is_err());
    }
}
