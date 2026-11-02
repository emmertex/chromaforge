//! Auto Sync of a crop to a photo of another frame shape keeps its tightness and position, in
//! that photo's own orientation (`sync_same::synced_crop`).

/// A crop of `frac` of the largest of shape `shape` at `angle` on a `w × h` frame, centred at the
/// normalized point `c` (moved inside), with `aspect` locked or free.
#[allow(clippy::too_many_arguments)]
fn crop_on(w: f64, h: f64, shape: f64, frac: f64, c: (f64, f64), angle: f64, aspect: Option<(u32, u32)>, flips: (bool, bool)) -> lcc_develop::Crop {
    let f = lcc_geom::crop_fit_angle(w, h, angle, Some(shape)).rect_px(w, h);
    let r = lcc_geom::Rect::from_center(lcc_geom::Point::new(c.0 * w, c.1 * h), f.width() * frac, f.height() * frac);
    let g = lcc_geom::CropGeometry { rect: r.scale(1.0 / w, 1.0 / h), angle }.constrained(w, h);
    lcc_develop::Crop { geometry: g, aspect, flip_h: flips.0, flip_v: flips.1 }
}

/// Test notes 2026-10-25 note 15: a crop synced from a vertical to a landscape (and back) keeps its
/// tightness and position: 80 % of the largest crop of its shape on the vertical is 80 % of the
/// largest of the turned shape on the landscape, its centre at the same normalized point, moved in
/// only as far as the frame needs. It used to be the landscape's largest crop, i.e. no change.
#[test]
fn a_synced_crop_keeps_its_tightness_and_position_across_orientations() {
    let (v, l) = ((4000.0, 6000.0), (6000.0, 4000.0));
    for aspect in [Some((2, 3)), None] {
        let src = crop_on(v.0, v.1, 2.0 / 3.0, 0.8, (0.5, 1.0 / 3.0), 0.0, aspect, (false, false));
        let out = crate::sync_same::synced_crop(&src, v, l);
        let r = out.geometry.rect_px(l.0, l.1);
        let largest = lcc_geom::crop_fit_angle(l.0, l.1, 0.0, Some(1.5)).rect_px(l.0, l.1);
        assert!((r.width() / r.height() - 1.5).abs() < 1e-6, "{aspect:?}: turned to 3:2, got {}", r.width() / r.height());
        assert!((r.width() / largest.width() - 0.8).abs() < 1e-6, "{aspect:?}: 80 % of the largest, got {}", r.width() / largest.width());
        assert!((r.center().x / l.0 - 0.5).abs() < 1e-6, "{aspect:?}: centred across");
        // the vertical's crop sits near its top: so does the landscape's, at the same normalized
        // centre, which still fits
        let sc = src.geometry.rect.center();
        assert!((r.center().y / l.1 - sc.y).abs() < 1e-6, "{aspect:?}: the same height in the frame, {} vs {}", r.center().y / l.1, sc.y);
        assert!(r.y0 >= -1e-6 && r.y0 < 0.01 * l.1, "{aspect:?}: near the top, y0 {}", r.y0);
        assert!(out.geometry.is_within_image(l.0, l.1));
        if aspect.is_some() {
            assert_eq!(out.aspect, Some((3, 2)));
        }
    }
    // a centre that would put the crop off the frame moves in only as far as it must
    let src = crop_on(v.0, v.1, 2.0 / 3.0, 0.9, (0.5, 0.45), 0.0, Some((2, 3)), (false, false));
    let mut low = src;
    let (w, h) = (low.geometry.rect.width(), low.geometry.rect.height());
    low.geometry.rect = lcc_geom::Rect::new(0.5 - w / 2.0, 1.0 - h, 0.5 + w / 2.0, 1.0);
    let out = crate::sync_same::synced_crop(&low, v, l);
    let r = out.geometry.rect_px(l.0, l.1);
    assert!((r.y1 - l.1).abs() < 1e-3, "down against the bottom edge: {r:?}");
    assert!(out.geometry.is_within_image(l.0, l.1));
    // a crop with room to keep its place keeps it exactly, normalized
    let src = crop_on(l.0, l.1, 1.5, 0.5, (0.3, 0.6), 0.0, Some((3, 2)), (false, false));
    let out = crate::sync_same::synced_crop(&src, l, v);
    let r = out.geometry.rect_px(v.0, v.1);
    assert!((r.center().x / v.0 - 0.3).abs() < 1e-6 && (r.center().y / v.1 - 0.6).abs() < 1e-6, "{:?}", r.center());
    assert_eq!(out.aspect, Some((2, 3)));
    // same shape: the very rectangle
    let same = crate::sync_same::synced_crop(&src, l, (3000.0, 2000.0));
    assert_eq!(same, src);
}

/// Test notes 2026-10-25 note 15, the user's warning (another raw developer gives a horizontal
/// photo a vertical crop when syncing from a vertical): whatever the crop, a landscape never gets a
/// portrait-shaped crop and a vertical never a landscape one, with the ratio locked or free, at a
/// straighten angle, with flips, from either orientation; and the crop always stays inside.
#[test]
fn a_synced_crop_never_gets_the_other_orientation() {
    let frames = [(4000.0, 6000.0), (6000.0, 4000.0), (3000.0, 4000.0), (5472.0, 3648.0)];
    for a in frames {
        for o in frames {
            for (shape, aspect) in [(2.0 / 3.0, Some((2, 3))), (1.5, Some((3, 2))), (0.8, None), (1.7, None), (1.0, Some((1, 1)))] {
                for angle in [0.0, 3.0, -7.5, 20.0] {
                    for flips in [(false, false), (true, false), (false, true), (true, true)] {
                        for (frac, c) in [(1.0, (0.5, 0.5)), (0.7, (0.2, 0.8)), (0.4, (0.9, 0.1))] {
                            let src = crop_on(a.0, a.1, shape, frac, c, angle, aspect, flips);
                            let out = crate::sync_same::synced_crop(&src, a, o);
                            let r = out.geometry.rect_px(o.0, o.1);
                            let s = r.width() / r.height();
                            let what = format!("{a:?} → {o:?}, shape {shape} {aspect:?}, {angle}°, {flips:?}, {frac} at {c:?}: {s}");
                            assert!(out.geometry.is_within_image(o.0, o.1), "inside: {what}");
                            assert_eq!((out.flip_h, out.flip_v), flips, "{what}");
                            assert_eq!(out.geometry.angle, angle, "{what}");
                            if (a.0 / a.1 - o.0 / o.1).abs() <= 0.005 * a.0 / a.1 {
                                continue;
                            }
                            let wide = o.0 > o.1;
                            if (shape - 1.0f64).abs() > 1e-3 {
                                assert_eq!(s > 1.0, wide, "the photo's own orientation: {what}");
                            }
                            if let Some((x, y)) = out.aspect.filter(|(x, y)| x != y) {
                                assert_eq!(x > y, wide, "the locked ratio turned too: {what}");
                            }
                        }
                    }
                }
            }
        }
    }
}
