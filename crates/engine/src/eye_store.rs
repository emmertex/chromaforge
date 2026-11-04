//! A photo's eyes ([`crate::eyes::EyeReport`]) kept on disk in the preview cache, so a photo
//! looked at once is never looked at again: not after a restart, not by the culling strip after
//! a headless prep run (`eyes.analyze`), not by Reject Closed and Blurry Eyes.
//!
//! **Key** ([`file_key`]): the file itself — its canonical path, size and modified time — with
//! [`crate::eyes::EYES_VERSION`], this format's version and the model files' names and sizes. It
//! needs no catalog entry, so photos browsed in the Import tab and library photos are alike, and
//! a file that is changed (or a model that is replaced) is looked at afresh.
//!
//! **Format** (a `.lcdata` record of [`lcc_preview::DiskCache`], counted in the cache's
//! size and let go with the thumbnails when it is full): `LCEY`, the format version (u32 LE), the
//! length of a JSON header (u32 LE), the header (everything but the crops), then each person's
//! eye crop as `width u16, height u16, length u32` and a JPEG (about 6 kB each). A record that
//! doesn't read — cut short, another version, nonsense — is dropped and the photo looked at
//! again; it never stops anything.

use std::path::Path;
use std::sync::Arc;

use lcc_catalog::PhotoId;
use lcc_preview::{Hash128, Hasher128, PreviewCache};
use lcc_raster::Rgba8;
use serde::{Deserialize, Serialize};

use crate::eyes::{CROP_H, CROP_W, EYES_VERSION, EyeLook, EyeReport, EyeShot, Focus, Lids, MAX_PEOPLE};

const MAGIC: &[u8; 4] = b"LCEY";
/// The record's layout (bump when it changes; the key holds it too).
pub const FORMAT: u32 = 1;
/// The largest header read (a report of [`MAX_PEOPLE`] people is a few tens of kB).
const MAX_HEADER: usize = 4 << 20;
/// The largest crop JPEG read.
const MAX_CROP_BYTES: usize = 1 << 20;

/// One person's eye, without its crop (that follows the header).
#[derive(Serialize, Deserialize)]
struct Shot {
    face: [f32; 4],
    center: [f32; 2],
    sharpness: f32,
    relative: f32,
    focus: Focus,
    both_eyes: bool,
    openness: Option<f32>,
    iris: Option<f32>,
    contrast: Option<f32>,
    look: Option<EyeLook>,
    eye_width: f32,
    score: f32,
    main: bool,
    landmarks: bool,
    lids: Lids,
}

#[derive(Serialize, Deserialize)]
struct Header {
    eyes: Vec<Shot>,
    faces: usize,
    no_eyes: usize,
    boxes: Vec<[f32; 4]>,
    size: (usize, usize),
    ms: f64,
}

/// The names and sizes of the files of the face model in `model_dir` and of the people model
/// beside it: a model that is replaced (or the small-face detector arriving) gives other results.
fn model_tag(model_dir: &Path) -> u64 {
    let mut h = Hasher128::new();
    let people = model_dir.parent().map(|p| p.join("mediapipe-selfie-multiclass"));
    for dir in std::iter::once(model_dir.to_path_buf()).chain(people) {
        let mut files: Vec<(String, u64)> = std::fs::read_dir(&dir)
            .map(|rd| {
                rd.flatten()
                    .filter_map(|e| {
                        let m = e.metadata().ok().filter(|m| m.is_file())?;
                        Some((e.file_name().to_string_lossy().to_string(), m.len()))
                    })
                    .take(64)
                    .collect()
            })
            .unwrap_or_default();
        files.sort();
        h.str(&dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default());
        for (name, len) in files {
            h.str(&name).u64(len);
        }
    }
    h.finish().0 as u64
}

/// The record's key for the file at `path` judged with the model in `model_dir`: `None` when the
/// file can't be looked up (gone, unreadable).
pub fn file_key(path: &str, model_dir: &Path) -> Option<Hash128> {
    let canon = std::fs::canonicalize(path).unwrap_or_else(|_| std::path::PathBuf::from(path));
    let m = std::fs::metadata(&canon).ok().filter(|m| m.is_file())?;
    let modified = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos() as u64);
    Some(
        Hasher128::new()
            .str("eyes-file")
            .str(&canon.to_string_lossy())
            .u64(m.len())
            .u64(modified)
            .u64(EYES_VERSION)
            .u64(u64::from(FORMAT))
            .u64(model_tag(model_dir))
            .finish(),
    )
}

