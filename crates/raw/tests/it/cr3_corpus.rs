//! CR3 structure checks (sensor size, crop, developed size, white balance, levels, probe/decode
//! agreement) and the camera-JPEG correlation, on CC0 inputs from raw.pixls.us kept in the gitignored
//! corpus/raw. Sample values are not pinned.

use lcc_raw::{RawData, RawFormat, decode, probe_info};
use std::path::{Path, PathBuf};

fn corpus() -> PathBuf {
    lcc_brand::env_os("CORPUS").map(PathBuf::from).unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus"))
}

fn check(name: &str, sensor: (usize, usize), developed: (usize, usize)) {
    check_against(name, sensor, Some(developed));
}

fn check_against(name: &str, sensor: (usize, usize), developed: Option<(usize, usize)>) {
    let path = corpus().join("raw").join(name);
    let Ok(bytes) = std::fs::read(&path) else {
        lcc_testkit::skip!(Corpus, "{} absent", path.display());
        return;
    };
    let raw = decode(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
    assert_eq!(raw.format, RawFormat::Cr3);
    raw.validate().unwrap();
    assert_eq!((raw.width, raw.height, raw.cpp), (sensor.0, sensor.1, 1));
    if let Some(developed) = developed {
        assert_eq!(raw.info().developed_size(), developed);
    }
    assert_eq!(raw.info(), probe_info(&bytes).unwrap());
    assert!(matches!(raw.data, RawData::U16(_)), "{name}: integer sensor data expected");
    assert!(raw.wb_multipliers.is_some(), "{name}: as-shot white balance missing");
    assert!(raw.white_at(0) > raw.black.mean(), "{name}: invalid black/white levels");
}

/// (file, sensor, developed size). The M50 CRAW was shot 1:1: its AspectInfo is a 4000 x 4000 square inside
/// the 6000 x 4000 image. The crop-mode RP files are a single tile.
#[test]
fn canon_cr3_sensor_and_developed_sizes() {
    let cases: &[(&str, (usize, usize), (usize, usize))] = &[
        ("cr3-canon-m50-raw.cr3", (6288, 4056), (6000, 4000)),
        ("cr3-canon-r100-raw.cr3", (6288, 4056), (6000, 4000)),
        ("cr3-canon-m50-craw.cr3", (6288, 4056), (4000, 4000)),
        ("cr3-canon-r100-craw.cr3", (6288, 4056), (6000, 4000)),
        ("cr3-canon-r8-raw.cr3", (6188, 4120), (6000, 4000)),
        ("cr3-canon-r8-craw.cr3", (6188, 4120), (6000, 4000)),
        ("cr3-canon-rp-raw.cr3", (6384, 4224), (6240, 4160)),
        ("cr3-canon-rp-craw.cr3", (6384, 4224), (6240, 4160)),
        ("cr3-canon-rp-raw-crop.cr3", (4032, 2656), (3888, 2592)),
        ("cr3-canon-rp-craw-crop.cr3", (4032, 2656), (3888, 2592)),
        // Private EOS R6 files supplied by the maintainer; absent for everyone else.
        ("cr3-canon-r6-1.cr3", (5568, 3708), (5472, 3648)),
        ("cr3-canon-r6-2.cr3", (5568, 3708), (5472, 3648)),
        ("cr3-canon-r6-3.cr3", (5568, 3708), (5472, 3648)),
        ("cr3-canon-r6-4.cr3", (5568, 3708), (5472, 3648)),
        ("cr3-canon-r6-5.cr3", (5568, 3708), (5472, 3648)),
    ];
    for &(name, sensor, developed) in cases {
        check(name, sensor, developed);
    }
}

#[test]
fn canon_m50_craw_crop_is_the_shot_aspect() {
    // raw.pixls.us/getfile.php/2663: the 1:1 square starts 1000 px right of the recommended crop's origin,
    // which is (12, 12) inside the valid sensor area.
    let path = corpus().join("raw").join("cr3-canon-m50-craw.cr3");
    let Ok(bytes) = std::fs::read(&path) else {
        lcc_testkit::skip!(Corpus, "{} absent", path.display());
        return;
    };
    let raw = decode(&bytes).unwrap();
    assert_eq!((raw.crop.x, raw.crop.y, raw.crop.width, raw.crop.height), (1012, 12, 4000, 4000));
}

/// Green of the decoded mosaic against the camera's embedded JPEG, on a `gw × gh` grid of the active area
/// (gamma-encoded, Pearson). A misread CRX stream (wrong tile, plane, band or quantiser) leaves a picture that
/// no longer follows the camera's own rendering; lens-corrected compact JPEGs keep it well above 0.6.
fn green_vs_camera_jpeg(raw: &lcc_raw::RawImage, jpeg: &[u8], gw: usize, gh: usize) -> f64 {
    use zune_core::{bytestream::ZCursor, colorspace::ColorSpace, options::DecoderOptions};
    let opts = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGB);
    let mut d = zune_jpeg::JpegDecoder::new_with_options(ZCursor::new(jpeg), opts);
    let px = d.decode().unwrap();
    let info = d.info().unwrap();
    let (jw, jh) = (info.width as usize, info.height as usize);
    let mean = |sum: Vec<f64>, n: Vec<f64>| sum.iter().zip(&n).map(|(s, n)| s / n.max(1.0)).collect::<Vec<f64>>();
    let (mut js, mut jn) = (vec![0f64; gw * gh], vec![0f64; gw * gh]);
    for y in 0..jh {
        for x in 0..jw {
            let i = (y * gh / jh) * gw + x * gw / jw;
            js[i] += f64::from(px[(y * jw + x) * 3 + 1]);
            jn[i] += 1.0;
        }
    }
    let RawData::U16(samples) = &raw.data else { panic!("integer sensor data expected") };
    let cfa = raw.cfa.clone().expect("Bayer layout");
    let (a, black, white) = (raw.active_area, f64::from(raw.black.mean()), f64::from(raw.white_at(0)));
    let (mut rs, mut rn) = (vec![0f64; gw * gh], vec![0f64; gw * gh]);
    for y in a.y..a.y + a.height {
        for x in a.x..a.x + a.width {
            if cfa.pattern[(y % 2) * 2 + x % 2] == 1 {
                let i = ((y - a.y) * gh / a.height) * gw + (x - a.x) * gw / a.width;
                rs[i] += ((f64::from(samples[y * raw.width + x]) - black).max(0.0) / (white - black)).powf(1.0 / 2.2);
                rn[i] += 1.0;
            }
        }
    }
    let (r, j) = (mean(rs, rn), mean(js, jn));
    let n = r.len() as f64;
    let (mr, mj) = (r.iter().sum::<f64>() / n, j.iter().sum::<f64>() / n);
    let cov: f64 = r.iter().zip(&j).map(|(x, y)| (x - mr) * (y - mj)).sum();
    let vr: f64 = r.iter().map(|x| (x - mr).powi(2)).sum();
    let vj: f64 = j.iter().map(|y| (y - mj).powi(2)).sum();
    cov / (vr * vj).sqrt()
}

