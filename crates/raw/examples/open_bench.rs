//! Measure sensor decoding and hot-pixel suppression separately, without verbose profiling.
//! `cargo run --release -p lcc-raw --example open_bench -- FILE...`
use std::time::Instant;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    for path in std::env::args().skip(1) {
        let bytes = std::fs::read(&path)?;
        let start = Instant::now();
        let mut raw = lcc_raw::decode(&bytes)?;
        let decode_ms = start.elapsed().as_secs_f64() * 1000.0;
        let start = Instant::now();
        let replaced = raw.suppress_hot_pixels();
        let hot_ms = start.elapsed().as_secs_f64() * 1000.0;
        let hash = match &raw.data {
            lcc_raw::RawData::U16(data) => data.iter().fold(0xcbf29ce484222325u64, |h, v| (h ^ u64::from(*v)).wrapping_mul(0x100000001b3)),
            lcc_raw::RawData::F32(data) => data.iter().fold(0xcbf29ce484222325u64, |h, v| (h ^ u64::from(v.to_bits())).wrapping_mul(0x100000001b3)),
        };
        println!("{path}\t{}\t{}\t{decode_ms:.3}\t{hot_ms:.3}\t{replaced}\t{hash:016x}", raw.width, raw.height);
    }
    Ok(())
}
