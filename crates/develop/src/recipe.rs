//! Mask recipes: masks as presets carry them — what to select, never the pixels a model found on
//! another photo. A recipe is an ordinary [`Mask`] whose AI components have no segmentation
//! ([`Mask::strip_ai`]); each photo computes its own when the mask is applied. Two additions make
//! recipes say more than "the sky":
//!
//! - [`Who`]: People by name ("Sam") or by role in the shoot ("Bride"), resolved per photo;
//! - [`Anchor`]: a gradient placed relative to a selection's box ("from the bottom of the
//!   couple"), resolved into the component's shape on each photo.
//!
//! Ported from OneSlips' fork (own design there). Subject, Sky, Background, People and
//! Landscape keep no pixels in our model (the segmenter's per-photo cache holds them), so
//! only Object and Describe selections carry a segmentation to drop.

use lcc_geom::Point;
use serde::{Deserialize, Serialize};

use crate::{Mask, MaskShape};

/// Who a People selection is about.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Who {
    /// A person by name, the same across the catalog ("Sam").
    Name(String),
    /// A role in the shoot ("Bride", "Groom"): the person tagged with it where the photo is.
    Role(String),
}

impl Who {
    /// The name or role as written.
    pub fn text(&self) -> &str {
        match self {
            Who::Name(s) | Who::Role(s) => s,
        }
    }

    /// For a mask's name: "Bride", "Sam".
    pub fn label(&self) -> String {
        self.text().trim().to_string()
    }
}

/// The aspect ratio a recipe's gradients are written for when no photo is at hand (3:2).
pub const RECIPE_ASPECT: f64 = 1.5;

/// What an anchored gradient is placed relative to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AnchorTo {
    /// The subject's box (BiRefNet, else the people).
    Subject,
    /// The box around everyone in the photo (MediaPipe people).
    People,
    /// One person's box.
    Who(Who),
}

/// A gradient in the frame of a selection's box: `shape` (Linear or Radial) has its points in
/// 0..1 of the box (0,0 its top left, 1,1 its bottom right), radial radii as fractions of the
/// box's width / height. `bounds` is the box found on this photo (normalized image coordinates),
/// filled in when the photo's AI masks are computed; the component's own shape is `shape` placed
/// through it (the whole image while none is known).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Anchor {
    pub to: AnchorTo,
    pub shape: MaskShape,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bounds: Option<[f64; 4]>,
}

/// A sane box: finite, inside 0..1, not empty.
pub fn valid_box(b: [f64; 4]) -> Option<[f64; 4]> {
    let c = b.map(|v| if v.is_finite() { v.clamp(0.0, 1.0) } else { f64::NAN });
    (c.iter().all(|v| v.is_finite()) && c[2] - c[0] > 1e-4 && c[3] - c[1] > 1e-4).then_some(c)
}

impl Anchor {
    /// The gradient placed through `bounds` (the whole image when unknown) on an image of
    /// `aspect` = width / height (oriented). `None` when `shape` isn't a gradient.
    pub fn placed(&self, aspect: f64) -> Option<MaskShape> {
        let b = self.bounds.and_then(valid_box).unwrap_or([0.0, 0.0, 1.0, 1.0]);
        let map = |p: Point| Point::new(b[0] + p.x.clamp(-2.0, 3.0) * (b[2] - b[0]), b[1] + p.y.clamp(-2.0, 3.0) * (b[3] - b[1]));
        match &self.shape {
            MaskShape::Linear { start, end } => Some(MaskShape::Linear { start: map(*start), end: map(*end) }),
            MaskShape::Radial { center, rx, ry, angle, feather, invert } => {
                // radii are long-edge units in a Radial shape: a box fraction of the width is
                // (box width × width / long edge)
                let a = if aspect.is_finite() && aspect > 0.0 { aspect.clamp(0.05, 20.0) } else { 1.0 };
                let (wf, hf) = if a >= 1.0 { (1.0, 1.0 / a) } else { (a, 1.0) };
                let fin = |v: f64| if v.is_finite() { v.clamp(0.0, 4.0) } else { 0.0 };
                Some(MaskShape::Radial {
                    center: map(*center),
                    rx: fin(*rx) * (b[2] - b[0]) * wf,
                    ry: fin(*ry) * (b[3] - b[1]) * hf,
                    angle: *angle,
                    feather: *feather,
                    invert: *invert,
                })
            }
            _ => None,
        }
    }
}