/// One CC0 file per further CR3 body (raw.pixls.us; URLs and SHA-256 in `RAW_SAMPLES`, xtask/src/main.rs).
/// No external decoder was run on these: every CRX stream
/// decodes, and the mosaic follows the camera's own embedded JPEG (`green_vs_camera_jpeg` on 24 × 16 cells).
/// The renders were also inspected at 100 % for tile, band and quantiser artefacts (none). See docs/cr3.md.
#[test]
fn canon_cr3_bodies_decode_like_the_camera_jpeg() {
    // (file, sensor width × height)
    let cases: &[(&str, (usize, usize))] = &[
        ("cr3-canon-m200.cr3", (6288, 4056)),
        ("cr3-canon-r10.cr3", (6192, 4060)),
        ("cr3-canon-r50v.cr3", (6192, 4060)),
        ("cr3-canon-r6m3.cr3", (7144, 4760)),
        ("cr3-canon-powershot-v1-craw.cr3", (5928, 3944)),
        ("cr3-canon-1dxm3-craw.cr3", (5568, 3708)),
        ("cr3-canon-250d-craw.cr3", (6288, 4056)),
        ("cr3-canon-850d.cr3", (6288, 4056)),
        ("cr3-canon-90d-craw.cr3", (7128, 4732)),
        ("cr3-canon-m50m2.cr3", (6288, 4056)),
        ("cr3-canon-m6m2-craw.cr3", (7128, 4732)),
        ("cr3-canon-r3.cr3", (6192, 4152)),
        ("cr3-canon-r50-craw.cr3", (6192, 4060)),
        ("cr3-canon-r5-craw.cr3", (8352, 5586)),
        ("cr3-canon-r6-craw.cr3", (5568, 3708)),
        ("cr3-canon-r6m2-craw.cr3", (6188, 4120)),
        ("cr3-canon-r7.cr3", (7128, 4732)),
        ("cr3-canon-r-craw.cr3", (6888, 4546)),
        ("cr3-canon-rp-craw.cr3", (6384, 4224)),
        ("cr3-canon-g5xm2-craw.cr3", (5632, 3710)),
        ("cr3-canon-g7xm3.cr3", (5632, 3710)),
        ("cr3-canon-sx70.cr3", (5344, 3950)),
    ];
    let mut seen = 0;
    for &(name, sensor) in cases {
        let path = corpus().join("raw").join(name);
        let Ok(bytes) = std::fs::read(&path) else {
            lcc_testkit::skip!(Corpus, "{} absent", path.display());
            continue;
        };
        check_against(name, sensor, None);
        let raw = decode(&bytes).unwrap();
        let jpeg = lcc_raw::embedded_preview(&bytes).unwrap_or_else(|| panic!("{name}: no camera JPEG"));
        let r = green_vs_camera_jpeg(&raw, &jpeg, 24, 16);
        eprintln!("{name:34} {}x{}: green follows the camera JPEG, r = {r:.3}", raw.width, raw.height);
        assert!(r > 0.6, "{name}: decoded mosaic does not follow the camera JPEG (r = {r:.3})");
        seen += 1;
    }
    eprintln!("CR3 bodies checked on {seen} files");
}
