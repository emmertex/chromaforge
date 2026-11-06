//! The values a photo takes for each Metadata filter column (issue #218, #777), and the column picks the filter
//! holds for the fields it has no field of its own for: file format, the IPTC text fields, aspect ratio, develop
//! preset, AI tools, and "no value" for every field ([`FacetPick`]).

use serde::{Deserialize, Serialize};

use crate::{Catalog, Flag, MediaKind, Photo};

/// Every field a Metadata column can show: camera to year have filter fields of their own (only their "no value"
/// pick goes through [`crate::Filter::facets`]), the rest are picked through it.
pub const FIELDS: [&str; 23] = [
    "camera",
    "lens",
    "keyword",
    "person",
    "label",
    "flag",
    "rating",
    "kind",
    "year",
    "iso",
    "focal",
    "format",
    "title",
    "headline",
    "caption",
    "creator",
    "copyright",
    "location",
    "city",
    "country",
    "aspect",
    "preset",
    "ai",
];

/// Whether `field` is a Metadata column field.
pub fn is_field(field: &str) -> bool {
    FIELDS.contains(&field)
}

/// Every Metadata column field, in menu order.
pub fn all_fields() -> impl Iterator<Item = &'static str> {
    FIELDS.into_iter()
}

/// A Metadata column's pick on one field: photos having any of `values` (spelled as [`values`] gives them, any
/// case), or with `none`, also photos without a value for it. Empty values and no `none`: no constraint.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct FacetPick {
    pub field: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub none: bool,
}

impl FacetPick {
    pub fn is_empty(&self) -> bool {
        self.values.is_empty() && !self.none
    }

    /// Whether photo `p` passes this pick.
    pub fn matches(&self, p: &Photo, cat: &Catalog) -> bool {
        if self.is_empty() {
            return true;
        }
        let have = values(p, cat, &self.field);
        (self.none && have.is_empty()) || have.iter().any(|h| self.values.iter().any(|v| v.eq_ignore_ascii_case(h)))
    }
}

pub fn flag_name(f: Flag) -> &'static str {
    match f {
        Flag::Pick => "pick",
        Flag::Reject => "reject",
        Flag::None => "none",
    }
}

pub fn kind_name(k: MediaKind) -> &'static str {
    match k {
        MediaKind::Image => "image",
        MediaKind::Raw => "raw",
        MediaKind::Video => "video",
    }
}

/// The values `field` takes for photo `p` (several for keywords, people, presets and AI tools; none when it has
/// no value).
pub fn values(p: &Photo, cat: &Catalog, field: &str) -> Vec<String> {
    let text = |t: &str| if t.trim().is_empty() { Vec::new() } else { vec![t.trim().to_string()] };
    match field {
        "camera" => text(&p.meta.camera),
        "lens" => text(&p.meta.lens),
        "keyword" => p.meta.keywords.iter().filter(|k| !k.trim().is_empty()).cloned().collect(),
        "person" => p.people().into_iter().map(str::to_string).collect(),
        "label" => p.label.map(|l| cat.label_name(l)).into_iter().collect(),
        "flag" => vec![flag_name(p.flag).to_string()],
        "rating" => vec![p.rating.to_string()],
        "kind" => vec![kind_name(p.kind).to_string()],
        "year" => {
            p.captured.as_deref().and_then(|c| c.get(..4)).filter(|y| y.chars().all(|c| c.is_ascii_digit())).map(str::to_string).into_iter().collect()
        }
        "iso" => p.meta.iso.map(|i| i.to_string()).into_iter().collect(),
        "focal" => p.meta.focal_mm.filter(|f| f.is_finite() && *f > 0.0).map(|f| format!("{}", (f * 10.0).round() / 10.0)).into_iter().collect(),
        "format" => text(&p.format.to_uppercase()),
        "title" => text(&p.meta.title),
        "headline" => text(&p.meta.headline),
        "caption" => text(&p.meta.caption),
        "creator" => text(&p.meta.creator),
        "copyright" => text(&p.meta.copyright),
        "location" => text(&p.meta.location),
        "city" => text(&p.meta.city),
        "country" => text(&p.meta.country),
        "aspect" => aspect(p).into_iter().collect(),
        "preset" => presets(p),
        "ai" => ai_tools(p),
        _ => Vec::new(),
    }
}

/// The named ratios an aspect is shown as, width first (a portrait photo turns them round).
const RATIOS: [(u32, u32); 10] = [(1, 1), (5, 4), (4, 3), (7, 5), (3, 2), (16, 10), (5, 3), (16, 9), (2, 1), (3, 1)];

