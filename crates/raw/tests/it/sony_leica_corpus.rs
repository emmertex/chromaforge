//! One CC0 file per Sony and Leica body not covered by an earlier check from raw.pixls.us (URLs and SHA-256 in `RAW_SAMPLES`,
//! xtask/src/main.rs; downloaded into the gitignored corpus/raw by `cargo xtask corpus --download`).
//!
//! No external decoder was run on these: the check against the camera is
//! black-box: the green sites of the active area, on a 24 × 16 grid, must follow the camera's own embedded
//! JPEG (gamma-encoded Pearson r; 0.62–1.00 on these files, a misread layout or stream gives r near 0). Five files are
//! not compared with the JPEG: the M8 and M9 DNGs carry no JPEG, the two M Monochrom bodies have no colour filter, and the
//! M (Typ 240) preview covers a different field (r 0.58); their renders were inspected by eye.
//! Missing files are reported as skipped.

use lcc_raw::{RawData, decode, embedded_preview, probe_info};
use std::path::{Path, PathBuf};

fn corpus() -> PathBuf {
    lcc_brand::env_os("CORPUS").map(PathBuf::from).unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus"))
}

/// Green of the decoded mosaic against the camera's embedded JPEG on a `gw × gh` grid of the active area.
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
    let cfa = raw.cfa.clone().expect("colour-filter layout");
    let (a, black, white) = (raw.active_area, f64::from(raw.black.mean()), f64::from(raw.white_at(0)));
    let (mut rs, mut rn) = (vec![0f64; gw * gh], vec![0f64; gw * gh]);
    for y in a.y..a.y + a.height {
        for x in a.x..a.x + a.width {
            if cfa.color_at(x, y) == 1 {
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

#[test]
fn sony_and_leica_bodies_decode_like_the_camera_jpeg() {
    // (file, sensor width × height, compare with the camera JPEG)
    let cases: &[(&str, (usize, usize), bool)] = &[
        ("arw-sony-dsc-rx100m7a.arw", (5504, 3672), true),
        ("arw-sony-dsc-rx1rm3.arw", (9600, 6376), true),
        ("arw-sony-ilce-1m2.arw", (8672, 5784), true),
        ("arw-sony-ilce-6400a.arw", (6048, 4024), true),
        ("arw-sony-ilce-7m5.arw", (7168, 5120), true),
        ("arw-sony-ilme-fx2.arw", (7040, 4688), true),
        ("arw-sony-zv-1m2.arw", (5504, 3672), true),
        ("arw-sony-zv-e10m2.arw", (6656, 4608), true),
        ("dng-leica-cl.dng", (6120, 4016), true),
        ("dng-leica-m-monochrom-typ-246.dng", (5984, 4000), false),
        ("dng-leica-m-monochrom.dng", (5216, 3472), false),
        ("dng-leica-m-typ-240.dng", (5984, 4000), false),
        ("dng-leica-m10-r.dng", (7872, 5208), true),
        ("dng-leica-m10.dng", (5984, 3992), true),
        ("dng-leica-m8.dng", (3920, 2638), false),
        ("dng-leica-m9.dng", (5216, 3472), false),
        ("dng-leica-q-typ-116.dng", (6120, 4016), true),
        ("dng-leica-q2.dng", (8424, 5632), true),
        ("dng-leica-sl-typ-601.dng", (6120, 4016), true),
        ("dng-leica-sl2.dng", (8424, 5632), true),
        ("dng-leica-x2.dng", (4944, 3272), true),
        ("rwl-leica-c-lux.rwl", (5488, 3664), true),
    ];
    let mut seen = 0;
    for &(name, sensor, jpeg_check) in cases {
        let path = corpus().join("raw").join(name);
        let Ok(bytes) = std::fs::read(&path) else {
            lcc_testkit::skip!(Corpus, "{} absent", path.display());
            continue;
        };
        let raw = decode(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
        raw.validate().unwrap();
        assert_eq!((raw.width, raw.height, raw.cpp), (sensor.0, sensor.1, 1), "{name}: sensor size");
        assert_eq!(raw.info(), probe_info(&bytes).unwrap(), "{name}: probe and decode disagree");
        assert!(matches!(raw.data, RawData::U16(_)), "{name}: integer sensor data expected");
        assert!(raw.white_at(0) > raw.black.mean(), "{name}: invalid black/white levels");
        if !jpeg_check {
            seen += 1;
            continue;
        }
        let jpeg = embedded_preview(&bytes).unwrap_or_else(|| panic!("{name}: no camera JPEG"));
        let r = green_vs_camera_jpeg(&raw, &jpeg, 24, 16);
        eprintln!("{name:40} {}x{}: green follows the camera JPEG, r = {r:.3}", raw.width, raw.height);
        assert!(r > 0.6, "{name}: decoded mosaic does not follow the camera JPEG (r = {r:.3})");
        seen += 1;
    }
    eprintln!("Sony / Leica bodies checked on {seen} files");
}
