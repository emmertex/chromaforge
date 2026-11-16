//! Our pipeline as a black box: scene-linear Rec.2020 (what decoding hands the pipeline) → display XYZ D50, with the
//! sliders at zero and a candidate camera tone curve, rendered by [`lcc_pipeline::render`] and tabulated as a 3-D LUT.
use crate::{Error, Result};
use lcc_color::PROPHOTO;
use lcc_pipeline::tone::CameraTone;
use lcc_pipeline::{DeepSamples, OutputDepth, OutputSpace, RenderRequest, SourceInfo};
use lcc_raster::Rgb32f;
use lcc_raw::profile::ToneCurve;

/// LUT nodes per axis.
const N: usize = 33;
/// Encoding knee and range of the LUT axes (scene-linear).
const EPS: f64 = 0.004;
const VMAX: f64 = 16.0;

/// A DNG `ProfileToneCurve` as the pipeline's camera tone: what the decode does with a profile's tone curve
/// (32 knots log-spaced over 12 stops, as `files::dng_tone_curve`).
pub fn camera_tone(curve: &ToneCurve) -> Option<CameraTone> {
    if curve.points.len() < 2 {
        return None;
    }
    let knots: [[f32; 2]; 32] = std::array::from_fn(|i| {
        let x = 2f32.powf(-12.0 + 12.0 * i as f32 / 31.0);
        [x, curve.eval(x).min(0.9995)]
    });
    CameraTone::new(knots)
}

fn enc(v: f64) -> f64 {
    ((v.clamp(0.0, VMAX) / EPS).ln_1p() / (VMAX / EPS).ln_1p()).clamp(0.0, 1.0)
}

fn dec(u: f64) -> f64 {
    EPS * ((u.clamp(0.0, 1.0) * (VMAX / EPS).ln_1p()).exp() - 1.0)
}

pub(crate) fn settings() -> lcc_develop::DevelopSettings {
    let mut s = lcc_develop::DevelopSettings::default();
    s.detail.sharpen_amount = 0.0;
    s.detail.nr_luminance = 0.0;
    s.detail.nr_color = 0.0;
    s
}

/// Render scene-linear Rec.2020 colours through the pipeline (sliders at zero, `tone` as the camera tone) and
/// return each as display XYZ D50 (white Y = 1).
pub fn render_xyz(pixels: &[[f32; 3]], tone: Option<&CameraTone>) -> Result<Vec<[f64; 3]>> {
    let n = pixels.len();
    if n == 0 {
        return Ok(Vec::new());
    }
    let w = (n as f64).sqrt().ceil().max(1.0) as usize;
    let h = n.div_ceil(w);
    let mut img = Rgb32f::try_new(w, h).map_err(|e| Error(format!("render: {e:?}")))?;
    for (d, p) in img.data.iter_mut().zip(pixels) {
        *d = *p;
    }
    let info = SourceInfo { raw: true, camera_tone: tone.cloned(), ..Default::default() };
    let req = RenderRequest { depth: OutputDepth::F32Linear, space: OutputSpace::ProPhoto, ..RenderRequest::fit(w, h) };
    let out = lcc_pipeline::render(&img, &info, &settings(), &req);
    let deep = out.deep.ok_or_else(|| Error("render: the pipeline gave no float output".into()))?;
    if deep.width != w || deep.height != h {
        return Err(Error(format!("render: asked for {w}×{h}, got {}×{}", deep.width, deep.height)));
    }
    let DeepSamples::F32(s) = deep.samples else {
        return Err(Error("render: the pipeline gave no float samples".into()));
    };
    let m = PROPHOTO.to_xyz();
    let px: Vec<[f64; 3]> = s.as_chunks::<3>().0.iter().take(n).map(|c| m.apply([f64::from(c[0]), f64::from(c[1]), f64::from(c[2])])).collect();
    if px.len() != n {
        return Err(Error("render: short output".into()));
    }
    Ok(px)
}

/// Display luminance (Y) of neutral scene values `xs` through the pipeline.
pub fn gray_response(xs: &[f64], tone: Option<&CameraTone>) -> Result<Vec<f64>> {
    let px: Vec<[f32; 3]> = xs.iter().map(|x| [*x as f32; 3]).collect();
    Ok(render_xyz(&px, tone)?.iter().map(|v| v[1]).collect())
}