/// A component's kind in words ("Sky", "Linear Gradient", "Groom - Clothes, Hair").
pub fn shape_name(shape: &MaskShape) -> String {
    match shape {
        MaskShape::Brush { .. } => "Brush".into(),
        MaskShape::Linear { .. } => "Linear Gradient".into(),
        MaskShape::Radial { .. } => "Radial Gradient".into(),
        MaskShape::ColorRange { .. } => "Color Range".into(),
        MaskShape::LuminanceRange { .. } => "Luminance Range".into(),
        MaskShape::DepthRange { .. } => "Depth Range".into(),
        MaskShape::Subject => "Subject".into(),
        MaskShape::Sky => "Sky".into(),
        MaskShape::Background => "Background".into(),
        MaskShape::Object { .. } => "Object".into(),
        MaskShape::Prompt { text, .. } => format!("“{}”", text.trim()),
        MaskShape::Landscape { class, .. } => {
            let c = match class.as_str() {
                "naturalGround" | "natural ground" => "Natural Ground",
                "artificialGround" | "artificial ground" => "Artificial Ground",
                "sky" => "Sky",
                "water" => "Water",
                "vegetation" => "Vegetation",
                "mountains" => "Mountains",
                "architecture" => "Architecture",
                other => other,
            };
            c.to_string()
        }
        MaskShape::People { person, parts, who, .. } => {
            let who = match who {
                Some(w) => w.label(),
                None if *person > 0 => format!("Person {person}"),
                None => "People".into(),
            };
            let parts: Vec<&str> = parts
                .iter()
                .map(String::as_str)
                .filter(|p| !matches!(p.to_ascii_lowercase().as_str(), "person" | "entire person" | "entireperson"))
                .collect();
            if parts.is_empty() { who } else { format!("{who} - {}", parts.join(", ")) }
        }
    }
}

/// A mask's name from what it selects: its first component, then the others with their operation
/// ("Background − Linear Gradient").
pub fn content_name(m: &Mask) -> String {
    let mut out = String::new();
    for (k, c) in m.components.iter().enumerate() {
        let n = shape_name(&c.shape);
        let inv = if c.invert { " (inverted)" } else { "" };
        if k == 0 {
            out.push_str(&format!("{n}{inv}"));
        } else {
            let op = match c.op {
                crate::MaskOp::Add => "+",
                crate::MaskOp::Subtract => "−",
                crate::MaskOp::Intersect => "∩",
            };
            out.push_str(&format!(" {op} {n}{inv}"));
        }
        if out.chars().count() > 80 {
            break;
        }
    }
    if out.is_empty() { "Mask".into() } else { out }
}

/// Whether `name` is a placeholder ("Mask 3", "Mask") that an automatic name may replace.
pub fn generic_name(name: &str) -> bool {
    let n = name.trim();
    n.is_empty() || n == "Mask" || n.strip_prefix("Mask ").is_some_and(|d| !d.is_empty() && d.chars().all(|c| c.is_ascii_digit()))
}

/// A preset's name without a leading catalogue code and trailing filler ("P10 Sun Kissed" → "Sun
/// Kissed", "P5 Enhance Portrait......" → "Enhance Portrait").
pub fn short_preset_name(name: &str) -> String {
    let n = name.trim().trim_end_matches(['.', ' ', '…']);
    let mut words = n.split_whitespace();
    let first = words.next().unwrap_or_default();
    let code = first.len() <= 5 && first.chars().any(|c| c.is_ascii_digit()) && first.chars().all(|c| c.is_ascii_alphanumeric());
    let rest: Vec<&str> = words.collect();
    if code && !rest.is_empty() { rest.join(" ") } else { n.to_string() }
}