/// The record of `r`.
pub fn encode(r: &EyeReport) -> Option<Vec<u8>> {
    let header = Header {
        eyes: r
            .eyes
            .iter()
            .map(|e| Shot {
                face: e.face,
                center: e.center,
                sharpness: e.sharpness,
                relative: e.relative,
                focus: e.focus,
                both_eyes: e.both_eyes,
                openness: e.openness,
                iris: e.iris,
                contrast: e.contrast,
                look: e.look,
                eye_width: e.eye_width,
                score: e.score,
                main: e.main,
                landmarks: e.landmarks,
                lids: e.lids,
            })
            .collect(),
        faces: r.faces,
        no_eyes: r.no_eyes,
        boxes: r.boxes.clone(),
        size: r.size,
        ms: r.ms,
    };
    let json = serde_json::to_vec(&header).ok()?;
    let mut out = Vec::with_capacity(json.len() + 12 + r.eyes.len() * 8000);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&FORMAT.to_le_bytes());
    out.extend_from_slice(&u32::try_from(json.len()).ok()?.to_le_bytes());
    out.extend_from_slice(&json);
    for e in &r.eyes {
        let (w, h) = (u16::try_from(e.crop.width).ok()?, u16::try_from(e.crop.height).ok()?);
        // (an empty crop is kept as an empty one)
        let jpeg = if w == 0 || h == 0 { Vec::new() } else { lcc_preview::encode_jpeg(&e.crop)? };
        out.extend_from_slice(&w.to_le_bytes());
        out.extend_from_slice(&h.to_le_bytes());
        out.extend_from_slice(&u32::try_from(jpeg.len()).ok()?.to_le_bytes());
        out.extend_from_slice(&jpeg);
    }
    Some(out)
}

/// Reads a record from the front of a byte slice.
struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let (head, rest) = (self.0.get(..n)?, self.0.get(n..)?);
        self.0 = rest;
        Some(head)
    }
    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.take(2)?.try_into().ok()?))
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
}

/// The report in record `bytes`, as photo `photo`'s (`None`: not a record of this format).
pub fn decode(bytes: &[u8], photo: PhotoId) -> Option<EyeReport> {
    let mut r = Reader(bytes);
    if r.take(4)? != MAGIC || r.u32()? != FORMAT {
        return None;
    }
    let len = r.u32()? as usize;
    if len > MAX_HEADER {
        return None;
    }
    let h: Header = serde_json::from_slice(r.take(len)?).ok()?;
    if h.eyes.len() > MAX_PEOPLE || h.boxes.len() > 100_000 {
        return None;
    }
    let mut eyes = Vec::with_capacity(h.eyes.len());
    for s in h.eyes {
        let (w, hh) = (r.u16()? as usize, r.u16()? as usize);
        let n = r.u32()? as usize;
        if w > CROP_W * 4 || hh > CROP_H * 4 || n > MAX_CROP_BYTES {
            return None;
        }
        let jpeg = r.take(n)?;
        let crop = if w == 0 || hh == 0 {
            Rgba8::new(w, hh)
        } else {
            let img = lcc_preview::decode_jpeg(jpeg)?;
            if (img.width, img.height) != (w, hh) {
                return None;
            }
            img
        };
        eyes.push(EyeShot {
            face: s.face,
            center: s.center,
            crop: Arc::new(crop),
            sharpness: s.sharpness,
            relative: s.relative,
            focus: s.focus,
            both_eyes: s.both_eyes,
            openness: s.openness,
            iris: s.iris,
            contrast: s.contrast,
            look: s.look,
            eye_width: s.eye_width,
            score: s.score,
            main: s.main,
            landmarks: s.landmarks,
            lids: s.lids,
        });
    }
    if !r.0.is_empty() {
        return None;
    }
    Some(EyeReport { photo, eyes, faces: h.faces, no_eyes: h.no_eyes, boxes: h.boxes, size: h.size, ms: h.ms })
}

/// The report kept for `key`; a record that doesn't read is dropped (`None`, look again).
pub fn read(cache: &PreviewCache, key: Hash128, photo: PhotoId) -> Option<EyeReport> {
    let bytes = cache.get_data(key)?;
    let r = decode(&bytes, photo);
    if r.is_none() {
        log::warn!("eyes: dropping an unreadable record {key} in the preview cache");
        cache.remove_data(key);
    }
    r
}

/// Keep `r` under `key`.
pub fn write(cache: &PreviewCache, key: Hash128, r: &EyeReport) {
    if let Some(bytes) = encode(r) {
        cache.put_data(key, &bytes);
    }
}

/// Where a job's results are kept: the preview cache and the record's key (`None`: nothing is
/// kept, e.g. a library in memory or a file that can't be looked up).
#[derive(Clone)]
pub struct Store {
    pub cache: Arc<PreviewCache>,
    pub path: String,
}

