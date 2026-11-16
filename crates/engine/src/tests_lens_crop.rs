//! Lens-database corrections for a smaller image area of a full-frame sensor (5:4, 1:1, 16:9, DX): every sensor
//! position gets the correction it gets in the camera's normal frame ([`super::lookup_framed`], [`super::ImageArea`]).

use super::*;
use lcc_geom::{Orientation, Point};
use lcc_pipeline::optics::{embedded_vignette_gain, embedded_warp};
use lcc_raw::{Rect, SensorWindow};

/// A full-frame body and a fast standard lens with distortion, lateral CA and strong vignetting (lensfun's own
/// models: `ptlens`, `poly3` TCA, `pa`).
const DB: &str = r#"<lensdatabase>
  <camera><maker>Testcam</maker><model>FF-1</model><mount>T</mount><cropfactor>1</cropfactor></camera>
  <lens><maker>Testcam</maker><model>Testcam 50mm f/1.8</model><mount>T</mount><cropfactor>1.0</cropfactor>
    <calibration>
      <distortion model="ptlens" focal="50" a="-0.009" b="0.026" c="-0.028" />
      <tca model="poly3" focal="50" br="-0.0003" vr="1.0004" bb="0.0004" vb="0.9997" />
      <vignetting model="pa" focal="50" aperture="1.8" distance="1000" k1="-1.1859649" k2="0.6114940" k3="-0.1598373" />
    </calibration>
  </lens></lensdatabase>"#;

/// The same lens with strong pincushion-correcting distortion (corners pulled from outside the frame).
const DB_STRONG: &str = r#"<lensdatabase>
  <camera><maker>Testcam</maker><model>FF-1</model><mount>T</mount><cropfactor>1</cropfactor></camera>
  <lens><maker>Testcam</maker><model>Testcam 50mm f/1.8</model><mount>T</mount><cropfactor>1.0</cropfactor>
    <calibration><distortion model="poly3" focal="50" k1="0.03" /></calibration>
  </lens></lensdatabase>"#;

fn meta(focal_35mm: Option<f64>) -> lcc_meta::Metadata {
    lcc_meta::Metadata {
        make: Some("Testcam".into()),
        model: Some("FF-1".into()),
        lens_model: Some("Testcam 50mm f/1.8".into()),
        focal_length: Some(50.0),
        focal_length_35mm: focal_35mm,
        f_number: Some(1.8),
        ..Default::default()
    }
}

/// The Z 7's sensor and its default-crop margins (16, 8 on every side, as its CropArea records).
const SENSOR: [usize; 2] = [8288, 5520];
const MARGIN: [usize; 2] = [16, 8];

/// A centred image area of `size` on [`SENSOR`] (or at `origin`), with the margins as its default crop.
fn area(size: [usize; 2], origin: Option<[usize; 2]>) -> ImageArea {
    let origin = origin.unwrap_or([(SENSOR[0] - size[0]) / 2, (SENSOR[1] - size[1]) / 2]);
    let window = SensorWindow { sensor: SENSOR, origin, size };
    let crop = Rect::new(MARGIN[0], MARGIN[1], size[0] - 2 * MARGIN[0], size[1] - 2 * MARGIN[1]);
    ImageArea::new(window, size, crop, 1.0).unwrap()
}

/// The full frame's developed picture and the correction it gets (no window: today's path).
fn full(db: &LensDb) -> ([f64; 2], EmbeddedLens) {
    let (w, h) = ((SENSOR[0] - 2 * MARGIN[0]) as f64, (SENSOR[1] - 2 * MARGIN[1]) as f64);
    ([w, h], lookup_framed(db, &meta(None), None, w, h, Orientation::Normal, None).unwrap().0)
}

/// The image areas a full-frame Nikon records (Z 7 sizes), by name.
fn crop_modes() -> Vec<(&'static str, ImageArea)> {
    vec![("5:4", area([6912, 5520], None)), ("1:1", area([5520, 5520], None)), ("16:9", area([8288, 4662], None)), ("DX", area([5408, 3600], None))]
}