impl MaskShape {
    /// Whether a model computes it per photo (Subject, Sky, Background, People, Landscape,
    /// Object, Describe).
    pub fn is_ai(&self) -> bool {
        matches!(
            self,
            MaskShape::Subject
                | MaskShape::Sky
                | MaskShape::Background
                | MaskShape::People { .. }
                | MaskShape::Landscape { .. }
                | MaskShape::Object { .. }
                | MaskShape::Prompt { .. }
        )
    }

    /// Whether it holds a segmentation computed for one photo (Object and Describe keep theirs in
    /// the mask; the other AI selections come from the segmenter's per-photo cache).
    pub fn has_seg(&self) -> bool {
        match self {
            MaskShape::Object { seg, detail, .. } | MaskShape::Prompt { seg, detail, .. } => seg.is_some() || !detail.is_empty(),
            _ => false,
        }
    }

    /// Whether a computed segmentation is stored in the shape (Object, Describe).
    pub fn stores_seg(&self) -> bool {
        matches!(self, MaskShape::Object { .. } | MaskShape::Prompt { .. })
    }

    /// The same selection without what a model computed for one photo.
    pub fn recipe(&self) -> MaskShape {
        let mut s = self.clone();
        if let MaskShape::Object { seg, detail, .. } | MaskShape::Prompt { seg, detail, .. } = &mut s {
            *seg = None;
            detail.clear();
        }
        s
    }
}

impl Mask {
    /// Drop every segmentation and anchor box computed for one photo, keeping the recipe (what
    /// to select, how, and the adjustments). Anchored gradients go back to their recipe frame.
    pub fn strip_ai(&mut self, aspect: f64) {
        for c in &mut self.components {
            c.shape = c.shape.recipe();
            if let Some(a) = &mut c.anchor {
                a.bounds = None;
                if let Some(s) = a.placed(aspect) {
                    c.shape = s;
                }
            }
        }
    }

    /// Whether its AI parts need computing on this photo (a component without a segmentation,
    /// or an anchored gradient without its box).
    pub fn needs_compute(&self) -> bool {
        self.components.iter().any(|c| (c.shape.stores_seg() && !c.shape.has_seg()) || c.anchor.as_ref().is_some_and(|a| a.bounds.is_none()))
    }

    /// Whether it uses an AI selection at all.
    pub fn uses_ai(&self) -> bool {
        self.components.iter().any(|c| c.shape.is_ai() || c.anchor.is_some())
    }

    /// The same recipe as `other` (ignoring ids, computed segmentations and anchor boxes).
    pub fn same_recipe(&self, other: &Mask) -> bool {
        let strip = |m: &Mask| {
            let mut m = m.clone();
            m.id = 0;
            m.preset = None;
            m.strip_ai(1.5);
            m
        };
        strip(self) == strip(other)
    }

    /// The same selection as `other`: the same components (by [`component_key`]) and inversion,
    /// whatever the adjustments, names and computed parts.
    fn same_selection(&self, other: &Mask) -> bool {
        self.invert == other.invert
            && self.components.len() == other.components.len()
            && self.components.iter().zip(&other.components).all(|(a, b)| component_key(a) == component_key(b))
    }

    /// The mask as it travels to another photo: its AI components as recipes (computed there) and
    /// an Object's clicks dropped (they point at this photo's pixels: the object is picked again
    /// on each photo).
    pub fn for_other_photo(&mut self, aspect: f64) {
        self.strip_ai(aspect);
        for c in &mut self.components {
            clear_clicks(c);
        }
    }
}

/// Drop an Object component's clicks (see [`Mask::for_other_photo`]).
fn clear_clicks(c: &mut crate::MaskComponent) {
    if let MaskShape::Object { hint, exclude, .. } = &mut c.shape {
        hint.clear();
        exclude.clear();
    }
}

