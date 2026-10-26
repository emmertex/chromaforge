//! Geometry / optics tests on synthetic images with known distortions.

use lcc_develop::{DevelopSettings, EmbeddedLens, EmbeddedVignette, EmbeddedWarp};
use lcc_geom::Point;
use lcc_raster::resample::{Filter, resize};
use lcc_raster::{Rgb32f, Rgba8};

use crate::{RenderRequest, SourceInfo, render};

/// A light grid: dark lines every `step` px (soft, ~3 px wide) on a light background.
pub(crate) fn grid_value(p: Point, step: f64) -> f32 {
    let d = |v: f64| {
        let m = v.rem_euclid(step);
        m.min(step - m)
    };
    let dist = d(p.x).min(d(p.y));
    (0.05 + 0.75 * ((dist - 1.0) / 1.5).clamp(0.0, 1.0)) as f32
}

/// Render a source of `w × h` where each pixel shows `pattern` at `undistort(pixel centre)`.
pub(crate) fn synth(w: usize, h: usize, undistort: impl Fn(Point) -> Point + Sync, pattern: impl Fn(Point) -> [f32; 3] + Sync) -> Rgb32f {
    Rgb32f::from_fn(w, h, |x, y| pattern(undistort(Point::new(x as f64 + 0.5, y as f64 + 0.5))))
}

#[test]
fn profile_corrections_use_embedded_lens_only_when_enabled() {
    let (w, h) = (300usize, 200usize);
    let src = synth(w, h, |p| p, |p| [grid_value(p, 30.0); 3]);
    let lens = EmbeddedLens {
        warp: Some(EmbeddedWarp { planes: [[1.0, -0.08, 0.0, 0.0, 0.0, 0.0]; 3], center: Point::new(0.5, 0.5), radius: 0.6 }),
        vignette: Some(EmbeddedVignette { k: [0.5, 0.0, 0.0, 0.0, 0.0], center: Point::new(0.5, 0.5), radius: 0.6 }),
        ..Default::default()
    };
    let info = SourceInfo { lens: Some(lens), ..SourceInfo::default() };
    let mut s = DevelopSettings::default();
    let off = render(&src, &info, &s, &RenderRequest::fit(w, h)).image;
    let plain = render(&src, &SourceInfo::default(), &s, &RenderRequest::fit(w, h)).image;
    assert_eq!(off.data, plain.data, "profile corrections off: embedded data ignored");
    s.optics.lens_profile = true;
    let on = render(&src, &info, &s, &RenderRequest::fit(w, h)).image;
    assert_ne!(on.data, off.data);
    // corners brightened by the embedded vignette correction
    assert!(on.get(2, 2)[1] as i32 >= off.get(2, 2)[1] as i32);
    s.optics.profile_distortion = 0.0;
    s.optics.profile_vignetting = 0.0;
    let zero = render(&src, &info, &s, &RenderRequest::fit(w, h)).image;
    assert_eq!(zero.data, off.data, "0 % = no correction");
}

fn mean_abs_diff_downscaled(small: &Rgba8, big: &Rgba8) -> f32 {
    let down = resize(&big.to_linear(), small.width, small.height, Filter::Box).to_srgb8();
    small.data.iter().zip(&down.data).map(|(a, b)| (0..3).map(|i| (a[i] as f32 - b[i] as f32).abs()).sum::<f32>() / 3.0).sum::<f32>()
        / small.len() as f32
}

#[test]
fn optics_preview_matches_export() {
    let src = lcc_scenes::demo_library()[2].render(960, 640);
    let mut s = DevelopSettings::default();
    s.optics.distortion = 40.0;
    s.optics.vignetting = 60.0;
    s.optics.vignetting_midpoint = 30.0;
    s.optics.ca_red = 30.0;
    s.optics.defringe_purple_amount = 8.0;
    let small = render(&src, &SourceInfo::default(), &s, &RenderRequest::fit(240, 240)).image;
    let big = render(&src, &SourceInfo::default(), &s, &RenderRequest::fit(960, 960)).image;
    let err = mean_abs_diff_downscaled(&small, &big);
    assert!(err < 4.0, "mean abs error {err}");
}

