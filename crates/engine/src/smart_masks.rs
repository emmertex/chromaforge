//! Smart masks: masks as recipes, completed on each photo. A preset, a paste or a sync brings
//! masks whose parts depend on the photo — People by name or role (`who`), gradients anchored to
//! the people or the subject (`anchor`), Object and Describe selections without their
//! segmentation. [`Session::complete_recipes`] fills in what can be found without a model (who
//! is where, from the photo's Face regions; the boxes anchors are placed in) and
//! [`Session::compute_masks`] (`masks.recompute`) also runs the segmenter for Object and Describe.
//!
//! People are ordered as the People mask counts them: the n-th person is the n-th face from the
//! left. `who` picks the named Face region's place in that order, so a mask follows "the Bride"
//! from photo to photo; nobody bound to the role means everyone (person 0).
//!
//! Ported from OneSlips' fork (own design there), fitted to our model: Subject, Sky, Background,
//! People and Landscape are evaluated from the per-photo cache at render time, so only the parts
//! above need completing.

use lcc_catalog::PhotoId;
use lcc_develop::recipe::{AnchorTo, Who};
use lcc_develop::{DevelopSettings, MaskShape};

use crate::Session;
use crate::people_smart::Resolved;

/// A face box `[x0, y0, x1, y1]` (normalized, oriented) and the person's name, if any.
type Face = ([f64; 4], Option<String>);

/// The faces on photo `id`, left to right.
fn faces(s: &Session, id: PhotoId) -> Vec<Face> {
    let Some(p) = s.catalog.photo(id) else { return Vec::new() };
    let mut f: Vec<Face> = p
        .meta
        .regions
        .iter()
        .filter(|r| r.kind == lcc_meta::RegionKind::Face)
        .filter_map(|r| {
            let b = lcc_develop::recipe::valid_box([r.rect.x0, r.rect.y0, r.rect.x1, r.rect.y1])?;
            Some((b, r.name.clone().filter(|n| !n.trim().is_empty())))
        })
        .take(256)
        .collect();
    f.sort_by(|a, b| (a.0[0] + a.0[2]).total_cmp(&(b.0[0] + b.0[2])));
    f
}

/// A person's box from their face: about three faces wide, from just above the head down to the
/// bottom of the frame (own estimate: a standing or seated person below their face).
fn body_box(face: [f64; 4]) -> [f64; 4] {
    let (w, h) = (face[2] - face[0], face[3] - face[1]);
    let cx = (face[0] + face[2]) / 2.0;
    [(cx - 1.5 * w).max(0.0), (face[1] - 0.5 * h).max(0.0), (cx + 1.5 * w).min(1.0), 1.0]
}

fn union(boxes: impl Iterator<Item = [f64; 4]>) -> Option<[f64; 4]> {
    boxes.reduce(|a, b| [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])])
}

/// What completing a photo's recipes found.
#[derive(Debug, Default, PartialEq)]
pub struct Completed {
    /// Components changed.
    pub changed: usize,
    /// What couldn't be found, for the user.
    pub notes: Vec<String>,
}

impl Session {
    /// The 1-based place (from the left) of `who` on photo `id`, `0` (everyone) and why when
    /// they aren't found.
    fn person_index(&mut self, id: PhotoId, who: &Who, faces: &[Face]) -> (u32, Option<String>) {
        let name = match self.resolve_who(id, who) {
            Resolved::Person(n) => n,
            Resolved::Everyone(why) => return (0, Some(why)),
        };
        match faces.iter().position(|(_, n)| n.as_deref().is_some_and(|n| n.trim().eq_ignore_ascii_case(name.trim()))) {
            Some(i) => (u32::try_from(i).map_or(0, |i| i.saturating_add(1)), None),
            None => (0, Some(format!("{name} isn't named on this photo, so the mask covers everyone"))),
        }
    }

