//! Nikon High Efficiency / HE★, the four-component JPEG XS-derived NEF profile.
//!
//! Original adapter around `oxideav-jpegxs` (MIT, pure Rust). Header layout, band/packet order and
//! reconstruction were checked against camera files and independently decoded mosaics. Nikon uses
//! half-resolution component grids, per-component decomposition in CDT, long packet headers, and
//! horizontal-before-vertical inverse lifting. See `docs/nikon-he.md` for provenance and limits.

use crate::{RawError, Result};
use oxideav_jpegxs::entropy::packet_body::PrecinctState;
use oxideav_jpegxs::entropy::{self, BandGeometry, PacketEntry, PacketLayout, PrecinctGeometry, PrecinctTop};

const MAX_PIXELS: usize = 64 * 1024 * 1024;
const BANDS: usize = 25;
pub(super) const MAGIC: [u8; 4] = [0xff, 0x10, 0xff, 0x50];

fn corrupt(why: impl std::fmt::Display) -> RawError {
    RawError::Corrupt(format!("Nikon High Efficiency NEF: {why}"))
}

fn unsupported(why: &str) -> RawError {
    RawError::Unsupported(format!("Nikon High Efficiency NEF ({why})"))
}

struct Reader<'a> {
    rest: &'a [u8],
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let (value, rest) = self.rest.split_at_checked(n).ok_or_else(|| corrupt("truncated codestream"))?;
        self.rest = rest;
        Ok(value)
    }

    fn marker(&mut self, marker: [u8; 2]) -> Result<&'a [u8]> {
        if self.take(2)? != marker {
            return Err(corrupt(format!("expected marker {:02x}{:02x}", marker[0], marker[1])));
        }
        let len = self.take(2)?;
        let len = usize::from(u16::from_be_bytes([len[0], len[1]]));
        self.take(len.checked_sub(2).ok_or_else(|| corrupt("invalid marker length"))?)
    }
}

#[derive(Clone, Copy)]
struct Transfer {
    black: u16,
    gain: u32,
    pivot: u16,
}

impl Transfer {
    /// Integer transfer rule documented by RdWing (2026-09-11); round the Q16 gain half up,
    /// then square. The final bias incorporates both rounding stages of the 14-bit output.
    fn apply(self, value: i32) -> u16 {
        let d = (i64::from(value) + 32768).clamp(0, 65535) - i64::from(self.pivot);
        let q = if d < 0 {
            -d * d
        } else {
            let t = (d * i64::from(self.gain) + 32768) >> 16;
            t * t
        };
        (i64::from(self.black) + ((q + 163840) >> 18)).clamp(0, 16383) as u16
    }
}

struct Header {
    slice_height: usize,
    weights: [(u8, u8); BANDS],
    transfer: Transfer,
}

fn header(reader: &mut Reader<'_>, length: usize, width: usize, height: usize) -> Result<Header> {
    if reader.take(2)? != [0xff, 0x10] {
        return Err(corrupt("missing start marker"));
    }
    let cap = reader.marker([0xff, 0x50])?;
    if cap != b"CONTACT_INTOPIX_\xef\xc0\0\0\0\0\0\0\0\0\0\0\0\0\0\0" {
        return Err(unsupported("unrecognized capabilities"));
    }
    let pih: &[u8; 37] = reader.marker([0xff, 0x12])?.try_into().map_err(|_| unsupported("unrecognized picture header"))?;
    let declared = u32::from_be_bytes([pih[0], pih[1], pih[2], pih[3]]) as usize;
    if declared != length {
        return Err(corrupt("codestream length disagrees with the NEF strip"));
    }
    let w = usize::from(u16::from_be_bytes([pih[8], pih[9]]));
    let h = usize::from(u16::from_be_bytes([pih[10], pih[11]]));
    if (w, h) != (width, height) {
        return Err(corrupt("codestream dimensions disagree with the NEF IFD"));
    }
    let slice_height = usize::from(u16::from_be_bytes([pih[14], pih[15]]));
    if slice_height == 0 {
        return Err(corrupt("zero slice height"));
    }
    // Nc=4, Ng=4, Ss=8, Bw=18, Fq=4, Br=4, Ppoc=1, NLx=5, NLy=1,
    // uniform quantization, separate signs. Cw=0 means a single precinct column.
    if pih[4..8] != [0; 4] || pih[12..14] != [0; 2] || pih[16..24] != [4, 4, 8, 18, 0x44, 0x10, 0x51, 0x14] {
        return Err(unsupported("unrecognized coding profile"));
    }
    let tail = &pih[24..];
    // Only the independently verified transfer fields may vary. Other vendor fields encode
    // gains/transform choices whose meanings are not yet established here: fail explicitly.
    if tail[0..3] != [0x50, 0x88, 0x70] || tail[3] & 0xf0 != 0x80 || tail[5..7] != [0x15, 0x23] || tail[7] & 0xfc != 0xd0 || tail[12] != 7 {
        return Err(unsupported("unrecognized vendor transform parameters"));
    }
    let transfer = Transfer {
        black: (u16::from(tail[3] & 15) << 8) | u16::from(tail[4]),
        gain: (u32::from(tail[7] & 3) << 16) | (u32::from(tail[8]) << 8) | u32::from(tail[9]),
        pivot: u16::from_be_bytes([tail[10], tail[11]]),
    };
    if reader.marker([0xff, 0x13])? != [14, 0x11, 0x51, 14, 0x11, 0x51, 14, 0x11, 0, 14, 0x11, 0x51] {
        return Err(unsupported("unrecognized component decomposition"));
    }
    let wgt = reader.marker([0xff, 0x14])?;
    if wgt.len() != 2 * BANDS {
        return Err(corrupt("expected 25 band weights"));
    }
    let mut weights = [(0, 0); BANDS];
    let mut priorities = 0u32;
    for (dst, pair) in weights.iter_mut().zip(wgt.as_chunks::<2>().0) {
        let (gain, priority) = (pair[0], pair[1]);
        if gain > 15 || priority >= BANDS as u8 || priorities & (1 << priority) != 0 {
            return Err(corrupt("invalid band gain or priority"));
        }
        priorities |= 1 << priority;
        *dst = (gain, priority);
    }
    Ok(Header { slice_height, weights, transfer })
}

