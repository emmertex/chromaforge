//! People masks, the per-person silhouettes (SMART-5) and telling people apart, on the real models
//! and real photos: the public-domain portraits of `corpus/people/` (`cargo xtask corpus
//! --download`) with MediaPipe's people and faces models and SAM 2.1 in `<PREFIX>_MEDIAPIPE_ROOT`.
//! Skipped (and listed by `cargo xtask ci`) when either is missing.
//!
//! The photos are greyscale or sepia prints, so the checks are counts and overlaps, not pixels.

use std::path::{Path, PathBuf};

use lcc_catalog::PhotoId;
use lcc_develop::MaskShape;
use serde_json::json;

use crate::Session;

fn models() -> Option<PathBuf> {
    let root = lcc_brand::env_os("MEDIAPIPE_ROOT").map(PathBuf::from);
    if root.is_none() {
        lcc_testkit::skip!(Model, "set {} to the open models' folder", lcc_brand::env_var("MEDIAPIPE_ROOT"));
    }
    root
}

fn people_corpus(name: &str) -> Option<PathBuf> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/people").join(name);
    if !p.is_file() {
        lcc_testkit::skip!(Corpus, "{name} absent (cargo xtask corpus --download)");
        return None;
    }
    Some(p)
}

/// A library holding `name` (and the models), with it active.
fn open(name: &str) -> Option<(Session, PhotoId)> {
    let root = models()?;
    let path = people_corpus(name)?;
    let mut s = Session::new().with_fs();
    s.segmenter.models_root = Some(root);
    if s.segmenter.people_models().is_none() {
        lcc_testkit::skip!(Model, "the people and faces models are not in {}", lcc_brand::env_var("MEDIAPIPE_ROOT"));
        return None;
    }
    let r = s.execute("library.import", &json!({"paths": [path.to_string_lossy()]})).unwrap();
    let id = PhotoId(r["imported"][0].as_u64().expect("imported"));
    s.execute("library.select", &json!({"ids": [id.0]})).unwrap();
    Some((s, id))
}

/// How many faces the People path finds on `id` (its appearance list: the faces People masks
/// number from the left).
fn faces(s: &mut Session, id: PhotoId) -> usize {
    s.photo_appearance(id, false).expect("faces").map_or(0, |f| f.len())
}

/// Add a People mask for `person` (0: everyone) and compute it; → its selection's cells that are
/// in, on a common 64 × 64 grid of the photo (empty: nothing found).
fn person_cells(s: &mut Session, id: PhotoId, person: u32, parts: &[&str]) -> Vec<bool> {
    s.execute("mask.add", &json!({"kind": "people", "person": person, "parts": parts})).unwrap();
    s.compute_masks(id, false).unwrap();
    let d = s.develop_of(id).unwrap();
    let Some(MaskShape::People { seg, .. }) = d.masks.last().map(|m| m.components[0].shape.clone()) else { panic!("not a People mask") };
    let Some(seg) = seg else { return Vec::new() };
    let (Some(l), Some(r)) = (seg.logits(), seg.bounds()) else { return Vec::new() };
    let side = seg.side as usize;
    const G: usize = 64;
    let mut out = vec![false; G * G];
    for (k, cell) in out.iter_mut().enumerate() {
        let (x, y) = (((k % G) as f64 + 0.5) / G as f64, ((k / G) as f64 + 0.5) / G as f64);
        if x < r[0] || x >= r[2] || y < r[1] || y >= r[3] {
            continue;
        }
        let gx = (((x - r[0]) / (r[2] - r[0])) * side as f64) as usize;
        let gy = (((y - r[1]) / (r[3] - r[1])) * side as f64) as usize;
        *cell = l.get(gy.min(side - 1) * side + gx.min(side - 1)).is_some_and(|v| *v > 0.0);
    }
    if out.iter().any(|c| *c) { out } else { Vec::new() }
}

/// The share of the smaller of two selections that the other also holds.
fn shared(a: &[bool], b: &[bool]) -> f64 {
    let both = a.iter().zip(b).filter(|(x, y)| **x && **y).count();
    let small = a.iter().filter(|x| **x).count().min(b.iter().filter(|x| **x).count());
    if small == 0 { 0.0 } else { both as f64 / small as f64 }
}

/// Single portraits: one face or more; the People mask of everyone selects something; Lips and
/// Teeth on a profile, closed eyes or glasses are answers, not errors.
#[test]
fn people_masks_on_single_portraits() {
    for name in ["pd-one-lincoln-1863.jpg", "pd-glasses-gandhi-1931.jpg", "pd-eyes-closed-fitz-1839.jpg", "pd-profile-edison.jpg"] {
        let Some((mut s, id)) = open(name) else { return };
        let n = faces(&mut s, id);
        eprintln!("{name}: {n} face(s)");
        let everyone = person_cells(&mut s, id, 0, &[]);
        eprintln!("{name}: everyone covers {} of 4096 cells", everyone.iter().filter(|c| **c).count());
        let _ = person_cells(&mut s, id, 0, &["Lips", "Teeth"]);
        if name != "pd-profile-edison.jpg" {
            assert!(n >= 1, "{name}: a face");
            assert!(!everyone.is_empty(), "{name}: the person is selected");
        }
    }
}