    /// Fill in `d`'s photo-dependent recipe parts that need no model: `who` → the person's place,
    /// anchor boxes (from the Face regions; the whole image when there are none).
    pub fn complete_recipes(&mut self, id: PhotoId, d: &mut DevelopSettings) -> Completed {
        let faces = faces(self, id);
        let mut out = Completed::default();
        let aspect = self.catalog.photo(id).map_or(1.5, |p| {
            let (w, h) = (f64::from(p.width.max(1)), f64::from(p.height.max(1)));
            if p.develop.orientation.swaps_axes() { h / w } else { w / h }
        });
        for m in &mut d.masks {
            for c in &mut m.components {
                if let MaskShape::People { person, who: Some(who), .. } = &mut c.shape {
                    let who = who.clone();
                    let (i, note) = self.person_index(id, &who, &faces);
                    out.notes.extend(note.map(|n| format!("{}: {n}", m.name)));
                    if *person != i {
                        *person = i;
                        out.changed += 1;
                    }
                }
                if let Some(a) = &mut c.anchor {
                    let b = match &a.to {
                        AnchorTo::People | AnchorTo::Subject => union(faces.iter().map(|f| body_box(f.0))),
                        AnchorTo::Who(w) => {
                            let (i, note) = self.person_index(id, w, &faces);
                            out.notes.extend(note.map(|n| format!("{}: {n}", m.name)));
                            let i = usize::try_from(i).unwrap_or(0);
                            if i == 0 { union(faces.iter().map(|f| body_box(f.0))) } else { faces.get(i - 1).map(|f| body_box(f.0)) }
                        }
                    };
                    let b = b.unwrap_or([0.0, 0.0, 1.0, 1.0]);
                    if a.bounds != Some(b) {
                        a.bounds = Some(b);
                        if let Some(shape) = a.placed(aspect) {
                            c.shape = shape;
                        }
                        out.changed += 1;
                    }
                }
            }
        }
        out.notes.sort();
        out.notes.dedup();
        out
    }

    /// Complete photo `id`'s recipes and run the segmenter for Object and Describe selections
    /// without a segmentation (waiting for it), as one undo step. → what changed and notes.
    pub fn compute_masks(&mut self, id: PhotoId, force: bool) -> crate::Result<Completed> {
        let before = self.develop_of(id).ok_or(lcc_catalog::CatalogError::NoPhoto(id))?;
        let mut d = (*before).clone();
        if force {
            to_recipes(&mut d, None);
        }
        let mut out = self.complete_recipes(id, &mut d);
        let mut model_missing = false;
        for mi in 0..d.masks.len() {
            for ci in 0..d.masks.get(mi).map_or(0, |m| m.components.len()) {
                let Some(shape) = d.masks.get(mi).and_then(|m| m.components.get(ci)).map(|c| c.shape.clone()) else { continue };
                if !shape.stores_seg() || shape.has_seg() || model_missing {
                    continue;
                }
                let name = d.masks.get(mi).map(|m| m.name.clone()).unwrap_or_default();
                let res = match &shape {
                    MaskShape::Object { hint, exclude, .. } => {
                        let clicks: Vec<crate::segment::Click> = hint
                            .iter()
                            .map(|p| crate::segment::Click { at: *p, include: true })
                            .chain(exclude.iter().map(|p| crate::segment::Click { at: *p, include: false }))
                            .take(crate::segment::MAX_CLICKS)
                            .collect();
                        self.segment_clicks(id, &clicks).map(Some)
                    }
                    MaskShape::Prompt { text, .. } => self.segment_text(id, text),
                    _ => continue,
                };
                match res {
                    Ok(seg) => {
                        if let Some(MaskShape::Object { seg: s, .. } | MaskShape::Prompt { seg: s, .. }) =
                            d.masks.get_mut(mi).and_then(|m| m.components.get_mut(ci)).map(|c| &mut c.shape)
                        {
                            *s = seg;
                            out.changed += 1;
                        }
                    }
                    Err(e) => {
                        model_missing = !self.segmenter.installed();
                        out.notes.push(format!("{name}: {e}"));
                    }
                }
            }
        }
        if *before != d {
            let op = self.develop_op(id, d, "Recompute AI Masks").ok_or(lcc_catalog::CatalogError::NoPhoto(id))?;
            self.commit("Recompute AI Masks", op)?;
        }
        Ok(out)
    }
}