/// What a component selects, for matching it across photos: its recipe, without what was
/// computed for one photo (segmentations, anchor boxes) and without an Object's clicks (each
/// photo has its own).
fn component_key(c: &crate::MaskComponent) -> crate::MaskComponent {
    let mut c = c.clone();
    c.shape = c.shape.recipe();
    if let Some(a) = &mut c.anchor {
        a.bounds = None;
        // the placed shape follows from the anchor
        c.shape = MaskShape::Linear { start: Point::new(0.0, 0.0), end: Point::new(0.0, 0.0) };
    }
    clear_clicks(&mut c);
    c
}

/// The other photo's counterpart of mask `m` among `masks`: the same id with the same selection
/// or name, else one with the same selection and name (a mask Auto Sync carried over earlier).
fn counterpart(m: &Mask, masks: &[Mask]) -> Option<usize> {
    masks
        .iter()
        .position(|t| t.id == m.id && (t.same_selection(m) || t.name == m.name))
        .or_else(|| masks.iter().position(|t| t.same_selection(m) && t.name == m.name))
}

/// The fields of `new` that differ from `old`, set on `target` (each a whole value: an
/// adjustment slider, a curve), so the other photo keeps the rest of its own.
fn carry_fields<T: Serialize + serde::de::DeserializeOwned + Clone>(old: &T, new: &T, target: &T) -> T {
    let (Ok(serde_json::Value::Object(o)), Ok(serde_json::Value::Object(n)), Ok(serde_json::Value::Object(mut t))) =
        (serde_json::to_value(old), serde_json::to_value(new), serde_json::to_value(target))
    else {
        return new.clone();
    };
    for (k, v) in &n {
        if o.get(k) != Some(v) {
            t.insert(k.clone(), v.clone());
        }
    }
    for k in o.keys() {
        if !n.contains_key(k) {
            t.remove(k);
        }
    }
    serde_json::from_value(serde_json::Value::Object(t)).unwrap_or_else(|_| new.clone())
}

