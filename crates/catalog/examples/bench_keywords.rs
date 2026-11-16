//! Keyword benchmark (`cargo xtask bench-keywords`): what the Keywording panel asks of a library
//! with a big nested keyword list (a Lightroom Classic keyword file of a few thousand), on each
//! selection change (the Suggestions grid) and each keystroke in the Add keyword box.
//!
//! ```text
//! bench_keywords [--listed N] [--photos N]
//! ```
//!
//! Output: one JSON line, then a table (milliseconds per call, the median of several runs).

use std::time::Instant;

use lcc_catalog::keywords::KeywordInfo;
use lcc_catalog::*;

type R<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn arg(args: &[String], name: &str, default: usize) -> R<usize> {
    match args.iter().position(|a| a == name) {
        Some(i) => Ok(args.get(i + 1).ok_or(format!("{name} needs a number"))?.parse()?),
        None => Ok(default),
    }
}

/// `listed` keywords three levels deep (`Topic 3|Group 12|Keyword 345`), as a keyword file gives.
fn listed_paths(listed: usize) -> Vec<String> {
    (0..listed).map(|i| format!("Topic {}|Group {}|Keyword {i}", i % 20, i % 200)).collect()
}

/// The median time of `runs` calls of `f`, in milliseconds.
fn time(runs: usize, mut f: impl FnMut()) -> f64 {
    let mut t: Vec<f64> = (0..runs)
        .map(|_| {
            let s = Instant::now();
            f();
            s.elapsed().as_secs_f64() * 1e3
        })
        .collect();
    t.sort_by(f64::total_cmp);
    t.get(t.len() / 2).copied().unwrap_or(0.0)
}

fn main() -> R<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (listed, photos) = (arg(&args, "--listed", 2000)?, arg(&args, "--photos", 20_000)?);
    let paths = listed_paths(listed);
    let mut c = Catalog::new();
    let ops = paths.iter().map(|p| Op::SetKeyword { path: p.clone(), info: Some(KeywordInfo::default()) }).collect();
    c.apply(Op::Batch { ops })?;
    // each photo has three of the listed keywords
    let mut ops = Vec::with_capacity(photos);
    for i in 0..photos {
        let id = c.alloc_photo_id();
        let mut p = Photo::new(id, Source::Demo { scene: 1 }, &format!("IMG_{i:06}.jpg"), "JPEG", 6000, 4000, "2026-01-01");
        p.meta.keywords = (0..3).filter_map(|k| paths.get((i * 7 + k * 131) % listed.max(1)).cloned()).collect();
        ops.push(Op::AddPhoto { photo: Box::new(p) });
    }
    c.apply(Op::Batch { ops })?;
    let current: Vec<String> = paths.iter().take(2).cloned().collect();

    let mut sink = 0usize;
    // a selection change: the Suggestions grid (no text typed)
    let suggestions = time(9, || sink += c.keyword_suggestions(&current, "", 9).len());
    // a keystroke: the offers for what is typed, and whether it is a new name
    let typing = time(9, || {
        sink += c.keyword_suggestions(&current, "keyword 12", 12).len();
        sink += usize::from(c.has_keyword("keyword 12"));
        sink += c.default_keyword_parent().map_or(0, |p| p.len());
    });
    let first = {
        // after an edit to a photo's keywords: the first ask builds what the others reuse
        let id = c.photos().next().map(|p| p.id).ok_or("no photo")?;
        let mut meta = c.photo(id).ok_or("no photo")?.meta.clone();
        meta.keywords.push("Fresh".into());
        let s = Instant::now();
        c.apply(Op::SetMeta { id, meta: Box::new(meta) })?;
        sink += c.keyword_suggestions(&current, "", 9).len();
        s.elapsed().as_secs_f64() * 1e3
    };
    println!(r#"{{"listed":{listed},"photos":{photos},"suggestions_ms":{suggestions:.3},"keystroke_ms":{typing:.3},"after_edit_ms":{first:.3}}}"#);
    println!("listed {listed}, photos {photos}");
    println!("  selection change (suggestions)  {suggestions:9.3} ms");
    println!("  keystroke (offers + new name)   {typing:9.3} ms");
    println!("  first ask after an edit         {first:9.3} ms");
    std::hint::black_box(sink);
    Ok(())
}