/// `d` (a preset, paste or sync just applied to photo `id`) with its recipes completed for that
/// photo ([`Session::complete_recipes`]; Object and Describe wait for `masks.recompute`).
pub(crate) fn completed(s: &mut Session, id: PhotoId, mut d: DevelopSettings) -> DevelopSettings {
    if d.masks.iter().any(|m| m.uses_ai()) {
        let _ = s.complete_recipes(id, &mut d);
    }
    if missing_seg(&d) && s.segmenter.background {
        // computed on the AI worker after this command (`segment_poll`)
        s.segmenter.wanted.push(id);
    }
    d
}

/// `d`'s masks (only mask `only`, when given) back to their recipes: Object and Describe without
/// their segmentation, anchored gradients without their box, to be computed on this photo again.
fn to_recipes(d: &mut DevelopSettings, only: Option<u32>) {
    for m in d.masks.iter_mut().filter(|m| only.is_none_or(|o| o == m.id)) {
        for c in &mut m.components {
            c.shape = c.shape.recipe();
            if let Some(a) = &mut c.anchor {
                a.bounds = None;
            }
        }
    }
}

/// Whether `d` has Object or Describe selections without a segmentation.
fn missing_seg(d: &DevelopSettings) -> bool {
    d.masks.iter().flat_map(|m| &m.components).any(|c| c.shape.stores_seg() && !c.shape.has_seg())
}

impl Session {
    /// In the app (background mode): compute photo `id`'s missing AI selections at the next poll.
    pub(crate) fn want_missing_masks(&mut self, id: PhotoId) {
        if self.segmenter.background && self.develop_of(id).is_some_and(|d| missing_seg(&d)) && !self.segmenter.wanted.contains(&id) {
            self.segmenter.wanted.push(id);
        }
    }

    /// Update AI Mask: mask `mask` of photo `id` (every AI mask without `mask`) computed again from
    /// this photo, replacing what it holds (a copied, synced or stale selection). In the app on the AI
    /// worker ([`Session::compute_masks_later`]), otherwise waited for. An Object selection without
    /// clicks on this photo can't be: it has to be picked here first.
    pub fn update_ai_mask(&mut self, id: PhotoId, mask: Option<u32>) -> crate::Result<(Completed, usize)> {
        let bad = |why: &str| crate::cmd::bad("mask.updateAi", why);
        let before = self.develop_of(id).ok_or(lcc_catalog::CatalogError::NoPhoto(id))?;
        let masks: Vec<&lcc_develop::Mask> = before.masks.iter().filter(|m| mask.is_none_or(|o| o == m.id)).collect();
        if let Some(o) = mask {
            let m = masks.first().ok_or_else(|| bad(&format!("no mask {o}")))?;
            if !m.uses_ai() {
                return Err(bad("the mask has no AI selection"));
            }
            if m.components.iter().any(|c| matches!(&c.shape, MaskShape::Object { hint, .. } if hint.is_empty())) {
                return Err(bad("pick the object on this photo first: click it with the Object tool"));
            }
        }
        let mut d = (*before).clone();
        to_recipes(&mut d, mask);
        if *before != d {
            let op = self.develop_op(id, d, "Update AI Mask").ok_or(lcc_catalog::CatalogError::NoPhoto(id))?;
            self.commit("Update AI Mask", op)?;
        }
        if self.segmenter.background {
            return self.compute_masks_later(id, false);
        }
        let done = self.compute_masks(id, false)?;
        Ok((done, 0))
    }

    /// The app's Recompute AI Masks: the recipes are completed now (from the photo's metadata, no
    /// pixels), the selections are queued on the AI worker as one batch with an activity row.
    pub fn compute_masks_later(&mut self, id: PhotoId, force: bool) -> crate::Result<(Completed, usize)> {
        let before = self.develop_of(id).ok_or(lcc_catalog::CatalogError::NoPhoto(id))?;
        let mut d = (*before).clone();
        if force {
            to_recipes(&mut d, None);
        }
        let mut out = self.complete_recipes(id, &mut d);
        if *before != d {
            let op = self.develop_op(id, d, "Recompute AI Masks").ok_or(lcc_catalog::CatalogError::NoPhoto(id))?;
            self.commit("Recompute AI Masks", op)?;
        }
        let queued = match self.segment_missing_later(id) {
            Ok(n) => n,
            Err(e) => {
                out.notes.push(e);
                0
            }
        };
        Ok((out, queued))
    }
}