// ------------------------------------------------------------------------------------------ M4.3 transforms

/// Source whose content is a straight grid after the forward transform `h` (centred coordinates of a `w × h`
/// image): each source pixel shows the grid at its transformed position.
fn keystoned_grid(w: usize, h: usize, fwd: lcc_geom::Homography, step: f64) -> Rgb32f {
    let l = w.max(h) as f64 / 2.0;
    let (cx, cy) = (w as f64 / 2.0, h as f64 / 2.0);
    synth(
        w,
        h,
        move |p| {
            let q = fwd.apply(Point::new((p.x - cx) / l, (p.y - cy) / l));
            Point::new(cx + q.x * l, cy + q.y * l)
        },
        move |p| [grid_value(p, step); 3],
    )
}

fn blank_fraction(img: &Rgba8) -> f64 {
    let blank =
        crate::render(&Rgb32f::filled(4, 4, crate::geometry::BLANK), &SourceInfo::default(), &DevelopSettings::default(), &RenderRequest::fit(4, 4))
            .image
            .get(1, 1);
    img.data.iter().filter(|p| (0..3).all(|i| (p[i] as i32 - blank[i] as i32).abs() <= 1)).count() as f64 / img.len() as f64
}

#[test]
fn constrain_crop_removes_blank_areas() {
    let src = Rgb32f::from_fn(300, 200, |x, y| [0.05 + 0.002 * x as f32, 0.1 + 0.002 * y as f32, 0.3]);
    let mut s = DevelopSettings::default();
    s.geometry.vertical = -60.0;
    s.geometry.rotate = 4.0;
    let free = render(&src, &SourceInfo::default(), &s, &RenderRequest::fit(300, 300)).image;
    assert!(blank_fraction(&free) > 0.03, "keystone leaves blank corners: {}", blank_fraction(&free));
    s.geometry.constrain_crop = true;
    let c = render(&src, &SourceInfo::default(), &s, &RenderRequest::fit(300, 300)).image;
    assert_eq!((c.width, c.height), (free.width, free.height), "constrain keeps the aspect");
    assert!(blank_fraction(&c) < 0.002, "constrained: {}", blank_fraction(&c));
}

/// Issue #245: lens corrections never show the blank past the photo's edge, with or without
/// Constrain Crop, in every colour plane; perspective alone still needs Constrain Crop.
#[test]
fn lens_corrections_leave_no_blank_corners() {
    let src = Rgb32f::from_fn(300, 200, |x, y| [0.05 + 0.002 * x as f32, 0.1 + 0.002 * y as f32, 0.3]);
    for (distortion, ca) in [(-80.0, 0.0), (80.0, 0.0), (-40.0, 100.0), (0.0, -100.0)] {
        let mut s = DevelopSettings::default();
        s.optics.distortion = distortion;
        s.optics.ca_red = ca;
        s.optics.ca_blue = -ca;
        let r = render(&src, &SourceInfo::default(), &s, &RenderRequest::fit(300, 300)).image;
        assert!(blank_fraction(&r) < 0.002, "distortion {distortion}, CA {ca}: {}", blank_fraction(&r));
    }
    let mut s = DevelopSettings::default();
    s.optics.distortion = -60.0;
    s.geometry.vertical = -60.0;
    let r = render(&src, &SourceInfo::default(), &s, &RenderRequest::fit(300, 300)).image;
    assert!(blank_fraction(&r) > 0.03, "the keystone keeps its corners: {}", blank_fraction(&r));
}

