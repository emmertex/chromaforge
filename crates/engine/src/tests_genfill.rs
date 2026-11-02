//! Generative fill end to end against the local mock provider (never the internet): consent and
//! keys, Generate, the patch file, renders that show it under later white balance and exposure
//! edits, variations, a missing patch file, the activity row, and `spot.fromMask`.

use std::sync::Arc;

use serde_json::{Value, json};

use crate::Session;
use lcc_genfill::cloud::mock::{Filler, MockServer, mean_filler};

const KEY: &str = "test-genfill-key-0123456789";

fn temp(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("lc-genfill-test-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A demo session wired to a mock provider with `filler`, keys in `dir`.
fn session(dir: &std::path::Path, filler: Filler) -> (Session, MockServer) {
    let mock = MockServer::start(0, Some(KEY), filler).unwrap();
    let mut s = Session::with_demo();
    s.genfill.keys = lcc_genfill::cloud::keys::KeyStore::new(Some(dir.join("genfill-keys.json")));
    s.genfill.mock = Some(mock.base.clone());
    (s, mock)
}

/// The rendered sRGB pixel at normalized `(x, y)` of the active photo (uncropped, 400 px).
fn pixel(s: &mut Session, x: f64, y: f64) -> [u8; 4] {
    let id = s.active().unwrap();
    let img = s.render_job(id, 400, 400, false, false).unwrap().run().rendered.unwrap().image;
    let (px, py) = ((x * img.width as f64) as usize, (y * img.height as f64) as usize);
    img.data[py * img.width + px]
}

/// A filler that paints the hole a strong blue, so the patch is easy to find in a render.
fn blue() -> Filler {
    Arc::new(|w, h, rgb, mask, var| {
        let mut out = rgb.to_vec();
        for i in 0..w * h {
            if mask[i] >= 128 {
                out[i * 3..i * 3 + 3].copy_from_slice(&[60, 80 + var as u8 * 25, 120]);
            }
        }
        out
    })
}

#[test]
fn generate_needs_consent_and_a_key_and_never_shows_the_key() {
    let dir = temp("consent");
    let (mut s, mock) = session(&dir, mean_filler());
    s.execute("spot.add", &json!({"mode": "generate", "points": [[0.5, 0.5]], "size": 0.03})).unwrap();
    let e = s.execute("spot.generate", &json!({"provider": "openai"})).unwrap_err().to_string();
    assert!(e.contains("terms"), "{e}");
    s.execute("genfill.consent", &json!({"provider": "openai", "accept": true})).unwrap();
    let e = s.execute("spot.generate", &json!({"provider": "openai"})).unwrap_err().to_string();
    assert!(e.contains("API key"), "{e}");
    let r = s.execute("genfill.setKey", &json!({"provider": "openai", "key": KEY})).unwrap();
    assert_eq!(r["configured"], true);
    let listing = s.execute("genfill.providers", &json!({})).unwrap();
    for v in [&r, &listing] {
        assert!(!v.to_string().contains(KEY), "the key is never returned: {v}");
    }
    let openai = listing["providers"].as_array().unwrap().iter().find(|p| p["id"] == "openai").unwrap().clone();
    assert_eq!(
        (openai["consented"].clone(), openai["keySource"].clone(), openai["keyHint"].clone()),
        (json!(true), json!("settings"), json!("6789"))
    );
    // setKey is never journaled (the key stays out of every log)
    assert!(!s.journal.iter().any(|(_, p)| p.to_string().contains(KEY)));
    // nothing went to the provider before Generate
    assert!(mock.seen().is_empty());
    // a key the provider refuses: an error without the key in it
    s.execute("genfill.setKey", &json!({"provider": "openai", "key": "wrong-key-0000000000"})).unwrap();
    let e = s.execute("spot.generate", &json!({"provider": "openai"})).unwrap_err().to_string();
    assert!(e.contains("refused") && !e.contains("wrong-key-0000000000"), "{e}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_generated_patch_renders_under_later_white_balance_and_exposure() {
    let dir = temp("render");
    let (mut s, mock) = session(&dir, blue());
    s.execute("genfill.setKey", &json!({"provider": "stability", "key": KEY})).unwrap();
    s.execute("genfill.consent", &json!({"provider": "stability", "accept": true})).unwrap();
    s.execute("spot.add", &json!({"mode": "generate", "points": [[0.5, 0.5]], "size": 0.04})).unwrap();
    let before = pixel(&mut s, 0.5, 0.5);
    let plan = s.execute("genfill.plan", &json!({"provider": "stability", "variations": 2})).unwrap();
    assert_eq!((plan["requests"].as_u64(), plan["soft"].as_bool()), (Some(2), Some(false)), "{plan}");
    let r = s.execute("spot.generate", &json!({"provider": "stability", "variations": 2})).unwrap();
    assert_eq!(r["generated"]["variants"], 2, "{r}");
    let id = s.active().unwrap();
    let sp = s.develop_of(id).unwrap().spots[0].clone();
    let g = sp.shown_gen().expect("the spot shows its patch").clone();
    assert!(s.genfill_dir(id).join(g.file_name(0)).is_file() && s.genfill_dir(id).join(g.file_name(1)).is_file());
    // the crop sent was the native-size context crop, not the whole photo
    let sent = mock.seen().into_iter().find(|x| x.path.starts_with("/v2beta/")).unwrap();
    let (pw, ph) = (s.catalog.photo(id).unwrap().width as usize, s.catalog.photo(id).unwrap().height as usize);
    let (cw, chh) = sent.image.unwrap();
    assert!(cw < pw || chh < ph, "a crop, not the photo ({cw}×{chh} of {pw}×{ph})");
    // the patch shows: blue in the middle of the spot
    let after = pixel(&mut s, 0.5, 0.5);
    assert!(after[2] > after[0] + 25 && after != before, "{before:?} → {after:?}");
    // white balance and exposure after generating still apply to the patch
    s.execute("develop.set", &json!({"control": "light.exposure", "value": 1.0})).unwrap();
    let brighter = pixel(&mut s, 0.5, 0.5);
    assert!(brighter[2] > after[2] || brighter[2] == 255, "{after:?} → {brighter:?}");
    assert!(brighter[1] > after[1], "{after:?} → {brighter:?}");
    s.execute("develop.set", &json!({"control": "wb.temp", "value": 10000.0})).unwrap();
    let warmer = pixel(&mut s, 0.5, 0.5);
    assert!(warmer[0] > brighter[0] || warmer[2] < brighter[2], "{brighter:?} → {warmer:?}");
    // the second variation differs (the mock paints it greener)
    s.execute("spot.variation", &json!({"delta": 1})).unwrap();
    let other = pixel(&mut s, 0.5, 0.5);
    assert_ne!(other, warmer);
    // undo goes back to the first variation; moving the spot shows a content-aware fill instead
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(pixel(&mut s, 0.5, 0.5), warmer);
    // a missing patch file: no crash, the photo's own pixels in its place, and the session knows
    std::fs::remove_file(s.genfill_dir(id).join(g.file_name(0))).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(3100));
    let missing = pixel(&mut s, 0.5, 0.5);
    assert_ne!(missing, warmer, "the blue patch is gone");
    assert!(s.genfill_missing(id, 0));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_background_generation_holds_an_activity_row_until_it_is_applied() {
    let dir = temp("activity");
    let slow: Filler = Arc::new(|w, h, rgb, mask, v| {
        std::thread::sleep(std::time::Duration::from_millis(700));
        (mean_filler())(w, h, rgb, mask, v)
    });
    let (mut s, _mock) = session(&dir, slow);
    s.genfill.background = true;
    s.execute("genfill.setKey", &json!({"provider": "bfl", "key": KEY})).unwrap();
    s.execute("genfill.consent", &json!({"provider": "bfl", "accept": true})).unwrap();
    s.execute("spot.add", &json!({"mode": "generate", "points": [[0.3, 0.6]], "size": 0.02})).unwrap();
    let r = s.execute("spot.generate", &json!({"provider": "bfl", "prompt": "a lawn"})).unwrap();
    assert_eq!(r["started"], true);
    let rows = s.activity.list();
    assert!(rows.iter().any(|t| t.kind == "genfill" && t.cancellable), "{:?}", rows.iter().map(|t| t.kind).collect::<Vec<_>>());
    assert!(s.execute("spot.generate", &json!({"provider": "bfl"})).is_err(), "one at a time");
    let t0 = std::time::Instant::now();
    let done = loop {
        if let Some(r) = s.genfill_poll() {
            break r;
        }
        assert!(t0.elapsed().as_secs() < 30, "the generation never finished");
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    assert!(done.is_ok(), "{done:?}");
    assert!(!s.activity.list().iter().any(|t| t.kind == "genfill"), "the row goes with the job");
    let id = s.active().unwrap();
    assert!(s.develop_of(id).unwrap().spots[0].shown_gen().is_some());
    let st = s.execute("genfill.status", &json!({})).unwrap();
    assert_eq!(st["last"]["ok"], true, "{st}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_mask_becomes_a_spot_that_covers_it() {
    let mut s = Session::with_demo();
    let r = s.execute("mask.add", &json!({"kind": "radial", "center": [0.5, 0.5], "rx": 0.1, "ry": 0.1, "feather": 0})).unwrap();
    let mid = r["id"].as_u64().or(s.active_mask.map(u64::from)).unwrap();
    let r = s.execute("spot.fromMask", &json!({"mask": mid, "mode": "generate"})).unwrap();
    let id = s.active().unwrap();
    let d = s.develop_of(id).unwrap();
    assert!(d.masks.is_empty(), "the mask was used up");
    let sp = &d.spots[r["index"].as_u64().unwrap() as usize];
    assert_eq!(sp.mode, lcc_develop::SpotMode::Generate);
    assert!(sp.points.len() > 20 && sp.points.len() <= 900, "{}", sp.points.len());
    // the dabs lie inside the ellipse (normalized: rx is relative to the long edge, so check loosely)
    let (xs, ys): (Vec<f64>, Vec<f64>) = sp.points.iter().map(|p| (p.x, p.y)).unzip();
    let ext = |v: &[f64]| (v.iter().cloned().fold(f64::MAX, f64::min), v.iter().cloned().fold(f64::MIN, f64::max));
    let (ex, ey) = (ext(&xs), ext(&ys));
    assert!(ex.0 > 0.3 && ex.1 < 0.7 && ey.0 > 0.2 && ey.1 < 0.8 && (ex.0 + ex.1 - 1.0).abs() < 0.03, "{ex:?} {ey:?}");
    assert!(s.execute("spot.fromMask", &json!({"mask": 999})).is_err());
    let _: Value = s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(s.develop_of(id).unwrap().masks.len(), 1);
}
