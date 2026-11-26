//! Measure the per-photo defect pass (`detect` at the learning thresholds, `refine`, `correct`) on its own, and fingerprint
//! what it found and the mosaic it leaves, so a faster version can be checked against the old one bit for bit.
//! `cargo run --release -p lcc-raw --example defects_bench -- FILE...`
use std::time::Instant;

use lcc_raw::defects::{Fix, Thresholds, correct, detect, refine};

fn fnv(h: u64, v: u64) -> u64 {
    (h ^ v).wrapping_mul(0x100000001b3)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    for path in std::env::args().skip(1) {
        let bytes = std::fs::read(&path)?;
        let mut raw = lcc_raw::decode(&bytes)?;
        let t0 = Instant::now();
        let found = detect(&raw, &Thresholds::LEARN);
        let detect_ms = t0.elapsed().as_secs_f64() * 1000.0;
        let t1 = Instant::now();
        let strict = refine(&raw, &found, &Thresholds::CORRECT);
        let refine_ms = t1.elapsed().as_secs_f64() * 1000.0;
        let mut h = 0xcbf29ce484222325u64;
        for d in &found.defects {
            h = fnv(h, u64::from(d.x) << 32 | u64::from(d.y));
            h = fnv(h, u64::from(d.s) << 8 | d.kind as u64);
            h = fnv(h, u64::from(d.level.to_bits()));
        }
        for n in found.noise {
            h = fnv(h, u64::from(n.to_bits()));
        }
        let fixes: Vec<Fix> = strict.iter().map(Fix::from).collect();
        let t2 = Instant::now();
        let n = correct(&mut raw, &fixes);
        let correct_ms = t2.elapsed().as_secs_f64() * 1000.0;
        let mosaic = match &raw.data {
            lcc_raw::RawData::U16(d) => d.iter().fold(0xcbf29ce484222325u64, |h, v| fnv(h, u64::from(*v))),
            lcc_raw::RawData::F32(d) => d.iter().fold(0xcbf29ce484222325u64, |h, v| fnv(h, u64::from(v.to_bits()))),
        };
        println!(
            "{path}\t{detect_ms:.1}\t{refine_ms:.1}\t{correct_ms:.1}\t{}\t{}\t{n}\t{}\t{h:016x}\t{mosaic:016x}",
            found.defects.len(),
            strict.len(),
            found.overflow
        );
    }
    Ok(())
}