/// Issue #211: the blank border a transform leaves is shown, but the histogram counts only the photo,
/// at every output size. A white photo stays white data.
#[test]
fn histogram_leaves_the_blank_border_out() {
    let src = Rgb32f::from_fn(300, 200, |x, y| [0.05 + 0.002 * x as f32, 0.1 + 0.002 * y as f32, 0.3]);
    let mut s = DevelopSettings::default();
    let plain = render(&src, &SourceInfo::default(), &s, &RenderRequest::fit(300, 300));
    assert!(plain.coverage.is_none(), "no transform: every pixel is the photo");
    assert_eq!(plain.histogram.total as usize, plain.image.len());
    s.geometry.vertical = -60.0;
    s.geometry.rotate = 4.0;
    for edge in [120, 300, 640] {
        let r = render(&src, &SourceInfo::default(), &s, &RenderRequest::fit(edge, edge));
        let blank = blank_fraction(&r.image);
        assert!(blank > 0.03, "{edge}: the keystone leaves blank corners ({blank})");
        let cov = r.coverage.as_deref().expect("a blank border");
        assert_eq!((cov.width(), cov.height()), (r.image.width, r.image.height));
        let counted = r.histogram.total as f64 / r.image.len() as f64;
        assert!((counted - (1.0 - blank)).abs() < 0.02, "{edge}: counted {counted}, blank {blank}");
    }
    let white = Rgb32f::filled(300, 200, [4.0; 3]);
    let r = render(&white, &SourceInfo::default(), &s, &RenderRequest::fit(300, 300));
    assert!((r.histogram.total as usize) < r.image.len(), "the border is left out");
    assert_eq!(r.histogram.luma[255], r.histogram.total, "and the photo's white is all there");
    let hdr = render(&src, &SourceInfo::default(), &s, &RenderRequest { depth: crate::OutputDepth::F32Hdr, ..RenderRequest::fit(300, 300) });
    assert!(hdr.coverage.is_some() && (hdr.histogram.total as usize) < hdr.image.len(), "high-bit-depth renders too");
}

#[test]
fn transforms_preview_matches_export() {
    let src = lcc_scenes::demo_library()[4].render(960, 640);
    let mut s = DevelopSettings::default();
    s.geometry.vertical = -30.0;
    s.geometry.horizontal = 15.0;
    s.geometry.rotate = 2.5;
    s.geometry.scale = 105.0;
    s.geometry.constrain_crop = true;
    let small = render(&src, &SourceInfo::default(), &s, &RenderRequest::fit(240, 240)).image;
    let big = render(&src, &SourceInfo::default(), &s, &RenderRequest::fit(960, 960)).image;
    assert_eq!(small.width * 4, big.width);
    let err = mean_abs_diff_downscaled(&small, &big);
    assert!(err < 4.0, "mean abs error {err}");
}

// ------------------------------------------------------------------------------------------ M4.4 Upright

use lcc_develop::Upright;

#[test]
fn upright_analysis_is_resolution_independent() {
    let shot = lcc_develop::Geometry { vertical: 30.0, rotate: 1.5, ..Default::default() };
    let fwd = crate::transform::manual(&shot);
    let big = keystoned_grid(1200, 800, fwd, 80.0);
    let small = keystoned_grid(600, 400, fwd, 40.0);
    let s = DevelopSettings::default();
    let a = crate::upright::auto_transform(&big, &SourceInfo::default(), &s, Upright::Vertical);
    let b = crate::upright::auto_transform(&small, &SourceInfo::default(), &s, Upright::Vertical);
    for p in [Point::new(-1.0, -0.66), Point::new(1.0, -0.66), Point::new(1.0, 0.66), Point::new(-1.0, 0.66)] {
        assert!(a.apply(p).dist(b.apply(p)) < 0.006, "{:?} vs {:?}", a.apply(p), b.apply(p));
    }
    // a setting that names the mode without a stored transform renders like the stored one
    let mut s1 = DevelopSettings::default();
    s1.geometry.upright = Upright::Vertical;
    let lazy = render(&small, &SourceInfo::default(), &s1, &RenderRequest::fit(300, 300)).image;
    s1.geometry.upright_transform = Some(b);
    let stored = render(&small, &SourceInfo::default(), &s1, &RenderRequest::fit(300, 300)).image;
    assert_eq!(lazy.data, stored.data);
}