/// Codestream band index -> (component, horizontal/vertical filter index).
/// The first precinct line carries each component's horizontal bands; the second carries
/// vertical detail. Component 2 bypasses the wavelet and has a sample row on both lines.
fn band_map() -> Vec<(usize, usize)> {
    (0..4)
        .flat_map(|c| (0..if c == 2 { 1 } else { 6 }).map(move |b| (c, b)))
        .chain([0, 1, 3].into_iter().flat_map(|c| (6..8).map(move |b| (c, b))))
        .collect()
}

fn geometry(width: usize, weights: &[(u8, u8); BANDS]) -> (PrecinctGeometry, Vec<PacketLayout>) {
    let widths = [
        width.div_ceil(32),
        width.div_ceil(16) / 2,
        width.div_ceil(8) / 2,
        width.div_ceil(4) / 2,
        width.div_ceil(2) / 2,
        width / 2,
        width.div_ceil(2),
        width / 2,
    ];
    let map = band_map();
    let bands = map
        .iter()
        .zip(weights)
        .map(|(&(c, b), &(gain, priority))| BandGeometry {
            wpb: if c == 2 { width as u32 } else { widths[b] as u32 },
            gain,
            priority,
            l0: 0,
            l1: if c == 2 { 2 } else { 1 },
            exists: true,
        })
        .collect();
    let mut packets = Vec::with_capacity(8);
    for line in 0..2 {
        for c in 0..4 {
            let entries = map
                .iter()
                .enumerate()
                .filter(|(_, (cc, b))| *cc == c && (c == 2 || ((*b < 6) == (line == 0))))
                .map(|(band, _)| PacketEntry { band: band as u16, line: if c == 2 { line } else { 0 } })
                .collect();
            packets.push(PacketLayout { entries });
        }
    }
    // Nikon always writes the seven-byte packet header, even with PIH Lh=0.
    (PrecinctGeometry { bands, ng: 4, ss: 8, br: 4, fs: 1, rm: 0, rl: 0, lh: 1, short_packet_header: false }, packets)
}

/// Validate a codestream's headers and return its encoded black level without decoding any samples: the header
/// probe of a High Efficiency NEF (issue #1304). Applies the same checks as [`decode`] up to the entropy-coded data,
/// so a strip the probe accepts fails in [`decode`] only on a damaged body.
pub(super) fn probe(bytes: &[u8], width: usize, height: usize, bits: u32) -> Result<u16> {
    width.checked_mul(height).filter(|&n| n <= MAX_PIXELS).ok_or_else(|| corrupt("image exceeds the pixel limit"))?;
    if bits != 14 || width < 64 || !width.is_multiple_of(2) || height < 4 || !height.is_multiple_of(4) {
        return Err(unsupported("requires a 14-bit image with even width >= 64 and height divisible by four"));
    }
    let mut reader = Reader { rest: bytes };
    let header = header(&mut reader, bytes.len(), width, height)?;
    Ok(header.transfer.black)
}

