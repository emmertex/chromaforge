//! Hot-path cost of the recorder: ns per call for leaf spans (1 and 8 threads), top spans (begin + end, with a real
//! directory) and events.
//! usage: `cargo run --release -p lcc-diag --example bench -- [CONFIG]` (CONFIG e.g. trace, normal, critical)
use lcc_diag::*;
use std::time::Instant;

fn leaf_ns(threads: usize, n: u64, sub: Subsystem) -> f64 {
    let t = Instant::now();
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| {
                for _ in 0..n {
                    span(sub, "probe_file", Class::Leaf, &[]).finish(Outcome::Ok);
                }
            });
        }
    });
    t.elapsed().as_nanos() as f64 / n as f64 // wall per call, every thread doing n calls at once
}

fn main() {
    let dir = std::env::temp_dir().join(format!("lcc-diag-bench-{}", std::process::id()));
    let cfg = std::env::args().nth(1).unwrap_or_else(|| "trace".into());
    let (config, _) = Config::parse(&cfg);
    start(Some(dir.clone()), &config);
    println!("config: {cfg}");
    for _ in 0..2 {
        // warm-up pass, then the numbers
        leaf_ns(1, 20_000, Subsystem::Probe);
    }
    println!("leaf span, 1 thread:   {:8.1} ns/call", leaf_ns(1, 400_000, Subsystem::Probe));
    println!("leaf span, 8 threads:  {:8.1} ns/call (wall per call, 8 threads at once)", leaf_ns(8, 400_000, Subsystem::Probe));
    let n = 3000u64;
    let t = Instant::now();
    for _ in 0..n {
        span(Subsystem::Commit, "stage", Class::Top, &[("k", Val::U64(1))]).finish(Outcome::Ok);
    }
    println!("top span begin+end:    {:8.1} us/pair", t.elapsed().as_nanos() as f64 / n as f64 / 1000.0);
    let t = Instant::now();
    for _ in 0..n {
        event(Subsystem::Ui, "slow_frame", &[("ms", Val::U64(1))]);
    }
    println!("event:                 {:8.1} us/call", t.elapsed().as_nanos() as f64 / n as f64 / 1000.0);
    if let Some(s) = snapshot(0) {
        println!(
            "self-measured: {} calls, {:.1} ns/call inside the recorder; writer thread {:.1} ms",
            s.overhead.calls,
            s.overhead.recorder_ns as f64 / s.overhead.calls.max(1) as f64,
            s.overhead.writer_ns as f64 / 1e6
        );
    }
    let _ = std::fs::remove_dir_all(dir);
}
