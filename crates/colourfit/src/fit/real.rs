//! The real decode: a raw rendered through the app's own decode and pipeline (`files::load_bytes_with` +
//! `lcc_pipeline::render`, the fit's baseline settings, linear float ProPhoto → XYZ D50) at a small size, read back
//! at the samples' positions. Used for the true "before" (camera choice `lc`), the closed loop and the "after"
//! (the candidate installed as an imported profile).
//!
//! Sources: own design.
use super::pipe;
use crate::{Error, Result};
use lcc_color::PROPHOTO;
use lcc_pipeline::{DeepSamples, OutputDepth, OutputSpace, RenderRequest};
use rayon::prelude::*;
use std::path::Path;

/// Long edge of the renders (memory stays small: one decode per worker).
pub const EDGE: usize = 512;
/// The registry id the candidate renders under.
pub const CANDIDATE: &str = "dcp:colourfit-candidate";
/// Renders in flight at once (each holds a full raw decode while it runs).
const PARALLEL: usize = 4;

/// One render as display XYZ D50 (white Y = 1).
pub struct Rendered {
    w: usize,
    h: usize,
    data: Vec<[f64; 3]>,
}

impl Rendered {
    /// The mean of the 3 × 3 pixels around normalised `pos` (`None` off the image or for a bad position).
    pub fn at(&self, pos: [f32; 2]) -> Option<[f64; 3]> {
        if !pos.iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v)) || self.w == 0 || self.h == 0 {
            return None;
        }
        let cx = ((pos[0] * self.w as f32) as usize).min(self.w - 1);
        let cy = ((pos[1] * self.h as f32) as usize).min(self.h - 1);
        let mut acc = [0.0; 3];
        let mut n = 0.0;
        for y in cy.saturating_sub(1)..=(cy + 1).min(self.h - 1) {
            for x in cx.saturating_sub(1)..=(cx + 1).min(self.w - 1) {
                let p = self.data.get(y * self.w + x)?;
                for c in 0..3 {
                    acc[c] += p[c];
                }
                n += 1.0;
            }
        }
        let v = acc.map(|a| a / n);
        v.iter().all(|x| x.is_finite()).then_some(v)
    }
}

/// Render the raw at `path` with camera choice `camera` (`lc`, or [`CANDIDATE`] while it is installed).
pub fn render(path: &Path, camera: &str) -> Result<Rendered> {
    let bytes = crate::dataset::read_file(path).map_err(Error)?;
    let options = lcc_engine::media::LoadOptions { camera: camera.to_string(), ..Default::default() };
    let (img, info) = lcc_engine::files::load_bytes_with(&bytes, EDGE, options).map_err(Error)?;
    drop(bytes);
    let req = RenderRequest { depth: OutputDepth::F32Linear, space: OutputSpace::ProPhoto, ..RenderRequest::fit(EDGE, EDGE) };
    let out = lcc_pipeline::render(&img, &info, &pipe::settings(), &req);
    let deep = out.deep.ok_or_else(|| Error("render: the pipeline gave no float output".into()))?;
    let DeepSamples::F32(s) = deep.samples else {
        return Err(Error("render: the pipeline gave no float samples".into()));
    };
    let (w, h) = (deep.width, deep.height);
    let m = PROPHOTO.to_xyz();
    let data: Vec<[f64; 3]> = s.as_chunks::<3>().0.iter().map(|c| m.apply([f64::from(c[0]), f64::from(c[1]), f64::from(c[2])])).collect();
    if w == 0 || h == 0 || data.len() < w.saturating_mul(h) {
        return Err(Error("render: short output".into()));
    }
    Ok(Rendered { w, h, data })
}

/// Run `f` with `dcp` installed as [`CANDIDATE`], then put the imported-profile registry back as it was.
pub fn with_candidate<T>(dcp: lcc_raw::dcp::Dcp, f: impl FnOnce() -> T) -> T {
    let saved = lcc_raw::dcp::library();
    let mut list: Vec<(String, lcc_raw::dcp::Dcp)> = saved.iter().map(|(id, d)| (id.clone(), (**d).clone())).collect();
    list.push((CANDIDATE.to_string(), dcp));
    lcc_raw::dcp::install_library(list);
    let out = f();
    lcc_raw::dcp::install_library(saved.iter().map(|(id, d)| (id.clone(), (**d).clone())).collect());
    out
}

/// Each listed raw rendered with `camera` and read at the given positions: `out[i][k]` is `paths[i]`'s colour at
/// `positions[i][k]`. Errors name the first raw that could not be rendered.
pub fn read_back(jobs: &[(&Path, Vec<[f32; 2]>)], camera: &str) -> Result<Vec<Vec<Option<[f64; 3]>>>> {
    let mut out = Vec::with_capacity(jobs.len());
    for chunk in jobs.chunks(PARALLEL) {
        let got: Vec<Result<Vec<Option<[f64; 3]>>>> = chunk
            .par_iter()
            .map(|(path, positions)| {
                let r = render(path, camera).map_err(|e| Error(format!("{}: {e}", crate::dataset::file_name(path))))?;
                Ok(positions.iter().map(|p| r.at(*p)).collect())
            })
            .collect();
        for g in got {
            out.push(g?);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reading_back_is_bounded_and_missing_raws_are_errors() {
        let r = Rendered { w: 4, h: 3, data: vec![[0.5, 0.4, 0.3]; 12] };
        assert_eq!(r.at([0.0, 0.0]), Some([0.5, 0.4, 0.3]));
        assert!(r.at([1.0, 1.0]).is_some());
        for bad in [[-0.1, 0.5], [0.5, 1.5], [f32::NAN, 0.5], [f32::INFINITY, 0.0]] {
            assert!(r.at(bad).is_none());
        }
        let empty = Rendered { w: 0, h: 0, data: Vec::new() };
        assert!(empty.at([0.5, 0.5]).is_none());
        assert!(read_back(&[(Path::new("/nonexistent/colourfit.raw"), vec![[0.5, 0.5]])], "lc").is_err());
    }
}
