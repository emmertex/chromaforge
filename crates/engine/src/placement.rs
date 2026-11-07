//! Where a picture of a photo lies in the photo, and the look it was made with: what lets any
//! picture on hand (a render made before a crop, the browse-time preview, a grid thumbnail) stand in
//! for the photo's current crop at once, as a texture transform, with no render (a crop is a
//! crop: the picture before it, a little zoomed in and cut), and that keeps a picture with another look (the as-shot preview of a photo imported
//! with a preset) from standing in at all.
//!
//! A picture's [`Placement`] maps its texture coordinates (0..1 across and down) onto the
//! normalized coordinates of the photo's transformed image (oriented, lens-corrected, Upright: what
//! crops, masks and spots are in, [`lcc_pipeline::Frame`]). Two pictures made with the same
//! warp share those coordinates, so the corners of the current view on a stand-in follow from the
//! two placements ([`stand_in_uv`]): a crop, a straighten angle, a flip and an aspect are all just
//! where the four corners land.

use lcc_catalog::Photo;
use lcc_develop::DevelopSettings;
use lcc_geom::{Affine, Point};
use lcc_pipeline::geometry::Frame;
use serde::{Deserialize, Serialize};

/// Where a picture lies in its photo and the look it shows ([`Placement::of`]).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Placement {
    /// Texture coordinates of the picture → normalized coordinates of the transformed image.
    pub uv_to_norm: [f64; 6],
    /// The warp the transformed image is of (orientation, lens corrections, perspective, Upright):
    /// only pictures made with the same one map onto each other.
    pub warp: u64,
    pub look: Look,
    /// The picture's shape (width / height): a picture of another shape (an unsupported raw's
    /// camera JPEG developed in its place) is not where this says.
    pub aspect: f32,
}

/// The look a picture was made with, as far as standing in goes ([`Look::close_to`]).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Look {
    /// Everything but the basic sliders below, the crop and the geometry: profile, treatment,
    /// curves, colour grading, effects, masks, Smooth Skin…
    pub family: u64,
    /// Exposure, contrast, highlights, shadows, whites, blacks.
    pub light: [f32; 6],
    /// White balance (mode as a number, temperature, tint).
    pub wb: [f32; 3],
    /// Vibrance, saturation.
    pub color: [f32; 2],
    /// Made with the camera's defaults, crop and geometry aside (the browse-time preview, the
    /// camera JPEG): it never stands in for an edit, nor an edit for it.
    #[serde(default)]
    pub as_shot: bool,
}

impl Look {
    pub fn of(d: &DevelopSettings) -> Look {
        let l = &d.light;
        let f = |v: f64| if v.is_finite() { v as f32 } else { 0.0 };
        let mode = match d.wb.mode {
            lcc_develop::WbMode::AsShot => 0.0,
            _ => 1.0,
        };
        Look {
            family: family(d),
            light: [f(l.exposure), f(l.contrast), f(l.highlights), f(l.shadows), f(l.whites), f(l.blacks)],
            wb: [mode, f(d.wb.temp), f(d.wb.tint)],
            color: [f(d.color.vibrance), f(d.color.saturation)],
            as_shot: false,
        }
    }

    /// Whether a picture with this look may stand in for one with `want`: the same profile, preset
    /// look, masks and retouching, and only small differences in the basic sliders (the photo as
    /// last seen while one slider was moved a little; never the as-shot preview of a photo that has
    /// a preset's look since). A crop, an angle or a flip is not a look: the picture is cut to it.
    pub fn close_to(&self, want: &Look) -> bool {
        if self.family != want.family || self.wb[0] != want.wb[0] || self.as_shot != want.as_shot {
            return false;
        }
        let d = |a: f32, b: f32| (a - b).abs();
        // (a slider dragged or synced: the picture before stands in better than none; a far one,
        // 2 EV and more, does not)
        let light = d(self.light[0], want.light[0]) < 2.0 && (1..6).all(|i| d(self.light[i], want.light[i]) <= 60.0);
        let wb = d(self.wb[1], want.wb[1]) <= 1500.0 && d(self.wb[2], want.wb[2]) <= 30.0;
        let color = (0..2).all(|i| d(self.color[i], want.color[i]) <= 60.0);
        light && wb && color
    }
}

