//! A pixel-shift merge made by this app (a DNG with a provenance block) is read like the camera's own raw: the same
//! data as the NEFX of its series, so no colour matrices, the white balance as multipliers, its first frame's JPEG as
//! its preview. A DNG of any other writer is read as it always was.

use lcc_color::Mat3;
use lcc_raw::merge_provenance::Provenance;
use lcc_raw::{
    BlackLevel, ColorData, DngPreview, DngWriteOptions, Orientation, PreviewColorSpace, RawData, RawFormat, RawImage, Rect, decode, embedded_preview,
    embedded_preview_color_space, write_dng,
};

/// 3 samples per pixel, as a merge writes them; the camera's colour as DNG tags with a baseline exposure.
fn linear_raw() -> RawImage {
    let (w, h) = (32usize, 24usize);
    let data: Vec<u16> = (0..w * h * 3).map(|i| 1100 + ((i * 37) % 9000) as u16).collect();
    RawImage {
        format: RawFormat::Dng,
        width: w,
        height: h,
        cpp: 3,
        data: RawData::U16(data),
        cfa: None,
        bits: 14,
        black: BlackLevel::uniform(1008.0),
        white: vec![16383.0],
        active_area: Rect::new(0, 0, w, h),
        crop: Rect::new(2, 2, 28, 20),
        orientation: Orientation::Normal,
        color: ColorData {
            illuminant: [21, 0],
            color_matrix: [Some(Mat3([[1.3, -0.2, -0.1], [-0.4, 1.5, 0.05], [0.0, -0.1, 0.8]])), None],
            as_shot_neutral: Some([0.5, 1.0, 0.7]),
            baseline_exposure: 0.018,
            ..Default::default()
        },
        wb_multipliers: None,
        linearized: false,
        pixel_aspect: 1.0,
        opcodes: Default::default(),
        metadata: lcc_meta::Metadata { make: Some("NIKON CORPORATION".into()), model: Some("NIKON Z 8".into()), ..Default::default() },
        lens_metadata: Default::default(),
    }
}

fn jpeg(w: u16, h: u16) -> Vec<u8> {
    let rgb: Vec<u8> = (0..usize::from(w) * usize::from(h) * 3).map(|i| (i * 7 % 251) as u8).collect();
    let mut out = Vec::new();
    jpeg_encoder::Encoder::new(&mut out, 90).encode(&rgb, w, h, jpeg_encoder::ColorType::Rgb).unwrap();
    out
}

fn provenance() -> Provenance {
    Provenance { engine: 1, shot_count: 16, reference: 1, scale: 2, window: None, frames: vec![], registration: None }
}

fn written(merged: bool, preview: Option<DngPreview>) -> Vec<u8> {
    let raw = linear_raw();
    let xmp = lcc_meta::write_xmp(&raw.metadata, None);
    let xmp = if merged { provenance().add_to_packet(&xmp) } else { xmp };
    write_dng(&raw, &DngWriteOptions { xmp: Some(xmp), preview, ..Default::default() }).unwrap()
}

#[test]
fn a_merged_dng_is_read_like_the_cameras_raw() {
    let jpeg = jpeg(48, 32);
    let bytes = written(true, Some(DngPreview { jpeg: jpeg.clone(), color_space: Some(PreviewColorSpace::AdobeRgb) }));
    let raw = decode(&bytes).unwrap();
    assert_eq!(raw.format, RawFormat::Nef, "read as the camera's raw");
    assert!(!lcc_raw::color::has_matrix(&raw.color), "no colour matrices");
    assert_eq!(raw.color.baseline_exposure, 0.0, "no baseline exposure");
    assert!(raw.color.as_shot_neutral.is_none() && raw.color.profile.tone_curve.is_none());
    let m = raw.wb_multipliers.expect("white balance as multipliers");
    assert!((m[0] - 2.0).abs() < 1e-4 && m[1] == 1.0 && (m[2] - 1.0 / 0.7).abs() < 1e-4, "{m:?}");
    assert_eq!((raw.cpp, raw.black.values.first().copied()), (3, Some(1008.0)));
    // the camera's JPEG, byte for byte, and the space the camera said it is in
    assert_eq!(embedded_preview(&bytes).as_deref(), Some(jpeg.as_slice()));
    assert_eq!(embedded_preview_color_space(&bytes), Some(PreviewColorSpace::AdobeRgb));
}

#[test]
fn a_dng_of_another_writer_is_read_as_before() {
    // the same file without the provenance block, with a preview that names its colour space
    let jpeg = jpeg(48, 32);
    let bytes = written(false, Some(DngPreview { jpeg: jpeg.clone(), color_space: Some(PreviewColorSpace::AdobeRgb) }));
    let raw = decode(&bytes).unwrap();
    assert_eq!(raw.format, RawFormat::Dng);
    assert!(lcc_raw::color::has_matrix(&raw.color), "its colour matrices");
    assert!((raw.color.baseline_exposure - 0.018).abs() < 1e-6, "its baseline exposure");
    assert_eq!(raw.color.as_shot_neutral, Some([0.5, 1.0, 0.7]));
    assert!(raw.wb_multipliers.is_none());
    // a DNG's preview is the converter's: its colour space is not taken from a tag nobody asked for
    assert_eq!(embedded_preview_color_space(&bytes), None);
    assert_eq!(embedded_preview(&bytes).as_deref(), Some(jpeg.as_slice()));
    // and without any preview there is none
    assert!(embedded_preview(&written(false, None)).is_none());
}

