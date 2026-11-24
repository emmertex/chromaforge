//! Glow (beyond Lightroom, after DaVinci Resolve's Glow effect): the picture's bright parts,
//! taken above a threshold and spread as a halo over it.
//!
//! Resolve's Glow runs on the rendered picture, so this does too: a post-pass on the finished,
//! output-encoded buffer (the same place the Display-stage pixel-filter plug-ins run), after
//! every develop slider and the vignette, before quantisation. The steps are Resolve's:
//!
//! - **Shine Threshold** picks the glow sources (a soft knee in encoded luminance, so a hair of
//!   over-threshold doesn't switch the whole region on);
//! - **Spread** sets the halo's width — relative to the frame (`px_per_long`), so a preview and an
//!   export agree — and **H/V Ratio** stretches it (the proxy below is downsampled anisotropically,
//!   which *is* the elliptical blur: resampling back spreads each axis by its own factor);
//! - **Gain** and **Gamma** shape the blurred glow (brightness and falloff);
//! - the glow composites in **Screen** (its gentler default) or **Add**, through **Amount**
//!   (Resolve's Opacity);
//! - **Recovery** (Bright Region Recovery) mixes the bright regions' own detail, which the blur
//!   smeared, back through the glow.
//!
//! The blur runs on a downsampled proxy of the window (at most 1024 px on its long side: a halo
//! holds no detail finer than that), [`lcc_raster::blur::gaussian`], then the result is sampled
//! back up at full size — a wide halo costs the same at any resolution. A render window is grown
//! by the halo's reach ([`reach`]) and cut back, so a zoomed view shows the halos that reach into
//! it from outside (as the joint colour NR's windows are).

use lcc_develop::{DevelopSettings, Glow, GlowComposite};
use lcc_raster::blur::gaussian;
use lcc_raster::{Rgb32f, par_rows};

/// Encoded-luminance weights (Rec. 709), the domain the threshold works in.
const LUMA: [f32; 3] = [0.2126, 0.7152, 0.0722];

/// The sliders, as the post-pass reads them. `None` from [`Params::of`] while Glow is off (or its
/// values are not numbers, which [`lcc_develop::controls`] keeps from happening to slider input).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Params {
    /// Glow contribution, 0..1 (Resolve's Opacity).
    pub amount: f32,
    /// Encoded luminance the sources start above.
    pub threshold: f32,
    /// Width of the knee over the threshold, in encoded luminance.
    pub knee: f32,
    /// Halo width as a fraction of the long edge.
    pub sigma: f32,
    /// Brightness of the glow itself (×1 at the slider's middle).
    pub gain: f32,
    /// Falloff exponent of the glow (1 at the slider's middle).
    pub gamma: f32,
    /// Horizontal spread relative to vertical (1 = even).
    pub ratio: f32,
    /// Bright regions' detail brought back, 0..1.
    pub recovery: f32,
    /// `true`: Add, `false`: Screen.
    pub add: bool,
}

impl Params {
    /// The parameters of `g`, or `None` while Glow is off or its sliders hold non-numbers.
    pub fn of(g: &Glow) -> Option<Params> {
        let f = |v: f64| (v / 100.0) as f32;
        let amount = f(g.amount);
        if !(amount > 0.0) {
            return None;
        }
        let (t, s, gain, gamma, ratio, recovery) =
            (f(g.threshold), f(g.spread), f(g.gain), f(g.gamma), f(g.ratio), f(g.recovery));
        if ![t, s, gain, gamma, ratio, recovery].iter().all(|v| v.is_finite()) {
            return None;
        }
        let p = Params {
            amount: amount.min(1.0),
            // a hair under 1, so at 100 the knee still has room and nothing glows
            threshold: (t * 0.98).clamp(0.0, 0.98),
            knee: ((1.0 - t) * 0.4).clamp(0.02, 0.15),
            // 0.25 % of the long edge at 0 to 10 % at 100 (exponential, like every blur control)
            sigma: 0.0025 * 40f32.powf(s.clamp(0.0, 1.0)),
            // ×0.25 .. ×4, ×1 in the middle; falloff 2 .. 0.5, 1 in the middle
            gain: 2f32.powf((gain.clamp(0.0, 1.0) * 2.0 - 1.0) * 2.0),
            gamma: 2f32.powf((0.5 - gamma.clamp(0.0, 1.0)) * 2.0),
            // 0.25 .. 4, 1 in the middle: the halo narrows one way and widens the other
            ratio: 2f32.powf((ratio.clamp(0.0, 1.0) - 0.5) * 4.0),
            recovery: recovery.clamp(0.0, 1.0),
            add: g.mode == GlowComposite::Add,
        };
        (p.sigma.is_finite() && p.sigma > 0.0).then_some(p)
    }
}

