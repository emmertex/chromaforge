//! What plug-ins see of a photo's pixels (`photo.pixels`) and change of its crop
//! (`develop.setCrop`): the whole uncropped frame, in the coordinates the crop is set in.

use std::path::{Path, PathBuf};

use lcc_catalog::PhotoId;
use lcc_plugins::HostApi;
use serde_json::json;

use crate::Session;
use crate::cmd::plugins::SessionHost;

/// A 60 × 40 PNG, black with a white box at x 15..45, y 10..30: a "frame" inside its rebate.
fn write_frame(path: &Path) {
    let (w, h) = (60, 40);
    let data: Vec<[u8; 4]> = (0..w * h)
        .map(|i| {
            let (x, y) = (i % w, i / w);
            let v = if (15..45).contains(&x) && (10..30).contains(&y) { 255 } else { 0 };
            [v, v, v, 255]
        })
        .collect();
    let img = lcc_raster::Rgba8 { width: w, height: h, data };
    let bytes = lcc_codecs::encode_png(&lcc_codecs::EncodeImage::rgba8(&img), &lcc_codecs::EncodeMeta::default()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

struct Fixture {
    dir: PathBuf,
    s: Session,
    photo: PhotoId,
}

impl Fixture {
    fn new(tag: &str) -> Fixture {
        let dir = std::env::temp_dir().join(format!("lcc-plugin-develop-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        write_frame(&dir.join("frame.png"));
        let mut s = Session::new().with_fs();
        let r = s.execute("library.import", &json!({"paths": [dir.join("frame.png").to_string_lossy()]})).unwrap();
        let photo = PhotoId(r["imported"][0].as_u64().unwrap());
        Fixture { dir, s, photo }
    }

    fn id(&self) -> String {
        self.photo.0.to_string()
    }

    fn crop(&self) -> lcc_geom::CropGeometry {
        self.s.develop_of(self.photo).unwrap().crop.geometry
    }

    /// The pixels as the SDK decodes them: `(width, height, rgb)`.
    fn pixels(&mut self, edge: u32) -> (usize, usize, Vec<u8>) {
        let id = self.id();
        let v = SessionHost(&mut self.s).photo_pixels(&id, edge).unwrap();
        assert_eq!((v["channels"].as_u64(), v["encoding"].as_str()), (Some(3), Some("srgb8")));
        let rgb = {
            use base64::Engine as _;
            base64::engine::general_purpose::STANDARD.decode(v["data"].as_str().unwrap()).unwrap()
        };
        (v["width"].as_u64().unwrap() as usize, v["height"].as_u64().unwrap() as usize, rgb)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn pixels_are_the_whole_uncropped_frame() {
    let mut f = Fixture::new("whole");
    let (w, h, rgb) = f.pixels(1024);
    assert_eq!((w, h), (60, 40), "never blown up past the photo");
    assert_eq!(rgb.len(), w * h * 3);
    let at = |rgb: &[u8], w: usize, x: usize, y: usize| rgb[3 * (y * w + x)];
    assert!(at(&rgb, w, 30, 20) > 200 && at(&rgb, w, 2, 2) < 30, "the box is bright, the rebate dark");
    // a crop doesn't change what the plug-in sees
    f.s.execute("crop.set", &json!({"rect": [0.5, 0.5, 1.0, 1.0]})).unwrap();
    let (w2, h2, rgb2) = f.pixels(1024);
    assert_eq!((w2, h2), (60, 40));
    assert_eq!(rgb2, rgb);
    // shrunk to the long edge asked for
    let (w, h, _) = f.pixels(30);
    assert_eq!((w, h), (30, 20));
}

#[test]
fn a_crop_found_in_the_pixels_is_set_as_it_is_and_undoes_in_one_step() {
    let mut f = Fixture::new("set");
    let id = f.id();
    let r = SessionHost(&mut f.s).set_crop(&id, [0.25, 0.25, 0.75, 0.75], None).unwrap();
    assert_eq!(r["rect"], json!([0.25, 0.25, 0.75, 0.75]));
    let g = f.crop();
    assert_eq!((g.rect.x0, g.rect.y0, g.rect.x1, g.rect.y1, g.angle), (0.25, 0.25, 0.75, 0.75, 0.0));
    f.s.execute("edit.undo", &json!({})).unwrap();
    assert!(f.crop().is_identity(), "one History step");
    // an angle straightens it; the crop is fitted inside the photo
    let r = SessionHost(&mut f.s).set_crop(&id, [0.0, 0.0, 1.0, 1.0], Some(3.0)).unwrap();
    assert_eq!(r["angle"], 3.0);
    let g = f.crop();
    assert!(g.rect.x0 > 0.0 && g.rect.x1 < 1.0, "a straightened crop takes in no canvas: {g:?}");
}

#[test]
fn bad_photos_and_slivers_are_refused_and_change_nothing() {
    let mut f = Fixture::new("refuse");
    let mut host = SessionHost(&mut f.s);
    assert!(host.photo_pixels("999999", 256).unwrap_err().contains("no photo"));
    assert!(host.photo_pixels("abc", 256).unwrap_err().contains("not a photo id"));
    assert!(host.set_crop("999999", [0.1, 0.1, 0.9, 0.9], None).unwrap_err().contains("no photo"));
    let id = f.id();
    let e = SessionHost(&mut f.s).set_crop(&id, [0.5, 0.5, 0.5001, 0.5001], None).unwrap_err();
    assert!(e.contains("smallest crop"), "{e}");
    assert!(f.crop().is_identity());
}

/// A hook on a worker: both calls go to the session's thread.
#[test]
fn from_a_worker_both_calls_go_through_the_session_thread() {
    let mut f = Fixture::new("relay");
    let id = f.id();
    let (mut host, rx) = crate::cmd::plugins::relay_host(&f.s);
    let worker = std::thread::spawn(move || {
        let px = host.photo_pixels(&id, 64);
        let crop = host.set_crop(&id, [0.25, 0.25, 0.75, 0.75], None);
        (px, crop)
    });
    rx.recv().unwrap().answer(&mut f.s);
    rx.recv().unwrap().answer(&mut f.s);
    let (px, crop) = worker.join().unwrap();
    assert_eq!(px.unwrap()["width"], 60);
    assert_eq!(crop.unwrap()["rect"], json!([0.25, 0.25, 0.75, 0.75]));
    assert_eq!(f.crop().rect.x0, 0.25);
}
