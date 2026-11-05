//! A batch of renders on the graphics cards, the way exports and preview builds take them: `lanes`
//! threads, each taking `wait_batch_turn_any` inside `as_batch` per photo. Prints the time and how
//! many photos each card rendered, to compare one card with two.
//!
//! `cargo run --release -p lcc-gpu --example batch_bench -- [--helper off|auto|same|<card key>]
//! [--photos 24] [--lanes 4] [--mp 24] [--all-to-helper]`
//!
//! - `--helper same`: a second device on the editing card itself (the two-card logic on one card);
//!   `--helper auto` with `<PREFIX>_GPU_HELPER=<part of a card's name>` picks a card by name.
//! - `--all-to-helper`: the previous design, every batch render on the second card while it is
//!   ready (no turn per photo), to compare.
//!
//! The source is a procedural demo photo (`lcc-scenes`); no files are read.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use lcc_develop::DevelopSettings;
use lcc_pipeline::{RenderRequest, SourceInfo};

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let helper = arg(&args, "--helper").unwrap_or_else(|| "off".into());
    let photos: usize = arg(&args, "--photos").and_then(|v| v.parse().ok()).unwrap_or(24);
    let lanes: usize = arg(&args, "--lanes").and_then(|v| v.parse().ok()).unwrap_or(4).max(1);
    let mp: f64 = arg(&args, "--mp").and_then(|v| v.parse().ok()).unwrap_or(24.0);
    let all_to_helper = args.iter().any(|a| a == "--all-to-helper");
    let w = ((mp * 1e6 * 1.5).sqrt()).round() as usize;
    let h = (w * 2 / 3).max(1);

    let t = Instant::now();
    if !lcc_gpu::available() {
        eprintln!("no GPU: {}", lcc_gpu::unavailable_reason().unwrap_or_default());
        return;
    }
    eprintln!("editing card: {} ({:.1} s)", lcc_gpu::adapter_name().unwrap_or_default(), t.elapsed().as_secs_f64());
    // (a choice is applied when it changes: from another one first, so it starts now; with
    // `<PREFIX>_GPU_HELPER` set, that variable picks the card)
    lcc_gpu::set_helper(if helper == "off" { "auto" } else { "off" });
    lcc_gpu::set_helper(&helper);
    let t = Instant::now();
    // (the set-up starts on a thread of its own: off until it has listed the cards)
    while helper != "off" && !matches!(lcc_gpu::helper_status().0, "ready" | "failed" | "lost") && t.elapsed() < Duration::from_secs(240) {
        std::thread::sleep(Duration::from_millis(50));
    }
    eprintln!("second card: {:?} ({:.1} s)", lcc_gpu::helper_status(), t.elapsed().as_secs_f64());

    let src = Arc::new(lcc_scenes::demo_library()[2].render(w, h));
    let info = SourceInfo { raw: true, ..Default::default() };
    let mut s = DevelopSettings::default();
    s.light.exposure = 0.4;
    s.effects.clarity = 25.0;
    s.detail.nr_luminance = 20.0;
    let req = RenderRequest::fit(w, h);
    let px = w * h;
    // one render first on each card in use (pipelines warm, buffers pooled)
    let _ = lcc_gpu::try_render(&src, &info, &s, &req, None);
    let _ = lcc_gpu::as_batch(|| lcc_gpu::try_render(&src, &info, &s, &req, None));

    let next = AtomicUsize::new(0);
    let before = lcc_gpu::render_counts();
    let failed = AtomicUsize::new(0);
    let t = Instant::now();
    std::thread::scope(|sc| {
        for _ in 0..lanes {
            sc.spawn(|| {
                while next.fetch_add(1, Ordering::Relaxed) < photos {
                    lcc_gpu::as_batch(|| {
                        let turn = if all_to_helper { None } else { lcc_gpu::wait_batch_turn_any(px, px, &|| false) };
                        if lcc_gpu::try_render(&src, &info, &s, &req, None).is_err() {
                            failed.fetch_add(1, Ordering::Relaxed);
                        }
                        drop(turn);
                    });
                }
            });
        }
    });
    let secs = t.elapsed().as_secs_f64();
    let after = lcc_gpu::render_counts();
    let mb = |r: Option<f64>| r.map_or("-".to_string(), |r| format!("{:.0} MB/s", r / 1e6));
    println!(
        "helper={helper}{} photos={photos} lanes={lanes} {w}x{h}: {secs:.2} s ({:.2} s/photo) | editing {} ({}) | second {} ({}) | cpu/failed {}",
        if all_to_helper { " (all to the second card)" } else { "" },
        secs / photos.max(1) as f64,
        after.editing_batch - before.editing_batch,
        mb(lcc_gpu::batch_rate_of(false)),
        after.helper_batch - before.helper_batch,
        mb(lcc_gpu::batch_rate_of(true)),
        failed.load(Ordering::Relaxed),
    );
}