#[test]
fn every_sensor_position_gets_the_full_frames_correction_in_every_image_area() {
    let db = LensDb::parse(DB).unwrap();
    let ([fw, fh], whole) = full(&db);
    let (fv, fwarp) = (whole.vignette.unwrap(), whole.warp.unwrap());
    for (name, a) in crop_modes() {
        let (w, h) = (a.crop.width as f64, a.crop.height as f64);
        // with a 35 mm equivalent the camera rounds (the Z 7 writes ×1.10 for 5:4): the window wins
        let lens = lookup_framed(&db, &meta(Some(55.0)), None, w, h, Orientation::Normal, Some(&a)).unwrap().0;
        let (v, warp) = (lens.vignette.unwrap(), lens.warp.unwrap());
        // this picture's top-left on the full frame's picture
        let at = [a.window.origin[0] as f64, a.window.origin[1] as f64];
        let (mut worst_gain, mut worst_px) = (0.0f64, 0.0f64);
        for j in 0..=10 {
            for i in 0..=10 {
                let p = Point::new(w * i as f64 / 10.0, h * j as f64 / 10.0);
                let q = Point::new(p.x + at[0], p.y + at[1]);
                let (g, gf) = (embedded_vignette_gain(&v, p, w, h), embedded_vignette_gain(&fv, q, fw, fh));
                worst_gain = worst_gain.max((g / gf - 1.0).abs());
                // distortion (green) and lateral CA (red and blue planes)
                for ch in 0..3 {
                    let (s, sf) = (embedded_warp(&warp, p, ch, w, h), embedded_warp(&fwarp, q, ch, fw, fh));
                    worst_px = worst_px.max((s.x + at[0] - sf.x).abs()).max((s.y + at[1] - sf.y).abs());
                }
            }
        }
        assert!(worst_gain < 1e-9, "{name}: vignetting differs by {worst_gain:e} (relative)");
        assert!(worst_px < 1e-6, "{name}: distortion / lateral CA differ by {worst_px:e} px");
        // the correction is centred on the sensor, which this picture is centred on too
        assert!((v.center.x - 0.5).abs() < 1e-12 && (v.center.y - 0.5).abs() < 1e-12, "{name}: {:?}", v.center);
    }
}

#[test]
fn dx_is_scaled_by_its_own_diagonal_not_by_the_rounded_crop_factor() {
    let db = LensDb::parse(DB).unwrap();
    let (_, whole) = full(&db);
    let dx = area([5408, 3600], None);
    let (w, h) = (dx.crop.width as f64, dx.crop.height as f64);
    let lens = lookup_framed(&db, &meta(Some(75.0)), None, w, h, Orientation::Normal, Some(&dx)).unwrap().0;
    // k1 of the vignette scales with s² (s = this picture's half diagonal over the full frame's): the crop factor
    let s = (lens.vignette.unwrap().k[0] / whole.vignette.unwrap().k[0]).sqrt();
    let crop = 1.0 / s;
    let expected = (8256f64.hypot(5504.0)) / (w.hypot(h));
    eprintln!("DX: effective crop factor {crop:.4} (frame diagonals {expected:.4}; the 35 mm equivalent says 1.5)");
    assert!((crop - expected).abs() < 1e-9 && (crop - 1.5).abs() > 0.02, "{crop}");
    // the Z 7's 5408 × 3600 DX area itself is 1.533 of its 8288 × 5520 sensor
    assert!((8288f64.hypot(5520.0) / 5408f64.hypot(3600.0) - 1.533).abs() < 0.001);
}

#[test]
fn the_full_sensor_and_an_unknown_window_render_as_before() {
    let db = LensDb::parse(DB).unwrap();
    let ([w, h], whole) = full(&db);
    let before = lookup_with(&db, &meta(None), None, w, h, Orientation::Normal).unwrap().0;
    assert_eq!(whole, before);
    // a window covering the whole sensor is no image area: the photo takes today's path
    let all = SensorWindow { sensor: SENSOR, origin: [0, 0], size: SENSOR };
    assert!(ImageArea::new(all, SENSOR, Rect::new(16, 8, 8256, 5504), 1.0).is_none());
    // windows that don't describe the raw are ignored, never a wrong offset
    let at = |origin, size, active, aspect| ImageArea::new(SensorWindow { sensor: SENSOR, origin, size }, active, Rect::new(0, 0, 0, 0), aspect);
    assert!(at([1400, 0], [6912, 5520], [6912, 5520], 1.0).is_none(), "past the sensor");
    assert!(at([688, 0], [6912, 5520], [6900, 5520], 1.0).is_none(), "another size than the active area");
    assert!(at([688, 0], [6912, 5520], [6912, 5520], 2.0).is_none(), "non-square pixels");
    assert!(at([688, 0], [6912, 5520], [6912, 5520], f32::NAN).is_none());
    assert!(at([0, 0], [0, 5520], [0, 5520], 1.0).is_none(), "empty");
    // an area whose picture isn't the size asked for takes today's path too
    let a = area([6912, 5520], None);
    let plain = lookup_framed(&db, &meta(None), None, 6000.0, 4000.0, Orientation::Normal, None).unwrap().0;
    assert_eq!(lookup_framed(&db, &meta(None), None, 6000.0, 4000.0, Orientation::Normal, Some(&a)).unwrap().0, plain);
}