/// FNV-1a of `bytes` into `h`.
fn fnv(mut h: u64, bytes: &[u8]) -> u64 {
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

const FNV0: u64 = 0xcbf2_9ce4_8422_2325;

/// The hash of `v` as JSON.
fn json_hash<T: Serialize>(h: u64, v: &T) -> u64 {
    fnv(h, serde_json::to_string(v).unwrap_or_default().as_bytes())
}

/// [`Look::family`]: the settings other than the basic sliders, crop and geometry. Masks count by
/// what they do (their kind and adjustments), not by their pixels: an AI mask computed after the
/// picture was made does not make it another look.
fn family(d: &DevelopSettings) -> u64 {
    let mut h = FNV0;
    h = json_hash(h, &d.profile);
    h = json_hash(h, &d.treatment);
    h = json_hash(h, &d.curve);
    h = json_hash(h, &d.mixer);
    h = json_hash(h, &d.point_colors);
    h = json_hash(h, &d.bw_mix);
    h = json_hash(h, &d.grading);
    h = json_hash(h, &d.effects);
    h = json_hash(h, &d.vignette);
    h = json_hash(h, &d.calibration);
    h = json_hash(h, &d.hdr);
    h = json_hash(h, &d.skin);
    h = json_hash(h, &d.retouch);
    h = json_hash(h, &d.disabled_sections);
    h = json_hash(h, &d.plugin_filters);
    h = json_hash(h, &d.lens_blur);
    h = fnv(h, &(d.masks.len() as u64).to_le_bytes());
    for m in &d.masks {
        h = json_hash(h, &(&m.adjust, &m.curve, &m.point_colors, &m.mixer, m.visible, m.invert, m.components.len()));
    }
    h
}

/// [`Placement::warp`]: what the transformed image depends on.
fn warp_key(d: &DevelopSettings) -> u64 {
    let mut h = FNV0;
    h = json_hash(h, &d.orientation);
    h = json_hash(h, &d.optics);
    let g = &d.geometry;
    h = json_hash(h, &(&g.upright, &g.upright_transform, g.vertical, g.horizontal, g.rotate, g.aspect, g.scale, g.offset_x, g.offset_y));
    h
}

impl Placement {
    /// The placement of a render of `p` with `settings` (`apply_crop`: cropped, as the loupe and
    /// the grid show it; else the whole photo, as the crop tool does).
    pub fn of(p: &Photo, settings: &DevelopSettings, apply_crop: bool) -> Placement {
        let frame = Frame::with_lens(p.width.max(1) as usize, p.height.max(1) as usize, settings, apply_crop, p.embedded_lens.as_deref());
        let aspect = frame.aspect() as f32;
        let neutral = |d: &DevelopSettings| {
            let mut d = d.clone();
            d.crop = Default::default();
            d.geometry = Default::default();
            d.profile.camera.clear();
            d.profile.id = lcc_develop::Profile::default().id;
            d
        };
        let look = Look { as_shot: neutral(settings) == neutral(&p.camera_defaults()), ..Look::of(settings) };
        Placement {
            uv_to_norm: frame.out_to_norm(1, 1).0,
            warp: warp_key(settings),
            look,
            aspect: if aspect.is_finite() && aspect > 0.0 { aspect } else { 1.0 },
        }
    }

    /// The placement of `p`'s camera JPEG (the whole frame as shot, before any edit).
    pub fn camera_jpeg(p: &Photo) -> Placement {
        Placement::of(p, &p.camera_defaults(), false)
    }
}

/// Where the corners of `want`'s picture (top-left, top-right, bottom-right, bottom-left) lie on a
/// picture placed as `have`, in its texture coordinates: drawing `have`'s texture with these
/// coordinates on the view's rectangle shows `want`'s crop, angle and flips. `None` when the two
/// can't be mapped (another warp, a degenerate placement).
pub fn stand_in_uv(have: &Placement, want: &Placement) -> Option<[[f64; 2]; 4]> {
    if have.warp != want.warp {
        return None;
    }
    let to_have = Affine(have.uv_to_norm).inverse()?;
    let m = to_have * Affine(want.uv_to_norm);
    let uv = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)].map(|(x, y)| {
        let q = m.apply(Point::new(x, y));
        [q.x, q.y]
    });
    uv.iter().flatten().all(|v| v.is_finite()).then_some(uv)
}

/// Whether a picture `w × h` pixels has the shape `p` says (within 2 %).
pub fn fits_shape(p: &Placement, w: usize, h: usize) -> bool {
    if w == 0 || h == 0 {
        return false;
    }
    let a = w as f32 / h as f32;
    (a / p.aspect - 1.0).abs() <= 0.02
}

/// Whether a picture placed as `have` may stand in for the view `want` asks for: the same warp and
/// a look close to it ([`Look::close_to`]), and every pixel of the view on it (within `slack`).
pub fn may_stand_in(have: &Placement, want: &Placement, slack: f64) -> bool {
    have.look.close_to(&want.look) && stand_in_uv(have, want).is_some_and(|uv| covers(&uv, slack))
}