/// Whether a render's Glow is on: the GPU renderer has no kernel for it yet and declines, and a
/// window grows by its halo's reach ([`reach`]).
pub fn needs_cpu(s: &DevelopSettings) -> bool {
    s.section_enabled("effects") && Params::of(&s.glow).is_some()
}

/// How far (px) a render window must grow so the halos that reach into it from outside are drawn:
/// three sigma of the halo's widest axis. 0 while Glow is off.
pub fn reach(s: &DevelopSettings, px_per_long: f64) -> f64 {
    let spread = || {
        let Some(p) = Params::of(&s.glow) else { return 0.0 };
        3.0 * p.sigma as f64 * px_per_long * p.ratio.sqrt().max(1.0 / p.ratio.sqrt()) as f64
    };
    if needs_cpu(s) { spread() } else { 0.0 }
}

/// `f` over the channels of `a` and `b` (arrays have `map`, not `zip`).
fn zip3(a: [f32; 3], b: [f32; 3], f: impl Fn(f32, f32) -> f32) -> [f32; 3] {
    [f(a[0], b[0]), f(a[1], b[1]), f(a[2], b[2])]
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0).max(1e-6)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The shine weight of an encoded colour: how far above the threshold its luminance sits.
#[inline]
fn shine(p: &Params, c: [f32; 3]) -> f32 {
    let y = LUMA[0] * c[0] + LUMA[1] * c[1] + LUMA[2] * c[2];
    if y.is_finite() { smoothstep(p.threshold, p.threshold + p.knee, y) } else { 0.0 }
}

/// The halo, computed on a proxy of `img` (`w × h`): the shine-extracted colours, downsampled,
/// blurred and shaped by Gain and Gamma, sampled back at any point of the window.
struct Halo {
    /// The proxy, and the window's size in its cells (the bilinear sample steps).
    proxy: Rgb32f,
    cw: f32,
    ch: f32,
}

