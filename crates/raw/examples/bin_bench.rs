//! Time the reduced-size development of a raw (superpixel binning) and fingerprint its output.
//! `cargo run --release -p lcc-raw --example bin_bench -- K FILE...` (K = block size, 4 for a 45 MP Z9 at the 2560 px level)
use std::time::Instant;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let k: usize = args.next().ok_or("block size")?.parse()?;
    for path in args {
        let bytes = std::fs::read(&path)?;
        let raw = lcc_raw::decode(&bytes)?;
        let mut best = f64::MAX;
        let mut hash = 0u64;
        for _ in 0..5 {
            let start = Instant::now();
            let Some((img, mask)) = raw.develop_binned_masked(k, 0.99)? else {
                println!("{path}\tnot binnable at {k}");
                break;
            };
            best = best.min(start.elapsed().as_secs_f64() * 1000.0);
            let fold = |h: u64, v: u64| (h ^ v).wrapping_mul(0x100000001b3);
            hash = img.data.iter().flatten().fold(0xcbf29ce484222325u64, |h, v| fold(h, u64::from(v.to_bits())));
            hash = mask.iter().fold(hash, |h, &m| fold(h, u64::from(m)));
        }
        println!("{path}\t{k}\t{best:.2}\t{hash:016x}");
    }
    Ok(())
}