/// Whether every corner of `uv` ([`stand_in_uv`]) is on the picture (with `slack` in texture
/// units): the view needs no pixel the stand-in doesn't have.
pub fn covers(uv: &[[f64; 2]; 4], slack: f64) -> bool {
    uv.iter().flatten().all(|v| (-slack..=1.0 + slack).contains(v))
}

/// Whether `uv` is the identity (the stand-in is the view as it is, to within `eps`).
pub fn is_identity(uv: &[[f64; 2]; 4], eps: f64) -> bool {
    let id = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
    uv.iter().flatten().zip(id.iter().flatten()).all(|(a, b)| (a - b).abs() <= eps)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lcc_catalog::{PhotoId, Source};

    fn photo() -> Photo {
        Photo::new(PhotoId(1), Source::File { path: "a.cr3".into() }, "a.cr3", "CR3", 6000, 4000, "2026-10-26")
    }

    fn near(a: [f64; 2], b: [f64; 2]) -> bool {
        (a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9
    }

    #[test]
    fn a_crop_is_the_full_frame_picture_cut_to_it() {
        let p = photo();
        let full = Placement::of(&p, &p.develop, true);
        let mut d = (*p.develop).clone();
        d.crop.geometry.rect = lcc_geom::Rect { x0: 0.2, y0: 0.1, x1: 0.8, y1: 0.7 };
        let cropped = Placement::of(&p, &d, true);
        let uv = stand_in_uv(&full, &cropped).unwrap();
        assert!(near(uv[0], [0.2, 0.1]) && near(uv[2], [0.8, 0.7]), "{uv:?}");
        assert!(covers(&uv, 1e-9));
        // the other way the view needs pixels the cropped picture doesn't have
        assert!(!covers(&stand_in_uv(&cropped, &full).unwrap(), 1e-3));
        assert!(is_identity(&stand_in_uv(&full, &full).unwrap(), 1e-9));
    }

    #[test]
    fn an_angle_and_a_flip_turn_and_mirror_the_corners() {
        let p = photo();
        let full = Placement::of(&p, &p.develop, false);
        let mut d = (*p.develop).clone();
        d.crop.geometry.rect = lcc_geom::Rect { x0: 0.25, y0: 0.25, x1: 0.75, y1: 0.75 };
        d.crop.geometry.angle = 3.0;
        let turned = stand_in_uv(&full, &Placement::of(&p, &d, true)).unwrap();
        // turned: the top edge is no longer level on the picture
        assert!((turned[0][1] - turned[1][1]).abs() > 1e-3, "{turned:?}");
        assert!(covers(&turned, 0.0));
        d.crop.geometry.angle = 0.0;
        d.crop.flip_h = true;
        let flipped = stand_in_uv(&full, &Placement::of(&p, &d, true)).unwrap();
        assert!(near(flipped[0], [0.75, 0.25]) && near(flipped[1], [0.25, 0.25]), "{flipped:?}");
    }

    #[test]
    fn another_look_or_warp_never_stands_in() {
        let p = photo();
        let shot = Placement::of(&p, &p.develop, true);
        let mut d = (*p.develop).clone();
        d.light.exposure = 0.2;
        // (the as-shot picture never stands in for an edit, however small)
        assert!(!shot.look.close_to(&Placement::of(&p, &d, true).look), "as shot vs an edit");
        let mut more = d.clone();
        more.light.exposure = 1.1;
        assert!(Placement::of(&p, &d, true).look.close_to(&Placement::of(&p, &more, true).look), "a slider moved");
        let mut cropped = (*p.develop).clone();
        cropped.crop.geometry.angle = 2.0;
        assert!(shot.look.close_to(&Placement::of(&p, &cropped, true).look), "a crop is not a look");
        let mut preset = (*p.develop).clone();
        preset.profile.id = "lc.portrait".into();
        preset.light.exposure = 0.2;
        assert!(!shot.look.close_to(&Placement::of(&p, &preset, true).look), "a preset's look is another look");
        let mut d2 = preset.clone();
        d2.light.exposure = 0.4;
        assert!(Placement::of(&p, &preset, true).look.close_to(&Placement::of(&p, &d2, true).look));
        d2.light.exposure = 2.5;
        assert!(!Placement::of(&p, &preset, true).look.close_to(&Placement::of(&p, &d2, true).look));
        let mut upright = preset.clone();
        upright.geometry.vertical = 20.0;
        assert!(stand_in_uv(&Placement::of(&p, &preset, true), &Placement::of(&p, &upright, true)).is_none());
    }
}
