//! Culling step latency through the whole app: how long after a step to the next photo in the
//! loupe its full-quality render is on screen ("sharp"), with the photos prepared ahead as
//! Settings ▸ Performance ▸ Prepare photos in the background says.
//!
//! `cargo run --release -p lcc-ui-egui --example step_latency -- [FOLDER]`
//! Without a folder: the procedural demo library. With one: its photos imported into a library in a
//! temporary folder (nothing is written next to them).
//! Env: `AHEAD=5` (photos ahead each way, 0 = none), `STEPS=20`, `DWELL_MS=1500` (how long the user
//! looks at each photo before the next step), `SIZE=1600x1000`, `GPU=0` for the CPU pipeline.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::{Duration, Instant};

use lcc_engine::Session;
use lcc_ui_egui::headless::Headless;
use lcc_ui_egui::render::Slot;
use lcc_ui_egui::{LccApp, Services};
use serde_json::json;

fn sharp(h: &Headless, id: lcc_catalog::PhotoId) -> bool {
    let r = &h.app.renderer;
    r.textures.get(&Slot::Main).is_some_and(|t| t.photo == id && Some(t.key) == r.wanted(Slot::Main)) && !r.is_pending(Slot::Main)
}

fn pct(v: &mut [f64], p: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(|a, b| a.total_cmp(b));
    v[((v.len() - 1) as f64 * p).round() as usize]
}

fn main() {
    let env = |k: &str, d: &str| std::env::var(k).unwrap_or_else(|_| d.to_string());
    let ahead: u8 = env("AHEAD", "5").parse().unwrap();
    let steps: usize = env("STEPS", "20").parse().unwrap();
    let dwell = Duration::from_millis(env("DWELL_MS", "1500").parse().unwrap());
    let (w, hgt) = env("SIZE", "1600x1000").split_once('x').map(|(a, b)| (a.parse::<f32>().unwrap(), b.parse::<f32>().unwrap())).unwrap();
    let gpu = env("GPU", "1") != "0";
    let mut s = match std::env::args().nth(1) {
        Some(folder) => {
            let lib = std::env::temp_dir().join(format!("step-latency-{}", std::process::id()));
            let mut s = Session::new().with_fs().with_system_clock();
            s.open_library(&lib, false).unwrap();
            let mut paths: Vec<String> =
                std::fs::read_dir(&folder).unwrap().filter_map(|e| e.ok()).map(|e| e.path().to_string_lossy().to_string()).collect();
            paths.sort();
            s.execute("library.import", &json!({"paths": paths, "wait": true})).unwrap();
            s
        }
        None => Session::with_demo(),
    };
    s.execute(
        "prefs.setBackgroundPrep",
        &json!({"settings": {"library": {"ahead": ahead, "previews": false}, "culling": {"ahead": ahead, "previews": false}}}),
    )
    .unwrap();
    let ids = s.visible_cloned();
    let mut app = LccApp::new(s, Services::default());
    app.ui.settings.gpu = gpu;
    let mut h = Headless::new(app, [w, hgt], 1.0);
    let t = Duration::from_secs(60);
    h.request("ui.set", json!({"view": "detail", "right": "none", "filmstrip": true}), t);
    let mut times = Vec::new();
    for (n, id) in ids.iter().take(steps + 1).enumerate() {
        h.request("engine.execute", json!({"command": "library.select", "params": {"ids": [id.0], "active": id.0}}), t);
        let t0 = Instant::now();
        while !sharp(&h, *id) && t0.elapsed() < Duration::from_secs(60) {
            h.step();
            std::thread::sleep(Duration::from_millis(2));
        }
        let ms = t0.elapsed().as_secs_f64() * 1e3;
        // (the first photo is the one opened, not a step)
        if n > 0 {
            times.push(ms);
        }
        // the user looks at it for a while: the photos ahead are made meanwhile
        let t1 = Instant::now();
        while t1.elapsed() < dwell {
            h.step();
            std::thread::sleep(Duration::from_millis(16));
        }
    }
    let mean = times.iter().sum::<f64>() / times.len().max(1) as f64;
    println!(
        "ahead={ahead} steps={} dwell_ms={} gpu={gpu}: sharp after a step mean {mean:.0} ms, median {:.0} ms, p90 {:.0} ms, max {:.0} ms",
        times.len(),
        dwell.as_millis(),
        pct(&mut times.clone(), 0.5),
        pct(&mut times.clone(), 0.9),
        pct(&mut times, 1.0)
    );
}