/// The photo's aspect ratio as developed (its crop and orientation): a common ratio (`3:2`, `4:5`, `16:9`…) when
/// within 1 % of one, else width:height to two decimals (`1.85:1`). `None` without a size.
pub fn aspect(p: &Photo) -> Option<String> {
    let d = &p.develop;
    let (mut w, mut h) = (f64::from(p.width), f64::from(p.height));
    if d.orientation.swaps_axes() {
        std::mem::swap(&mut w, &mut h);
    }
    let r = d.crop.geometry.rect;
    let (cw, ch) = match d.crop.aspect {
        Some((a, b)) if a > 0 && b > 0 => (f64::from(a), f64::from(b)),
        _ => (w * (r.x1 - r.x0).abs(), h * (r.y1 - r.y0).abs()),
    };
    if !(cw.is_finite() && ch.is_finite() && cw > 0.0 && ch > 0.0) {
        return None;
    }
    let (long, short, portrait) = if cw >= ch { (cw, ch, false) } else { (ch, cw, true) };
    let ratio = long / short;
    let named = RATIOS.iter().find(|(a, b)| (ratio / (f64::from(*a) / f64::from(*b)) - 1.0).abs() < 0.01);
    Some(match (named, portrait) {
        (Some((a, b)), false) => format!("{a}:{b}"),
        (Some((a, b)), true) => format!("{b}:{a}"),
        (None, false) => format!("{:.2}:1", ratio),
        (None, true) => format!("1:{:.2}", ratio),
    })
}

/// The develop presets applied to the photo, as its history names them (`Preset: <name>`), each once.
fn presets(p: &Photo) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for step in &p.history {
        if let Some(name) = step.label.strip_prefix("Preset: ").map(str::trim).filter(|n| !n.is_empty())
            && !out.iter().any(|o| o.eq_ignore_ascii_case(name))
        {
            out.push(name.to_string());
        }
    }
    out
}

/// The AI and machine-learning tools the photo's current settings use.
fn ai_tools(p: &Photo) -> Vec<String> {
    use lcc_develop::MaskShape as S;
    let d = &p.develop;
    let mut out: Vec<&str> = Vec::new();
    if d.enhance.denoise_enabled() {
        out.push("Denoise");
    }
    if d.enhance.raw_details {
        out.push("Raw Details");
    }
    if d.enhance.super_resolution {
        out.push("Super Resolution");
    }
    if d.lens_blur.enabled {
        out.push("Lens Blur");
    }
    if d.skin.active() {
        out.push("Smooth Skin");
    }
    for c in d.masks.iter().flat_map(|m| m.components.iter()) {
        let name = match c.shape {
            S::Subject => "Subject mask",
            S::Sky => "Sky mask",
            S::Background => "Background mask",
            S::Object { .. } => "Object mask",
            S::Prompt { .. } | S::Concept { .. } => "Prompt mask",
            S::People { .. } => "People mask",
            S::Landscape { .. } => "Landscape mask",
            S::DepthRange { .. } => "Depth mask",
            _ => continue,
        };
        if !out.contains(&name) {
            out.push(name);
        }
    }
    out.into_iter().map(str::to_string).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PhotoId, Source};

    fn photo() -> Photo {
        Photo::new(PhotoId(1), Source::Demo { scene: 0 }, "a.jpg", "jpg", 6000, 4000, "2026-09-20T10:00:00")
    }

    #[test]
    fn a_pick_with_none_lets_photos_without_a_value_through() {
        let cat = Catalog::new();
        let mut p = photo();
        let none = FacetPick { field: "title".into(), values: vec![], none: true };
        assert!(none.matches(&p, &cat));
        p.meta.title = "Sunset".into();
        assert!(!none.matches(&p, &cat));
        let sunset = FacetPick { field: "title".into(), values: vec!["sunset".into()], none: false };
        assert!(sunset.matches(&p, &cat));
    }

    #[test]
    fn aspect_names_common_ratios_and_turns_them_for_portraits() {
        let mut p = photo();
        (p.width, p.height) = (6000, 4000);
        assert_eq!(aspect(&p).as_deref(), Some("3:2"));
        (p.width, p.height) = (4000, 5000);
        assert_eq!(aspect(&p).as_deref(), Some("4:5"));
        (p.width, p.height) = (0, 5000);
        assert_eq!(aspect(&p), None);
    }
}
