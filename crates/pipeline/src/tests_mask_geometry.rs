//! Masks stay on the photo's content under lens corrections, Upright, Transform and Rotate
//! (`Mask::on_photo`, `mask_space`).

use lcc_develop::{DevelopSettings, LocalAdjustments, Mask, MaskComponent, MaskOp, MaskShape};
use lcc_geom::Point;
use lcc_raster::{Rgb32f, Rgba8};

use crate::{RenderRequest, SourceInfo, render};

const W: usize = 300;
const H: usize = 200;

/// Grey with a small red dot at `p` (normalized).
fn scene(p: Point) -> Rgb32f {
    Rgb32f::from_fn(W, H, |x, y| {
        let (dx, dy) = (x as f64 / W as f64 - p.x, (y as f64 / H as f64 - p.y) * H as f64 / W as f64);
        if dx.hypot(dy) < 0.012 { [0.6, 0.02, 0.02] } else { [0.18, 0.18, 0.18] }
    })
}

fn shot(src: &Rgb32f, s: &DevelopSettings) -> Rgba8 {
    render(src, &SourceInfo::default(), s, &RenderRequest::fit(W, H)).image
}

/// Where the red dot is drawn (the reddest pixel), normalized.
fn dot(img: &Rgba8) -> Point {
    let i = (0..img.data.len()).max_by_key(|&i| i32::from(img.data[i][0]) - i32::from(img.data[i][1])).unwrap();
    Point::new((i % img.width) as f64 / img.width as f64, (i / img.width) as f64 / img.height as f64)
}

/// Where the mask brightens the most (`with` against `without`), normalized: the centre of the
/// pixels brightened by more than half the most.
fn brightened(with: &Rgba8, without: &Rgba8) -> Point {
    let d: Vec<i32> = with.data.iter().zip(&without.data).map(|(a, b)| i32::from(a[1]) - i32::from(b[1])).collect();
    let top = *d.iter().max().unwrap();
    assert!(top > 20, "the mask brightens something");
    let (mut sx, mut sy, mut n) = (0.0, 0.0, 0.0);
    for (i, v) in d.iter().enumerate() {
        if *v * 2 > top {
            (sx, sy, n) = (sx + (i % with.width) as f64, sy + (i / with.width) as f64, n + 1.0);
        }
    }
    Point::new(sx / n / with.width as f64, sy / n / with.height as f64)
}

fn spot(p: Point) -> Mask {
    Mask {
        id: 1,
        components: vec![MaskComponent {
            anchor: None,
            name: None,
            op: MaskOp::Add,
            invert: false,
            shape: MaskShape::Radial { center: p, rx: 0.05, ry: 0.05, angle: 0.0, feather: 30.0, invert: false },
        }],
        adjust: LocalAdjustments { exposure: 2.0, ..Default::default() },
        ..Default::default()
    }
}

#[test]
fn a_mask_on_the_photo_follows_its_content_through_transform_and_lens_corrections() {
    let p = Point::new(0.3, 0.35);
    let src = scene(p);
    for (name, edit) in [
        (
            "vertical + rotate",
            (|s: &mut DevelopSettings| {
                s.geometry.vertical = 40.0;
                s.geometry.rotate = 6.0;
            }) as fn(&mut DevelopSettings),
        ),
        ("distortion", |s: &mut DevelopSettings| s.optics.distortion = 60.0),
    ] {
        let mut plain = DevelopSettings::default();
        edit(&mut plain);
        let mut masked = plain.clone();
        masked.masks.push(spot(p));
        let (a, b) = (shot(&src, &masked), shot(&src, &plain));
        let (at, want) = (brightened(&a, &b), dot(&b));
        assert!((want.x - p.x).hypot(want.y - p.y) > 0.01, "{name}: the edit moves the content");
        assert!((at.x - want.x).hypot(at.y - want.y) < 0.02, "{name}: the mask stays on the dot: {at:?} vs {want:?}");
    }
}

#[test]
fn an_old_mask_renders_where_it_did_until_it_is_attached() {
    let p = Point::new(0.3, 0.35);
    let src = scene(p);
    let mut s = DevelopSettings::default();
    s.geometry.vertical = 40.0;
    let mut old = spot(p);
    old.on_photo = false;
    let mut masked = s.clone();
    masked.masks.push(old.clone());
    let before = brightened(&shot(&src, &masked), &shot(&src, &s));
    // on the corrected frame: the mask's numbers are where it renders
    assert!((before.x - p.x).hypot(before.y - p.y) < 0.02, "{before:?}");
    // attached, it renders in the same place
    let frame = crate::geometry::Frame::new(W, H, &s, false);
    crate::mask_space::attach(&mut old, &frame);
    assert!(old.on_photo);
    masked.masks = vec![old];
    let after = brightened(&shot(&src, &masked), &shot(&src, &s));
    assert!((after.x - before.x).hypot(after.y - before.y) < 0.01, "{after:?} vs {before:?}");
}

/// A brush point the warp can't place lays no dab, rather than one at its unwarped place, which
/// can be far from the stroke (review of upstream #3550).
#[test]
fn a_brush_point_the_warp_cannot_place_lays_no_dab() {
    let mut s = DevelopSettings::default();
    s.geometry.vertical = -100.0;
    s.geometry.horizontal = -100.0;
    s.optics.distortion = 100.0;
    s.geometry.scale = 70.0;
    let frame = crate::geometry::Frame::new(1000, 1500, &s, true).for_mask(true).into_owned();
    let (w, h) = frame.fit(1000, 1500);
    let warp = frame.mask_warp().cloned().expect("a warp");
    // a point on the photo the inverse can't reach (found by search, so the test doesn't depend on
    // the solver's exact limits)
    let lost = (0..200 * 200)
        .map(|i| Point::new((i % 200) as f64 / 199.0, (i / 200) as f64 / 199.0))
        .find(|p| warp.from_source(Point::new(p.x * frame.ow, p.y * frame.oh)).is_none());
    let lost = lost.expect("these settings leave points the inverse can't reach");
    let stroke = lcc_develop::BrushStroke { points: vec![lost], size: 0.01, ..Default::default() };
    let dabs = crate::masks::brush_dabs_within(&stroke, &frame, w, h, 1000);
    assert!(dabs.dabs.is_empty(), "{lost:?} laid {:?}", dabs.dabs);
}
