//! Run the EoMT scene model on photos and write what it selects, to look at.
//!
//! `cargo run --release -p lcc-segment --example scene_overlay -- <model dir> <out dir> <photo.ppm>…`
//! Each photo is a binary PPM (P6, 8-bit). For each, `<name>-{sky,water,vegetation,person}.pgm`
//! are written on the model's grid (0..255 = probability), and one timing line per photo on stdout.

use std::path::{Path, PathBuf};
use std::time::Instant;

use lcc_segment::scene::{self, SceneModel};

fn read_ppm(path: &Path) -> Result<(Vec<u8>, usize, usize), String> {
    let b = std::fs::read(path).map_err(|e| e.to_string())?;
    // header: P6 <w> <h> <max>, whitespace separated, then one byte of whitespace
    let mut fields = Vec::new();
    let mut i = 0;
    while fields.len() < 4 {
        while b.get(i).is_some_and(|c| c.is_ascii_whitespace()) {
            i += 1;
        }
        let start = i;
        while b.get(i).is_some_and(|c| !c.is_ascii_whitespace()) {
            i += 1;
        }
        fields.push(String::from_utf8_lossy(b.get(start..i).ok_or("short header")?).to_string());
    }
    if fields[0] != "P6" || fields[3] != "255" {
        return Err("not an 8-bit P6 PPM".into());
    }
    let (w, h): (usize, usize) = (fields[1].parse().map_err(|_| "width")?, fields[2].parse().map_err(|_| "height")?);
    let data = b.get(i + 1..i + 1 + w * h * 3).ok_or("short pixel data")?.to_vec();
    Ok((data, w, h))
}

fn write_pgm(path: &Path, w: usize, h: usize, v: &[f32]) -> std::io::Result<()> {
    let mut out = format!("P5\n{w} {h}\n255\n").into_bytes();
    out.extend(v.iter().map(|p| (p.clamp(0.0, 1.0) * 255.0).round() as u8));
    std::fs::write(path, out)
}

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [dir, out, photos @ ..] = args.as_slice() else {
        return Err("usage: scene_overlay <model dir> <out dir> <photo.ppm>…".into());
    };
    let t = Instant::now();
    let model = SceneModel::load(Path::new(dir)).map_err(|e| e.to_string())?;
    println!("load {:?} on {:?}", t.elapsed(), model.device());
    let classes: [(&str, fn(&str) -> bool); 4] = [
        ("sky", |l| l.starts_with("sky")),
        ("water", |l| matches!(l, "sea" | "river" | "water-other")),
        ("vegetation", |l| matches!(l, "tree-merged" | "grass-merged" | "flower" | "pottedplant")),
        ("person", |l| l == "person"),
    ];
    for p in photos {
        let (rgb, w, h) = read_ppm(Path::new(p))?;
        // the first run of a size warms up; time the second
        let _ = model.segment(&rgb, w, h).map_err(|e| e.to_string())?;
        let t = Instant::now();
        let s = model.segment(&rgb, w, h).map_err(|e| e.to_string())?;
        let ms = t.elapsed().as_secs_f64() * 1e3;
        let name = Path::new(p).file_stem().and_then(|s| s.to_str()).unwrap_or("photo");
        let mut cover = Vec::new();
        for (c, pred) in classes {
            let v = s.class_probability(&scene::class_ids(pred));
            let share = v.iter().filter(|x| **x >= 0.5).count() as f64 / v.len().max(1) as f64;
            cover.push(format!("{c} {:.0}%", share * 100.0));
            write_pgm(&PathBuf::from(out).join(format!("{name}-{c}.pgm")), s.width, s.height, &v).map_err(|e| e.to_string())?;
        }
        let found: Vec<String> = s.instances.iter().take(6).map(|i| format!("{} {:.2}", scene::COCO_PANOPTIC_LABELS[i.class], i.score)).collect();
        println!("{name}: {w}x{h} in {ms:.0} ms; {}; {found:?}", cover.join(", "));
    }
    Ok(())
}