/// Sign convention for Auto straighten: a horizon descending 6° to the right needs crop angle −6°,
/// which `level_degrees` must report (the crop panel uses it as the crop angle directly).
#[test]
fn level_degrees_sign_matches_crop_angle() {
    let (w, h) = (900usize, 600usize);
    let t = 6f32.to_radians().tan();
    let src = Rgb32f::from_fn(w, h, |x, y| {
        let yh = h as f32 / 2.0 + (x as f32 - w as f32 / 2.0) * t;
        if (y as f32) < yh {
            [0.6, 0.7, 0.9]
        } else if (((y as f32 - yh) / 30.0) as i32) % 2 == 0 {
            [0.05, 0.1, 0.2]
        } else {
            [0.1, 0.2, 0.3]
        }
    });
    let deg = crate::upright::level_degrees(&src, &SourceInfo::default(), &DevelopSettings::default()).expect("lines found");
    assert!((deg + 6.0).abs() < 0.5, "{deg}");
}

/// A "horizon" of `deg` degrees: sky above, below it bands of `band` px (0: one edge only).
fn tilted_scene(deg: f32, band: f32) -> Rgb32f {
    let (w, h) = (900usize, 600usize);
    let t = deg.to_radians().tan();
    Rgb32f::from_fn(w, h, |x, y| {
        let yh = h as f32 / 2.0 + (x as f32 - w as f32 / 2.0) * t;
        if (y as f32) < yh {
            [0.6, 0.7, 0.9]
        } else if band > 0.0 && (((y as f32 - yh) / band) as i32) % 2 == 1 {
            [0.1, 0.2, 0.3]
        } else {
            [0.05, 0.1, 0.2]
        }
    })
}

/// Auto straighten acts on several agreeing lines only: one edge (the frame of a window, a branch)
/// turned straight photos in a measurement on real ones, so it is left alone.
#[test]
fn auto_straighten_needs_more_than_one_line() {
    let level = |src: &Rgb32f| crate::upright::level_degrees(src, &SourceInfo::default(), &DevelopSettings::default());
    assert_eq!(level(&tilted_scene(4.0, 0.0)), None, "one edge");
    let deg = level(&tilted_scene(4.0, 30.0)).expect("a horizon and the bands below it");
    assert!((deg + 4.0).abs() < 0.5, "{deg}");
}

/// Auto straighten turns by at most `STRAIGHTEN_MAX_DEGREES`: larger "levels" came from texture
/// (planks, a bird's wing) on photos that were straight.
#[test]
fn auto_straighten_leaves_steep_lines_alone() {
    let level = |src: &Rgb32f| crate::upright::level_degrees(src, &SourceInfo::default(), &DevelopSettings::default());
    assert_eq!(level(&tilted_scene(20.0, 30.0)), None, "20° is not a horizon to level");
    let deg = level(&tilted_scene(8.0, 30.0)).expect("8° is");
    assert!((deg + 8.0).abs() < 0.5, "{deg}");
}

