//! Whole-render tests of Recolor on a mask ([`crate::recolor`]): only the masked main colour
//! changes, Amount 0 changes nothing, and previews and exports agree.

use lcc_develop::{DevelopSettings, Mask, MaskComponent, MaskOp, MaskShape};
use lcc_geom::Point;
use lcc_raster::{Rgb32f, Rgba8};

use crate::{OutputDepth, RenderRequest, SourceInfo, render};

const W: usize = 160;
const H: usize = 80;

/// A red gradient of varying lightness (a "red boot"), the same in every column.
fn scene() -> Rgb32f {
    Rgb32f::from_fn(W, H, |_, y| {
        let v = 0.10 + 0.55 * y as f32 / H as f32;
        [v, v * 0.16, v * 0.12]
    })
}

/// Recolor settings: the area takes purple (`#7a3fb0`-ish) at `amount`.
fn recolor(amount: f64) -> DevelopSettings {
    let mut a = lcc_develop::LocalAdjustments::default();
    (a.recolor, a.recolor_r, a.recolor_g, a.recolor_b) = (amount, 0.48, 0.25, 0.69);
    masked(a, all())
}

fn masked(a: lcc_develop::LocalAdjustments, shape: MaskShape) -> DevelopSettings {
    let mut s = DevelopSettings::default();
    s.masks.push(Mask {
        id: 1,
        adjust: a,
        components: vec![MaskComponent { anchor: None, name: None, op: MaskOp::Add, invert: false, shape }],
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
fn amount_zero_renders_byte_for_byte_as_before() {
    let src = scene();
    let plain = shot(&src, &masked(lcc_develop::LocalAdjustments::default(), all()));
    let off = shot(&src, &recolor(0.0));
    assert_eq!(plain.data, off.data, "Amount 0 is off");
    // and with a colour picked but the amount still 0
    let mut s = recolor(0.0);
    let m = &mut s.masks[0].adjust;
    (m.recolor_from_r, m.recolor_from_g, m.recolor_from_b, m.recolor_range) = (0.2, 0.4, 0.1, 50.0);
    assert_eq!(plain.data, shot(&src, &s).data);
}

#[test]
fn the_masked_area_takes_the_target_and_the_rest_stays() {
    let src = scene();
    let a = {
        let mut a = lcc_develop::LocalAdjustments::default();
        (a.recolor, a.recolor_r, a.recolor_g, a.recolor_b) = (100.0, 0.48, 0.25, 0.69);
        a
    };
    let s = masked(a, left());
    let plain = shot(&src, &masked(lcc_develop::LocalAdjustments::default(), left()));
    let out = shot(&src, &s);
    let inside = max_diff(&plain, &out, 0..W / 2 - 8);
    let outside = max_diff(&plain, &out, W / 2 + 8..W);
    assert!(inside > 20, "the masked area changes: {inside}");
    assert_eq!(outside, 0, "outside the mask nothing moves");
    // the changed pixels are purple: red and blue up, green below both
    let p = out.data[H / 2 * W + 10];
    assert!(p[2] > p[1] && p[0] > p[1], "purple-ish, got {p:?}");
}

#[test]
fn a_preview_and_an_export_agree() {
    let src = scene();
    let s = recolor(100.0);
    let mut req = RenderRequest::fit(320, 160);
    req.depth = OutputDepth::F32Linear;
    let small = render(&src, &SourceInfo::default(), &s, &req);
    let big_req = RenderRequest { depth: OutputDepth::F32Linear, ..RenderRequest::fit(640, 320) };
    let big = render(&src, &SourceInfo::default(), &s, &big_req);
    let (Some(small), Some(big)) = (small.deep.as_ref(), big.deep.as_ref()) else { panic!("deep renders") };
    // the same relative pixel: the same developed colour within a resample tolerance
    for (fx, fy) in [(0.25, 0.3), (0.5, 0.5), (0.75, 0.8)] {
        let at = |d: &crate::DeepImage, w: usize, h: usize| {
            let (x, y) = ((fx * w as f64) as usize, (fy * h as f64) as usize);
            let i = (y * d.width + x) * 3;
            match &d.samples {
                crate::DeepSamples::F32(v) => [v[i], v[i + 1], v[i + 2]],
                _ => [0.0; 3],
            }
        };
        let (p, q) = (at(small, 320, 160), at(big, 640, 320));
        for k in 0..3 {
            assert!((p[k] - q[k]).abs() < 0.04, "({fx},{fy}) ch {k}: {p:?} vs {q:?}");
        }
    }
}
