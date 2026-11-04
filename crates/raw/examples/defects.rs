//! `cargo run --release -p lcc-raw --example defects -- FILE...`
//!
//! Decodes each raw and prints what the hot / dead / stuck pixel detector finds (both thresholds), with timings,
//! the body serial and exposure, as one JSON line per file (`--list` adds the positions).

use lcc_raw::defects::{Thresholds, detect};
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let list = args.iter().any(|a| a == "--list");
    for path in args.iter().filter(|a| !a.starts_with("--")) {
        let Ok(bytes) = std::fs::read(path) else {
            eprintln!("{path}: unreadable");
            continue;
        };
        let t0 = Instant::now();
        let raw = match lcc_raw::decode(&bytes) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("{path}: {e}");
                continue;
            }
        };
        let decode_ms = t0.elapsed().as_secs_f64() * 1e3;
        let t1 = Instant::now();
        let fix = detect(&raw, &Thresholds::CORRECT);
        let fix_ms = t1.elapsed().as_secs_f64() * 1e3;
        let learn = detect(&raw, &Thresholds::LEARN);
        let m = &raw.metadata;
        let pos = |d: &lcc_raw::defects::Detection| -> Vec<String> {
            d.defects.iter().take(200).map(|q| format!("{},{},{:?},{:.3}", q.x, q.y, q.kind, q.level)).collect()
        };
        println!(
            "{{\"file\":{:?},\"serial\":{:?},\"model\":{:?},\"iso\":{:?},\"t\":{:?},\"f\":{:?},\"date\":{:?},\"decodeMs\":{decode_ms:.0},\"detectMs\":{fix_ms:.0},\"noise\":{:?},\"correct\":{},\"learn\":{},\"overflow\":{}{}}}",
            path,
            m.serial_number,
            m.model,
            m.iso,
            m.exposure_time,
            m.f_number,
            m.capture_time.map(|t| t.to_iso()),
            fix.noise,
            fix.defects.len(),
            learn.defects.len(),
            learn.overflow,
            if list { format!(",\"fixes\":{:?},\"learnt\":{:?}", pos(&fix), pos(&learn)) } else { String::new() }
        );
    }
}