/// A ceremony aisle under a pavilion, shot from below and slightly rolled. Two thick white posts,
/// left and right, clipped by the frame's top and bottom, converge; a level hedge line; in the middle, people and
/// a dress whose edges are near-vertical but point nowhere in common; tree texture behind. Upright Vertical / Auto
/// did nothing on such a photo.
fn pavilion(w: usize, h: usize, fwd: lcc_geom::Homography) -> Rgb32f {
    let l = w.max(h) as f64 / 2.0;
    let (cx, cy) = (w as f64 / 2.0, h as f64 / 2.0);
    // cheap deterministic noise
    let noise = |x: f64, y: f64| {
        let n = ((x * 12.9898 + y * 78.233).sin() * 43758.5453).fract().abs();
        n as f32
    };
    synth(
        w,
        h,
        move |p| {
            let q = fwd.apply(Point::new((p.x - cx) / l, (p.y - cy) / l));
            Point::new(q.x, q.y)
        },
        move |q| {
            let (x, y) = (q.x, q.y);
            // posts: thick, bright, the whole height
            for px in [-0.78, 0.8] {
                if (x - px).abs() < 0.045 {
                    return [0.95; 3];
                }
            }
            // people in the middle: tilted shapes (a dress, suits), near-vertical edges in every direction
            let people: [(f64, f64, f64, f64, f32); 6] = [
                (-0.32, 0.25, 0.09, 9.0, 0.08),
                (-0.12, 0.2, 0.13, -11.0, 0.92),
                (0.05, 0.22, 0.08, 14.0, 0.1),
                (0.22, 0.28, 0.07, -7.0, 0.85),
                (0.38, 0.3, 0.06, 18.0, 0.15),
                (-0.5, 0.33, 0.05, -16.0, 0.12),
            ];
            for (px, py, half_w, tilt, v) in people {
                let (s, c) = tilt.to_radians().sin_cos();
                let (dx, dy) = (x - px, y - py);
                let (u, t) = (dx * c - dy * s, dx * s + dy * c);
                // a body from py − 0.35 (head) to py + 0.35, wider at the bottom (a dress)
                if t.abs() < 0.35 && u.abs() < half_w * (1.0 + 0.8 * (t + 0.35)) {
                    return [v; 3];
                }
            }
            if y < -0.12 {
                // sky (bright, but not as bright as the white posts)
                [0.62 - 0.05 * y.abs() as f32; 3]
            } else if y < 0.08 {
                // trees: dark, textured
                [0.18 + 0.12 * noise((x * 90.0).floor(), (y * 90.0).floor()); 3]
            } else {
                // lawn, below a level hedge line
                [0.42 + 0.06 * noise((x * 140.0).floor(), (y * 140.0).floor()); 3]
            }
        },
    )
}

fn upright_render(src: &Rgb32f, mode: Upright) -> Rgba8 {
    let mut s = DevelopSettings::default();
    s.geometry.upright = mode;
    s.geometry.upright_transform = Some(crate::upright::auto_transform(src, &SourceInfo::default(), &s, mode));
    render(src, &SourceInfo::default(), &s, &RenderRequest::fit(src.width, src.height)).image
}

#[test]
fn upright_corrects_converging_posts_among_people() {
    let (w, h) = (900usize, 600usize);
    let shot = lcc_develop::Geometry { vertical: 30.0, rotate: -1.5, ..Default::default() };
    let src = pavilion(w, h, crate::transform::manual(&shot));
    let a = crate::upright::analyze_source(&src, &SourceInfo::default(), &DevelopSettings::default());
    assert!(a.vertical_vp.is_some(), "the posts give a vertical vanishing point: {:?}", a.vertical_why);
    // the middle of the bright post in a quarter of the frame, on a row (centroid of the white pixels)
    let post_x = |img: &Rgba8, y: usize, right: bool| {
        let (a, b) = if right { (img.width * 3 / 4, img.width) } else { (0, img.width / 4) };
        let (mut s, mut n) = (0.0f64, 0.0f64);
        for x in a..b {
            if img.get(x, y)[1] > 225 {
                s += x as f64;
                n += 1.0;
            }
        }
        s / n.max(1.0)
    };
    let lean = |img: &Rgba8, right: bool| post_x(img, img.height / 6, right) - post_x(img, img.height * 5 / 6, right);
    let raw = render(&src, &SourceInfo::default(), &DevelopSettings::default(), &RenderRequest::fit(w, h)).image;
    assert!(lean(&raw, false).abs() > 8.0 && lean(&raw, true).abs() > 8.0, "the posts lean in the photo: {} {}", lean(&raw, false), lean(&raw, true));
    for mode in [Upright::Vertical, Upright::Auto] {
        let img = upright_render(&src, mode);
        for right in [false, true] {
            assert!(
                lean(&img, right).abs() < 2.5,
                "{mode:?}: the {} post still leans {} px",
                if right { "right" } else { "left" },
                lean(&img, right)
            );
        }
    }
}

