//! `cargo run --release -p lcc-pipeline --example dustscan -- IMAGE.rgb WIDTH HEIGHT [SENSITIVITY]`
//!
//! Runs the dust detector on a raw 8-bit RGB file (`WIDTH × HEIGHT × 3` bytes, e.g. a rendered photo saved as
//! headerless RGB) and prints one line per spot: x, y (0..1), radius (fraction of the long edge), dip, texture,
//! confidence. A diagnostic for checking the detector on real photos.

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let (Some(path), Some(w), Some(h)) = (a.first(), a.get(1).and_then(|v| v.parse::<usize>().ok()), a.get(2).and_then(|v| v.parse::<usize>().ok()))
    else {
        eprintln!("usage: dustscan IMAGE.rgb WIDTH HEIGHT [SENSITIVITY]");
        return;
    };
    let sens = a.get(3).and_then(|v| v.parse().ok()).unwrap_or(50.0);
    let Ok(bytes) = std::fs::read(path) else { return };
    if bytes.len() != w * h * 3 {
        eprintln!("{path}: {} bytes, not {w} × {h} × 3", bytes.len());
        return;
    }
    let data = bytes.as_chunks::<3>().0.iter().map(|p| [p[0], p[1], p[2], 255]).collect();
    let img = lcc_raster::Rgba8 { width: w, height: h, data };
    let t = std::time::Instant::now();
    let found = lcc_pipeline::dust::detect(&img, sens);
    eprintln!("{} spots in {:.0} ms", found.len(), t.elapsed().as_secs_f64() * 1e3);
    for s in found {
        println!("{:.5}\t{:.5}\t{:.5}\t{:.4}\t{:.3}\t{:.2}", s.x, s.y, s.radius, s.dip, s.texture, s.confidence);
    }
}