/// Decode to the original 14-bit Bayer sample domain; also return its encoded black level.
pub(super) fn decode(bytes: &[u8], width: usize, height: usize, bits: u32) -> Result<(Vec<u16>, u16)> {
    let count = width.checked_mul(height).filter(|&n| n <= MAX_PIXELS).ok_or_else(|| corrupt("image exceeds the pixel limit"))?;
    if bits != 14 || width < 64 || !width.is_multiple_of(2) || height < 4 || !height.is_multiple_of(4) {
        return Err(unsupported("requires a 14-bit image with even width >= 64 and height divisible by four"));
    }
    let mut reader = Reader { rest: bytes };
    let header = header(&mut reader, bytes.len(), width, height)?;
    let (w, h) = (width / 2, height / 2);
    // Each precinct has a 12-byte header and eight seven-byte packet headers, before any
    // entropy payload. Reject implausibly short input before reserving image-sized storage.
    if reader.rest.len() < (h / 2) * (12 + 8 * 7) + (h / 2).div_ceil(header.slice_height) * 6 + 2 {
        return Err(corrupt("insufficient data for the declared dimensions"));
    }
    let (geom, packets) = geometry(w, &header.weights);
    // The precincts and their entropy-coded bodies, in order. Slices are independent (each resets the vertical
    // prediction), so they are decoded in parallel below.
    let mut precincts = Vec::with_capacity(h / 2);
    for precinct in 0..h / 2 {
        if precinct % header.slice_height == 0 {
            let slice = reader.marker([0xff, 0x20])?;
            if slice != ((precinct / header.slice_height) as u16).to_be_bytes() {
                return Err(corrupt("invalid slice index"));
            }
        }
        let ph = entropy::parse_precinct_header(reader.rest, &geom).map_err(corrupt)?;
        reader.take(ph.header_bytes)?;
        let body = reader.take(ph.lprc as usize)?;
        precincts.push((ph, body));
    }
    if reader.take(2)? != [0xff, 0x11] || !reader.rest.is_empty() {
        return Err(corrupt("missing end marker or unexpected trailing data"));
    }
    use rayon::prelude::*;
    // The dequantised coefficients of every precinct go straight into their place in the final bands: a precinct's part
    // of a band is `wpb × l1` samples, so slice `s` owns one contiguous range of each band and the slices write theirs in
    // parallel (the per-slice bands that were gathered into these afterwards cost the same memory again, and a copy).
    let stride: Vec<usize> = geom.bands.iter().map(|b| b.wpb as usize * b.l1 as usize).collect();
    let mut gathered: Vec<Vec<i32>> = stride.iter().map(|&n| vec![0; n * (h / 2)]).collect();
    let slice_height = header.slice_height;
    let ranges = band_ranges(&mut gathered, &stride, slice_height, precincts.len());
    // one list of ranges per slice, one range per band: the zips below stop at the shorter side, so a mismatch would
    // drop bands or slices silently
    if ranges.len() != precincts.len().div_ceil(slice_height) || ranges.iter().any(|r| r.len() != stride.len()) {
        return Err(corrupt("unexpected band layout"));
    }
    precincts.par_chunks(slice_height).zip(ranges).try_for_each(|(slice, mut ranges)| -> Result<()> {
        // Vertical GCLI prediction is signalled even at Nikon slice starts. Its external
        // predecessor is zero; the second row of component 2 still predicts within the precinct.
        let mut top = PrecinctTop::default();
        for (i, (ph, body)) in slice.iter().enumerate() {
            let mut body = Reader { rest: body };
            let mut state = PrecinctState::default();
            for layout in &packets {
                let packet = entropy::parse_packet_header(body.rest, &geom).map_err(corrupt)?;
                body.take(packet.header_bytes)?;
                let consumed =
                    entropy::decode_packet_body(body.rest, &geom, ph, &packet, layout, &mut state, Some(&top)).map_err(corrupt)?.bytes_consumed;
                body.take(consumed)?;
            }
            // Lprc includes encoder padding (not necessarily zero), which is not another packet.
            let dequantized =
                oxideav_jpegxs::dequant::dequantize_precinct(1, &geom, &entropy::precinct_truncation(&geom, ph), &state.coefficients, 4);
            place_precinct(&mut ranges, i, dequantized, &stride)?;
            top = PrecinctTop::capture(&geom, ph, &state);
        }
        Ok(())
    })?;
    let map = band_map();
    let mut component_bands: Vec<Vec<Vec<i32>>> = Vec::with_capacity(4);
    for c in 0..4 {
        let mut bands = vec![Vec::new(); if c == 2 { 1 } else { 8 }];
        for ((cc, b), source) in map.iter().zip(&mut gathered) {
            if *cc == c {
                let dest = bands.get_mut(*b).ok_or_else(|| corrupt("invalid band index"))?;
                *dest = std::mem::take(source);
            }
        }
        component_bands.push(bands);
    }
    let planes: Vec<Vec<i32>> = component_bands
        .into_par_iter()
        .enumerate()
        .map(|(c, mut bands)| {
            let mut plane = if c == 2 { std::mem::take(&mut bands[0]) } else { inverse_wavelet(&bands, w, h)? };
            // Br=4 bounds dequantized coefficients to <2^19; six 5/3 passes fit i32. Keep the
            // following four-component colour transform's neighbour sums well inside i32 as well.
            if plane.len() != count / 4 || plane.iter().any(|&v| !(-(1 << 24)..=1 << 24).contains(&v)) {
                return Err(corrupt("reconstructed component is out of range"));
            }
            plane.reverse();
            Ok(plane)
        })
        .collect::<Result<_>>()?;
    let Ok(mut planes) = <[Vec<i32>; 4]>::try_from(planes) else { return Err(corrupt("missing component")) };
    // Nikon's superpixel origin is opposite to the standard transform's origin. Reverse both
    // axes (the contiguous plane), order Ya/Cb/Cr/Delta, then undo the reversal on its outputs.
    planes.swap(2, 3);
    let mut planes = inverse_star_tetrix(planes, w, h);
    for plane in &mut planes {
        plane.reverse();
    }
    let mut out = vec![0; count];
    out.par_chunks_exact_mut(width).enumerate().try_for_each(|(y, row)| {
        for (x, value) in row.iter_mut().enumerate() {
            let c = 3 - (y % 2 * 2 + x % 2);
            let sample = planes[c].get((y / 2) * w + x / 2).ok_or_else(|| corrupt("incomplete reconstructed plane"))?;
            *value = header.transfer.apply(*sample);
        }
        Ok::<_, RawError>(())
    })?;
    Ok((out, header.transfer.black))
}

/// The ranges of `gathered` (one buffer per band, `stride[b] × precincts` samples) that each slice of `slice_height`
/// precincts owns: element `s` holds slice `s`'s range of every band, in band order (a short final slice gets a short range).
fn band_ranges<'a>(gathered: &'a mut [Vec<i32>], stride: &[usize], slice_height: usize, precincts: usize) -> Vec<Vec<&'a mut [i32]>> {
    let slice_height = slice_height.max(1);
    let mut ranges: Vec<Vec<&mut [i32]>> = (0..precincts.div_ceil(slice_height)).map(|_| Vec::with_capacity(stride.len())).collect();
    for (band, &n) in gathered.iter_mut().zip(stride) {
        for (slice, chunk) in ranges.iter_mut().zip(band.chunks_mut((n * slice_height).max(1))) {
            slice.push(chunk);
        }
    }
    ranges
}

/// Writes the dequantised bands of the slice's `i`-th precinct into its place in each band's range of `ranges`. The
/// bands and the strides must be the same in number, and every band exactly its geometry's size (`num_lines × wpb`).
fn place_precinct(ranges: &mut [&mut [i32]], i: usize, dequantized: Vec<Vec<i32>>, stride: &[usize]) -> Result<()> {
    if dequantized.len() != stride.len() || ranges.len() != stride.len() {
        return Err(corrupt("unexpected number of bands in a precinct"));
    }
    for ((dst, src), &n) in ranges.iter_mut().zip(dequantized).zip(stride) {
        let range = i.checked_mul(n).and_then(|start| Some(start..start.checked_add(n)?));
        let Some(place) = range.and_then(|r| dst.get_mut(r)).filter(|p| p.len() == src.len()) else {
            return Err(corrupt("unexpected size of a precinct's band"));
        };
        place.copy_from_slice(&src);
    }
    Ok(())
}