/// A ceremony under a white gazebo, shot level with a long lens but rolled 1.6°
/// (portrait). Two white posts stand up the whole frame; the deck's boards run away to the horizon (they meet
/// inside the photo); two drapes hang diagonally from the roof; a couple stands between the posts. Upright
/// Vertical said "the straight lines point in different directions" and did nothing.
fn gazebo(w: usize, h: usize, fwd: lcc_geom::Homography) -> Rgb32f {
    let l = w.max(h) as f64 / 2.0;
    let (cx, cy) = (w as f64 / 2.0, h as f64 / 2.0);
    synth(
        w,
        h,
        move |p| fwd.apply(Point::new((p.x - cx) / l, (p.y - cy) / l)),
        move |q| {
            let (x, y) = (q.x, q.y);
            for px in [-0.47, 0.47] {
                if (x - px).abs() < 0.035 {
                    return [0.95; 3];
                }
            }
            // drapes from the roof's middle out and down, thin bright bands
            for (x0, x1) in [(-0.05, -0.3), (0.05, 0.3)] {
                let (y0, y1) = (-1.0, 0.25);
                if y > y0 && y < y1 {
                    let t = (y - y0) / (y1 - y0);
                    if (x - (x0 + (x1 - x0) * t)).abs() < 0.02 {
                        return [0.7; 3];
                    }
                }
            }
            // the couple: a dark suit and a white gown, a little tilted
            for (px, half_w, tilt, v) in [(-0.1, 0.07, 6.0f64, 0.85f32), (0.1, 0.06, -5.0, 0.08)] {
                let (s, c) = tilt.to_radians().sin_cos();
                let (dx, dy) = (x - px, y - 0.25);
                let (u, t) = (dx * c - dy * s, dx * s + dy * c);
                if t.abs() < 0.4 && u.abs() < half_w * (1.0 + 0.8 * (t + 0.4)) {
                    return [v; 3];
                }
            }
            if y > 0.3 {
                // deck boards running to a point on the horizon (inside the photo)
                let a = x.atan2(y + 0.1);
                if (a * 14.0).rem_euclid(1.0) < 0.12 {
                    return [0.15; 3];
                }
                return [0.55; 3];
            }
            // sky, a railing line
            if (y - 0.05).abs() < 0.01 { [0.3; 3] } else { [0.62; 3] }
        },
    )
}

#[test]
fn upright_vertical_follows_the_posts_not_the_deck_or_the_drapes() {
    let (w, h) = (600usize, 900usize);
    let shot = lcc_develop::Geometry { rotate: 1.6, ..Default::default() };
    let src = gazebo(w, h, crate::transform::manual(&shot));
    let a = crate::upright::analyze_source(&src, &SourceInfo::default(), &DevelopSettings::default());
    assert!(a.vertical_vp.is_some(), "the posts give a vertical vanishing point: {:?}", a.vertical_why);
    let post_x = |img: &Rgba8, y: usize, right: bool| {
        let (a, b) = if right { (img.width * 3 / 4, img.width) } else { (0, img.width / 4) };
        let (mut s, mut n) = (0.0f64, 0.0f64);
        for x in a..b {
            if img.get(x, y)[1] > 235 {
                s += x as f64;
                n += 1.0;
            }
        }
        s / n.max(1.0)
    };
    let lean = |img: &Rgba8, right: bool| post_x(img, img.height / 8, right) - post_x(img, img.height * 5 / 8, right);
    let raw = render(&src, &SourceInfo::default(), &DevelopSettings::default(), &RenderRequest::fit(w, h)).image;
    assert!(lean(&raw, false).abs() > 6.0 && lean(&raw, true).abs() > 6.0, "the posts lean: {} {}", lean(&raw, false), lean(&raw, true));
    for mode in [Upright::Vertical, Upright::Auto] {
        let img = upright_render(&src, mode);
        for right in [false, true] {
            assert!(
                lean(&img, right).abs() < 2.0,
                "{mode:?}: the {} post still leans {} px",
                if right { "right" } else { "left" },
                lean(&img, right)
            );
        }
    }
}

