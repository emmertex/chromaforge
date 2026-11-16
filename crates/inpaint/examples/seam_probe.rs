//! Probe completion seams on a real crop: `seam_probe IN.png OUT_PREFIX x0,y0,x1,y1 cx,cy0,cy1,r`
//! (box in the PNG, a vertical stroke at cx from cy0 to cy1 with radius r, all in PNG pixels).
//! Writes `<prefix>-<variant>.png` for a few parameter variants. Dev tool.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use lcc_inpaint::{CompleteParams, complete};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let dec = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(&a[0]).unwrap()));
    let mut r = dec.read_info().unwrap();
    let mut buf = vec![0u8; r.output_buffer_size().unwrap()];
    let info = r.next_frame(&mut buf).unwrap();
    let (w, ch) = (info.width as usize, info.color_type.samples());
    let b: Vec<usize> = a[2].split(',').map(|v| v.parse().unwrap()).collect();
    let s: Vec<f32> = a[3].split(',').map(|v| v.parse().unwrap()).collect();
    let (bw, bh) = (b[2] - b[0], b[3] - b[1]);
    let buf = &buf;
    // linear-ish values, square-rooted like the pipeline
    let img: Vec<f32> = (0..bw * bh)
        .flat_map(|i| {
            let (x, y) = (b[0] + i % bw, b[1] + i / bw);
            (0..3).map(move |c| (buf_at(buf, (y * w + x) * ch + c) / 255.0).powf(2.2).sqrt())
        })
        .collect();
    let hole: Vec<bool> = (0..bw * bh)
        .map(|i| {
            let (x, y) = ((b[0] + i % bw) as f32 + 0.5, (b[1] + i / bw) as f32 + 0.5);
            let yy = y.clamp(s[1], s[2]);
            (x - s[0]).hypot(y - yy) < s[3]
        })
        .collect();
    let lines = lcc_inpaint::structure::detect_lines(bw, bh, 3, &img, &hole, None, &|| false);
    for l in &lines {
        println!("line n={:?} rho={:.1} t={:.0}..{:.0} contrast={:.3}", l.n, l.rho, l.t0, l.t1, l.contrast);
    }
    for (name, p) in [
        ("default", CompleteParams::default()),
        ("nostructure", CompleteParams { structure: 0.0, ..Default::default() }),
        ("norefine", CompleteParams { refine_extent: usize::MAX, ..Default::default() }),
        ("r5", CompleteParams { patch_radius: 5, ..Default::default() }),
        ("r5norefine", CompleteParams { patch_radius: 5, refine_extent: usize::MAX, ..Default::default() }),
        ("em16", CompleteParams { em_iters: 16, min_em_iters: 4, ..Default::default() }),
    ] {
        let t = std::time::Instant::now();
        let out = complete(bw, bh, 3, &img, &hole, &p).unwrap();
        println!("{name}: {:.2} s", t.elapsed().as_secs_f64());
        let bytes: Vec<u8> = out.iter().map(|v| ((v * v).powf(1.0 / 2.2).clamp(0.0, 1.0) * 255.0 + 0.5) as u8).collect();
        let f = std::fs::File::create(format!("{}-{name}.png", a[1])).unwrap();
        let mut e = png::Encoder::new(std::io::BufWriter::new(f), bw as u32, bh as u32);
        e.set_color(png::ColorType::Rgb);
        e.set_depth(png::BitDepth::Eight);
        e.write_header().unwrap().write_image_data(&bytes).unwrap();
    }
}

fn buf_at(b: &[u8], i: usize) -> f32 {
    f32::from(b[i])
}