/// Auto Sync of a masks edit on one photo (`old` → `new`) onto another photo's `target` masks
/// (whose image is `aspect` = width / height, oriented), merged rather than copied over:
///
/// - a mask added is added (its AI parts as recipes, computed on that photo; an Object without
///   clicks, picked again there);
/// - a mask changed changes its counterpart there: only the settings that changed, and only the
///   components whose recipe changed are replaced (as recipes), so the other photo keeps its own
///   computed selections; a change of computed parts only (an AI mask recomputed) carries nothing;
/// - a mask deleted deletes its counterpart;
/// - the other photo's own masks stay.
///
/// Returns the merged masks and whether any component now needs computing on that photo.
pub fn sync_masks(old: &[Mask], new: &[Mask], target: &[Mask], aspect: f64) -> (Vec<Mask>, bool) {
    let mut out = target.to_vec();
    let mut compute = false;
    for o in old.iter().filter(|o| !new.iter().any(|n| n.id == o.id)) {
        if let Some(k) = counterpart(o, &out) {
            out.remove(k);
        }
    }
    for n in new {
        let old_one = old.iter().find(|o| o.id == n.id);
        if old_one == Some(n) {
            continue;
        }
        let at = old_one.and_then(|o| counterpart(o, &out));
        let (Some(o), Some(k)) = (old_one, at) else {
            // new here (or its counterpart was deleted): added as a recipe
            let mut m = n.clone();
            m.for_other_photo(aspect);
            if out.iter().any(|t| t.id == m.id) {
                m.id = out.iter().map(|t| t.id).max().unwrap_or(0).saturating_add(1);
            }
            compute |= m.needs_compute();
            out.push(m);
            continue;
        };
        let Some(t) = out.get_mut(k) else { continue };
        let mut m = t.clone();
        if o.name != n.name {
            m.name = n.name.clone();
        }
        if o.visible != n.visible {
            m.visible = n.visible;
        }
        if o.invert != n.invert {
            m.invert = n.invert;
        }
        if o.refine != n.refine {
            m.refine = n.refine;
        }
        if o.preset != n.preset {
            m.preset = n.preset.clone();
        }
        if o.adjust != n.adjust {
            m.adjust = carry_fields(&o.adjust, &n.adjust, &t.adjust);
        }
        if o.curve != n.curve {
            m.curve = n.curve.clone();
        }
        if o.point_colors != n.point_colors {
            m.point_colors = n.point_colors.clone();
        }
        if o.mixer != n.mixer {
            m.mixer = n.mixer;
        }
        if o.feather != n.feather {
            m.feather = n.feather;
        }
        let keys_old: Vec<_> = o.components.iter().map(component_key).collect();
        let keys_new: Vec<_> = n.components.iter().map(component_key).collect();
        if keys_old != keys_new {
            // each component the other photo already has (the same recipe) keeps its own
            // computed selection; the rest come over as recipes
            let mut own: Vec<Option<crate::MaskComponent>> = t.components.iter().cloned().map(Some).collect();
            m.components = n
                .components
                .iter()
                .zip(&keys_new)
                .map(|(c, key)| {
                    let reuse = own.iter_mut().find(|x| x.as_ref().is_some_and(|x| component_key(x) == *key)).and_then(Option::take);
                    reuse.unwrap_or_else(|| {
                        let mut one = Mask { components: vec![c.clone()], ..Mask::default() };
                        one.for_other_photo(aspect);
                        let c = one.components.into_iter().next().unwrap_or_else(|| c.clone());
                        compute |= (c.shape.is_ai() && !c.shape.has_seg()) || c.anchor.as_ref().is_some_and(|a| a.bounds.is_none());
                        c
                    })
                })
                .collect();
        }
        *t = m;
    }
    (out, compute)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MaskComponent, MaskOp, SegMask};

    fn seg() -> SegMask {
        SegMask::from_logits(4, &[1.0; 16])
    }

    #[test]
    fn recipes_drop_segmentations_and_keep_the_selection() {
        let s = MaskShape::People { person: 0, parts: vec!["Clothes".into()], who: Some(Who::Role("Groom".into())) };
        assert!(s.is_ai() && !s.has_seg());
        assert!(matches!(s.recipe(), MaskShape::People { who: Some(Who::Role(ref g)), .. } if g == "Groom"));
        let o = MaskShape::Object { hint: vec![Point::new(0.5, 0.5)], exclude: vec![], seg: Some(seg()), detail: vec![seg()], edge: 0.0 };
        assert!(o.has_seg() && !o.recipe().has_seg());
        let lin = MaskShape::Linear { start: Point::new(0.0, 0.0), end: Point::new(1.0, 1.0) };
        assert!(!lin.is_ai() && lin.recipe() == lin);
    }

    #[test]
    fn anchored_gradients_follow_the_box() {
        let a = Anchor {
            to: AnchorTo::People,
            shape: MaskShape::Linear { start: Point::new(0.5, 1.0), end: Point::new(0.5, 0.5) },
            bounds: Some([0.2, 0.4, 0.6, 0.8]),
        };
        let MaskShape::Linear { start, end } = a.placed(1.5).unwrap() else { panic!() };
        assert!((start.x - 0.4).abs() < 1e-9 && (start.y - 0.8).abs() < 1e-9);
        assert!((end.y - 0.6).abs() < 1e-9);
        // no box yet: the whole image is the frame
        let none = Anchor { bounds: None, ..a.clone() };
        let MaskShape::Linear { start, .. } = none.placed(1.5).unwrap() else { panic!() };
        assert!((start.y - 1.0).abs() < 1e-9);
        // hostile boxes and values stay finite
        for b in [[f64::NAN, 0.0, 1.0, 1.0], [0.5, 0.5, 0.5, 0.5], [2.0, -3.0, 9.0, 1e300]] {
            let h = Anchor { bounds: Some(b), ..a.clone() };
            let MaskShape::Linear { start, end } = h.placed(f64::INFINITY).unwrap() else { panic!() };
            assert!(start.x.is_finite() && end.y.is_finite());
        }
        let r = Anchor {
            to: AnchorTo::Subject,
            shape: MaskShape::Radial { center: Point::new(0.5, 0.5), rx: 0.5, ry: f64::NAN, angle: 0.0, feather: 50.0, invert: false },
            bounds: Some([0.0, 0.0, 0.5, 1.0]),
        };
        let MaskShape::Radial { rx, ry, .. } = r.placed(2.0).unwrap() else { panic!() };
        assert!((rx - 0.25).abs() < 1e-9 && ry == 0.0);
        let not_gradient = Anchor { shape: MaskShape::Sky, ..a };
        assert!(not_gradient.placed(1.0).is_none());
    }

    #[test]
    fn masks_know_what_they_need_and_compare_by_recipe() {
        let comp = |shape| MaskComponent { anchor: None, name: None, op: MaskOp::Add, invert: false, shape };
        let obj = MaskShape::Object { hint: vec![Point::new(0.5, 0.5)], exclude: vec![], seg: Some(seg()), detail: vec![], edge: 0.0 };
        let mut m = Mask { components: vec![comp(obj)], ..Default::default() };
        assert!(!m.needs_compute() && m.uses_ai());
        let mut other = m.clone();
        other.id = 7;
        other.strip_ai(1.0);
        assert!(other.needs_compute());
        assert!(m.same_recipe(&other));
        m.components.push(comp(MaskShape::LuminanceRange { lo: 0.1, hi: 0.9, lo_feather: 0.1, hi_feather: 0.1 }));
        assert!(!m.same_recipe(&other));
        let json = serde_json::to_value(&other).unwrap();
        assert!(json.pointer("/components/0/shape/seg").is_none() && json.get("preset").is_none());
    }

    #[test]
    fn masks_are_named_after_what_they_select() {
        let comp = |op, shape| MaskComponent { anchor: None, name: None, op, invert: false, shape };
        let m = Mask {
            components: vec![
                comp(MaskOp::Add, MaskShape::People { person: 0, parts: vec!["Clothes".into()], who: Some(Who::Role("Groom".into())) }),
                comp(MaskOp::Subtract, MaskShape::Linear { start: Point::new(0.0, 0.0), end: Point::new(1.0, 1.0) }),
            ],
            ..Default::default()
        };
        assert_eq!(content_name(&m), "Groom - Clothes − Linear Gradient");
        assert_eq!(content_name(&Mask::default()), "Mask");
        assert!(generic_name("Mask 3") && generic_name(" ") && !generic_name("Mask three") && !generic_name("Sky"));
        assert_eq!(short_preset_name("P10 Sun Kissed"), "Sun Kissed");
        assert_eq!(short_preset_name("P5 Enhance Portrait......"), "Enhance Portrait");
        assert_eq!(short_preset_name("2026"), "2026");
    }

    #[test]
    fn who_and_anchor_serialize_plainly() {
        let w: Who = serde_json::from_value(serde_json::json!({"role": "Bride"})).unwrap();
        assert_eq!(w, Who::Role("Bride".into()));
        let s: MaskShape =
            serde_json::from_value(serde_json::json!({"kind": "people", "person": 0, "parts": ["Clothes"], "who": {"name": "Jack"}})).unwrap();
        assert!(matches!(s, MaskShape::People { who: Some(Who::Name(ref n)), .. } if n == "Jack"));
        let a: AnchorTo = serde_json::from_value(serde_json::json!({"who": {"role": "Groom"}})).unwrap();
        assert_eq!(a, AnchorTo::Who(Who::Role("Groom".into())));
        let a: AnchorTo = serde_json::from_value(serde_json::json!("people")).unwrap();
        assert_eq!(a, AnchorTo::People);
    }
}
