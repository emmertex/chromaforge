//! One CC0 file per Canon CR2, Pentax and Ricoh body from raw.pixls.us (URLs and SHA-256 in `RAW_SAMPLES`,
//! xtask/src/main.rs; downloaded into the gitignored corpus/raw by `cargo xtask corpus --download`).
//!
//! No external decoder was run on these: the check against the camera is
//! black-box: the green sites of the active area, on a 24 × 16 grid, must follow the camera's own embedded
//! JPEG (gamma-encoded Pearson r; 0.69–1.00 on these files, a misread layout or stream gives r near 0). Four files are
//! not compared with the JPEG: two have no embedded JPEG (GX200, and the PowerShot S2 IS, a DNG written by CHDK), one is monochrome (K-3 Mark III
//! Monochrome), and the GXR's preview is not oriented like its sensor; their renders were inspected by eye.
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
fn canon_pentax_and_ricoh_bodies_decode_like_the_camera_jpeg() {
    // (file, sensor width × height, compare with the camera JPEG)
    let cases: &[(&str, (usize, usize), bool)] = &[
        ("cr2-canon-eos-1000d.cr2", (3948, 2622), true),
        ("cr2-canon-eos-1d-mark-ii-n.cr2", (3596, 2360), true),
        ("cr2-canon-eos-1d-mark-iii.cr2", (3984, 2622), true),
        ("cr2-canon-eos-1d-mark-iv.cr2", (5120, 3318), true),
        ("cr2-canon-eos-1d-x-mark-ii.cr2", (5568, 3708), true),
        ("cr2-canon-eos-2000d.cr2", (6096, 4051), true),
        ("cr2-canon-eos-20d.cr2", (3596, 2360), true),
        ("cr2-canon-eos-30d.cr2", (3596, 2360), true),
        ("cr2-canon-eos-4000d.cr2", (5344, 3516), true),
        ("cr2-canon-eos-400d.cr2", (3948, 2622), true),
        ("cr2-canon-eos-450d.cr2", (4312, 2876), true),
        ("cr2-canon-eos-500d.cr2", (4832, 3204), true),
        ("cr2-canon-eos-50d.cr2", (4832, 3228), true),
        ("cr2-canon-eos-5d-mark-iv.cr2", (6880, 4544), true),
        ("cr2-canon-eos-5d.cr2", (4476, 2954), true),
        ("cr2-canon-eos-5ds.cr2", (8896, 5920), true),
        ("cr2-canon-eos-600d.cr2", (5344, 3516), true),
        ("cr2-canon-eos-60d.cr2", (5344, 3516), true),
        ("cr2-canon-eos-6d-mark-ii.cr2", (6384, 4224), true),
        ("cr2-canon-eos-70d.cr2", (5568, 3708), true),
        ("cr2-canon-eos-750d.cr2", (6096, 4056), true),
        ("cr2-canon-eos-760d.cr2", (6096, 4056), true),
        ("cr2-canon-eos-77d.cr2", (6288, 4056), true),
        ("cr2-canon-eos-7d-mark-ii.cr2", (5568, 3708), true),
        ("cr2-canon-eos-kiss-digital-n.cr2", (3516, 2328), true),
        ("cr2-canon-eos-kiss-x80.cr2", (5344, 3516), true),
        ("cr2-canon-eos-kiss-x9.cr2", (6288, 4056), true),
        ("cr2-canon-eos-m.cr2", (5280, 3528), true),
        ("cr2-canon-eos-m10.cr2", (5280, 3528), true),
        ("cr2-canon-eos-m100.cr2", (6288, 4056), true),
        ("cr2-canon-eos-m2.cr2", (5280, 3528), true),
        ("cr2-canon-eos-m3.cr2", (6096, 4056), true),
        ("cr2-canon-eos-m5.cr2", (6288, 4056), true),
        ("cr2-canon-eos-m6.cr2", (6288, 4056), true),
        ("cr2-canon-eos-rebel-sl1.cr2", (5280, 3528), true),
        ("cr2-canon-eos-rebel-t3.cr2", (4352, 2874), true),
        ("cr2-canon-eos-rebel-t4i.cr2", (5280, 3528), true),
        ("cr2-canon-eos-rebel-t5.cr2", (5344, 3516), true),
        ("cr2-canon-eos-rebel-t5i.cr2", (5280, 3528), true),
        ("cr2-canon-eos-rebel-t7i.cr2", (6288, 4056), true),
        ("cr2-canon-powershot-g1-x-mark-ii.cr2", (4480, 3366), true),
        ("cr2-canon-powershot-g1-x-mark-iii.cr2", (6288, 4056), true),
        ("cr2-canon-powershot-g1-x.cr2", (4496, 3366), true),
        ("cr2-canon-powershot-g11.cr2", (3744, 2784), true),
        ("cr2-canon-powershot-g12.cr2", (3744, 2784), true),
        ("cr2-canon-powershot-g15.cr2", (4160, 3124), true),
        ("cr2-canon-powershot-g16.cr2", (4192, 3062), true),
        ("cr2-canon-powershot-g3-x.cr2", (5632, 3710), true),
        ("cr2-canon-powershot-g5-x.cr2", (5632, 3710), true),
        ("cr2-canon-powershot-g7-x-mark-ii.cr2", (5632, 3710), true),
        ("cr2-canon-powershot-g7-x.cr2", (5632, 3710), true),
        ("cr2-canon-powershot-g9-x-mark-ii.cr2", (5632, 3710), true),
        ("cr2-canon-powershot-g9-x.cr2", (5632, 3710), true),
        ("cr2-canon-powershot-g9.cr2", (4104, 3048), true),
        ("cr2-canon-powershot-s100.cr2", (4160, 3124), true),
        ("cr2-canon-powershot-s100v.cr2", (4160, 3124), true),
        ("cr2-canon-powershot-s110.cr2", (4160, 3124), true),
        ("cr2-canon-powershot-s120.cr2", (4192, 3062), true),
        ("dng-canon-powershot-s2-is.dng", (2672, 1968), false),
        ("cr2-canon-powershot-s95.cr2", (3744, 2784), true),
        ("cr2-canon-powershot-sx50-hs.cr2", (4176, 3062), true),
        ("cr2-canon-powershot-sx60-hs.cr2", (4768, 3516), true),
        ("dng-pentax-645d.dng", (7424, 5552), true),
        ("dng-pentax-645z.dng", (8384, 6208), true),
        ("dng-pentax-k-01.dng", (4960, 3300), true),
        ("dng-pentax-k-1-mark-ii.dng", (7392, 4950), true),
        ("dng-pentax-k-3-ii.dng", (6080, 4032), true),
        ("dng-pentax-k-3-mark-iii-monochrome.dng", (6304, 4224), false),
        ("dng-pentax-k-30.dng", (4960, 3300), true),
        ("dng-pentax-k-5-ii.dng", (4992, 3284), true),
        ("dng-pentax-k-5.dng", (4992, 3284), true),
        ("dng-pentax-k-50.dng", (4960, 3300), true),
        ("dng-pentax-k-500.dng", (4960, 3300), true),
        ("dng-pentax-k-7.dng", (4736, 3136), true),
        ("dng-pentax-k-s1.dng", (5504, 3664), true),
        ("dng-pentax-k-s2.dng", (5504, 3664), true),
        ("dng-pentax-k-x.dng", (4352, 2868), true),
        ("dng-pentax-k200d.dng", (3936, 2624), true),
        ("dng-pentax-k20d.dng", (4864, 3136), true),
        ("dng-pentax-kp.dng", (6112, 4060), true),
        ("dng-pentax-mx-1.dng", (4016, 3010), true),
        ("dng-pentax-q.dng", (4064, 3000), true),
        ("dng-pentax-q7.dng", (4064, 3000), true),
        ("dng-ricoh-gr-ii.dng", (4960, 3280), true),
        ("dng-ricoh-gr-iiix.dng", (6112, 4064), true),
        ("dng-ricoh-gr.dng", (4960, 3280), true),
        ("dng-ricoh-gx200.dng", (4096, 3014), false),
        ("dng-ricoh-gxr.dng", (3672, 2748), false),
        ("pef-pentax-ist-d.pef", (3040, 2024), true),
        ("pef-pentax-ist-dl.pef", (3040, 2024), true),
        ("pef-pentax-ist-ds.pef", (3040, 2024), true),
        ("pef-pentax-k-1.pef", (7392, 4950), true),
        ("pef-pentax-k-3-mark-iii.pef", (6304, 4224), true),
        ("pef-pentax-k-70.pef", (6080, 4064), true),
        ("pef-pentax-k-r.pef", (4352, 2868), true),
        ("pef-pentax-k100d-super.pef", (3040, 2024), true),
        ("pef-pentax-k100d.pef", (3040, 2024), true),
        ("pef-pentax-k110d.pef", (3040, 2024), true),
        ("pef-pentax-k2000.pef", (3936, 2624), true),
        ("pef-pentax-kf.pef", (6080, 4064), true),
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
    eprintln!("Canon / Pentax / Ricoh bodies checked on {seen} files");
}