#[test]
fn an_off_centre_image_area_is_corrected_about_the_sensors_centre() {
    let db = LensDb::parse(DB).unwrap();
    let ([fw, fh], whole) = full(&db);
    let a = area([6912, 5520], Some([0, 0]));
    let (w, h) = (a.crop.width as f64, a.crop.height as f64);
    let lens = lookup_framed(&db, &meta(None), None, w, h, Orientation::Normal, Some(&a)).unwrap().0;
    for p in [Point::new(0.0, 0.0), Point::new(w, h), Point::new(w * 0.3, h * 0.8)] {
        let (g, gf) = (embedded_vignette_gain(&lens.vignette.unwrap(), p, w, h), embedded_vignette_gain(&whole.vignette.unwrap(), p, fw, fh));
        assert!((g / gf - 1.0).abs() < 1e-9, "{p:?}: {g} vs {gf}");
    }
}

/// Strong distortion pulls the corners from outside the frame, so the crop shrinks until no blank shows
/// (`Frame::constrain_to_image`). It must shrink for the image area's own corners: just enough, and no more than the
/// full-frame-scaled profile (the old behaviour) would have.
#[test]
fn strong_distortion_in_an_image_area_leaves_no_blank_and_no_extra_shrink() {
    let db = LensDb::parse(DB_STRONG).unwrap();
    let a = area([6912, 5520], None);
    let (w, h) = (a.crop.width, a.crop.height);
    let fixed = lookup_framed(&db, &meta(None), None, w as f64, h as f64, Orientation::Normal, Some(&a)).unwrap().0;
    let old = lookup_framed(&db, &meta(None), None, w as f64, h as f64, Orientation::Normal, None).unwrap().0;
    let mut s = lcc_develop::DevelopSettings::default();
    s.optics.lens_profile = true;
    let frame = |lens: &EmbeddedLens| lcc_pipeline::geometry::Frame::with_lens(w, h, &s, true, Some(lens));
    let (f, o) = (frame(&fixed), frame(&old));
    let (t, t_old) = (f.crop.rect.width(), o.crop.rect.width());
    eprintln!("5:4, strong distortion: crop kept {t:.5} of the width (the full-frame-scaled profile: {t_old:.5})");
    assert!(t < 1.0, "this distortion needs the crop to shrink");
    assert!(t > t_old, "less shrink than the old, over-scaled profile");
    // no blank: every edge point of the output comes from inside the picture; minimal: one lands on its edge
    let warp = f.warp.as_ref().unwrap();
    let out = f.crop.output_to_source(f.ow, f.oh, 1.0, 1.0);
    let (mut outside, mut nearest) = (0.0f64, f64::INFINITY);
    for i in 0..=400 {
        let u = i as f64 / 400.0;
        for (x, y) in [(u, 0.0), (u, 1.0), (0.0, u), (1.0, u)] {
            for ch in 0..3 {
                let src = warp.to_source(out.apply(Point::new(x, y)), ch);
                let gap = [src.x, w as f64 - src.x, src.y, h as f64 - src.y].into_iter().fold(f64::INFINITY, f64::min);
                outside = outside.max(-gap);
                nearest = nearest.min(gap.abs());
            }
        }
    }
    // (the shrink is searched on 49 points per edge, so between them an edge may reach a hair past the picture; the
    // renderer counts half a pixel past its edge as picture, `Warp::edges`, so that never shows as blank)
    eprintln!("   farthest past the edge {outside:.4} px, nearest to it {nearest:.4} px");
    assert!(outside < 0.5, "a corner shows {outside} px of blank");
    assert!(nearest < 1.0, "the crop shrank {nearest} px more than needed");
}