/// The pipeline tabulated: scene-linear Rec.2020 → display XYZ D50.
#[derive(Clone, Debug)]
pub struct Lut {
    data: Vec<[f64; 3]>,
    /// Rendering the same colours in another order gave the same result (the zero-slider render is per-pixel).
    #[cfg_attr(not(test), allow(dead_code))]
    pub per_pixel: bool,
}

impl Lut {
    pub fn build(tone: Option<&CameraTone>) -> Result<Lut> {
        let mut px = Vec::with_capacity(N * N * N);
        for b in 0..N {
            for g in 0..N {
                for r in 0..N {
                    let f = |i: usize| dec(i as f64 / (N - 1) as f64) as f32;
                    px.push([f(r), f(g), f(b)]);
                }
            }
        }
        let data = render_xyz(&px, tone)?;
        // the same colours reversed: a spatial stage would make them differ
        let rev: Vec<[f32; 3]> = px.iter().rev().copied().collect();
        let back = render_xyz(&rev, tone)?;
        let per_pixel = data.iter().zip(back.iter().rev()).all(|(a, b)| (0..3).all(|i| (a[i] - b[i]).abs() < 2e-3));
        Ok(Lut { data, per_pixel })
    }

    fn node(&self, r: usize, g: usize, b: usize) -> [f64; 3] {
        self.data.get((b * N + g) * N + r).copied().unwrap_or([0.0; 3])
    }

    fn eval_u(&self, u: [f64; 3]) -> [f64; 3] {
        let mut i0 = [0usize; 3];
        let mut t = [0.0; 3];
        for c in 0..3 {
            let f = u[c].clamp(0.0, 1.0) * (N - 1) as f64;
            let i = (f as usize).min(N - 2);
            i0[c] = i;
            t[c] = f - i as f64;
        }
        let mut out = [0.0; 3];
        for corner in 0..8 {
            let (dr, dg, db) = (corner & 1, (corner >> 1) & 1, (corner >> 2) & 1);
            let w = (if dr == 1 { t[0] } else { 1.0 - t[0] }) * (if dg == 1 { t[1] } else { 1.0 - t[1] }) * (if db == 1 { t[2] } else { 1.0 - t[2] });
            if w == 0.0 {
                continue;
            }
            let v = self.node(i0[0] + dr, i0[1] + dg, i0[2] + db);
            for c in 0..3 {
                out[c] += w * v[c];
            }
        }
        out
    }

    /// Display XYZ D50 of a scene-linear Rec.2020 colour (negative channels clamp to zero).
    pub fn eval(&self, scene: [f64; 3]) -> [f64; 3] {
        self.eval_u(scene.map(enc))
    }

    /// The scene-linear Rec.2020 colour the pipeline shows as `target` (display XYZ D50), searched from `start`
    /// (Newton steps on the LUT, clamped to its range). `None` when the search doesn't get close.
    pub fn invert(&self, target: [f64; 3], start: [f64; 3]) -> Option<[f64; 3]> {
        let mut u = start.map(enc);
        let err = |u: [f64; 3]| {
            let v = self.eval_u(u);
            [v[0] - target[0], v[1] - target[1], v[2] - target[2]]
        };
        let norm = |e: [f64; 3]| e.iter().map(|x| x * x).sum::<f64>().sqrt();
        let mut e = err(u);
        let h = 1e-3;
        for _ in 0..30 {
            if norm(e) < 1e-5 {
                break;
            }
            let mut jac = vec![vec![0.0; 3]; 3];
            for c in 0..3 {
                let mut up = u;
                let step = if up[c] + h > 1.0 { -h } else { h };
                up[c] += step;
                let ep = err(up);
                for r in 0..3 {
                    jac[r][c] = (ep[r] - e[r]) / step;
                }
            }
            let Some(d) = super::linalg::solve(jac, e.map(|x| -x).to_vec()) else {
                break;
            };
            let mut lambda = 1.0;
            let mut moved = false;
            for _ in 0..6 {
                let un = [0, 1, 2].map(|c| (u[c] + lambda * d.get(c).copied().unwrap_or(0.0)).clamp(0.0, 1.0));
                let en = err(un);
                if norm(en) < norm(e) {
                    u = un;
                    e = en;
                    moved = true;
                    break;
                }
                lambda *= 0.5;
            }
            if !moved {
                break;
            }
        }
        let scale = target[1].abs().max(0.01);
        (norm(e) < 0.02 * scale.max(0.05)).then(|| u.map(dec))
    }
}