#[test]
fn a_merged_dng_without_a_preview_still_reads_as_the_cameras_raw() {
    let bytes = written(true, None);
    let raw = decode(&bytes).unwrap();
    assert_eq!(raw.format, RawFormat::Nef);
    assert!(embedded_preview(&bytes).is_none());
    assert_eq!(embedded_preview_color_space(&bytes), None);
}

/// A merged DNG whose provenance block is `block` (any block, valid or not), with `pad` spaces after the XMP.
fn written_with(block: &Provenance, pad: usize) -> Vec<u8> {
    let raw = linear_raw();
    let mut xmp = block.add_to_packet(&lcc_meta::write_xmp(&raw.metadata, None));
    xmp.push_str(&" ".repeat(pad));
    write_dng(&raw, &DngWriteOptions { xmp: Some(xmp), ..Default::default() }).unwrap()
}

fn frame(i: u16) -> lcc_raw::merge_provenance::SourceFrame {
    lcc_raw::merge_provenance::SourceFrame { index: i, file_name: format!("f{i}.nef"), captured: String::new(), counter: 100 + u32::from(i) }
}

/// A provenance block that no merge wrote does not make a merge: the file reads as the normal DNG it is, with its own
/// colour tags, and the reader allocates nothing for the block.
#[test]
fn a_dng_with_a_hostile_provenance_reads_as_a_normal_dng() {
    use lcc_raw::merge_provenance::{MAX_PACKET_BYTES, Window};
    let good = Provenance { frames: (1..=16).map(frame).collect(), ..provenance() };
    let hostile: Vec<(&str, Provenance, usize)> = vec![
        ("shot count outside the set", Provenance { shot_count: 3, ..good.clone() }, 0),
        ("shot count at the end of its range", Provenance { shot_count: u16::MAX, ..good.clone() }, 0),
        ("scale of nothing", Provenance { scale: 0, ..good.clone() }, 0),
        ("more frames than shots", Provenance { shot_count: 4, frames: (1..=5).map(frame).collect(), ..good.clone() }, 0),
        ("a frame twice", Provenance { frames: vec![frame(1), frame(1)], ..good.clone() }, 0),
        ("a frame number past the shots", Provenance { frames: vec![frame(17)], ..good.clone() }, 0),
        (
            "a window outside its sensor",
            Provenance { window: Some(Window { sensor: [100, 100], origin: [90, 0], size: [20, 20] }), ..good.clone() },
            0,
        ),
        (
            "a window at the end of numbers",
            Provenance { window: Some(Window { sensor: [100, 100], origin: [usize::MAX, 0], size: [1, 1] }), ..good.clone() },
            0,
        ),
        ("a packet past the limit", good.clone(), MAX_PACKET_BYTES),
    ];
    for (what, block, pad) in hostile {
        let bytes = written_with(&block, pad);
        let raw = decode(&bytes).unwrap_or_else(|e| panic!("{what}: {e}"));
        assert_eq!(raw.format, RawFormat::Dng, "{what}");
        assert!(lcc_raw::color::has_matrix(&raw.color), "{what}: its own colour tags");
        assert!(raw.wb_multipliers.is_none() && raw.metadata.software != Some(lcc_raw::merge_provenance::software()), "{what}");
        assert_eq!(lcc_raw::merge_provenance::from_dng(&bytes), None, "{what}");
    }
    // the same file with a good block is a merge
    let raw = decode(&written_with(&good, 0)).unwrap();
    assert_eq!(raw.format, RawFormat::Nef);
}

/// Mutated merged DNGs (their provenance, preview, tags and strips damaged): read, previewed and checked without a panic.
#[test]
fn mutated_merged_dngs_never_panic() {
    let jpeg = jpeg(48, 32);
    let preview = DngPreview { jpeg, color_space: Some(PreviewColorSpace::AdobeRgb) };
    let seeds =
        [written(true, Some(preview)), written_with(&Provenance { frames: (1..=16).map(frame).collect(), ..provenance() }, 0), written(false, None)];
    let refs: Vec<&[u8]> = seeds.iter().map(Vec::as_slice).collect();
    lcc_fuzzkit::run("merged_dng", &refs, 1500, |b| {
        let _ = decode(b);
        let _ = embedded_preview(b);
        let _ = embedded_preview_color_space(b);
        let _ = lcc_raw::merge_provenance::from_dng(b);
    });
}

/// The provenance text itself, mutated: a block or nothing, never a panic.
#[test]
fn mutated_provenance_packets_never_panic() {
    let packet = Provenance { frames: (1..=16).map(frame).collect(), ..provenance() }.add_to_packet(&lcc_meta::write_xmp(&Default::default(), None));
    lcc_fuzzkit::run_str("merge_provenance.packet", &[packet.as_str()], 5000, |s| {
        if let Some(p) = Provenance::from_packet(s) {
            // (whatever reads is within the limits)
            assert!(p.frames.len() <= lcc_raw::merge_provenance::MAX_FRAMES && lcc_raw::merge_provenance::SHOT_COUNTS.contains(&p.shot_count));
        }
        let _ = lcc_raw::merge_provenance::is_valid(s.as_bytes());
    });
}
