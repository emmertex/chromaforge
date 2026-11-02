//! Real-photo checks for content-aware and generative fill. Generative goes to the in-process mock
//! provider (LightCraft's own content-aware fill), or with `GENFILL_REAL=1` to the real provider
//! (`PROVIDER`, default openai) with the key from `LCC_GENFILL_<PROVIDER>_KEY`: a paid
//! request per run. `PROMPT` sets the prompt (empty: remove); `PLAN_ONLY=1` stops after the plan.
//!
//! ```text
//! cargo run --release -p lcc-engine --example genfill_check -- thumbs OUT_DIR FILES…
//! cargo run --release -p lcc-engine --example genfill_check -- fill FILE OUT_PREFIX x,y,size[;x,y…]
//! cargo run --release -p lcc-engine --example genfill_check -- generate FILE OUT_PREFIX x,y,size[;x,y…]
//! ```
//! Coordinates are normalized (the uncropped photo), `size` is the radius as a fraction of the long
//! edge; several `x,y` after the first make a brushed stroke. `fill` writes `<prefix>-before.png`
//! and `-after.png` (a native-resolution crop around the spot); `generate` also writes `-wb.png`
//! (the same patch after a white balance and exposure change made once it was generated) and prints
//! the plan and the time taken.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use lcc_engine::Session;
use serde_json::json;

fn save(path: &str, img: &lcc_raster::Rgba8) {
    let rgb: Vec<u8> = img.data.iter().flat_map(|p| [p[0], p[1], p[2]]).collect();
    std::fs::write(path, lcc_genfill::cloud::util::png_encode(img.width, img.height, 3, &rgb).unwrap()).unwrap();
}

fn open(file: &str) -> (Session, lcc_catalog::PhotoId) {
    let mut s = Session::new().with_fs();
    let r = s.execute("library.import", &json!({"paths": [file]})).unwrap();
    let id = lcc_catalog::PhotoId(r["imported"][0].as_u64().unwrap());
    s.execute("library.select", &json!({"ids": [id.0], "active": id.0})).unwrap();
    (s, id)
}

/// The full-size uncropped render's crop around normalized (x, y), `side` px.
fn crop(s: &mut Session, id: lcc_catalog::PhotoId, x: f64, y: f64, side: usize) -> lcc_raster::Rgba8 {
    let p = s.catalog.photo(id).unwrap().clone();
    let long = p.width.max(p.height) as usize;
    let img = s.render_job(id, long, long, false, false).unwrap().run().rendered.unwrap().image;
    let (cx, cy) = ((x * img.width as f64) as usize, (y * img.height as f64) as usize);
    let (w, h) = (side.min(img.width), side.min(img.height));
    let x0 = cx.saturating_sub(w / 2).min(img.width - w);
    let y0 = cy.saturating_sub(h / 2).min(img.height - h);
    img.crop(x0, y0, w, h)
}

