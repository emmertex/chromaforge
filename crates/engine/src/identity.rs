//! Telling people apart by how they look ([`lcc_segment::identity`]: clothes, hair, face
//! proportions; SAM 2.1's body features when it is installed): every face MediaPipe finds on a
//! photo gets an appearance signature. `people.learn` remembers the signatures of the faces named
//! on photos (`people.json` examples); a People mask by name then finds that person on a photo
//! where nobody named them, by the closest look. Matching only ever picks among the faces on the
//! photo, and a face named as someone else is never taken.
//!
//! Ported from OneSlips' fork (own design there).

use std::collections::HashMap;
use std::sync::Arc;

use lcc_catalog::PhotoId;
use serde_json::Value;

use crate::Session;

/// A face found on a photo: its box (normalized, oriented, uncropped) and appearance signature.
#[derive(Clone, Debug, PartialEq)]
pub struct FoundFace {
    pub rect: [f64; 4],
    pub signature: Value,
}

/// Faces kept per photo look (the segment key), so a second People mask doesn't run the models.
#[derive(Default)]
pub(crate) struct Appearance {
    seen: HashMap<u64, Arc<Vec<FoundFace>>>,
}

#[cfg_attr(not(feature = "sam"), allow(dead_code))]
const KEEP: usize = 512;
/// A face whose look scores at least this (0..1) against a person's examples is taken for them …
pub const PERSON_MATCH: f32 = 0.55;
/// … when it leads the next face on the photo by this share of its score (relative: with fewer
/// cues — a greyscale group in matching suits has no hair or clothes colour to tell — every score
/// is lower, the leader's included).
pub const LOOK_MARGIN: f32 = 0.15;