impl Store {
    /// The record's key with the model in `model_dir`, when the cache keeps records on disk.
    pub fn key(&self, model_dir: &Path) -> Option<Hash128> {
        self.cache.disk()?;
        file_key(&self.path, model_dir)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("lc-eye-store-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    pub(crate) fn report() -> EyeReport {
        let mut crop = Rgba8::new(CROP_W, CROP_H);
        for (i, p) in crop.data.iter_mut().enumerate() {
            let (x, y) = (i % CROP_W, i / CROP_W);
            *p = [(x * 255 / CROP_W) as u8, (y * 255 / CROP_H) as u8, 90, 255];
        }
        let shot = EyeShot {
            face: [0.1, 0.2, 0.3, 0.4],
            center: [0.15, 0.25],
            crop: Arc::new(crop),
            sharpness: 31.5,
            relative: 100.0,
            focus: Focus::Sharp,
            both_eyes: true,
            openness: Some(0.52),
            iris: None,
            contrast: Some(0.2),
            look: Some(EyeLook { sclera: 0.3, socket: 0.6, socket_dark: 0.1, under: 0.7 }),
            eye_width: 44.0,
            score: 0.93,
            main: true,
            landmarks: true,
            lids: Lids::Open,
        };
        let edge = EyeShot { crop: Arc::new(Rgba8::new(0, 0)), lids: Lids::Closed, focus: Focus::Blurry, ..shot.clone() };
        EyeReport {
            photo: PhotoId(0),
            eyes: vec![shot, edge],
            faces: 3,
            no_eyes: 1,
            boxes: vec![[0.1, 0.2, 0.3, 0.4]],
            size: (6000, 4000),
            ms: 812.5,
        }
    }

    /// A record reads back as written (crops within JPEG error), as the photo asked for.
    #[test]
    fn record_roundtrips() {
        let r = report();
        let back = decode(&encode(&r).unwrap(), PhotoId(7)).unwrap();
        assert_eq!(back.photo, PhotoId(7));
        assert_eq!((back.faces, back.no_eyes, back.size, back.ms, back.boxes.clone()), (3, 1, (6000, 4000), 812.5, r.boxes.clone()));
        assert_eq!(back.eyes.len(), 2);
        let (a, b) = (&r.eyes[0], &back.eyes[0]);
        assert_eq!(
            (a.face, a.center, a.sharpness, a.openness, a.iris, a.look, a.lids, a.focus),
            (b.face, b.center, b.sharpness, b.openness, b.iris, b.look, b.lids, b.focus)
        );
        assert_eq!((b.crop.width, b.crop.height), (CROP_W, CROP_H));
        let err = a
            .crop
            .data
            .iter()
            .zip(&b.crop.data)
            .map(|(x, y)| (x[0] as i32 - y[0] as i32).abs().max((x[1] as i32 - y[1] as i32).abs()))
            .max()
            .unwrap();
        assert!(err < 24, "{err}");
        assert_eq!((back.eyes[1].crop.width, back.eyes[1].lids), (0, Lids::Closed));
    }

    /// Cut short, another version, trailing bytes, nonsense: no report, never a panic.
    #[test]
    fn broken_records_are_refused() {
        let good = encode(&report()).unwrap();
        for n in 0..good.len() {
            assert!(decode(&good[..n], PhotoId(1)).is_none(), "cut at {n}");
        }
        let mut other = good.clone();
        other[4] = 99;
        assert!(decode(&other, PhotoId(1)).is_none());
        let mut longer = good.clone();
        longer.push(0);
        assert!(decode(&longer, PhotoId(1)).is_none());
        assert!(decode(b"LCEY\x01\x00\x00\x00\xff\xff\xff\xff", PhotoId(1)).is_none());
        let mut flipped = good;
        for i in (12..flipped.len()).step_by(97) {
            flipped[i] ^= 0x5a;
            let _ = decode(&flipped, PhotoId(1));
        }
    }

    /// The key follows the file: a changed file (size, time) gets another; so does another model.
    #[test]
    fn key_changes_with_the_file_and_the_model() {
        let d = tmp("key");
        let models = d.join("models").join("mediapipe-face-landmarker");
        std::fs::create_dir_all(&models).unwrap();
        std::fs::write(models.join("face_landmarker.task"), b"model").unwrap();
        let f = d.join("a.jpg");
        std::fs::write(&f, b"one").unwrap();
        let fs = f.to_string_lossy().to_string();
        let k1 = file_key(&fs, &models).unwrap();
        assert_eq!(file_key(&fs, &models), Some(k1));
        std::fs::write(&f, b"one, changed").unwrap();
        let k2 = file_key(&fs, &models).unwrap();
        assert_ne!(k1, k2);
        // same size, later time
        let t = std::time::SystemTime::now() + std::time::Duration::from_secs(120);
        std::fs::File::options().write(true).open(&f).unwrap().set_modified(t).unwrap();
        let k3 = file_key(&fs, &models).unwrap();
        assert_ne!(k2, k3);
        std::fs::write(models.join("face_detection_full_range.tflite"), b"far").unwrap();
        assert_ne!(file_key(&fs, &models).unwrap(), k3);
        assert!(file_key(&d.join("gone.jpg").to_string_lossy(), &models).is_none());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Kept in the preview cache; a corrupt record there is dropped and reads as none.
    #[test]
    fn cache_keeps_records_and_drops_corrupt_ones() {
        let d = tmp("cache");
        let cache = PreviewCache::with_disk(1 << 20, &d.join("thumbs"), 1 << 30);
        let key = lcc_preview::hash_bytes(b"photo");
        assert!(read(&cache, key, PhotoId(1)).is_none());
        write(&cache, key, &report());
        assert_eq!(read(&cache, key, PhotoId(1)).unwrap().eyes.len(), 2);
        cache.put_data(key, b"LCEY garbage");
        assert!(read(&cache, key, PhotoId(1)).is_none());
        assert!(cache.get_data(key).is_none(), "the corrupt record is gone");
        let _ = std::fs::remove_dir_all(&d);
    }
}
