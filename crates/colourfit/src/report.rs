//! The local report: held-out ΔE00 per look and side-by-side crops (the app before, the app with the calibration,
//! Lightroom) of a few photos. It holds the contributor's photos, so it stays on their machine: never sent.
use crate::dataset::PhotoSamples;
use crate::{Error, Result};
use lcc_camdb::calibration::{Calibration, Look};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// Long edge of the images in the report.
const EDGE: usize = 360;
/// The registry id the candidate is rendered under.
const CANDIDATE: &str = "dcp:colourfit-candidate";

fn escape(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '<' => "&lt;".to_string(),
            '>' => "&gt;".to_string(),
            '&' => "&amp;".to_string(),
            '"' => "&quot;".to_string(),
            c => c.to_string(),
        })
        .collect()
}

/// A file rendered at the report's size with the sliders at zero, as PNG bytes.
fn render_png(bytes: &[u8], camera: &str) -> Result<Vec<u8>> {
    let options = lcc_engine::media::LoadOptions { camera: camera.to_string(), ..Default::default() };
    let (img, info) = lcc_engine::files::load_bytes_with(bytes, EDGE, options).map_err(Error)?;
    let out = lcc_pipeline::render(&img, &info, &lcc_develop::DevelopSettings::default(), &lcc_pipeline::RenderRequest::fit(EDGE, EDGE));
    lcc_codecs::encode::encode_png(&lcc_codecs::encode::EncodeImage::rgba8(&out.image), &Default::default()).map_err(|e| Error(e.to_string()))
}

/// The three images of one photo, written next to the report as `p<n>-before|after|lightroom.png`.
fn photo_images(dir: &Path, n: usize, p: &PhotoSamples) -> Result<[String; 3]> {
    let raw = std::fs::read(&p.raw_path).map_err(|e| Error(format!("reading a raw: {e}")))?;
    let export = std::fs::read(&p.export_path).map_err(|e| Error(format!("reading an export: {e}")))?;
    let images = [render_png(&raw, "lc")?, render_png(&raw, CANDIDATE)?, render_png(&export, "")?];
    let mut names = [String::new(), String::new(), String::new()];
    for ((png, tag), name) in images.iter().zip(["before", "after", "lightroom"]).zip(names.iter_mut()) {
        *name = format!("p{n}-{tag}.png");
        std::fs::write(dir.join(&*name), png).map_err(|e| Error(format!("writing the report: {e}")))?;
    }
    Ok(names)
}

/// Write `index.html` (and its images) into `dir`: the held-out evidence per look and, for up to `max_photos`
/// held-out photos, the app's colour before, with the calibration and Lightroom's export side by side.
/// Returns the report's path.
pub fn write(dir: &Path, cal: &Calibration, photos: &[PhotoSamples], max_photos: usize) -> Result<PathBuf> {
    std::fs::create_dir_all(dir).map_err(|e| Error(format!("creating the report folder: {e}")))?;
    let mut html = String::new();
    let _ = write!(
        html,
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>Calibration report</title><style>\
         body{{font-family:sans-serif;margin:16px;background:#1e1e1e;color:#ddd}}\
         .banner{{background:#a33;color:#fff;padding:12px;font-weight:bold}}\
         table{{border-collapse:collapse;margin:12px 0}}td,th{{border:1px solid #555;padding:4px 8px;text-align:right}}\
         .row{{display:flex;gap:8px;flex-wrap:wrap;margin:12px 0}}figure{{margin:0}}img{{max-width:{EDGE}px}}\
         </style></head><body><div class=\"banner\">This report contains your photos. Do not send it. \
         Only the calibration file is contributed.</div><h1>{} {}</h1>",
        escape(&cal.make),
        escape(&cal.model)
    );
    let e = &cal.evidence;
    let _ = write!(
        html,
        "<p>{} photos, {} samples, {} white points, hue coverage {:.0} %.</p>\
         <table><tr><th>Look</th><th>photos</th><th>held out</th><th>before median</th><th>before p95</th>\
         <th>after median</th><th>after p95</th></tr>",
        e.photos,
        e.samples,
        e.white_points,
        e.hue_coverage * 100.0
    );
    for l in &e.looks {
        let _ = write!(
            html,
            "<tr><td>{}</td><td>{}</td><td>{}</td><td>{:.2}</td><td>{:.2}</td><td>{:.2}</td><td>{:.2}</td></tr>",
            escape(&l.name),
            l.photos,
            l.held_out,
            l.before.median,
            l.before.p95,
            l.after.median,
            l.after.p95
        );
    }
    html.push_str("</table><p>ΔE00 against Lightroom on photos the fit did not see.</p>");
    // the candidate renders through the imported-profile registry: keep what was there and put it back after
    let saved = lcc_raw::dcp::library();
    let mut shown = 0usize;
    let mut chosen: Vec<&PhotoSamples> = photos.iter().filter(|p| p.held_out).collect();
    if chosen.is_empty() {
        chosen = photos.iter().collect();
    }
    for p in chosen.into_iter().take(max_photos) {
        let look: Option<&Look> = cal.looks.iter().find(|l| l.name == p.look.trim()).or_else(|| cal.default_look());
        let Some(look) = look else { continue };
        let mut list: Vec<(String, lcc_raw::dcp::Dcp)> = saved.iter().map(|(id, d)| (id.clone(), (**d).clone())).collect();
        list.push((CANDIDATE.to_string(), cal.look_dcp(look)));
        lcc_raw::dcp::install_library(list);
        shown += 1;
        let _ = write!(html, "<h2>Photo {shown} ({})</h2>", escape(&look.name));
        match photo_images(dir, shown, p) {
            Ok([b, a, l]) => {
                let _ = write!(
                    html,
                    "<div class=\"row\"><figure><img src=\"{b}\"><figcaption>Before</figcaption></figure>\
                     <figure><img src=\"{a}\"><figcaption>With the calibration</figcaption></figure>\
                     <figure><img src=\"{l}\"><figcaption>Lightroom</figcaption></figure></div>"
                );
            }
            Err(err) => {
                let _ = write!(html, "<p>Could not render this photo: {}</p>", escape(&err.0));
            }
        }
    }
    lcc_raw::dcp::install_library(saved.iter().map(|(id, d)| (id.clone(), (**d).clone())).collect());
    html.push_str("</body></html>\n");
    let path = dir.join("index.html");
    std::fs::write(&path, html).map_err(|e| Error(format!("writing the report: {e}")))?;
    Ok(path)
}