#[cfg_attr(not(feature = "sam"), allow(dead_code))]
pub(crate) fn parse(extra: &Value) -> Vec<FoundFace> {
    extra
        .as_array()
        .map(|a| {
            a.iter()
                .take(64)
                .filter_map(|f| {
                    let r = f.get("rect")?.as_array()?;
                    let v: Vec<f64> = r.iter().filter_map(Value::as_f64).filter(|v| v.is_finite()).collect();
                    let rect = [*v.first()?, *v.get(1)?, *v.get(2)?, *v.get(3)?];
                    Some(FoundFace { rect, signature: f.get("signature")?.clone() })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// How much two boxes overlap, as a share of the smaller one (a Face region and a detected face
/// box are drawn to different sizes around the same face).
fn overlap(a: [f64; 4], b: [f64; 4]) -> f64 {
    let ix = (a[2].min(b[2]) - a[0].max(b[0])).max(0.0);
    let iy = (a[3].min(b[3]) - a[1].max(b[1])).max(0.0);
    let small = ((a[2] - a[0]).max(0.0) * (a[3] - a[1]).max(0.0)).min((b[2] - b[0]).max(0.0) * (b[3] - b[1]).max(0.0));
    if small > 0.0 { ix * iy / small } else { 0.0 }
}

impl Session {
    pub(crate) fn appearance_key(&self, id: PhotoId) -> Option<u64> {
        self.segment_key_of(id)
    }

    /// The faces of photo `id` with their signatures when they are known (kept from an earlier
    /// look, or computed now when commands wait for the models). In the app they are computed on
    /// the AI worker: `Ok(None)` now, kept when [`Session::segment_poll`] receives them (and
    /// learned from when `learn`).
    pub fn photo_appearance(&mut self, id: PhotoId, learn: bool) -> Result<Option<Arc<Vec<FoundFace>>>, String> {
        let key = self.appearance_key(id).ok_or("no such photo")?;
        if let Some(f) = self.appearance.seen.get(&key) {
            return Ok(Some(f.clone()));
        }
        let face = self
            .segmenter
            .open_model_installed("face")
            .then(|| self.segmenter.open_model_dir("face"))
            .flatten()
            .ok_or("telling people apart needs the MediaPipe faces model: `ai.models.download {\"id\": \"face\", \"acknowledged\": true}`")?;
        let sam2 = self.segmenter.require_sam2().ok();
        self.appearance_compute(id, key, face, sam2, learn)
    }

    #[cfg(feature = "sam")]
    fn appearance_compute(
        &mut self,
        id: PhotoId,
        key: u64,
        face: std::path::PathBuf,
        sam2: Option<std::path::PathBuf>,
        learn: bool,
    ) -> Result<Option<Arc<Vec<FoundFace>>>, String> {
        let kind = crate::segment::FacesKind { face, sam2 };
        match self.segment_faces(id, kind, learn)? {
            None => Ok(None),
            Some(extra) => Ok(Some(self.keep_appearance(key, &extra))),
        }
    }

    #[cfg(not(feature = "sam"))]
    fn appearance_compute(
        &mut self,
        _: PhotoId,
        _: u64,
        _: std::path::PathBuf,
        _: Option<std::path::PathBuf>,
        _: bool,
    ) -> Result<Option<Arc<Vec<FoundFace>>>, String> {
        Err("telling people apart needs a build with the AI models".into())
    }

    #[cfg_attr(not(feature = "sam"), allow(dead_code))]
    pub(crate) fn keep_appearance(&mut self, key: u64, extra: &Value) -> Arc<Vec<FoundFace>> {
        if self.appearance.seen.len() >= KEEP {
            self.appearance.seen.clear();
        }
        let f = Arc::new(parse(extra));
        self.appearance.seen.insert(key, f.clone());
        f
    }

    /// The faces of the photo with a known look, without computing anything.
    #[cfg_attr(not(feature = "sam"), allow(dead_code))]
    pub fn known_appearance(&self, id: PhotoId) -> Option<Arc<Vec<FoundFace>>> {
        self.appearance.seen.get(&self.appearance_key(id)?).cloned()
    }

    /// Remember the look of every face named on photo `id` (its Face regions with a name) in
    /// `people.json`. → how many faces were learned.
    pub fn learn_named_faces(&mut self, id: PhotoId, faces: &[FoundFace]) -> Result<usize, String> {
        let named: Vec<([f64; 4], String)> = self
            .catalog
            .photo(id)
            .map(|p| {
                p.meta
                    .regions
                    .iter()
                    .filter(|r| r.kind == lcc_meta::RegionKind::Face)
                    .filter_map(|r| Some(([r.rect.x0, r.rect.y0, r.rect.x1, r.rect.y1], r.name.clone().filter(|n| !n.trim().is_empty())?)))
                    .collect()
            })
            .unwrap_or_default();
        let mut todo = Vec::new();
        for (rect, name) in named {
            let best = faces.iter().map(|f| (overlap(f.rect, rect), f)).max_by(|a, b| a.0.total_cmp(&b.0));
            if let Some((o, f)) = best
                && o > 0.5
            {
                todo.push((name, crate::people_store::Example { photo: id.0, rect: f.rect, signature: f.signature.clone() }));
            }
        }
        let n = todo.len();
        if n > 0 {
            self.with_people(|st| {
                for (name, ex) in todo {
                    st.add_example(&name, ex);
                }
            })?;
        }
        Ok(n)
    }

    /// Which of the faces on photo `id` (1-based, left to right) looks like `name` by the
    /// examples `people.json` keeps; `None` when no face does clearly, or the look isn't known.
    pub(crate) fn find_by_look(&mut self, id: PhotoId, name: &str) -> Option<u32> {
        #[cfg(feature = "sam")]
        {
            use lcc_segment::identity::{Signature, similarity};
            let known: Vec<Signature> = self
                .people_store()
                .person(name)
                .map(|p| p.examples.iter().filter_map(|e| serde_json::from_value(e.signature.clone()).ok()).collect())
                .unwrap_or_default();
            if known.is_empty() {
                return None;
            }
            let faces = match self.known_appearance(id) {
                Some(f) => f,
                None => self.photo_appearance(id, false).ok().flatten()?,
            };
            // a face named on the photo as someone else is theirs
            let others: Vec<[f64; 4]> = self
                .catalog
                .photo(id)
                .map(|p| {
                    p.meta
                        .regions
                        .iter()
                        .filter(|r| {
                            r.kind == lcc_meta::RegionKind::Face && r.name.as_deref().is_some_and(|n| !n.trim().eq_ignore_ascii_case(name.trim()))
                        })
                        .map(|r| [r.rect.x0, r.rect.y0, r.rect.x1, r.rect.y1])
                        .collect()
                })
                .unwrap_or_default();
            // each face's look against the person's examples: the mean of its two best scores
            let mut looks: Vec<(usize, f32)> = faces
                .iter()
                .enumerate()
                .filter(|(_, f)| !others.iter().any(|o| overlap(*o, f.rect) > 0.5))
                .filter_map(|(i, f)| {
                    let probe = serde_json::from_value::<Signature>(f.signature.clone()).ok()?;
                    let mut sc: Vec<f32> = known.iter().map(|k| similarity(&probe, k)).filter(|v| v.is_finite()).collect();
                    sc.sort_by(|a, b| b.total_cmp(a));
                    let top: Vec<f32> = sc.into_iter().take(2).collect();
                    (!top.is_empty()).then(|| (i, top.iter().sum::<f32>() / top.len() as f32))
                })
                .collect();
            looks.sort_by(|a, b| b.1.total_cmp(&a.1));
            let best = looks.first().copied().filter(|(_, l)| *l >= PERSON_MATCH && *l - looks.get(1).map_or(0.0, |n| n.1) >= LOOK_MARGIN * *l);
            best.and_then(|(i, _)| u32::try_from(i).ok()).map(|i| i.saturating_add(1))
        }
        #[cfg(not(feature = "sam"))]
        {
            let _ = (id, name);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A People mask by name on a photo where nobody is named so takes the face that looks like
    /// them; a face named as someone else is never taken; learning writes `people.json` examples.
    #[cfg(feature = "sam")]
    #[test]
    fn a_person_not_named_on_the_photo_is_found_by_their_look() {
        use lcc_develop::recipe::Who;
        use serde_json::json;
        let mut s = Session::with_demo();
        let vis: Vec<PhotoId> = s.visible_cloned().iter().take(2).copied().collect();
        let (a, b) = (vis[0], vis[1]);
        let white = json!({"clothes": [0.95, 0.0, 0.0], "hair": [0.4, 0.05, 0.1]});
        let dark = json!({"clothes": [0.1, 0.0, 0.0], "hair": [0.1, 0.0, 0.0]});
        let face = |x: f64, sig: &Value| json!({"rect": [x, 0.2, x + 0.1, 0.35], "signature": sig});
        // photo a: Sam (white) and Alex (dark) are named
        crate::tests_face_recognize::set_regions(
            &mut s,
            a,
            vec![crate::tests_face_recognize::region(0.1, Some("Sam")), crate::tests_face_recognize::region(0.6, Some("Alex"))],
        );
        let ka = s.appearance_key(a).unwrap();
        let fa = s.keep_appearance(ka, &json!([face(0.12, &white), face(0.62, &dark)]));
        assert_eq!(s.learn_named_faces(a, &fa).unwrap(), 2);
        assert_eq!(s.people_store().person("sam").map(|p| p.examples.len()), Some(1));
        // photo b: nobody named; Alex on the left this time, Sam on the right
        let kb = s.appearance_key(b).unwrap();
        s.keep_appearance(kb, &json!([face(0.1, &dark), face(0.6, &white)]));
        assert_eq!(s.find_by_look(b, "Sam"), Some(2));
        assert_eq!(s.find_by_look(b, "alex"), Some(1));
        assert_eq!(s.find_by_look(b, "Kim"), None, "nobody learned as Kim");
        // the complete recipe takes the place found by the look
        let mut d = (*s.develop_of(b).unwrap()).clone();
        let mut m = lcc_develop::Mask { id: 1, name: "Sam".into(), ..Default::default() };
        m.components.push(lcc_develop::MaskComponent {
            name: None,
            anchor: None,
            op: lcc_develop::MaskOp::Add,
            invert: false,
            // computed for everyone before the looks were known
            shape: lcc_develop::MaskShape::People {
                person: 0,
                parts: vec!["Clothes".into()],
                who: Some(Who::Name("Sam".into())),
                seg: Some(lcc_develop::SegMask::from_logits(8, &[4.0; 64])),
            },
        });
        d.masks.push(m);
        let out = s.complete_recipes(b, &mut d);
        assert!(
            matches!(d.masks[0].components[0].shape, lcc_develop::MaskShape::People { person: 2, seg: None, .. }),
            "{:?} {:?}",
            d.masks[0].components[0].shape,
            out.notes
        );
        // a face named as someone else on the photo is theirs
        crate::tests_face_recognize::set_regions(&mut s, b, vec![crate::tests_face_recognize::region(0.6, Some("Kim"))]);
        let kb = s.appearance_key(b).unwrap();
        s.keep_appearance(kb, &json!([face(0.1, &dark), face(0.6, &white)]));
        assert_eq!(s.find_by_look(b, "Sam"), None);
    }

    /// With the real MediaPipe models (`<PREFIX>_MEDIAPIPE_ROOT`: the folder holding
    /// mediapipe-face-landmarker and mediapipe-selfie-multiclass), learning runs on the demo
    /// photos (no faces there) without an error.
    #[cfg(feature = "sam")]
    #[test]
    fn learning_runs_with_the_real_models() {
        let Some(root) = lcc_brand::env_os("MEDIAPIPE_ROOT").map(std::path::PathBuf::from) else {
            lcc_testkit::skip!(Model, "set {} to the open models' folder", lcc_brand::env_var("MEDIAPIPE_ROOT"));
            return;
        };
        let mut s = Session::with_demo();
        s.segmenter.models_root = Some(root);
        let r = s.execute("people.learn", &serde_json::json!({})).unwrap();
        assert_eq!(r["learned"], 0, "{r}");
        let id = s.active().unwrap();
        assert!(s.known_appearance(id).is_some(), "the look is kept");
    }

    #[test]
    fn hostile_face_lists_parse_to_what_is_valid() {
        assert!(parse(&Value::Null).is_empty());
        assert!(parse(&serde_json::json!([{"rect": [0.1, 0.2]}, {"rect": "x"}, 7])).is_empty());
        let f = parse(&serde_json::json!([{"rect": [0.1, 0.2, 0.3, 0.4], "signature": {}}]));
        assert_eq!(f.len(), 1);
    }
}