fn parse(spec: &str) -> (Vec<[f64; 2]>, f64) {
    let mut pts = Vec::new();
    let mut size = 0.02;
    for (i, part) in spec.split(';').enumerate() {
        let v: Vec<f64> = part.split(',').map(|t| t.trim().parse().unwrap()).collect();
        pts.push([v[0], v[1]]);
        if i == 0 && v.len() > 2 {
            size = v[2];
        }
    }
    // like a brush drag: dabs every half radius along the stroke (the long edge is ~1.5 × the short)
    let mut dense: Vec<[f64; 2]> = Vec::new();
    for w in pts.windows(2) {
        let (a, b) = (w[0], w[1]);
        let d = ((b[0] - a[0]) * 1.5).hypot(b[1] - a[1]);
        let n = (d / (size * 0.5)).ceil().max(1.0) as usize;
        for k in 0..n {
            let t = k as f64 / n as f64;
            dense.push([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]);
        }
    }
    dense.push(*pts.last().unwrap());
    (dense, size)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("thumbs") => {
            let out = &args[1];
            std::fs::create_dir_all(out).unwrap();
            for (i, f) in args[2..].iter().enumerate() {
                let (mut s, id) = open(f);
                let img = s.render_job(id, 640, 640, false, false).unwrap().run().rendered.unwrap().image;
                let name = format!("{out}/{i:02}.png");
                save(&name, &img);
                println!("{i:02} {f}");
            }
        }
        Some(mode @ ("fill" | "generate")) => {
            let (file, prefix, spec) = (&args[1], &args[2], &args[3]);
            let (pts, size) = parse(spec);
            let (mut s, id) = open(file);
            let p = s.catalog.photo(id).unwrap().clone();
            let long = p.width.max(p.height) as f64;
            // the crop shows the spot and as much again around it, at native resolution
            let (xs, ys): (Vec<f64>, Vec<f64>) = pts.iter().map(|p| (p[0], p[1])).unzip();
            let ext = |v: &[f64]| v.iter().cloned().fold(f64::MIN, f64::max) - v.iter().cloned().fold(f64::MAX, f64::min);
            let side = (((ext(&xs) * p.width as f64).max(ext(&ys) * p.height as f64) + size * long * 2.0) * 2.2).clamp(256.0, 2400.0) as usize;
            let (cx, cy) = (
                (xs.iter().cloned().fold(f64::MAX, f64::min) + xs.iter().cloned().fold(f64::MIN, f64::max)) / 2.0,
                (ys.iter().cloned().fold(f64::MAX, f64::min) + ys.iter().cloned().fold(f64::MIN, f64::max)) / 2.0,
            );
            save(&format!("{prefix}-before.png"), &crop(&mut s, id, cx, cy, side));
            let t = std::time::Instant::now();
            if mode == "fill" {
                s.execute("spot.add", &json!({"mode": "fill", "points": pts, "size": size, "feather": 30})).unwrap();
                save(&format!("{prefix}-after.png"), &crop(&mut s, id, cx, cy, side));
                println!("fill: {:.2} s (render included)", t.elapsed().as_secs_f64());
                return;
            }
            // GENFILL_REAL=1: the real provider, with the key from LCC_GENFILL_<PROVIDER>_KEY
            // (every run is a paid request)
            let real = std::env::var("GENFILL_REAL").is_ok_and(|v| v == "1");
            let provider = std::env::var("PROVIDER").unwrap_or_else(|_| if real { "openai".into() } else { "stability".into() });
            let mut _mock = None;
            if real {
                s.genfill.keys = lcc_genfill::cloud::keys::KeyStore::new(None);
            } else {
                let filler: lcc_genfill::cloud::mock::Filler = Arc::new(|w, h, rgb: &[u8], mask: &[u8], var| {
                    let img: Vec<f32> = rgb.iter().map(|v| f32::from(*v) / 255.0).collect();
                    let hole: Vec<bool> = mask.iter().map(|m| *m >= 128).collect();
                    let p = lcc_inpaint::CompleteParams { seed: u64::from(var) + 1, ..Default::default() };
                    let out = lcc_inpaint::complete(w, h, 3, &img, &hole, &p)
                        .unwrap_or_else(|| lcc_inpaint::membrane_fill(w, h, 3, &img, &hole));
                    out.iter().map(|v| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8).collect()
                });
                let mock = lcc_genfill::cloud::mock::MockServer::start(0, Some("mock-key-12345678"), filler).unwrap();
                let dir = std::env::temp_dir().join("genfill-check-keys");
                s.genfill.keys = lcc_genfill::cloud::keys::KeyStore::new(Some(dir.join("keys.json")));
                s.genfill.mock = Some(mock.base.clone());
                s.execute("genfill.setKey", &json!({"provider": provider, "key": "mock-key-12345678"})).unwrap();
                _mock = Some(mock);
            }
            s.execute("genfill.consent", &json!({"provider": provider, "accept": true})).unwrap();
            let prompt = std::env::var("PROMPT").unwrap_or_default();
            s.execute("spot.add", &json!({"mode": "generate", "points": pts, "size": size, "feather": 30})).unwrap();
            let plan = s.execute("genfill.plan", &json!({"provider": provider, "prompt": prompt, "previewPath": format!("{prefix}-sent.png")})).unwrap();
            println!("plan: {plan}");
            if std::env::var("PLAN_ONLY").is_ok() {
                return;
            }
            let r = match s.execute("spot.generate", &json!({"provider": provider, "prompt": prompt, "variations": 1})) {
                Ok(r) => r,
                Err(e) => {
                    println!("generate failed after {:.2} s: {e}", t.elapsed().as_secs_f64());
                    return;
                }
            };
            println!("generate: {r} in {:.2} s", t.elapsed().as_secs_f64());
            save(&format!("{prefix}-after.png"), &crop(&mut s, id, cx, cy, side));
            s.execute("develop.set", &json!({"control": "wb.temp", "value": 3600.0})).unwrap();
            s.execute("develop.set", &json!({"control": "light.exposure", "value": 0.7})).unwrap();
            save(&format!("{prefix}-wb.png"), &crop(&mut s, id, cx, cy, side));
            for seen in _mock.iter().flat_map(|m| m.seen()) {
                println!("sent: {} {} image {:?}", seen.method, seen.path, seen.image);
            }
        }
        _ => eprintln!("usage: genfill_check thumbs OUT FILES… | fill FILE PREFIX x,y,size[;x,y…] | generate FILE PREFIX x,y,size[;x,y…]"),
    }
}