impl Halo {
    fn new(img: &[[f32; 3]], w: usize, h: usize, p: &Params) -> Halo {
        let long = w.max(h) as f32;
        // a quarter of the window, kept between 384 and 1024 cells: a halo holds no finer detail,
        // and every size renders the same field (within the proxy's tolerance)
        let target = (long * 0.25).clamp(384.0, 1024.0);
        let k = long / target;
        // the H/V ratio lives here: resampling x by k·√ratio and y by k/√ratio, then sampling
        // back, spreads the halo √ratio times wider horizontally than vertically
        let r = p.ratio.sqrt();
        let (cw, ch) = (k * r, k / r);
        let pw = ((w as f32 / cw).round() as usize).clamp(1, w.max(1));
        let ph = ((h as f32 / ch).round() as usize).clamp(1, h.max(1));
        let mut proxy = Rgb32f { width: pw, height: ph, data: vec![[0.0; 3]; pw * ph] };
        // box-average the shine-weighted colours into the proxy's cells (parallel over its rows)
        par_rows(&mut proxy.data, pw, |py, rows| {
            let y0 = (py as f32 * ch) as usize;
            let y1 = (((py + 1) as f32 * ch).ceil() as usize).clamp(y0 + 1, h);
            for (px, o) in rows.iter_mut().enumerate() {
                let x0 = (px as f32 * cw) as usize;
                let x1 = (((px + 1) as f32 * cw).ceil() as usize).clamp(x0 + 1, w);
                let mut acc = [0.0f32; 3];
                for y in y0..y1 {
                    let row = &img[y * w..y * w + w];
                    for x in x0..x1 {
                        let c = row[x];
                        if !c.iter().all(|v| v.is_finite()) {
                            continue;
                        }
                        let s = shine(p, c);
                        if s > 0.0 {
                            for k in 0..3 {
                                acc[k] += c[k] * s;
                            }
                        }
                    }
                }
                let n = (y1 - y0) * (x1 - x0);
                *o = acc.map(|v| v / n.max(1) as f32);
            }
        });
        // the blur's sigma keeps the halo's width relative to the long edge (the ratio came out of
        // the resampling), then Gain brightens and Gamma reshapes the glow's falloff
        let blurred = gaussian(&proxy, p.sigma * pw.max(ph) as f32);
        let (gain, gamma) = (p.gain, p.gamma);
        let mut proxy = blurred;
        for c in &mut proxy.data {
            *c = c.map(|v| (v.max(0.0) * gain).powf(gamma));
        }
        Halo { proxy, cw, ch }
    }

    /// The halo at window pixel `(x, y)`, bilinearly sampled from the proxy (clamped at the
    /// edges, as the blur itself is).
    fn at(&self, x: usize, y: usize) -> [f32; 3] {
        let (pw, ph) = (self.proxy.width as f32, self.proxy.height as f32);
        let fx = ((x as f32 + 0.5) / self.cw - 0.5).clamp(0.0, pw - 1.0);
        let fy = ((y as f32 + 0.5) / self.ch - 0.5).clamp(0.0, ph - 1.0);
        let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(self.proxy.width - 1), (y0 + 1).min(self.proxy.height - 1));
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        let p = |x: usize, y: usize| self.proxy.data[y * self.proxy.width + x];
        let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
        let top = zip3(p(x0, y0), p(x1, y0), |a, b| lerp(a, b, tx));
        let bot = zip3(p(x0, y1), p(x1, y1), |a, b| lerp(a, b, tx));
        zip3(top, bot, |a, b| lerp(a, b, ty))
    }
}

/// Glow over the finished, output-encoded window (`img`, `w × h`), in place.
pub fn apply(img: &mut [[f32; 3]], w: usize, h: usize, p: &Params) {
    if w == 0 || h == 0 || img.len() < w * h {
        return;
    }
    let halo = Halo::new(img, w, h, p);
    par_rows(img, w, |y, row| {
        for x in 0..w {
            let c = row[x];
            if !c.iter().all(|v| v.is_finite()) {
                continue;
            }
            let g = halo.at(x, y);
            if !g.iter().all(|v| v.is_finite()) {
                continue;
            }
            // Screen (the default, the gentler one) or Add, Resolve's two composite modes; both
            // extend past 1 the same way, so an HDR render keeps its headroom
            let k = p.amount;
            let mut o = if p.add {
                zip3(c, g, |v, gl| v + k * gl)
            } else {
                zip3(c, g, |v, gl| v + (1.0 - v) * (k * gl))
            };
            // Bright Region Recovery: the bright regions' own detail (what the blur smeared) comes
            // back through the glow, weighted by how bright the pixel itself is
            if p.recovery > 0.0 {
                let s = shine(p, c);
                if s > 0.0 {
                    o = zip3(o, zip3(c, g, |v, gl| (v - gl).max(0.0)), |v, d| v + p.recovery * s * d);
                }
            }
            if o.iter().all(|v| v.is_finite()) {
                row[x] = o;
            }
        }
    });
}
