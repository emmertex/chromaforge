//! Photo ▸ Enhance through the command layer: a Bayer DNG on disk → Raw Details / Super Resolution DNG next to it,
//! imported with the source's settings and stacked with it; hostile files give errors.

use lcc_color::Mat3;
use lcc_geom::Orientation;
use lcc_meta::Metadata;
use lcc_raw::{BlackLevel, Cfa, ColorData, DngWriteOptions, OpcodeLists, RawData, RawFormat, RawImage, Rect};
use serde_json::json;

use crate::Session;

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("lc-enhance-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A small RGGB DNG of edges and stripes.
fn bayer_dng(w: usize, h: usize) -> Vec<u8> {
    let cfa = Cfa::bayer("RGGB").unwrap();
    let data = (0..w * h)
        .map(|i| {
            let (x, y) = (i % w, i / w);
            let lum = if (x / 3) % 2 == 0 { 0.7 } else { 0.2 } * if y > h / 2 { 1.0 } else { 0.6 };
            let tint = [0.9, 1.0, 0.7];
            (lum * tint[cfa.color_at(x, y) as usize] * 4000.0 + 64.0) as u16
        })
        .collect();
    let raw = RawImage {
        format: RawFormat::Dng,
        width: w,
        height: h,
        cpp: 1,
        data: RawData::U16(data),
        cfa: Some(cfa),
        bits: 12,
        black: BlackLevel::uniform(64.0),
        white: vec![4095.0],
        active_area: Rect::new(0, 0, w, h),
        crop: Rect::new(0, 0, w, h),
        orientation: Orientation::Normal,
        color: ColorData {
            illuminant: [21, 0],
            color_matrix: [Some(Mat3::IDENTITY), None],
            as_shot_neutral: Some([1.0, 1.0, 1.0]),
            ..Default::default()
        },
        wb_multipliers: None,
        linearized: false,
        pixel_aspect: 1.0,
        opcodes: OpcodeLists::default(),
        metadata: Metadata { make: Some("Test".into()), model: Some("Bayer".into()), ..Default::default() },
        lens_metadata: Default::default(),
    };
    lcc_raw::write_dng(&raw, &DngWriteOptions::default()).unwrap()
}

fn import(s: &mut Session, path: &std::path::Path) -> u64 {
    let r = s.execute("library.import", &json!({"paths": [path.to_string_lossy()]})).unwrap();
    r["imported"][0].as_u64().unwrap()
}

#[test]
fn raw_details_and_super_resolution_make_enhanced_dngs() {
    let dir = temp_dir("cmd");
    let src = dir.join("IMG_1.dng");
    std::fs::write(&src, bayer_dng(64, 48)).unwrap();
    let mut s = Session::new().with_fs();
    let id = import(&mut s, &src);
    s.execute("develop.set", &json!({"id": id, "values": {"light.exposure": 0.5}})).ok();

    let rd = s.execute("enhance.rawDetails", &json!({"ids": [id], "wait": true})).unwrap();
    let made = &rd["made"][0];
    let path = made["path"].as_str().unwrap();
    assert!(path.ends_with("IMG_1-Enhanced-RD.dng"), "{path}");
    let back = lcc_raw::decode(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!((back.width, back.height, back.cpp), (64, 48, 3), "a demosaiced linear DNG of the same size");
    let rd_id = lcc_catalog::PhotoId(made["id"].as_u64().unwrap());
    assert!(s.catalog.photo(rd_id).is_some(), "imported");

    let sr = s.execute("enhance.superResolution", &json!({"ids": [id], "wait": true})).unwrap();
    let path = sr["made"][0]["path"].as_str().unwrap();
    assert!(path.ends_with("IMG_1-Enhanced-SR.dng"), "{path}");
    let back = lcc_raw::decode(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!((back.width, back.height), (128, 96), "twice each side");

    // in the background: queued, then imported by the pump, with a row in the activity stack meanwhile
    let q = s.execute("enhance.rawDetails", &json!({"ids": [id]})).unwrap();
    assert_eq!(q["queued"], 1);
    assert!(s.activity.list().iter().any(|t| t.kind == "enhance"), "a row in the activity stack");
    let mut done = false;
    for _ in 0..600 {
        let v = s.execute("enhance.pump", &json!({})).unwrap();
        if v["made"].as_array().is_some_and(|m| !m.is_empty()) {
            done = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(done, "the background run finished");
    assert!(!s.activity.list().iter().any(|t| t.kind == "enhance"), "the row goes when it is done");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn hostile_and_unsuitable_photos_give_errors() {
    let dir = temp_dir("bad");
    let mut s = Session::new().with_fs();
    // a PNG is not a raw: Raw Details refuses it, Super Resolution takes it
    let jpg = dir.join("a.png");
    let img = lcc_raster::Rgba8::from_fn(24, 16, |x, y| [(x * 10) as u8, (y * 15) as u8, 90, 255]);
    std::fs::write(&jpg, lcc_codecs::encode_png(&lcc_codecs::EncodeImage::rgba8(&img), &Default::default()).unwrap()).unwrap();
    let id = import(&mut s, &jpg);
    assert!(s.execute("enhance.rawDetails", &json!({"ids": [id], "wait": true})).is_err());
    let sr = s.execute("enhance.superResolution", &json!({"ids": [id], "wait": true})).unwrap();
    assert_eq!(sr["made"][0]["size"], json!([48, 32]));
    // a raw whose file was damaged after import
    let raw = dir.join("b.dng");
    std::fs::write(&raw, bayer_dng(32, 24)).unwrap();
    let id = import(&mut s, &raw);
    let mut bytes = std::fs::read(&raw).unwrap();
    bytes.truncate(bytes.len() / 3);
    for b in bytes.iter_mut().skip(200).step_by(7) {
        *b ^= 0x5a;
    }
    std::fs::write(&raw, &bytes).unwrap();
    for cmd in ["enhance.rawDetails", "enhance.superResolution"] {
        assert!(s.execute(cmd, &json!({"ids": [id], "wait": true})).is_err(), "{cmd}");
    }
    std::fs::remove_file(&raw).unwrap();
    assert!(s.execute("enhance.rawDetails", &json!({"ids": [id], "wait": true})).is_err(), "a missing file");
    assert!(s.execute("enhance.rawDetails", &json!({"ids": [999_999], "wait": true})).is_err(), "no such photo");
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------------------------------------------------
// AI Super Resolution and AI Sharpen with a stand-in upscaler (adapted from upstream #3493)

mod ai {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use lcc_denoise::manifest::{DenoiserManifest, Domain};
    use lcc_denoise::run::TileRunner;
    use serde_json::{Value, json};

    use crate::Session;
    use crate::denoise::{Model, RunOn};
    use crate::export::ExportOptions;
    use crate::tests_denoise::manifest;

    /// A stand-in upscaler: every pixel of its tile becomes a 4 × 4 block.
    struct Repeat4;
    impl TileRunner for Repeat4 {
        fn run(&self, input: &[f32]) -> Result<Vec<f32>, lcc_denoise::Error> {
            let t = ((input.len() / 3) as f64).sqrt() as usize;
            let big = 4 * t;
            Ok((0..3 * big * big).map(|k| input[(k / (big * big)) * t * t + (k % (big * big)) / big / 4 * t + k % big / 4]).collect())
        }
    }
    impl Model for Repeat4 {
        fn runner(&self, _: RunOn, threads: usize) -> (Box<dyn TileRunner + '_>, usize) {
            (Box::new(crate::denoise::Lent(self)), threads)
        }

        fn self_test(&self, _: RunOn) -> Result<Value, String> {
            Ok(json!({"ok": true}))
        }
    }

    /// The stand-in upscaler's `.onnx` (never read: the loader is the stand-in) with its manifest beside it.
    fn upscaler_files(dir: &Path) -> PathBuf {
        let src = dir.join("src-upscaler");
        std::fs::create_dir_all(&src).unwrap();
        let m = DenoiserManifest { domain: Domain::RgbToRgb4x, ..manifest("stand-in-upscaler") };
        std::fs::write(src.join("denoise-model.json"), serde_json::to_vec(&m).unwrap()).unwrap();
        std::fs::write(src.join("model.onnx"), "not an onnx file, only a stand-in").unwrap();
        src.join("model.onnx")
    }

    /// A session with a library holding one PNG of 150 × 100 (a ramp), selected, and the stand-in loader.
    fn setup(tag: &str) -> (PathBuf, Session, lcc_catalog::PhotoId) {
        let dir = super::temp_dir(tag);
        let originals = dir.join("originals");
        std::fs::create_dir_all(&originals).unwrap();
        let img = lcc_raster::Rgba8::from_fn(150, 100, |x, y| [(40 + x) as u8, (60 + y) as u8, 120, 255]);
        let o = ExportOptions { format: crate::export::ExportFormat::Png, ..Default::default() };
        std::fs::write(originals.join("ramp.png"), crate::export::encode_image(&img, &o).unwrap()).unwrap();
        let mut s = Session::new().with_fs();
        s.open_library(dir.join("library"), false).unwrap();
        s.execute("library.import", &json!({"paths": [originals.join("ramp.png").to_string_lossy()]})).unwrap();
        let id = s.catalog.photos().next().unwrap().id;
        s.execute("library.select", &json!({"ids": [id.0]})).unwrap();
        s.set_denoise_models_dir(Some(dir.join("denoise-models")));
        s.denoise.loader = Arc::new(|_, _| Ok(Arc::new(Repeat4) as Arc<dyn Model>));
        (dir, s, id)
    }

    fn decoded(path: &str) -> lcc_codecs::Decoded {
        lcc_codecs::decode(&std::fs::read(path).unwrap(), lcc_codecs::DecodeOptions::default()).unwrap()
    }

    fn made(r: &Value) -> Value {
        r["made"][0].clone()
    }

    #[test]
    fn ai_super_resolution_writes_a_larger_tiff_stacked_with_its_photo() {
        let (dir, mut s, original) = setup("ai-sr");
        // without an upscaler it says where to get one; `auto` falls back to the classical method
        let e = s.execute("enhance.superResolution", &json!({"method": "ai", "wait": true})).unwrap_err().to_string();
        assert!(e.contains("no upscaler is installed"), "{e}");
        assert!(!s.enhance_upscaler_ready());

        // an upscaler fills its own slot: the denoise models of both kinds of photo stay as they were
        let r = s.execute("denoise.models.install", &json!({"path": upscaler_files(&dir).to_string_lossy(), "acknowledged": true})).unwrap();
        assert_eq!((r["upscaleModel"].clone(), r["rgbModel"].clone()), (json!("stand-in-upscaler"), Value::Null), "{r}");
        assert!(s.enhance_upscaler_ready());

        let r = made(&s.execute("enhance.superResolution", &json!({"method": "auto", "wait": true})).unwrap());
        assert_eq!(r["size"], json!([300, 200]), "{r}");
        let path = r["path"].as_str().unwrap().to_string();
        assert!(path.ends_with("ramp-Enhanced-SR.tif"), "{path}");
        let id = lcc_catalog::PhotoId(r["id"].as_u64().unwrap());
        assert!(s.catalog.stack_of(original).unwrap().photos.contains(&id), "stacked with its photo");

        // a 16-bit TIFF of the picture, every pixel twice as large (the stand-in repeats, averaged down by 2)
        let d = decoded(&path);
        assert_eq!((d.image.width, d.image.height, d.bit_depth), (300, 200, 16));
        let srgb = crate::denoise::srgb_encoded(&d);
        for (x, y) in [(0, 0), (149, 99), (75, 50), (10, 90)] {
            let want = [(40 + x) as f32, (60 + y) as f32, 120.0].map(|v| v / 255.0);
            for (dx, dy) in [(0, 0), (1, 1)] {
                let got = srgb.data[(2 * y + dy) * 300 + 2 * x + dx];
                for c in 0..3 {
                    assert!((got[c] - want[c]).abs() < 1.5 / 255.0, "({x}, {y}): {got:?} vs {want:?}");
                }
            }
        }

        // 4× on request, into a new file beside it
        let r = made(&s.execute("enhance.superResolution", &json!({"ids": [original.0], "method": "ai", "scale": 4, "wait": true})).unwrap());
        assert!(r["path"].as_str().unwrap().ends_with("ramp-Enhanced-SR-1.tif"), "{r}");
        assert_eq!(r["size"], json!([600, 400]));

        // the same request gives the same bytes: the photo that has them is the result, and no copy is left behind
        let photos = s.catalog.photos().count();
        let again = made(&s.execute("enhance.superResolution", &json!({"ids": [original.0], "method": "ai", "wait": true})).unwrap());
        assert_eq!((again["id"].clone(), again["path"].as_str()), (json!(id.0), Some(path.as_str())), "{again}");
        assert_eq!(s.catalog.photos().count(), photos);
        drop(s);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn ai_sharpen_keeps_the_photos_size_and_bad_requests_are_errors() {
        let (dir, mut s, original) = setup("ai-sharpen");
        s.execute("denoise.models.install", &json!({"path": upscaler_files(&dir).to_string_lossy(), "acknowledged": true})).unwrap();
        let r = made(&s.execute("enhance.aiSharpen", &json!({"wait": true})).unwrap());
        assert_eq!(r["size"], json!([150, 100]), "{r}");
        assert_eq!((decoded(r["path"].as_str().unwrap()).image.width, s.catalog.photos().count()), (150, 2));

        for scale in [json!(3), json!(1), json!("2")] {
            let e = s.execute("enhance.superResolution", &json!({"ids": [original.0], "method": "ai", "scale": scale})).unwrap_err().to_string();
            assert!(e.contains("`scale` must be 2 or 4"), "{e}");
        }
        let e = s.execute("enhance.superResolution", &json!({"ids": [original.0], "scale": 4})).unwrap_err().to_string();
        assert!(e.contains("needs `method"), "the classical method only doubles: {e}");
        let e = s.execute("enhance.superResolution", &json!({"ids": [original.0], "method": "magic"})).unwrap_err().to_string();
        assert!(e.contains("unknown method"), "{e}");
        // switched off, AI Sharpen asks for an upscaler again
        s.execute("denoise.models.select", &json!({"id": null, "kind": "upscale"})).unwrap();
        let e = s.execute("enhance.aiSharpen", &json!({"ids": [original.0], "wait": true})).unwrap_err().to_string();
        assert!(e.contains("no upscaler"), "{e}");
        drop(s);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_damaged_photo_fails_ai_super_resolution_with_a_reason() {
        let (dir, mut s, original) = setup("ai-hostile");
        s.execute("denoise.models.install", &json!({"path": upscaler_files(&dir).to_string_lossy(), "acknowledged": true})).unwrap();
        let path = dir.join("originals").join("ramp.png");
        std::fs::write(&path, b"\x89PNG\r\n\x1a\nbroken").unwrap();
        s.media.file_bytes = None;
        assert!(s.execute("enhance.superResolution", &json!({"ids": [original.0], "method": "ai", "wait": true})).is_err());
        drop(s);
        let _ = std::fs::remove_dir_all(dir);
    }
}