/// The inverse Star-Tetrix colour transform of JPEG XS (ISO/IEC 21122-1 Annex F) with `Ct = 0`, `Cf = 0` and
/// `e1 = e2 = 0`, which Nikon writes: `[Ya, Cb, Cr, Δ]` half-resolution planes in, `[R, G1, G2, B]` out. The four
/// lifting steps each read only the previous step's planes, so every step's rows are computed in parallel.
///
/// Every step updates one component from the neighbours it has in *other* components (diagonal or cross, the parity of
/// the offset changes the component: see [`component_of`]), so each plane is updated in place and no step allocates:
/// the six steps used to allocate six planes of the picture's size, written once and dropped.
fn inverse_star_tetrix(mut planes: [Vec<i32>; 4], w: usize, h: usize) -> [Vec<i32>; 4] {
    const DIAGONAL: [(i32, i32); 4] = [(-1, -1), (1, -1), (-1, 1), (1, 1)];
    const CROSS: [(i32, i32); 4] = [(-1, 0), (1, 0), (0, -1), (0, 1)];
    // inverse average: Ya from the four diagonal Δ neighbours
    lift_in_place(&mut planes, (w, h), 0, DIAGONAL, -1, 3);
    // inverse delta: Δ from the four diagonal luma neighbours
    lift_in_place(&mut planes, (w, h), 3, DIAGONAL, 1, 2);
    // inverse Y: both greens from their four chroma neighbours
    lift_in_place(&mut planes, (w, h), 0, CROSS, -1, 3);
    lift_in_place(&mut planes, (w, h), 3, CROSS, -1, 3);
    // inverse CbCr: blue and red from their four green neighbours
    lift_in_place(&mut planes, (w, h), 1, CROSS, 1, 2);
    lift_in_place(&mut planes, (w, h), 2, CROSS, 1, 2);
    let [ya, cb, cr, delta] = planes;
    [cr, delta, ya, cb]
}

/// [`lift`] on component `c` of `planes`, in place. The plane is taken out of the array while it is updated, so the
/// neighbours are read from the other three; none of a component's neighbours is itself (the offsets change the
/// parity of the position, see [`component_of`]), which the test `no_neighbour_is_the_component_itself` pins.
fn lift_in_place(planes: &mut [Vec<i32>; 4], (w, h): (usize, usize), c: usize, dirs: [(i32, i32); 4], sign: i32, shift: u32) {
    let Some(slot) = planes.get_mut(c) else { return };
    let mut target = std::mem::take(slot);
    // as the allocating version: the output is `w × h`, a base that is short counts as zeros
    target.resize(w * h, 0);
    let others = [&planes[0][..], &planes[1][..], &planes[2][..], &planes[3][..]];
    lift(&others, &mut target, (w, h), c, dirs, sign, shift);
    planes[c] = target;
}

/// One lifting step over the whole plane, in place: `plane[x, y] += sign · ⌊(sum of the four neighbours of component
/// `c` at `dirs`) / 2^shift⌋`, the neighbours read from `planes` as [`neighbour`] defines (`planes[c]` is not read).
///
/// Away from the image edges nothing is mirrored, so each neighbour is a fixed plane at a fixed offset (from
/// [`component_of`]) and a row is four slices added up; the first and last row and column, where the access is
/// mirrored, go through [`neighbour`] itself.
fn lift(planes: &[&[i32]; 4], plane: &mut [i32], (w, h): (usize, usize), c: usize, dirs: [(i32, i32); 4], sign: i32, shift: u32) {
    if w == 0 {
        return;
    }
    let tap = taps(c, dirs);
    let border = |x: usize, y: usize, own: i32| -> i32 {
        let sum: i32 = dirs.iter().map(|&(rx, ry)| neighbour(planes, w, h, c, x, y, rx, ry)).sum();
        own + sign * (sum >> shift)
    };
    use rayon::prelude::*;
    plane.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        // the row's interior columns 1..w-1, when the row itself is interior and every slice exists
        let interior = (y >= 1 && y + 1 < h && w >= 3)
            .then(|| {
                let slice_of = |(component, ox, oy): (usize, isize, isize)| {
                    let start = ((y as isize + oy) * w as isize + 1 + ox) as usize;
                    planes.get(component)?.get(start..start + (w - 2))
                };
                Some([slice_of(tap[0])?, slice_of(tap[1])?, slice_of(tap[2])?, slice_of(tap[3])?])
            })
            .flatten();
        match interior {
            Some([a, b, c4, d]) => {
                for (i, v) in row[1..w - 1].iter_mut().enumerate() {
                    *v += sign * ((a[i] + b[i] + c4[i] + d[i]) >> shift);
                }
                row[0] = border(0, y, row[0]);
                row[w - 1] = border(w - 1, y, row[w - 1]);
            }
            None => {
                for (x, v) in row.iter_mut().enumerate() {
                    *v = border(x, y, *v);
                }
            }
        }
    });
}

/// Where component `c` sits in each 2×2 cell of the full-resolution CFA grid, `(δx, δy)` (Table F.10, `Ct = 0`): the one
/// definition of the layout, which [`component_of`], [`neighbour`] and [`taps`] all read.
const DISPLACEMENT: [(i32, i32); 4] = [(0, 1), (1, 1), (0, 0), (1, 0)];

/// Per neighbour of component `c` at `dirs`, away from the image edges where nothing is mirrored: the component read, and
/// its offset in plane samples, `(component, columns, rows)`. What [`lift`]'s fast rows read, and what the test
/// `the_unmirrored_taps_agree_with_the_neighbour_definition` checks against [`neighbour`].
fn taps(c: usize, dirs: [(i32, i32); 4]) -> [(usize, isize, isize); 4] {
    let (dx, dy) = DISPLACEMENT[c & 3];
    dirs.map(|(rx, ry)| (component_of(c, rx, ry), (dx + rx).div_euclid(2) as isize, (dy + ry).div_euclid(2) as isize))
}

