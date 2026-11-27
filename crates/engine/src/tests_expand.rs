//! Crop ▸ Extend Past the Edge and Fill Edges end to end (issue #861): the crop reaching past the photo, the blank
//! canvas there, a content-aware fill, a generative one against the local mock provider (never the internet), and
//! turning it off again.

use std::sync::Arc;

use serde_json::json;

use crate::Session;
use lcc_genfill::cloud::mock::{Filler, MockServer};

const KEY: &str = "test-edge-fill-key-0123456789";

/// The cropped render of the active photo (400 px): its pixels, size and whether some show blank canvas.
fn render(s: &mut Session) -> (Vec<[u8; 4]>, usize, usize, bool) {
    let id = s.active().unwrap();
    let r = s.render_job(id, 400, 400, false, true).unwrap().run().rendered.unwrap();
    (r.image.data.clone(), r.image.width, r.image.height, r.coverage.is_some())
}

/// The spread of the green channel over the left fifth of the render (blank canvas: none).
fn left_spread(px: &[[u8; 4]], w: usize, h: usize) -> u8 {
    let g: Vec<u8> = (0..h).flat_map(|y| (0..w / 6).map(move |x| (x, y))).map(|(x, y)| px[y * w + x][1]).collect();
    g.iter().max().unwrap() - g.iter().min().unwrap()
}

fn extended() -> Session {
    let mut s = Session::with_demo();
    s.execute("crop.extend", &json!({"on": true})).unwrap();
    s.execute("crop.set", &json!({"rect": [-0.25, 0.0, 1.0, 1.0]})).unwrap();
    s
}

#[test]
fn an_extended_crop_shows_blank_canvas_until_filled() {
    let mut s = Session::with_demo();
    // off: the crop stays on the photo
    s.execute("crop.set", &json!({"rect": [-0.25, 0.0, 1.0, 1.0]})).unwrap();
    let id = s.active().unwrap();
    assert!(s.develop_of(id).unwrap().crop.geometry.rect.x0 >= -1e-9);
    let mut s = extended();
    let d = s.develop_of(id).unwrap();
    assert!((d.crop.geometry.rect.x0 + 0.25).abs() < 1e-9, "{:?}", d.crop.geometry);
    let (px, w, h, blank) = render(&mut s);
    assert!(blank, "the canvas past the photo is not the photo");
    // (the display dither moves a flat grey by a level)
    assert!(left_spread(&px, w, h) <= 2, "blank canvas, not the photo's edge drawn out: {}", left_spread(&px, w, h));
    // filling it: the photo's own content, and the render counts it as photo
    let r = s.execute("crop.fillEdges", &json!({"mode": "contentAware"})).unwrap();
    assert_eq!(r["filled"]["kind"], "contentAware", "{r}");
    let (px, w, h, blank) = render(&mut s);
    assert!(!blank);
    assert!(left_spread(&px, w, h) > 4, "filled with content");
    // the photo itself is untouched by the fill
    let d = s.develop_of(id).unwrap();
    let e = d.edge_fill.as_ref().unwrap();
    assert!(e.rect[0] < -0.2 && e.rect[2] >= 1.0, "{e:?}");
    assert!(s.edge_fill_dir(id).join(e.fill.file_name(0)).is_file());
    // undo takes the fill away, redo brings it back
    s.execute("edit.undo", &json!({})).unwrap();
    assert!(s.develop_of(id).unwrap().edge_fill.is_none());
    assert!(render(&mut s).3);
    s.execute("edit.redo", &json!({})).unwrap();
    assert!(!render(&mut s).3);
    // off again: the crop fits the photo and the fill goes
    s.execute("crop.extend", &json!({"on": false})).unwrap();
    let d = s.develop_of(id).unwrap();
    assert!(d.edge_fill.is_none() && !d.crop.extend && d.crop.geometry.rect.x0 >= -1e-9, "{:?}", d.crop);
    assert!(!render(&mut s).3);
}

