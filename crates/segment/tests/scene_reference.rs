//! The scene model (EoMT-S COCO panoptic, `tue-mps/coco_panoptic_eomt_small_640_2x`) with its
//! real weights.
//!
//! `{ENV_PREFIX}_SCENE_DIR=<model dir> cargo test -p lcc-segment --release --test scene_reference -- --nocapture`
//! runs the model on a drawn landscape and on damaged copies of its checkpoint. With
//! `{ENV_PREFIX}_SCENE_REF=<ref.safetensors>` (a reference file made with Hugging Face
//! `transformers`, fp32 on the CPU, by a scratch script outside the repository) it is also
//! compared with the reference stage by stage. Each test is skipped (passes) without its inputs.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use candle_core::{Device, Tensor};
use lcc_segment::scene::{self, SceneModel};

fn model_dir() -> Option<PathBuf> {
    let d = PathBuf::from(lcc_brand::env_os("SCENE_DIR")?);
    scene::is_scene_dir(&d).then_some(d)
}

fn reference() -> Option<HashMap<String, Tensor>> {
    let path = PathBuf::from(lcc_brand::env_os("SCENE_REF")?);
    candle_core::safetensors::load(&path, &Device::Cpu).ok()
}

fn vec(t: &Tensor) -> Vec<f32> {
    t.to_device(&Device::Cpu).unwrap().to_dtype(candle_core::DType::F32).unwrap().flatten_all().unwrap().to_vec1::<f32>().unwrap()
}

/// Max |a − b| relative to the reference's max |b|.
fn rel_err(name: &str, a: &[f32], b: &[f32]) -> f32 {
    assert_eq!(a.len(), b.len(), "{name}: length");
    let scale = b.iter().fold(0f32, |m, v| m.max(v.abs())).max(1e-6);
    let err = a.iter().zip(b).fold(0f32, |m, (x, y)| m.max((x - y).abs()));
    eprintln!("{name:>22}: max abs err {err:.2e}, rel {:.2e} (scale {scale:.2e})", err / scale);
    err / scale
}

fn stage<'a>(stages: &'a [(&str, Tensor)], name: &str) -> &'a Tensor {
    &stages.iter().find(|(n, _)| *n == name).unwrap().1
}

/// A drawn landscape, 480 × 320: a blue sky with soft clouds over a green meadow.
fn landscape() -> (Vec<u8>, usize, usize) {
    let (w, h) = (480usize, 320usize);
    let mut rgb = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        for x in 0..w {
            let p = if y < h / 2 {
                let t = y as f32 / (h / 2) as f32;
                let cloud = (((x as f32 / 37.0).sin() + (y as f32 / 11.0).cos()) * 0.5).max(0.0) * 40.0;
                [(70.0 + 60.0 * t + cloud) as u8, (130.0 + 50.0 * t + cloud) as u8, (220.0 + 20.0 * t).min(255.0) as u8]
            } else {
                let n = ((x * 7 + y * 13) % 23) as f32;
                [(50.0 + n) as u8, (120.0 + 2.0 * n) as u8, (40.0 + n) as u8]
            };
            rgb.extend_from_slice(&p);
        }
    }
    (rgb, w, h)
}

/// Mean of `plane` (a `w × h` grid) over the rows `y0..y1` (fractions of the height).
fn band(plane: &[f32], w: usize, h: usize, y0: f32, y1: f32) -> f32 {
    let (a, b) = ((y0 * h as f32) as usize, (y1 * h as f32) as usize);
    let cells: Vec<f32> = (a..b).flat_map(|y| plane[y * w..(y + 1) * w].iter().copied()).collect();
    cells.iter().sum::<f32>() / cells.len().max(1) as f32
}

#[test]
fn the_real_model_finds_sky_above_a_meadow() {
    let Some(dir) = model_dir() else {
        lcc_testkit::skip!(Model, "set {} to the EoMT-S checkpoint", lcc_brand::env_var("SCENE_DIR"));
        return;
    };
    let t = Instant::now();
    let model = SceneModel::load_on(&dir, Device::Cpu).unwrap();
    eprintln!("scene load: {:?}", t.elapsed());
    let (rgb, w, h) = landscape();
    let t = Instant::now();
    let s = model.segment(&rgb, w, h).unwrap();
    eprintln!("scene segment: {:?} ({} × {} grid)", t.elapsed(), s.width, s.height);
    assert!(s.height > 0 && s.height < s.width, "a landscape grid");
    let sky = s.class_probability(&scene::class_ids(|l| l.contains("sky")));
    let ground = s.class_probability(&scene::class_ids(|l| l.starts_with("grass") || l.starts_with("tree") || l.starts_with("dirt")));
    let (top, bottom) = (band(&sky, s.width, s.height, 0.05, 0.4), band(&sky, s.width, s.height, 0.6, 0.95));
    eprintln!("sky: top {top:.2}, bottom {bottom:.2}; ground bottom {:.2}", band(&ground, s.width, s.height, 0.6, 0.95));
    assert!(top > 0.6, "the sky is sky: {top}");
    assert!(bottom < 0.2, "the meadow is not sky: {bottom}");
    assert!(band(&ground, s.width, s.height, 0.6, 0.95) > 0.4, "the meadow is ground");
    assert!(s.probs.iter().all(|p| p.is_finite()));
    let found: Vec<String> = s.instances.iter().map(|i| format!("{} {:.2}", scene::COCO_PANOPTIC_LABELS[i.class], i.score)).collect();
    eprintln!("instances: {found:?}");
}

