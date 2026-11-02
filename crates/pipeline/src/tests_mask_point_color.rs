//! Pixel tests of a mask's local Point Color (`Mask::point_colors`).

use lcc_develop::{DevelopSettings, Mask, MaskComponent, MaskOp, MaskShape, PointColor, Treatment};
use lcc_geom::Point;
use lcc_raster::{Rgb32f, Rgba8};

use crate::{RenderRequest, SourceInfo, render};

const W: usize = 160;
const H: usize = 80;

/// Orange-ish colours of varying lightness, the same in every column.
fn scene() -> Rgb32f {
    Rgb32f::from_fn(W, H, |_, y| {
        let v = 0.05 + 0.6 * y as f32 / H as f32;
        [v, v * 0.55, v * 0.2]
    })
}

fn sample() -> PointColor {
    PointColor { lum: 0.6, chroma: 0.1, hue: 55.0, hue_shift: 60.0, sat_shift: -50.0, lum_shift: 30.0, range: 100.0, ..Default::default() }
}

fn masked(shape: MaskShape, point_colors: Vec<PointColor>) -> DevelopSettings {
    let mut s = DevelopSettings::default();
    s.masks.push(Mask {
        id: 1,
        components: vec![MaskComponent { anchor: None, name: None, op: MaskOp::Add, invert: false, shape }],
        point_colors,
        ..Default::default()
    });
    s
}

/// The whole frame.
fn all() -> MaskShape {
    MaskShape::Radial { center: Point::new(0.5, 0.5), rx: 4.0, ry: 4.0, angle: 0.0, feather: 0.0, invert: false }
}

/// A hard mask over the left part (x < 0.45).
fn left() -> MaskShape {
    MaskShape::Linear { start: Point::new(0.45, 0.5), end: Point::new(0.46, 0.5) }
}

fn shot(src: &Rgb32f, s: &DevelopSettings) -> Rgba8 {
    render(src, &SourceInfo::default(), s, &RenderRequest::fit(W, H)).image
}

fn max_diff(a: &Rgba8, b: &Rgba8, cols: std::ops::Range<usize>) -> i32 {
    let mut m = 0;
    for y in 0..H {
        for x in cols.clone() {
            let (p, q) = (a.data[y * W + x], b.data[y * W + x]);
            for k in 0..3 {
                m = m.max((p[k] as i32 - q[k] as i32).abs());
            }
        }
    }
    m
}

#[test]
fn under_a_full_mask_local_point_color_renders_as_the_global_one() {
    let src = scene();
    let local = shot(&src, &masked(all(), vec![sample()]));
    let global = shot(&src, &DevelopSettings { point_colors: vec![sample()], ..masked(all(), Vec::new()) });
    let plain = shot(&src, &masked(all(), Vec::new()));
    assert!(max_diff(&plain, &global, 0..W) > 10, "the sample changes the photo");
    assert!(max_diff(&local, &global, 0..W) <= 1, "local at full opacity = global");
}

#[test]
fn local_point_color_stays_inside_its_mask() {
    let src = scene();
    let plain = shot(&src, &DevelopSettings::default());
    let local = shot(&src, &masked(left(), vec![sample()]));
    assert!(max_diff(&plain, &local, 0..W * 2 / 5) > 10, "inside the mask the colours change");
    assert_eq!(max_diff(&plain, &local, W * 3 / 5..W), 0, "outside the mask nothing changes");
}

#[test]
fn black_and_white_ignores_local_point_color_and_the_list_is_capped() {
    let src = scene();
    let mut bw = masked(all(), vec![sample()]);
    bw.treatment = Treatment::Bw;
    let plain = DevelopSettings { treatment: Treatment::Bw, ..masked(all(), Vec::new()) };
    assert_eq!(shot(&src, &bw).data, shot(&src, &plain).data);
    // more samples than allowed: the stored list is cut on load, never a panic
    let mut many = masked(all(), vec![sample(); lcc_develop::MAX_POINT_COLORS + 5]);
    assert!(many.over_list_limits());
    many.cap_lists();
    assert_eq!(many.masks[0].point_colors.len(), lcc_develop::MAX_POINT_COLORS);
    let json = serde_json::to_string(&masked(all(), vec![sample(); lcc_develop::MAX_POINT_COLORS + 5])).unwrap();
    let back: DevelopSettings = serde_json::from_str(&json).unwrap();
    assert_eq!(back.masks[0].point_colors.len(), lcc_develop::MAX_POINT_COLORS);
}

// --- a mask's local Color Mixer (`Mask::mixer`)

fn mixer() -> lcc_develop::Mixer {
    use lcc_develop::Hsl;
    lcc_develop::Mixer { orange: Hsl { hue: 60.0, sat: -60.0, lum: 40.0 }, red: Hsl { hue: -30.0, sat: 50.0, lum: 0.0 }, ..Default::default() }
}

fn mixed(shape: MaskShape) -> DevelopSettings {
    let mut s = masked(shape, Vec::new());
    s.masks[0].mixer = mixer();
    s
}

#[test]
fn under_a_full_mask_the_local_mixer_renders_as_the_global_one() {
    let src = scene();
    let local = shot(&src, &mixed(all()));
    let global = shot(&src, &DevelopSettings { mixer: mixer(), ..masked(all(), Vec::new()) });
    let plain = shot(&src, &masked(all(), Vec::new()));
    assert!(max_diff(&plain, &global, 0..W) > 10, "the mixer changes the photo");
    assert!(max_diff(&local, &global, 0..W) <= 1, "local at full opacity = global");
}

#[test]
fn the_local_mixer_stays_inside_its_mask_and_neutral_changes_nothing() {
    let src = scene();
    let plain = shot(&src, &DevelopSettings::default());
    let local = shot(&src, &mixed(left()));
    assert!(max_diff(&plain, &local, 0..W * 2 / 5) > 10, "inside the mask the colours change");
    assert_eq!(max_diff(&plain, &local, W * 3 / 5..W), 0, "outside the mask nothing changes");
    // a neutral mixer is no local stage, and isn't written
    let s = masked(left(), Vec::new());
    assert!(crate::colorops::mask_mixer_ks(&s).is_empty());
    assert!(serde_json::to_value(&s.masks[0]).unwrap().get("mixer").is_none(), "the mask's neutral mixer is left out");
    // a hostile value renders, never NaN
    let mut bad = mixed(left());
    bad.masks[0].mixer.red.hue = f64::NAN;
    let _ = shot(&src, &bad);
}