/// The component read at offset `(rx, ry)` on the full-resolution CFA grid from component `c`'s sample (Table F.10,
/// `Ct = 0`: component `c` sits at offset `(δx, δy)` of each 2×2 cell and the parity of the offset reached names it).
fn component_of(c: usize, rx: i32, ry: i32) -> usize {
    let (dx, dy) = DISPLACEMENT[c & 3];
    match ((rx + dx).rem_euclid(2), (ry + dy).rem_euclid(2)) {
        (0, 1) => 0,
        (1, 1) => 1,
        (0, 0) => 2,
        _ => 3,
    }
}

/// The sample at offset `(rx, ry)` on the full-resolution CFA grid from component `c`'s sample `(x, y)`, mirrored at
/// the image edges (Table F.12): component `c` sits at offset `(δx, δy)` of each 2×2 cell (Table F.10, `Ct = 0`),
/// and the parity of the offset reached names the component read.
#[allow(clippy::too_many_arguments)]
fn neighbour(planes: &[&[i32]; 4], w: usize, h: usize, c: usize, x: usize, y: usize, rx: i32, ry: i32) -> i32 {
    let (dx, dy) = DISPLACEMENT[c & 3];
    let (gx, gy) = (2 * x as i64, 2 * y as i64);
    let fx = |rx: i32| gx + i64::from(rx + dx);
    let fy = |ry: i32| gy + i64::from(ry + dy);
    let rx = if fx(rx) < 0 || fx(rx) >= 2 * w as i64 { -rx } else { rx };
    let ry = if fy(ry) < 0 || fy(ry) >= 2 * h as i64 { -ry } else { ry };
    let component = component_of(c, rx, ry);
    let (xo, yo) = (fx(rx).max(0) as usize / 2, fy(ry).max(0) as usize / 2);
    planes[component].get(yo * w + xo).copied().unwrap_or(0)
}

fn band_row(bands: &[Vec<i32>], band: usize, y: usize, width: usize) -> Result<&[i32]> {
    bands.get(band).and_then(|b| b.get(y * width..(y + 1) * width)).ok_or_else(|| corrupt("incomplete wavelet band"))
}