/// Copy `dir`'s checkpoint into a scratch folder, with `damage` applied to the weights' bytes.
fn damaged_copy(dir: &Path, name: &str, damage: impl FnOnce(&mut Vec<u8>)) -> PathBuf {
    let out = std::env::temp_dir().join(format!("lcc-scene-damaged-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(&out).unwrap();
    std::fs::copy(dir.join("config.json"), out.join("config.json")).unwrap();
    let mut bytes = std::fs::read(dir.join("model.safetensors")).unwrap();
    damage(&mut bytes);
    std::fs::write(out.join("model.safetensors"), bytes).unwrap();
    out
}

#[test]
fn a_damaged_real_checkpoint_is_an_error_not_a_panic() {
    let Some(dir) = model_dir() else {
        lcc_testkit::skip!(Model, "set {} to the EoMT-S checkpoint", lcc_brand::env_var("SCENE_DIR"));
        return;
    };
    let (rgb, w, h) = landscape();
    // cut short, a broken header, a header that claims more than there is, no weights at all
    let cases: Vec<(&str, Box<dyn FnOnce(&mut Vec<u8>)>)> = vec![
        ("truncated", Box::new(|b: &mut Vec<u8>| b.truncate(b.len() / 3))),
        ("header", Box::new(|b: &mut Vec<u8>| b.iter_mut().skip(8).take(64).for_each(|v| *v = b'#'))),
        ("length", Box::new(|b: &mut Vec<u8>| b[..8].copy_from_slice(&u64::MAX.to_le_bytes()))),
        ("empty", Box::new(|b: &mut Vec<u8>| b.clear())),
    ];
    for (name, damage) in cases {
        let d = damaged_copy(&dir, name, damage);
        let r = std::panic::catch_unwind(|| SceneModel::load_on(&d, Device::Cpu).map(|m| m.segment(&rgb, w, h).map(|_| ())));
        let _ = std::fs::remove_dir_all(&d);
        let r = r.unwrap_or_else(|_| panic!("{name}: loading a damaged checkpoint panicked"));
        assert!(r.is_err(), "{name}: a damaged checkpoint loads");
        eprintln!("{name}: {}", r.err().map(|e| e.to_string()).unwrap_or_default());
    }
    // weights scrambled (the header intact): it loads; whatever comes out, no panic and no NaN
    let d = damaged_copy(&dir, "scrambled", |b| {
        let header = u64::from_le_bytes(b[..8].try_into().unwrap()) as usize + 8;
        for (i, v) in b.iter_mut().enumerate().skip(header) {
            if i % 7 == 0 {
                *v = 0xff;
            }
        }
    });
    let r = std::panic::catch_unwind(|| SceneModel::load_on(&d, Device::Cpu).and_then(|m| m.segment(&rgb, w, h)));
    let _ = std::fs::remove_dir_all(&d);
    match r.expect("scrambled weights panicked") {
        Ok(s) => assert!(s.probs.iter().all(|p| p.is_finite()), "the probabilities stay finite"),
        Err(e) => eprintln!("scrambled: {e}"),
    }
}

#[test]
fn matches_reference() {
    let (Some(dir), Some(r)) = (model_dir(), reference()) else {
        lcc_testkit::skip!(Model, "set {} and {}", lcc_brand::env_var("SCENE_DIR"), lcc_brand::env_var("SCENE_REF"));
        return;
    };
    let model = SceneModel::load_on(&dir, Device::Cpu).unwrap();
    let t = &r["rgb"];
    let (h, w, _) = t.dims3().unwrap();
    let img: Vec<u8> = t.flatten_all().unwrap().to_vec1::<u8>().unwrap();

    // preprocessing: Pillow bilinear, exact up to Pillow's own rounding
    let (pixels, iw, ih) = SceneModel::preprocess(&img, w, h).unwrap();
    assert!(rel_err("pixels", &pixels, &vec(&r["eomt_pixels"])) < 1e-3);

    // the network on the reference input, stage by stage
    let raw = model.raw(&r["eomt_pixels"]).unwrap();
    for (ours, theirs) in [("emb", "eomt_emb"), ("layer8", "eomt_layer8"), ("final", "eomt_final"), ("upscaled", "eomt_upscaled")] {
        assert!(rel_err(ours, &vec(stage(&raw.stages, ours)), &vec(&r[theirs])) < 1e-3, "{ours}");
    }
    let ml = vec(&raw.mask_logits);
    let want = vec(&r["eomt_mask_logits"]);
    assert!(rel_err("mask logits", &ml, &want) < 1e-3);
    let dis = ml.iter().zip(&want).filter(|(a, b)| (**a > 0.0) != (**b > 0.0)).count() as f32 / ml.len() as f32;
    assert!(dis < 1e-3, "mask disagreement {dis}");
    assert!(rel_err("class logits", &vec(&raw.class_logits), &vec(&r["eomt_class_logits"])) < 1e-3);

    let (cw, ch) = scene::mask_crop(iw, ih);
    let s = scene::postprocess(&raw, cw, ch).unwrap();
    let sem = &r["eomt_sem"];
    assert_eq!(sem.dims(), &[scene::CLASSES, ch, cw]);
    assert!(rel_err("semantic probs", &s.probs, &vec(sem)) < 1e-3);
    let e2e = model.segment(&img, w, h).unwrap();
    assert!(rel_err("semantic probs (e2e)", &e2e.probs, &vec(sem)) < 2e-2);
}
