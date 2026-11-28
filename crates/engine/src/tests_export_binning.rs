//! An export of a raw decodes it at the size the output needs (#1377 step 2): binned by the smallest block that
//! leaves 1.15 times the output's pixels, kept at that size; a larger output demosaics the whole sensor as before.

use serde_json::json;

use crate::Session;
use crate::media::SourceRef;

/// A 960 × 640 RGGB DNG of smooth colour with stripes, in a scratch folder.
fn dng(dir: &std::path::Path) -> String {
    let (w, h) = (960usize, 640usize);
    let data = (0..w * h)
        .map(|i| {
            let (x, y) = (i % w, i / w);
            let base = 3000.0 + 6000.0 * x as f32 / w as f32 + 2000.0 * y as f32 / h as f32;
            // (detail at a quarter of the output's resolution: a fine stripe right at the output's limit is the zone-plate
            // worst case the `bin_sharpness` example measures)
            let stripes = if (x / 12) % 2 == 0 { 800.0 } else { 0.0 };
            (base + stripes) as u16
        })
        .collect();
    let raw = lcc_raw::RawImage {
        format: lcc_raw::RawFormat::Dng,
        width: w,
        height: h,
        cpp: 1,
        data: lcc_raw::RawData::U16(data),
        cfa: lcc_raw::Cfa::bayer("RGGB"),
        bits: 14,
        black: lcc_raw::BlackLevel::uniform(512.0),
        white: vec![16000.0],
        active_area: lcc_raw::Rect::new(0, 0, w, h),
        crop: lcc_raw::Rect::new(0, 0, w, h),
        orientation: lcc_raw::Orientation::Normal,
        color: lcc_raw::ColorData {
            illuminant: [21, 0],
            color_matrix: [Some(lcc_raw::Mat3([[0.8, -0.2, -0.05], [-0.45, 1.3, 0.15], [-0.08, 0.22, 0.62]])), None],
            as_shot_neutral: Some([0.5, 1.0, 0.7]),
            ..Default::default()
        },
        wb_multipliers: None,
        linearized: false,
        pixel_aspect: 1.0,
        opcodes: Default::default(),
        metadata: Default::default(),
        lens_metadata: Default::default(),
    };
    let p = dir.join("BIN.dng");
    std::fs::write(&p, lcc_raw::write_dng(&raw, &Default::default()).unwrap()).unwrap();
    p.to_string_lossy().into_owned()
}

fn scratch(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("lcc-export-binning-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn keeps_binned(s: &mut Session, id: lcc_catalog::PhotoId, edge: usize) -> Option<usize> {
    let job = s.export_job(id, edge, edge, Default::default(), Default::default()).unwrap();
    match &job.source {
        SourceRef::File { max_edge, options, .. } if options.keep_binned => Some(*max_edge),
        _ => None,
    }
}

#[test]
fn a_small_export_bins_the_raw_and_a_large_one_demosaics_it() {
    let dir = scratch("choose");
    let path = dng(&dir);
    let mut s = Session::new().with_fs();
    let r = s.execute("library.import", &json!({"paths": [path]})).unwrap();
    let id = lcc_catalog::PhotoId(r["imported"][0].as_u64().unwrap());
    // 400 px wants 460 source pixels: half the sensor (480) has them
    assert_eq!(keeps_binned(&mut s, id, 400), Some(460));
    // 450 px wants 518: more than half the sensor, so the whole sensor as before
    assert_eq!(keeps_binned(&mut s, id, 450), None);
    // an export through a tight crop needs more of the source: no binning
    let mut st = (*s.catalog.photo(id).unwrap().develop).clone();
    st.crop.geometry.rect = lcc_geom::Rect::new(0.25, 0.25, 0.75, 0.75);
    s.set_develop(id, st, "crop").unwrap();
    assert_eq!(keeps_binned(&mut s, id, 400), None);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_binned_export_matches_the_full_demosaic_closely() {
    let dir = scratch("match");
    let path = dng(&dir);
    let mut s = Session::new().with_fs();
    let r = s.execute("library.import", &json!({"paths": [path]})).unwrap();
    let id = lcc_catalog::PhotoId(r["imported"][0].as_u64().unwrap());
    let binned = s.export_job(id, 400, 400, Default::default(), Default::default()).unwrap();
    assert!(matches!(&binned.source, SourceRef::File { options, .. } if options.keep_binned));
    let mut full = s.export_job(id, 400, 400, Default::default(), Default::default()).unwrap();
    if let SourceRef::File { max_edge, options, .. } = &mut full.source {
        *max_edge = usize::MAX;
        options.keep_binned = false;
    }
    let a = binned.run().rendered.unwrap().image;
    let b = full.run().rendered.unwrap().image;
    assert_eq!((a.width, a.height), (b.width, b.height), "the output size doesn't depend on the source");
    assert_eq!(a.width.max(a.height), 400);
    // the same picture: no shift in colour or level, and only the stripes' hard edges differ a little (SSIM is
    // harsh on those; the demo scenes in the `bin_sharpness` example give ≥ 0.998)
    let n = a.data.len() as f64;
    for c in 0..3 {
        let abs: f64 = a.data.iter().zip(&b.data).map(|(p, q)| (p[c] as f64 - q[c] as f64).abs()).sum::<f64>() / n;
        let bias: f64 = a.data.iter().zip(&b.data).map(|(p, q)| p[c] as f64 - q[c] as f64).sum::<f64>() / n;
        assert!(abs < 2.5, "channel {c}: mean difference {abs:.2} levels");
        assert!(bias.abs() < 0.5, "channel {c}: shifted by {bias:.2} levels");
    }
    let ssim = lcc_fidelity::ssim(&a, &b).unwrap();
    assert!(ssim > 0.95, "binned export against the full demosaic: SSIM {ssim}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_export_block_is_the_smallest_the_sensor_allows() {
    let bytes = std::fs::read(dng(&scratch("factor")).as_str()).unwrap();
    let raw = lcc_raw::decode(&bytes).unwrap();
    assert_eq!(crate::files::export_bin_factor(&raw, 460), Some(2));
    assert_eq!(crate::files::export_bin_factor(&raw, 481), None, "never fewer pixels than asked");
    assert_eq!(crate::files::export_bin_factor(&raw, 100), Some(2), "a small output still takes the smallest block");
    assert_eq!(crate::files::export_bin_factor(&raw, 0), Some(2));
}