#[test]
fn a_drag_reaches_past_the_photo_only_when_extending() {
    let mut s = Session::with_demo();
    let id = s.active().unwrap();
    let drag = json!({"handle": "left", "from": [0.0, 0.5], "to": [-0.3, 0.5]});
    s.execute("crop.drag", &drag).unwrap();
    assert!(s.develop_of(id).unwrap().crop.geometry.rect.x0 >= -1e-9);
    s.execute("crop.reset", &json!({})).unwrap();
    s.execute("crop.extend", &json!({"on": true})).unwrap();
    s.execute("crop.drag", &drag).unwrap();
    let x0 = s.develop_of(id).unwrap().crop.geometry.rect.x0;
    assert!((x0 + 0.3).abs() < 1e-6, "{x0}");
    // the reach is bounded
    s.execute("crop.drag", &json!({"handle": "left", "from": [-0.3, 0.5], "to": [-50.0, 0.5]})).unwrap();
    let x0 = s.develop_of(id).unwrap().crop.geometry.rect.x0;
    assert!((-0.5 - 1e-9..-0.3).contains(&x0), "{x0}");
}

#[test]
fn nothing_to_fill_and_bad_arguments_are_errors() {
    let mut s = Session::with_demo();
    let e = s.execute("crop.fillEdges", &json!({})).unwrap_err().to_string();
    assert!(e.contains("Extend Past the Edge"), "{e}");
    s.execute("crop.extend", &json!({})).unwrap();
    let e = s.execute("crop.fillEdges", &json!({})).unwrap_err().to_string();
    assert!(e.contains("doesn't reach past"), "{e}");
    assert!(s.execute("crop.fillEdges", &json!({"mode": "sideways"})).is_err());
    assert!(s.execute("crop.extend", &json!({"on": "yes"})).is_err());
    assert_eq!(s.execute("crop.clearFill", &json!({})).unwrap()["cleared"], false);
    // generative needs consent and a key, and sends nothing without them
    s.execute("crop.set", &json!({"rect": [-0.25, 0.0, 1.0, 1.0]})).unwrap();
    let e = s.execute("crop.fillEdges", &json!({"mode": "generative", "provider": "openai"})).unwrap_err().to_string();
    assert!(e.contains("terms"), "{e}");
    let e = s.execute("crop.fillEdges", &json!({"mode": "generative", "provider": "nobody"})).unwrap_err().to_string();
    assert!(e.contains("unknown provider"), "{e}");
}

#[test]
fn a_generative_fill_comes_from_the_service() {
    let dir = std::env::temp_dir().join(format!("lc-edge-fill-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // a filler that paints what it is asked to fill a strong blue
    let blue: Filler = Arc::new(|w, h, rgb, mask, _| {
        let mut out = rgb.to_vec();
        for (i, m) in mask.iter().enumerate().take(w * h) {
            if *m >= 128
                && let Some(o) = out.get_mut(i * 3..i * 3 + 3)
            {
                o.copy_from_slice(&[40, 60, 200]);
            }
        }
        out
    });
    let mock = MockServer::start(0, Some(KEY), blue).unwrap();
    let mut s = extended();
    s.genfill.keys = lcc_genfill::cloud::keys::KeyStore::new(Some(dir.join("genfill-keys.json")));
    s.genfill.mock = Some(mock.base.clone());
    s.execute("genfill.consent", &json!({"provider": "openai", "accept": true})).unwrap();
    s.execute("genfill.setKey", &json!({"provider": "openai", "key": KEY})).unwrap();
    let (before, ..) = render(&mut s);
    let r = s.execute("crop.fillEdges", &json!({"mode": "generative", "provider": "openai"})).unwrap();
    assert_eq!((r["filled"]["kind"].clone(), r["filled"]["provider"].clone()), (json!("generative"), json!("openai")), "{r}");
    assert_eq!(mock.seen().len(), 1, "one request");
    let (px, w, h, blank) = render(&mut s);
    assert!(!blank);
    let p = px[(h / 2) * w + w / 20];
    assert!(p[2] > p[0] + 40, "the service's fill shows past the photo: {p:?}");
    // the photo's own pixels are not the service's
    let i = (h / 2) * w + w * 3 / 4;
    assert_eq!(px[i], before[i]);
    let _ = std::fs::remove_dir_all(dir);
}