/// Nikon synthesizes all horizontal levels before the vertical level. Swapping these passes
/// changes integer rounding, even though the real-valued separable transform would commute.
fn inverse_wavelet(bands: &[Vec<i32>], w: usize, h: usize) -> Result<Vec<i32>> {
    let mut out = vec![0; w * h];
    for (y, rows) in out.chunks_exact_mut(2 * w).enumerate() {
        let mut low = band_row(bands, 0, y, w.div_ceil(32))?.to_vec();
        for (band, level) in (1..=5).zip((1..=5).rev()) {
            let next_width = w.div_ceil(1 << (level - 1));
            let high = band_row(bands, band, y, next_width / 2)?;
            let mut next = vec![0; next_width];
            oxideav_jpegxs::dwt::inverse_horizontal_1d(&low, high, &mut next).map_err(corrupt)?;
            low = next;
        }
        let (even, odd) = rows.split_at_mut(w);
        even.copy_from_slice(&low);
        oxideav_jpegxs::dwt::inverse_horizontal_1d(band_row(bands, 6, y, w.div_ceil(2))?, band_row(bands, 7, y, w / 2)?, odd).map_err(corrupt)?;
    }
    // Inverse 5/3 vertical lifting, with symmetric extension of the updated even samples.
    for y in (0..h).step_by(2) {
        let above = if y == 0 { 1 } else { y - 1 };
        for x in 0..w {
            let a = *out.get(above * w + x).ok_or_else(|| corrupt("invalid vertical neighbour"))?;
            let b = *out.get((y + 1) * w + x).ok_or_else(|| corrupt("invalid vertical neighbour"))?;
            let value = out.get_mut(y * w + x).ok_or_else(|| corrupt("invalid vertical output"))?;
            *value -= (a + b + 2) >> 2;
        }
    }
    for y in (1..h).step_by(2) {
        let below = if y + 1 == h { y - 1 } else { y + 1 };
        for x in 0..w {
            let a = *out.get((y - 1) * w + x).ok_or_else(|| corrupt("invalid vertical neighbour"))?;
            let b = *out.get(below * w + x).ok_or_else(|| corrupt("invalid vertical neighbour"))?;
            let value = out.get_mut(y * w + x).ok_or_else(|| corrupt("invalid vertical output"))?;
            *value += (a + b) >> 1;
        }
    }
    Ok(out)
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    fn bits(out: &mut Vec<bool>, value: u64, count: usize) {
        out.extend((0..count).rev().map(|n| value & (1 << n) != 0));
    }

    fn pack(bits: &[bool]) -> Vec<u8> {
        bits.chunks(8).map(|chunk| chunk.iter().enumerate().fold(0, |v, (i, b)| v | (u8::from(*b) << (7 - i)))).collect()
    }

    fn marker(out: &mut Vec<u8>, code: u8, payload: &[u8]) {
        out.extend([0xff, code]);
        out.extend(((payload.len() + 2) as u16).to_be_bytes());
        out.extend(payload);
    }

    /// Original procedural fixture: constant but distinct Ya/Cr/Delta/Cb, raw GCLI, padded groups,
    /// and independently assembled packet headers. No camera bytes or external decoder code.
    pub(crate) fn fixture(width: usize, height: usize) -> Vec<u8> {
        fixture_sliced(width, height, 1)
    }

    /// [`fixture`] with `slice_height` precincts per slice (the last slice may be short).
    fn fixture_sliced(width: usize, height: usize, slice_height: usize) -> Vec<u8> {
        let mut out = vec![0xff, 0x10];
        marker(&mut out, 0x50, b"CONTACT_INTOPIX_\xef\xc0\0\0\0\0\0\0\0\0\0\0\0\0\0\0");
        let mut pih = vec![0; 16];
        pih[8..10].copy_from_slice(&(width as u16).to_be_bytes());
        pih[10..12].copy_from_slice(&(height as u16).to_be_bytes());
        pih[14..16].copy_from_slice(&(slice_height as u16).to_be_bytes()); // `fixture`: one precinct per slice, to exercise repeated slice headers.
        pih.extend([4, 4, 8, 18, 0x44, 0x10, 0x51, 0x14]);
        pih.extend([0x50, 0x88, 0x70, 0x83, 0xf0, 0x15, 0x23, 0xd1, 0x49, 0xcd, 0x3f, 0x7f, 7]);
        marker(&mut out, 0x12, &pih);
        marker(&mut out, 0x13, &[14, 0x11, 0x51, 14, 0x11, 0x51, 14, 0x11, 0, 14, 0x11, 0x51]);
        let weights: [(u8, u8); BANDS] = std::array::from_fn(|i| (0, i as u8));
        marker(&mut out, 0x14, &weights.iter().flat_map(|&(g, p)| [g, p]).collect::<Vec<_>>());
        let (geom, packets) = geometry(width / 2, &weights);
        let map = band_map();
        for precinct in 0..height / 4 {
            if precinct % slice_height == 0 {
                marker(&mut out, 0x20, &((precinct / slice_height) as u16).to_be_bytes());
            }
            let mut body = Vec::new();
            for packet in &packets {
                let (mut gcli, mut data, mut signs) = (Vec::new(), Vec::new(), Vec::new());
                for entry in &packet.entries {
                    let bi = entry.band as usize;
                    let (component, band) = map[bi];
                    let value: i32 = if band == 0 { [-800, 25, -6, -19][component] } else { 0 };
                    let magnitude = value.unsigned_abs();
                    let planes = 32 - magnitude.leading_zeros();
                    let width = geom.bands[bi].wpb as usize;
                    for group in 0..width.div_ceil(4) {
                        bits(&mut gcli, planes as u64, 4);
                        for plane in (0..planes).rev() {
                            for x in 0..4 {
                                data.push(group * 4 + x < width && magnitude & (1 << plane) != 0);
                            }
                        }
                        for x in 0..4 {
                            if magnitude != 0 && group * 4 + x < width {
                                signs.push(value < 0);
                            }
                        }
                    }
                }
                let (gcli, data, signs) = (pack(&gcli), pack(&data), pack(&signs));
                let mut header = Vec::new();
                bits(&mut header, 1, 1); // Raw GCLI: significance and predictor are bypassed.
                bits(&mut header, data.len() as u64, 20);
                bits(&mut header, gcli.len() as u64, 20);
                bits(&mut header, signs.len() as u64, 15);
                body.extend(pack(&header));
                body.extend(gcli);
                body.extend(data);
                body.extend(signs);
            }
            body.extend([0xa5, 0x5a]); // Precinct padding need not be zero.
            let length = (body.len() as u32).to_be_bytes();
            out.extend(&length[1..]);
            out.extend([0, 0]); // Q=R=0: no coefficient truncation.
            out.extend([0xff; 6]); // Predictor modes must not affect raw-GCLI packets.
            out.push(0xc0);
            out.extend(body);
        }
        out.extend([0xff, 0x11]);
        let length = out.len() as u32;
        out[42..46].copy_from_slice(&length.to_be_bytes());
        out
    }

    #[test]
    fn raw_gcli_constant_components_reconstruct_distinct_bayer_sites() {
        // Half-width 35 includes partial groups and uneven wavelet splits; three slices.
        let (w, h) = (70, 12);
        let (samples, black) = decode(&fixture(w, h), w, h, 14).unwrap();
        assert_eq!(black, 1008);
        // Hand calculation: pre-transfer R/G1/G2/B = -12424/-12776/-12872/-13128.
        for y in 0..h {
            for x in 0..w {
                assert_eq!(samples[y * w + x], [1114, 1097, 1092, 1081][2 * (y % 2) + x % 2]);
            }
        }
    }

    /// The decoded samples of the fixtures, and of 300 deterministic mutations of them (which make the bands
    /// non-constant, or fail), recorded before the band placement was factored out of `decode`: the placement must
    /// not change what comes out, valid or not.
    #[test]
    fn the_decode_of_the_fixtures_and_their_mutations_is_unchanged() {
        let mut h = 0xcbf29ce484222325u64;
        let mut feed = |bytes: &[u8]| bytes.iter().for_each(|&b| h = (h ^ u64::from(b)).wrapping_mul(0x100000001b3));
        let (mut ok, mut failed) = (0u32, 0u32);
        let mut seed = 12345u64;
        let mut next = move || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (seed >> 33) as usize
        };
        for (w, hgt, slice_height) in [(70usize, 12usize, 1usize), (130, 36, 1), (130, 36, 4), (64, 8, 2)] {
            let base = fixture_sliced(w, hgt, slice_height);
            for round in 0..75 {
                let mut bytes = base.clone();
                if round > 0 {
                    for _ in 0..1 + round % 4 {
                        let at = 160 + next() % (bytes.len() - 160);
                        bytes[at] = next() as u8;
                    }
                }
                match decode(&bytes, w, hgt, 14) {
                    Ok((samples, black)) => {
                        ok += 1;
                        feed(&black.to_le_bytes());
                        samples.iter().for_each(|v| feed(&v.to_le_bytes()));
                    }
                    Err(_) => {
                        // (which of several parallel failures is reported first varies, so only the refusal is recorded)
                        failed += 1;
                        feed(b"refused");
                    }
                }
            }
        }
        assert_eq!((ok, failed, h), (55, 245, 0x9f562f930d899ad6), "{ok} decoded, {failed} refused, hash {h:#x}");
    }

    /// The slice height only groups precincts under slice markers: the same precincts give the same samples whether
    /// the slices are one precinct, several (with a short final slice), or one slice for the whole picture.
    #[test]
    fn the_slice_height_does_not_change_the_samples() {
        let (w, h) = (130, 36); // nine precincts
        let want = decode(&fixture(w, h), w, h, 14).unwrap();
        for slice_height in [2, 3, 4, 8, 9, 10, 64] {
            let got = decode(&fixture_sliced(w, h, slice_height), w, h, 14).unwrap();
            assert_eq!(got, want, "slice height {slice_height}");
        }
    }

    /// Sample `k` of band `band`'s part of precinct `p`: distinct for every (band, precinct, sample), so a part that lands
    /// in the wrong place, or twice, shows.
    fn part(band: usize, p: usize, len: usize) -> Vec<i32> {
        (0..len).map(|k| (band * 100_003 + p * 1_009 + k * 7 + 1) as i32).collect()
    }

    /// Places every precinct of `precincts` (slices of `slice_height`) through `band_ranges` and `place_precinct`, as
    /// `decode` does; returns the bands as placed and the plain concatenation of each band's parts.
    fn placed(stride: &[usize], slice_height: usize, precincts: usize) -> (Vec<Vec<i32>>, Vec<Vec<i32>>) {
        let mut gathered: Vec<Vec<i32>> = stride.iter().map(|&n| vec![-1; n * precincts]).collect();
        let expected: Vec<Vec<i32>> = stride.iter().enumerate().map(|(b, &n)| (0..precincts).flat_map(|p| part(b, p, n)).collect()).collect();
        let mut ranges = band_ranges(&mut gathered, stride, slice_height, precincts);
        assert_eq!(ranges.len(), precincts.div_ceil(slice_height), "slices, stride {stride:?}, height {slice_height}, {precincts} precincts");
        for (s, slice) in ranges.iter_mut().enumerate() {
            assert_eq!(slice.len(), stride.len());
            for i in 0..slice_height.min(precincts - s * slice_height) {
                let dequantized = stride.iter().enumerate().map(|(b, &n)| part(b, s * slice_height + i, n)).collect();
                place_precinct(slice, i, dequantized, stride).unwrap();
            }
        }
        (gathered, expected)
    }

    /// Every precinct's part of every band lands where concatenating the parts puts it, for bands of different strides,
    /// slice heights that do and do not divide the precinct count, a slice taller than the picture, and one precinct.
    #[test]
    fn placed_precincts_equal_the_concatenation_of_their_parts() {
        for stride in [vec![3, 5, 1, 8, 2], vec![7, 2, 6], vec![4], vec![2, 2, 2, 2]] {
            for slice_height in [1, 2, 3, 5, 7, 8] {
                for precincts in [1, 2, 7] {
                    let (got, want) = placed(&stride, slice_height, precincts);
                    assert_eq!(got, want, "stride {stride:?}, slice height {slice_height}, {precincts} precincts");
                }
            }
        }
    }

    /// A precinct whose band has the wrong size, or a precinct with the wrong number of bands, is refused and writes nothing
    /// outside its place; so is a position past the slice's range.
    #[test]
    fn a_misshapen_precinct_is_refused() {
        let stride = [3usize, 5, 2];
        let fresh = || -> Vec<Vec<i32>> { stride.iter().map(|&n| vec![-1; n * 3]).collect() };
        let good = || -> Vec<Vec<i32>> { stride.iter().enumerate().map(|(b, &n)| part(b, 0, n)).collect() };
        for bad_band in 0..stride.len() {
            for len in [0, stride[bad_band] - 1, stride[bad_band] + 1] {
                let mut gathered = fresh();
                let mut ranges = band_ranges(&mut gathered, &stride, 3, 3);
                let mut dequantized = good();
                dequantized[bad_band] = vec![9; len];
                assert!(place_precinct(&mut ranges[0], 1, dequantized, &stride).is_err(), "band {bad_band} of {len} samples");
            }
        }
        for bands in [0, 2, 4] {
            let mut gathered = fresh();
            let mut ranges = band_ranges(&mut gathered, &stride, 3, 3);
            let mut dequantized = good();
            dequantized.resize(bands, vec![0; 3]);
            assert!(place_precinct(&mut ranges[0], 0, dequantized, &stride).is_err(), "{bands} bands");
        }
        let mut gathered = fresh();
        let mut ranges = band_ranges(&mut gathered, &stride, 3, 3);
        for i in [3, 4, usize::MAX / 2, usize::MAX] {
            assert!(place_precinct(&mut ranges[0], i, good(), &stride).is_err(), "precinct {i} of a slice of three");
        }
        // the last one fits exactly
        place_precinct(&mut ranges[0], 2, good(), &stride).unwrap();
        // a slice with the wrong number of band ranges
        let mut short = ranges.remove(0);
        short.pop();
        assert!(place_precinct(&mut short, 0, good(), &stride).is_err());
    }

    #[test]
    fn the_parallel_star_tetrix_matches_the_reference_transform() {
        // odd and even sizes, down to one sample: every edge case of the mirrored access, and sizes with an interior
        // (three or more rows and columns) where the fast rows run, at the size limit of the codec's range
        // (±2^19 after dequantising, ±2^24 as the largest the decoder's range check lets through before this step)
        let sizes = [
            (1, 1),
            (2, 1),
            (1, 3),
            (3, 1),
            (2, 2),
            (3, 3),
            (3, 4),
            (4, 3),
            (5, 4),
            (8, 7),
            (35, 6),
            (64, 33),
            (130, 9),
            (2, 100),
            (100, 2),
            (257, 5),
        ];
        // the generator's values span [-magnitude / 2, magnitude / 2), so the second reaches the decoder's ±2^24 bound
        for magnitude in [1i32 << 20, 1 << 25] {
            for (w, h) in sizes {
                let mut seed = (w * 31 + h) as u64 ^ magnitude as u64;
                let mut next = || {
                    seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                    ((seed >> 33) as i32 % magnitude) - magnitude / 2
                };
                let planes: [Vec<i32>; 4] = std::array::from_fn(|_| (0..w * h).map(|_| next()).collect());
                let mut reference = planes.clone();
                let mut slices: Vec<&mut [i32]> = reference.iter_mut().map(Vec::as_mut_slice).collect();
                oxideav_jpegxs::colour_transform::inverse_star_tetrix(&mut slices, w, h, 0, 0, 0, 0).unwrap();
                assert_eq!(inverse_star_tetrix(planes, w, h), reference, "{w}x{h}, magnitude {magnitude}");
            }
        }
    }

    /// The tap table of `lift` (which plane and offset each neighbour comes from away from the edges) must agree with
    /// `neighbour`, the definition, at every interior sample of a marked-up picture and every component.
    #[test]
    fn the_unmirrored_taps_agree_with_the_neighbour_definition() {
        let (w, h) = (9usize, 7usize);
        let planes: [Vec<i32>; 4] = std::array::from_fn(|k| (0..w * h).map(|i| (k * 1000 + i) as i32).collect());
        let refs = [&planes[0][..], &planes[1][..], &planes[2][..], &planes[3][..]];
        for c in 0..4 {
            for (rx, ry) in [(-1, -1), (1, -1), (-1, 1), (1, 1), (-1, 0), (1, 0), (0, -1), (0, 1)] {
                let (component, ox, oy) = taps(c, [(rx, ry); 4])[0];
                for y in 1..h - 1 {
                    for x in 1..w - 1 {
                        let want = neighbour(&refs, w, h, c, x, y, rx, ry);
                        let got = planes[component][((y as isize + oy) as usize) * w + (x as isize + ox) as usize];
                        assert_eq!(got, want, "component {c}, offset ({rx}, {ry}) at ({x}, {y})");
                    }
                }
            }
        }
    }

    /// `lift_in_place` takes the component's own plane out of the array: that is only right if no neighbour of a
    /// component is the component itself, at any of the offsets the transform uses (edges mirror the offset, which keeps
    /// its parity).
    #[test]
    fn no_neighbour_is_the_component_itself() {
        let diagonal = [(-1, -1), (1, -1), (-1, 1), (1, 1)];
        let cross = [(-1, 0), (1, 0), (0, -1), (0, 1)];
        for c in 0..4 {
            for (rx, ry) in diagonal.into_iter().chain(cross) {
                assert_ne!(component_of(c, rx, ry), c, "component {c} reads itself at ({rx}, {ry})");
                assert_ne!(component_of(c, -rx, -ry), c, "component {c} reads itself at ({}, {})", -rx, -ry);
            }
        }
    }

    /// Planes whose length is not `w × h` (short, long, empty) give planes of `w × h` and never panic, as before.
    #[test]
    fn star_tetrix_with_wrong_plane_lengths_does_not_panic() {
        for (w, h) in [(0, 0), (0, 5), (5, 0), (1, 1), (3, 3), (9, 7)] {
            for len in [0, 1, w * h / 2, w * h, w * h + 5] {
                let planes: [Vec<i32>; 4] = std::array::from_fn(|k| (0..len).map(|i| (i + k) as i32 - 3).collect());
                let out = inverse_star_tetrix(planes, w, h);
                assert!(out.iter().all(|p| p.len() == w * h), "{w}x{h} from planes of {len}");
            }
        }
    }

    #[test]
    fn every_truncation_is_rejected() {
        let bytes = fixture(70, 12);
        for end in 0..bytes.len() {
            let mut truncated = bytes[..end].to_vec();
            // Keep the declared size consistent to reach the inner packet bounds checks too.
            if end >= 46 {
                truncated[42..46].copy_from_slice(&(end as u32).to_be_bytes());
            }
            assert!(decode(&truncated, 70, 12, 14).is_err(), "accepted truncation at {end}");
        }
    }

    #[test]
    fn malformed_headers_dimensions_and_lengths_are_rejected() {
        let bytes = fixture(70, 12);
        for (at, value) in
            [(4, 0xff), (57, 0), (58, 0), (63, 0), (64, 0), (67, 0), (78, 0), (99, 255), (100, 255), (154, 1), (155, 0xff), (158, 32), (159, 25)]
        {
            let mut bad = bytes.clone();
            bad[at] = value;
            assert!(decode(&bad, 70, 12, 14).is_err(), "accepted mutation at {at}");
        }
        assert!(decode(&bytes, 72, 12, 14).is_err());
        assert!(decode(&bytes, 70, 12, 8).is_err());
        assert!(decode(&bytes, usize::MAX, 12, 14).is_err());
        assert!(decode(&bytes, 65534, 65532, 14).is_err());
    }

    #[test]
    fn transfer_rounding_negative_branch_and_clipping() {
        let normal = Transfer { black: 1008, gain: 84429, pivot: 16255 };
        assert_eq!(normal.apply(16255 - 32768), 1008);
        assert_eq!(normal.apply(16000 - 32768), 1008);
        assert_eq!(normal.apply(i32::MIN), 0);
        assert_eq!(normal.apply(i32::MAX), 16383);
        let ties = Transfer { gain: 65905, ..normal };
        assert_eq!(ties.apply(49023 - 32768), 5151); // Half-up, not ties-to-even (5150).
        assert_eq!(ties.apply(i32::MAX), ties.apply(65535 - 32768));
    }

    proptest::proptest! {
        #![proptest_config(proptest::test_runner::Config::with_cases(128))]
        #[test]
        fn mutated_codestreams_do_not_panic(changes in proptest::collection::vec((0usize..2048, proptest::num::u8::ANY), 1..16)) {
            let mut bytes = fixture(70, 12);
            for (offset, value) in changes { let at = offset % bytes.len(); bytes[at] = value; }
            if let Ok((samples, black)) = decode(&bytes, 70, 12, 14) {
                proptest::prop_assert_eq!(samples.len(), 70 * 12);
                proptest::prop_assert!(samples.iter().all(|&v| v <= 16383));
                proptest::prop_assert!(black <= 4095);
            }
        }
    }
}