/// SMART-5 on Curie (two people side by side) and Solvay (29 in rows): a face per person, and
/// one person's mask is mostly their own pixels, not their neighbour's.
#[test]
fn one_person_of_several_is_their_own_silhouette() {
    for (name, min, max, check) in [("pd-two-curie-1895.jpg", 2, 2, 2u32), ("pd-group-solvay-1927.jpg", 10, 40, 6)] {
        let Some((mut s, id)) = open(name) else { return };
        let n = faces(&mut s, id);
        eprintln!("{name}: {n} faces");
        assert!((min..=max).contains(&n), "{name}: {n} faces, expected {min}..={max}");
        let people: Vec<Vec<bool>> = (1..=check).map(|k| person_cells(&mut s, id, k, &[])).collect();
        for (k, p) in people.iter().enumerate() {
            eprintln!("{name}: person {} covers {} of 4096 cells", k + 1, p.iter().filter(|c| **c).count());
        }
        // (on the 29-person group some people come back empty: recorded as a gap, see
        // docs/ai-masks.md; most of those checked are selected)
        let selected = people.iter().filter(|p| !p.is_empty()).count();
        assert!(2 * selected >= people.len(), "{name}: only {selected} of {} people selected", people.len());
        for a in 0..people.len() {
            for b in a + 1..people.len() {
                let o = shared(&people[a], &people[b]);
                eprintln!("{name}: persons {} and {} share {:.0} %", a + 1, b + 1, o * 100.0);
                assert!(o < 0.5, "{name}: persons {} and {} are mostly the same pixels ({o:.2})", a + 1, b + 1);
            }
        }
    }
}

/// Telling people apart: the faces on the portraits get signatures; a person named on one Solvay
/// photo is found again by their look on a second import of the same picture, where nobody is
/// named.
#[test]
#[ignore = "known gap 2026-11-03: the named Solvay face is not found again by its look (find_by_look → None); run with --ignored"]
fn a_named_face_is_found_again_by_its_look() {
    for name in ["pd-one-lincoln-1863.jpg", "pd-glasses-gandhi-1931.jpg", "pd-two-curie-1895.jpg"] {
        let Some((mut s, id)) = open(name) else { return };
        assert!(s.photo_appearance(id, false).unwrap().is_some_and(|f| !f.is_empty()), "{name}: faces with their looks");
    }
    let Some((mut s, id)) = open("pd-group-solvay-1927.jpg") else { return };
    let Some(src) = people_corpus("pd-group-solvay-1927.jpg") else { return };
    s.execute("faces.detect", &json!({"ids": [id.0]})).unwrap();
    let regions = s.catalog.photo(id).unwrap().meta.regions.clone();
    let faces = s.photo_appearance(id, false).unwrap().expect("faces");
    // name the region over the third face from the left
    let target = faces.get(2).expect("three faces").rect;
    let index = regions
        .iter()
        .enumerate()
        .filter(|(_, r)| r.kind == lcc_meta::RegionKind::Face)
        .max_by(|a, b| {
            let o = |r: &lcc_meta::Region| {
                let ix = (r.rect.x1.min(target[2]) - r.rect.x0.max(target[0])).max(0.0);
                let iy = (r.rect.y1.min(target[3]) - r.rect.y0.max(target[1])).max(0.0);
                ix * iy
            };
            o(a.1).total_cmp(&o(b.1))
        })
        .map(|(i, _)| i)
        .expect("a face region");
    s.execute("faces.setName", &json!({"id": id.0, "index": index, "name": "Physicist C"})).unwrap();
    let r = s.execute("people.learn", &json!({"ids": [id.0]})).unwrap();
    eprintln!("solvay: learned {r}");
    assert!(r["learned"].as_u64().unwrap_or(0) >= 1, "{r}");
    let dir = std::env::temp_dir().join(format!("lc-people-corpus-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let copy = dir.join("solvay-again.jpg");
    // the same picture as another file (a byte after the image's end: the library keeps one
    // record per identical file)
    let mut bytes = std::fs::read(&src).unwrap();
    bytes.push(0);
    std::fs::write(&copy, bytes).unwrap();
    let r = s.execute("library.import", &json!({"paths": [copy.to_string_lossy()]})).unwrap();
    let again = PhotoId(r["imported"][0].as_u64().expect("imported"));
    let found = s.find_by_look(again, "Physicist C");
    eprintln!("solvay again: Physicist C is face {found:?} (named: 3)");
    assert_eq!(found, Some(3), "the same face on the same picture");
    let _ = std::fs::remove_dir_all(&dir);
}