#[cfg(test)]
mod tests {
    use lcc_develop::recipe::{Anchor, AnchorTo, Who};
    use lcc_develop::{Mask, MaskComponent, MaskOp, MaskShape};
    use lcc_geom::{Point, Rect};
    use lcc_meta::{Region, RegionKind};
    use serde_json::json;

    use crate::Session;

    fn face(x: f64, name: &str) -> Region {
        Region { rect: Rect { x0: x, y0: 0.2, x1: x + 0.1, y1: 0.35 }, kind: RegionKind::Face, name: Some(name.into()), description: None }
    }

    #[test]
    fn who_and_anchors_follow_the_people_of_each_photo() {
        let mut s = Session::with_demo();
        let id = s.active().expect("demo photo");
        let mut meta = s.catalog.photo(id).expect("photo").meta.clone();
        meta.regions = vec![face(0.6, "Sam"), face(0.2, "Alex")];
        s.commit("faces", lcc_catalog::Op::SetMeta { id, meta: Box::new(meta) }).expect("meta");
        s.execute("people.setRole", &json!({"role": "Bride", "person": "Sam", "scope": {"ids": [id.0]}})).expect("role");
        let comp = |shape, anchor| MaskComponent { anchor, name: None, op: MaskOp::Add, invert: false, shape };
        let mut d = (*s.develop_of(id).expect("develop")).clone();
        d.masks.push(Mask {
            id: 1,
            components: vec![
                comp(MaskShape::People { person: 0, parts: vec!["Clothes".into()], who: Some(Who::Role("Bride".into())) }, None),
                comp(
                    MaskShape::Linear { start: Point::new(0.5, 1.0), end: Point::new(0.5, 0.5) },
                    Some(Anchor {
                        to: AnchorTo::Who(Who::Name("Alex".into())),
                        shape: MaskShape::Linear { start: Point::new(0.5, 1.0), end: Point::new(0.5, 0.0) },
                        bounds: None,
                    }),
                ),
                comp(MaskShape::People { person: 3, parts: vec![], who: Some(Who::Role("Groom".into())) }, None),
            ],
            ..Default::default()
        });
        let done = s.complete_recipes(id, &mut d);
        let comps = &d.masks[0].components;
        // Sam is the second face from the left
        assert!(matches!(comps[0].shape, MaskShape::People { person: 2, .. }), "{:?}", comps[0].shape);
        // no Groom here: everyone, and said so
        assert!(matches!(comps[2].shape, MaskShape::People { person: 0, .. }));
        assert!(done.notes.iter().any(|n| n.contains("Groom")), "{:?}", done.notes);
        // the gradient sits on Alex (left), its top at the top of Alex's box
        let MaskShape::Linear { start, end } = comps[1].shape else { panic!("linear") };
        assert!(start.x < 0.4 && end.y < 0.3 && (start.y - 1.0).abs() < 1e-9, "{start:?} {end:?}");
        // again: nothing left to change
        assert_eq!(s.complete_recipes(id, &mut d).changed, 0);
    }

    #[test]
    fn recompute_without_a_model_reports_instead_of_failing() {
        let mut s = Session::with_demo();
        let id = s.active().expect("demo photo");
        let mut d = (*s.develop_of(id).expect("develop")).clone();
        d.masks.push(Mask {
            id: 1,
            components: vec![MaskComponent {
                anchor: None,
                name: None,
                op: MaskOp::Add,
                invert: false,
                shape: MaskShape::Prompt { text: "the red car".into(), seg: None, detail: vec![], edge: 0.0 },
            }],
            ..Default::default()
        });
        s.set_develop(id, d, "test").expect("set");
        let r = s.execute("masks.recompute", &json!({})).expect("recompute");
        assert!(r["photos"][0]["changed"].as_u64().is_some(), "{r}");
        assert!(s.execute("masks.recompute", &json!({"ids": ["x"]})).is_err());
    }
}