/// Do no harm: a gown's folds spreading to the floor point at a far point too, but they are short and there is
/// no post, wall or door among them: Upright Vertical leaves such a photo alone.
#[test]
fn upright_vertical_leaves_a_gown_without_structure_alone() {
    let (w, h) = (600usize, 900usize);
    let l = 450.0;
    let src = synth(
        w,
        h,
        |p| Point::new((p.x - 300.0) / l, (p.y - 450.0) / l),
        |q| {
            let (x, y) = (q.x, q.y);
            // the gown: from the waist (y = −0.1) to the floor (y = 0.9), widening
            if y > -0.1 && y < 0.9 && x.abs() < 0.08 + 0.45 * (y + 0.1) {
                // folds: soft, wavy dark lines that all aim at a point far below (y = 3)
                let a = x.atan2(3.0 - y) + 0.012 * (y * 23.0).sin();
                let band = (a * 40.0).rem_euclid(1.0) < 0.15;
                let piece = ((y * 9.0).floor() as i64 + (a * 40.0).floor() as i64).rem_euclid(3) == 0;
                return if band && piece { [0.55; 3] } else { [0.92; 3] };
            }
            // a plain, soft background
            [0.35 + 0.05 * (x as f32); 3]
        },
    );
    let a = crate::upright::analyze_source(&src, &SourceInfo::default(), &DevelopSettings::default());
    assert!(a.vertical_vp.is_none(), "folds are not structure: {:?}", a.vertical_vp);
    assert!(crate::upright::shortfall(Upright::Vertical, &a).is_some(), "and it says why");
}

/// Auto's note follows what Auto did: a tilted horizon it won't fix (over 5°) and a vertical correction too steep
/// to make both say why, while a level horizon it accepts says nothing.
#[test]
fn upright_auto_says_why_whenever_it_leaves_the_photo_alone() {
    use crate::upright::{Analysis, Reject, Segment, shortfall, solve};
    use lcc_develop::Upright;
    use lcc_geom::Homography;
    let line = |deg: f64, y: f64| {
        let (s, c) = deg.to_radians().sin_cos();
        Segment { a: Point::new(-0.4 * c, y - 0.4 * s), b: Point::new(0.4 * c, y + 0.4 * s) }
    };
    // a level horizon: Auto accepts a zero-degree correction, so it has nothing to explain
    let level = Analysis { horizontal: vec![line(0.0, -0.2), line(0.0, 0.2)], ..Analysis::default() };
    assert_eq!(shortfall(Upright::Auto, &level), None);
    // a horizon 10° off: Auto leaves it alone, and says so
    let tilted = Analysis { horizontal: vec![line(10.0, -0.2), line(10.0, 0.2)], ..Analysis::default() };
    assert_eq!(solve(Upright::Auto, &tilted), Homography::IDENTITY);
    assert_eq!(shortfall(Upright::Auto, &tilted), Some(Reject::TooFew.message()));
    // lines meeting just below the photo: too steep for Auto, which says so; Vertical still corrects them
    let steep = Analysis { vertical_vp: Some([0.0, 0.3, 1.0]), ..Analysis::default() };
    assert_eq!(solve(Upright::Auto, &steep), Homography::IDENTITY);
    assert_eq!(shortfall(Upright::Auto, &steep), Some(Reject::TooSteep.message()));
    assert_ne!(solve(Upright::Vertical, &steep), Homography::IDENTITY);
    // Level with no lines at all says there are too few, not that they disagree
    assert_eq!(shortfall(Upright::Level, &Analysis::default()), Some(Reject::TooFew.message()));
}
