//! Presets and copy/paste subsets.
//!
//! A preset is a *partial* settings object (JSON) — only the groups it includes. Applying merges it
//! into the photo's settings; `amount` (0..200 %) blends between the current settings and the
//! preset's: sliders and point curves continuously, switches and choices by a fixed rule (the
//! preset's at any amount above 0, with the strength of a new choice fading in; see
//! `scale_patch`). Copy/paste/sync use the same group selection ([`SettingsGroup`]).
//!
//! The rendering process ([`crate::ProcessVersion`]) is in no group: copy, paste, sync and presets
//! made from a photo never carry it, so the photos they change keep their own. A partial that names
//! `process` itself (Apply Settings JSON) sets it like any other field.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::borrow::Cow;

use crate::DevelopSettings;

/// Groups shown in "Choose settings to copy" / "Create preset" (Lightroom's checklist).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SettingsGroup {
    Profile,
    Treatment,
    WhiteBalance,
    Light,
    ToneCurve,
    Color,
    ColorMixer,
    ColorGrading,
    Effects,
    Vignette,
    Grain,
    Detail,
    Optics,
    Geometry,
    Crop,
    Masks,
    Spots,
    RedEye,
    LensBlur,
    Calibration,
    PluginFilters,
}

impl SettingsGroup {
    pub const ALL: [SettingsGroup; 21] = [
        SettingsGroup::Profile,
        SettingsGroup::Treatment,
        SettingsGroup::WhiteBalance,
        SettingsGroup::Light,
        SettingsGroup::ToneCurve,
        SettingsGroup::Color,
        SettingsGroup::ColorMixer,
        SettingsGroup::ColorGrading,
        SettingsGroup::Effects,
        SettingsGroup::Vignette,
        SettingsGroup::Grain,
        SettingsGroup::Detail,
        SettingsGroup::Optics,
        SettingsGroup::Geometry,
        SettingsGroup::Crop,
        SettingsGroup::Masks,
        SettingsGroup::Spots,
        SettingsGroup::RedEye,
        SettingsGroup::LensBlur,
        SettingsGroup::Calibration,
        SettingsGroup::PluginFilters,
    ];

    /// Groups a new preset includes by default: the copy defaults without White Balance (each
    /// photo keeps its own unless the photographer ticks it; the user's decision).
    pub fn default_preset() -> Vec<SettingsGroup> {
        SettingsGroup::default_copy().into_iter().filter(|g| *g != SettingsGroup::WhiteBalance).collect()
    }

    /// Groups included by default when copying (Lightroom excludes crop, masks and spots by default).
    pub fn default_copy() -> Vec<SettingsGroup> {
        SettingsGroup::ALL
            .into_iter()
            .filter(|g| !matches!(g, SettingsGroup::Crop | SettingsGroup::Masks | SettingsGroup::Spots | SettingsGroup::RedEye))
            .collect()
    }

    pub fn label(self) -> &'static str {
        match self {
            SettingsGroup::Profile => "Profile",
            SettingsGroup::Treatment => "Treatment",
            SettingsGroup::WhiteBalance => "White Balance",
            SettingsGroup::Light => "Light",
            SettingsGroup::ToneCurve => "Tone Curve",
            SettingsGroup::Color => "Color",
            SettingsGroup::ColorMixer => "Color Mixer",
            SettingsGroup::ColorGrading => "Color Grading",
            SettingsGroup::Effects => "Effects",
            SettingsGroup::Vignette => "Vignette",
            SettingsGroup::Grain => "Grain",
            SettingsGroup::Detail => "Detail",
            SettingsGroup::Optics => "Optics",
            SettingsGroup::Geometry => "Geometry",
            SettingsGroup::Crop => "Crop",
            SettingsGroup::Masks => "Masking",
            SettingsGroup::Spots => "Remove",
            SettingsGroup::RedEye => "Red Eye",
            SettingsGroup::LensBlur => "Lens Blur",
            SettingsGroup::Calibration => "Calibration",
            SettingsGroup::PluginFilters => "Plug-in Filters",
        }
    }

    /// Top-level JSON keys of `DevelopSettings` that belong to the group.
    pub fn keys(self) -> &'static [&'static str] {
        match self {
            SettingsGroup::Profile => &["profile"],
            SettingsGroup::Treatment => &["treatment"],
            SettingsGroup::WhiteBalance => &["wb"],
            SettingsGroup::Light => &["light", "hdr"],
            SettingsGroup::ToneCurve => &["curve"],
            SettingsGroup::Color => &["color"],
            SettingsGroup::ColorMixer => &["mixer", "bw_mix", "point_colors"],
            SettingsGroup::ColorGrading => &["grading"],
            SettingsGroup::Effects => &["effects"],
            SettingsGroup::Vignette => &["vignette"],
            SettingsGroup::Grain => &["grain"],
            SettingsGroup::Detail => &["detail", "enhance"],
            SettingsGroup::Optics => &["optics"],
            SettingsGroup::Geometry => &["geometry"],
            SettingsGroup::Crop => &["crop", "orientation"],
            SettingsGroup::Masks => &["masks"],
            SettingsGroup::Spots => &["spots"],
            SettingsGroup::RedEye => &["red_eye"],
            SettingsGroup::LensBlur => &["lens_blur"],
            SettingsGroup::Calibration => &["calibration"],
            SettingsGroup::PluginFilters => &["plugin_filters"],
        }
    }
}

/// The subset of `s` covering `groups`, as a partial JSON object.
pub fn extract_groups(s: &DevelopSettings, groups: &[SettingsGroup]) -> Value {
    let full = s.to_json();
    let mut out = Map::new();
    for g in groups {
        for k in g.keys() {
            if let Some(v) = full.get(*k) {
                out.insert((*k).to_string(), v.clone());
            } else if *k == "plugin_filters" {
                // Not written while empty: an empty stack still replaces the target's on paste.
                out.insert((*k).to_string(), Value::Array(Vec::new()));
            }
        }
    }
    // the Camera colour is left out of the settings while it is Auto; a preset or a sync of the Profile group still
    // carries it, so that applying it turns a Colorimetric photo back to Auto
    if let Some(Value::Object(profile)) = out.get_mut("profile") {
        profile.entry("camera_colour").or_insert_with(|| serde_json::to_value(s.profile.camera_colour).unwrap_or(Value::Null));
    }
    Value::Object(out)
}

/// One setting inside a [`SettingsGroup`] that can be ticked on its own in the New Preset, Update
/// Preset and Copy Settings trees (Lightroom's per-setting checklist): Exposure inside Light, the
/// Sharpening sliders inside Detail… `id` is `<group id>.<name>`; `paths` are the dot paths of the
/// settings JSON it covers (a group's items together cover every key of the group).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SettingItem {
    pub id: &'static str,
    pub label: &'static str,
    pub paths: &'static [&'static str],
}

const fn item(id: &'static str, label: &'static str, paths: &'static [&'static str]) -> SettingItem {
    SettingItem { id, label, paths }
}

const LIGHT_ITEMS: &[SettingItem] = &[
    item("light.exposure", "Exposure", &["light.exposure"]),
    item("light.contrast", "Contrast", &["light.contrast"]),
    item("light.highlights", "Highlights", &["light.highlights"]),
    item("light.shadows", "Shadows", &["light.shadows"]),
    item("light.whites", "Whites", &["light.whites"]),
    item("light.blacks", "Blacks", &["light.blacks"]),
    item("light.hdr", "HDR Output", &["hdr"]),
];
const CURVE_ITEMS: &[SettingItem] = &[
    item(
        "toneCurve.parametric",
        "Parametric Curve",
        &["curve.highlights", "curve.lights", "curve.darks", "curve.shadows", "curve.split_shadows", "curve.split_mid", "curve.split_highlights"],
    ),
    item("toneCurve.point", "Point Curve", &["curve.master", "curve.red", "curve.green", "curve.blue"]),
    item("toneCurve.refineSaturation", "Refine Saturation", &["curve.refine_saturation"]),
];
const COLOR_ITEMS: &[SettingItem] =
    &[item("color.vibrance", "Vibrance", &["color.vibrance"]), item("color.saturation", "Saturation", &["color.saturation"])];
const MIXER_ITEMS: &[SettingItem] = &[
    item("colorMixer.mixer", "Color Mixer", &["mixer"]),
    item("colorMixer.bw", "B&W Mix", &["bw_mix"]),
    item("colorMixer.pointColor", "Point Color", &["point_colors"]),
];
const EFFECTS_ITEMS: &[SettingItem] = &[
    item("effects.texture", "Texture", &["effects.texture"]),
    item("effects.clarity", "Clarity", &["effects.clarity"]),
    item("effects.dehaze", "Dehaze", &["effects.dehaze"]),
];
const DETAIL_ITEMS: &[SettingItem] = &[
    item("detail.sharpening", "Sharpening", &["detail.sharpen_amount", "detail.sharpen_radius", "detail.sharpen_detail", "detail.sharpen_masking"]),
    item("detail.noise", "Noise Reduction", &["detail.nr_luminance", "detail.nr_detail", "detail.nr_contrast", "detail.nr_outliers"]),
    item("detail.colorNoise", "Color Noise Reduction", &["detail.nr_color", "detail.nr_color_detail", "detail.nr_color_smoothness"]),
    item("detail.enhance", "AI Denoise and Enhance", &["enhance"]),
];
const OPTICS_ITEMS: &[SettingItem] = &[
    item("optics.lensProfile", "Lens Profile Corrections", &["optics.lens_profile", "optics.profile_distortion", "optics.profile_vignetting"]),
    item("optics.chromatic", "Chromatic Aberration", &["optics.remove_ca", "optics.ca_red", "optics.ca_blue"]),
    item(
        "optics.defringe",
        "Defringe",
        &[
            "optics.defringe_purple_amount",
            "optics.defringe_purple_hue_lo",
            "optics.defringe_purple_hue_hi",
            "optics.defringe_green_amount",
            "optics.defringe_green_hue_lo",
            "optics.defringe_green_hue_hi",
        ],
    ),
    item("optics.distortion", "Manual Distortion", &["optics.distortion"]),
    item("optics.vignetting", "Manual Vignetting", &["optics.vignetting", "optics.vignetting_midpoint"]),
];
const GEOMETRY_ITEMS: &[SettingItem] = &[
    item("geometry.upright", "Upright", &["geometry.upright", "geometry.guides", "geometry.upright_transform"]),
    item(
        "geometry.transform",
        "Transform Sliders",
        &[
            "geometry.vertical",
            "geometry.horizontal",
            "geometry.rotate",
            "geometry.aspect",
            "geometry.scale",
            "geometry.offset_x",
            "geometry.offset_y",
        ],
    ),
    item("geometry.constrainCrop", "Constrain Crop", &["geometry.constrain_crop"]),
];
const CROP_ITEMS: &[SettingItem] =
    &[item("crop.crop", "Crop and Straighten", &["crop"]), item("crop.orientation", "Rotation and Flip", &["orientation"])];

impl SettingsGroup {
    /// The settings that can be ticked one by one (empty: the group is one setting).
    pub fn items(self) -> &'static [SettingItem] {
        match self {
            SettingsGroup::Light => LIGHT_ITEMS,
            SettingsGroup::ToneCurve => CURVE_ITEMS,
            SettingsGroup::Color => COLOR_ITEMS,
            SettingsGroup::ColorMixer => MIXER_ITEMS,
            SettingsGroup::Effects => EFFECTS_ITEMS,
            SettingsGroup::Detail => DETAIL_ITEMS,
            SettingsGroup::Optics => OPTICS_ITEMS,
            SettingsGroup::Geometry => GEOMETRY_ITEMS,
            SettingsGroup::Crop => CROP_ITEMS,
            _ => &[],
        }
    }

    /// The group's id (`light`, `whiteBalance`…), as commands take it.
    pub fn id(self) -> &'static str {
        match self {
            SettingsGroup::Profile => "profile",
            SettingsGroup::Treatment => "treatment",
            SettingsGroup::WhiteBalance => "whiteBalance",
            SettingsGroup::Light => "light",
            SettingsGroup::ToneCurve => "toneCurve",
            SettingsGroup::Color => "color",
            SettingsGroup::ColorMixer => "colorMixer",
            SettingsGroup::ColorGrading => "colorGrading",
            SettingsGroup::Effects => "effects",
            SettingsGroup::Vignette => "vignette",
            SettingsGroup::Grain => "grain",
            SettingsGroup::Detail => "detail",
            SettingsGroup::Optics => "optics",
            SettingsGroup::Geometry => "geometry",
            SettingsGroup::Crop => "crop",
            SettingsGroup::Masks => "masks",
            SettingsGroup::Spots => "spots",
            SettingsGroup::RedEye => "redEye",
            SettingsGroup::LensBlur => "lensBlur",
            SettingsGroup::Calibration => "calibration",
            SettingsGroup::PluginFilters => "pluginFilters",
        }
    }

    /// The group with id `id`.
    pub fn from_id(id: &str) -> Option<SettingsGroup> {
        SettingsGroup::ALL.into_iter().find(|g| g.id() == id)
    }
}

/// The setting item with id `id` and its group.
pub fn find_item(id: &str) -> Option<(SettingsGroup, &'static SettingItem)> {
    SettingsGroup::ALL.into_iter().find_map(|g| g.items().iter().find(|i| i.id == id).map(|i| (g, i)))
}

/// What to copy, save in a preset or sync: whole groups, plus single settings of groups that are only
/// partly ticked ([`SettingItem`] ids). Remembered by the dialogs that choose it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SettingsPick {
    pub groups: Vec<SettingsGroup>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub settings: Vec<String>,
}

impl SettingsPick {
    /// Whole groups only.
    pub fn of_groups(groups: Vec<SettingsGroup>) -> SettingsPick {
        SettingsPick { groups, settings: Vec::new() }.normalized()
    }

    /// A list of group and setting ids (`light`, `detail.sharpening`) as the dialogs keep them: a group
    /// whose every setting is ticked becomes the whole group. Unknown ids are left out.
    pub fn from_ids<S: AsRef<str>>(ids: &[S]) -> SettingsPick {
        let mut pick = SettingsPick::default();
        for id in ids {
            let id = id.as_ref();
            if let Some(g) = SettingsGroup::from_id(id) {
                if !pick.groups.contains(&g) {
                    pick.groups.push(g);
                }
            } else if find_item(id).is_some() && !pick.settings.iter().any(|s| s == id) {
                pick.settings.push(id.to_string());
            }
        }
        pick.normalized()
    }

    /// The pick as ids, one per tickable box: a group with settings lists each of them (the tree's leaves).
    pub fn to_ids(&self) -> Vec<String> {
        let mut out = Vec::new();
        for g in SettingsGroup::ALL {
            if self.groups.contains(&g) {
                if g.items().is_empty() {
                    out.push(g.id().to_string());
                } else {
                    out.extend(g.items().iter().map(|i| i.id.to_string()));
                }
            } else {
                out.extend(g.items().iter().filter(|i| self.settings.iter().any(|s| s == i.id)).map(|i| i.id.to_string()));
            }
        }
        out
    }

    /// Single settings of a group that is whole anyway are dropped; a group whose settings are all
    /// there becomes whole; unknown setting ids are dropped.
    pub fn normalized(mut self) -> SettingsPick {
        let groups = self.groups.clone();
        self.settings.retain(|s| find_item(s).is_some_and(|(g, _)| !groups.contains(&g)));
        for g in SettingsGroup::ALL {
            let items = g.items();
            if !items.is_empty() && !self.groups.contains(&g) && items.iter().all(|i| self.settings.iter().any(|s| s == i.id)) {
                self.groups.push(g);
                self.settings.retain(|s| !items.iter().any(|i| i.id == s));
            }
        }
        // in the groups' own order, once each
        self.groups = SettingsGroup::ALL.into_iter().filter(|g| self.groups.contains(g)).collect();
        let mut seen = Vec::new();
        self.settings.retain(|s| {
            let new = !seen.contains(s);
            seen.push(s.clone());
            new
        });
        self
    }

    pub fn is_empty(&self) -> bool {
        self.groups.is_empty() && self.settings.is_empty()
    }

    /// The pick includes all or part of `g`.
    pub fn touches(&self, g: SettingsGroup) -> bool {
        self.groups.contains(&g) || g.items().iter().any(|i| self.settings.iter().any(|s| s == i.id))
    }

    /// The pick includes setting (or group) `id`.
    pub fn has(&self, id: &str) -> bool {
        if let Some(g) = SettingsGroup::from_id(id) {
            return self.groups.contains(&g);
        }
        self.settings.iter().any(|s| s == id) || find_item(id).is_some_and(|(g, _)| self.groups.contains(&g))
    }

    /// Only what both picks hold (Paste Selected: the ticked part of what was copied).
    pub fn intersect(&self, other: &SettingsPick) -> SettingsPick {
        let ids: Vec<String> = self.to_ids().into_iter().filter(|id| other.has(id)).collect();
        SettingsPick::from_ids(&ids)
    }

    /// The part of `s` the pick covers, as a partial JSON object.
    pub fn extract(&self, s: &DevelopSettings) -> Value {
        let mut out = extract_groups(s, &self.groups);
        if self.settings.is_empty() {
            return out;
        }
        let full = s.to_json();
        let Some(o) = out.as_object_mut() else { return out };
        for id in &self.settings {
            let Some((_, it)) = find_item(id) else { continue };
            for path in it.paths {
                copy_path(&full, o, path);
            }
            // (left out of the JSON while off, as the whole group knows: copying it off switches it off)
            if it.paths.contains(&"hdr") && !o.contains_key("hdr") {
                o.insert("hdr".into(), serde_json::to_value(crate::Hdr::default()).unwrap_or(Value::Null));
            }
        }
        out
    }

    /// What the partial settings `partial` (a preset's) hold, as a pick: a key of a group with several
    /// settings counts for the settings it has.
    pub fn of_partial(partial: &Value) -> SettingsPick {
        let Some(o) = partial.as_object() else { return SettingsPick::default() };
        let mut ids: Vec<String> = Vec::new();
        for g in SettingsGroup::ALL {
            if g.items().is_empty() {
                if g.keys().iter().any(|k| o.contains_key(*k)) {
                    ids.push(g.id().to_string());
                }
                continue;
            }
            for it in g.items() {
                if it.paths.iter().any(|p| path_in(partial, p)) {
                    ids.push(it.id.to_string());
                }
            }
        }
        SettingsPick::from_ids(&ids)
    }
}

/// `path` (dot separated) is in `v`.
fn path_in(v: &Value, path: &str) -> bool {
    let mut cur = v;
    for k in path.split('.') {
        match cur.get(k) {
            Some(n) => cur = n,
            None => return false,
        }
    }
    true
}

/// Copy the value at `path` in `from` into `to` (creating the objects on the way).
fn copy_path(from: &Value, to: &mut Map<String, Value>, path: &str) {
    let mut parts = path.split('.').peekable();
    let mut src = from;
    let mut dst = to;
    while let Some(k) = parts.next() {
        let Some(v) = src.get(k) else { return };
        if parts.peek().is_none() {
            dst.insert(k.to_string(), v.clone());
            return;
        }
        src = v;
        let entry = dst.entry(k.to_string()).or_insert_with(|| Value::Object(Map::new()));
        if !entry.is_object() {
            *entry = Value::Object(Map::new());
        }
        let Some(next) = entry.as_object_mut() else { return };
        dst = next;
    }
}

/// Recursively merge `patch` into `base` (objects merge; everything else replaces).
pub fn deep_merge(base: &mut Value, patch: &Value) {
    match (base, patch) {
        (Value::Object(b), Value::Object(p)) => {
            for (k, v) in p {
                match b.get_mut(k) {
                    Some(bv) if bv.is_object() && v.is_object() => deep_merge(bv, v),
                    _ => {
                        b.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        (b, p) => *b = p.clone(),
    }
}

/// Interpolate numbers in `patch` towards/away from `base` by `t` (1 = patch as-is, 0 = base as-is).
/// Point curves blend point by point. Discrete values (profile id, treatment, switches, seeds) take
/// the preset's at any amount above 0, so the look fades in from nothing instead of jumping when
/// they flip half-way (#37). When the preset changes a choice (a string) or an object's `enabled`
/// switch, that object's `amount` blends from 0, not from the photo's (which was the strength of
/// its own choice), so a new profile or LUT fades in; white balance `temp` / `tint` on another
/// scale than the photo's can't be blended and are taken whole (upstream #2669 / #2896).
fn scale_patch(base: &Value, patch: &Value, t: f64) -> Value {
    if t <= 0.0 {
        return if base.is_null() { patch.clone() } else { base.clone() };
    }
    match (base, patch) {
        (Value::Object(b), Value::Object(p)) => {
            let choice_changed = p.iter().any(|(k, pv)| {
                let changed = b.get(k).is_none_or(|bv| bv != pv);
                changed && (pv.is_string() || (k == "enabled" && pv.is_boolean()))
            });
            let other_scale = (p.contains_key("temp") || p.contains_key("tint"))
                && p.get("scale").and_then(Value::as_str).unwrap_or("legacy") != b.get("scale").and_then(Value::as_str).unwrap_or("legacy");
            let zero = Value::from(0.0);
            let mut out = Map::new();
            for (k, pv) in p {
                let bv = match b.get(k) {
                    Some(Value::Number(_)) if choice_changed && k == "amount" => &zero,
                    Some(bv) => bv,
                    None if choice_changed && k == "amount" && pv.is_number() => &zero,
                    None => &Value::Null,
                };
                let v = if other_scale && t > 0.0 && (k == "temp" || k == "tint") { pv.clone() } else { scale_patch(bv, pv, t) };
                out.insert(k.clone(), v);
            }
            Value::Object(out)
        }
        // Integer fields (the grain seed, ids) are discrete like strings: an interpolated seed would
        // not deserialize as an integer, and the whole preset would silently not apply.
        (Value::Number(b), Value::Number(p)) if b.is_f64() || p.is_f64() => {
            let (b, p) = (b.as_f64().unwrap_or(0.0), p.as_f64().unwrap_or(0.0));
            serde_json::Number::from_f64(b + (p - b) * t).map(Value::Number).unwrap_or(Value::Null)
        }
        (b, Value::Array(p)) => match (curve_points(b), curve_points(&Value::Array(p.clone()))) {
            (Some(bc), Some(pc)) => blend_curves(&bc, &pc, t),
            _ => match b {
                Value::Array(ba) if ba.len() == p.len() => Value::Array(ba.iter().zip(p).map(|(b, p)| scale_patch(b, p, t)).collect()),
                _ => Value::Array(p.clone()),
            },
        },
        (_, p) => p.clone(),
    }
}

/// Most points a blended curve keeps (input-derived lengths are capped).
const MAX_CURVE_POINTS: usize = 256;

/// A point curve (`[{x, y}, …]`, empty = identity) as sorted points, or `None` when `v` is not one.
fn curve_points(v: &Value) -> Option<Vec<(f64, f64)>> {
    let a = v.as_array()?;
    let mut pts = Vec::with_capacity(a.len().min(MAX_CURVE_POINTS));
    for p in a.iter().take(MAX_CURVE_POINTS) {
        let o = p.as_object()?;
        if o.len() != 2 {
            return None;
        }
        let (x, y) = (o.get("x")?.as_f64()?, o.get("y")?.as_f64()?);
        if !x.is_finite() || !y.is_finite() {
            return None;
        }
        pts.push((x, y));
    }
    pts.sort_by(|a, b| a.0.total_cmp(&b.0));
    Some(pts)
}

/// Piecewise-linear value of a curve at `x` (empty = identity).
fn curve_at(c: &[(f64, f64)], x: f64) -> f64 {
    let (Some(first), Some(last)) = (c.first(), c.last()) else { return x };
    if x <= first.0 {
        return first.1;
    }
    if x >= last.0 {
        return last.1;
    }
    for w in c.windows(2) {
        if let [(x0, y0), (x1, y1)] = *w
            && x <= x1
        {
            let d = x1 - x0;
            return if d > 1e-12 { y0 + (y1 - y0) * (x - x0) / d } else { y1 };
        }
    }
    last.1
}

/// Blend two point curves: the union of their x positions, each y moved from the base curve's
/// value towards the preset's by `t`.
fn blend_curves(b: &[(f64, f64)], p: &[(f64, f64)], t: f64) -> Value {
    if p.is_empty() && b.is_empty() {
        return Value::Array(Vec::new());
    }
    let mut xs: Vec<f64> = b.iter().chain(p).map(|q| q.0).collect();
    if b.is_empty() || p.is_empty() {
        xs.extend([0.0, 1.0]);
    }
    xs.sort_by(f64::total_cmp);
    xs.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
    xs.truncate(MAX_CURVE_POINTS);
    let pts = xs
        .into_iter()
        .map(|x| {
            let (yb, yp) = (curve_at(b, x), curve_at(p, x));
            serde_json::json!({"x": x, "y": (yb + (yp - yb) * t).clamp(0.0, 1.0)})
        })
        .collect();
    Value::Array(pts)
}

/// Apply a partial settings object with an amount (1.0 = 100 %; blending rules: [`scale_patch`]).
/// Out-of-range values are clamped through the control specs, as every merge does
/// ([`DevelopSettings::merged`]).
/// At 0 % nothing changes; at 100 % the partial is merged as it is.
pub fn apply_partial(s: &DevelopSettings, partial: &Value, amount: f64) -> DevelopSettings {
    if amount <= 0.0 {
        return s.clone();
    }
    let patch = if (amount - 1.0).abs() < 1e-9 {
        Cow::Borrowed(partial)
    } else {
        // the base with every group written out, also the ones serialization leaves out while at
        // their defaults (HDR), so their numbers blend from the photo's values too
        let mut base = s.to_json();
        if let (Some(o), Ok(hdr)) = (base.as_object_mut(), serde_json::to_value(s.hdr)) {
            o.entry("hdr").or_insert(hdr);
        }
        Cow::Owned(scale_patch(&base, partial, amount))
    };
    // (a white balance without a scale keeps its old meaning: `DevelopSettings::merged`)
    s.merged(&patch).unwrap_or_else(|_| s.clone())
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Preset {
    pub id: String,
    pub name: String,
    pub group: String,
    /// Partial settings.
    pub settings: Value,
    #[serde(default)]
    pub favorite: bool,
    /// Built-in presets ship with the app and can't be deleted.
    #[serde(default)]
    pub builtin: bool,
}

impl Preset {
    pub fn from_settings(id: &str, name: &str, group: &str, s: &DevelopSettings, groups: &[SettingsGroup]) -> Preset {
        Preset { id: id.into(), name: name.into(), group: group.into(), settings: extract_groups(s, groups), favorite: false, builtin: false }
    }
    /// Apply at `amount` (1.0 = 100 %). Masks in the preset are added to the photo's own (ones it
    /// already has are skipped), their effect scaled by the amount.
    pub fn apply(&self, s: &DevelopSettings, amount: f64) -> DevelopSettings {
        let Some(masks) = self.settings.get("masks").and_then(Value::as_array).filter(|m| !m.is_empty()) else {
            return apply_partial(s, &self.settings, amount);
        };
        let mut rest = self.settings.clone();
        if let Some(o) = rest.as_object_mut() {
            o.remove("masks");
        }
        let mut out = apply_partial(s, &rest, amount);
        for m in masks {
            let Ok(mut m) = serde_json::from_value::<crate::Mask>(m.clone()) else { continue };
            if out.masks.iter().any(|q| q.name == m.name && q.components == m.components) {
                continue;
            }
            m.id = out.next_mask_id();
            m.adjust.amount = (m.adjust.amount * amount).clamp(0.0, 200.0);
            out.masks.push(m);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extract_and_apply_roundtrip() {
        let mut a = DevelopSettings::default();
        a.light.exposure = 1.0;
        a.effects.clarity = 30.0;
        a.crop.geometry.angle = 5.0;
        let p = Preset::from_settings("p", "P", "User", &a, &SettingsGroup::default_copy());
        assert!(p.settings.get("crop").is_none());
        let b = p.apply(&DevelopSettings::default(), 1.0);
        assert_eq!(b.light.exposure, 1.0);
        assert_eq!(b.effects.clarity, 30.0);
        assert_eq!(b.crop.geometry.angle, 0.0);
    }

    #[test]
    fn amount_scales_numbers() {
        let base = DevelopSettings::default();
        let partial = json!({"light": {"exposure": 1.0, "contrast": 40}});
        let half = apply_partial(&base, &partial, 0.5);
        assert!((half.light.exposure - 0.5).abs() < 1e-9);
        assert!((half.light.contrast - 20.0).abs() < 1e-9);
        let more = apply_partial(&base, &partial, 2.0);
        assert!((more.light.contrast - 80.0).abs() < 1e-9);
        // clamped through specs
        let lots = apply_partial(&base, &json!({"light": {"exposure": 4.0}}), 2.0);
        assert_eq!(lots.light.exposure, 5.0);
    }

    /// A preset with the Grain group (its seed is an integer) used to do nothing at any amount
    /// other than 100 %: the interpolated seed failed to deserialize and the preset was dropped.
    /// The seed is discrete: the preset's at any amount above 0 (the grain itself fades in).
    #[test]
    fn amount_keeps_integer_fields_whole() {
        let mut a = DevelopSettings::default();
        a.light.exposure = 1.0;
        a.grain.amount = 40.0;
        a.grain.seed = 12345;
        let p = Preset::from_settings("p", "P", "User", &a, &SettingsGroup::default_copy());
        assert!(p.settings.pointer("/grain/seed").is_some_and(Value::is_u64), "the default copy groups include the grain seed");
        let base = DevelopSettings::default();
        for (amount, seed) in [(0.3, 12345), (0.5, 12345), (1.5, 12345), (2.0, 12345)] {
            let out = p.apply(&base, amount);
            assert!((out.light.exposure - amount.min(5.0)).abs() < 1e-9, "{amount}: exposure {}", out.light.exposure);
            assert!((out.grain.amount - (40.0 * amount).min(100.0)).abs() < 1e-9, "{amount}: grain {}", out.grain.amount);
            assert_eq!(out.grain.seed, seed, "{amount}: seed");
        }
    }

    /// #37: lowering the amount faded the preset until about 50 %, then jumped to the unedited
    /// image because the curve, profile and switches flipped back to the photo's own there.
    #[test]
    fn amount_fades_gradually_to_nothing() {
        let mut a = DevelopSettings::default();
        a.profile.id = "lc.vivid".into();
        a.light.contrast = 40.0;
        a.curve.master = vec![lcc_geom::Point { x: 0.0, y: 0.1 }, lcc_geom::Point { x: 0.5, y: 0.6 }, lcc_geom::Point { x: 1.0, y: 1.0 }];
        let p = Preset::from_settings("p", "P", "User", &a, &SettingsGroup::default_copy());
        let base = DevelopSettings::default();
        assert_eq!(p.apply(&base, 0.0), apply_partial(&base, &json!({}), 1.0), "0 % is the photo as it was");
        let mid = |s: &DevelopSettings| lcc_develop_curve_mid(&s.curve.master);
        let mut last = (0.0, 0.5);
        for step in 1..=20 {
            let t = f64::from(step) / 10.0;
            let out = p.apply(&base, t);
            assert_eq!(out.profile.id, "lc.vivid", "{t}: profile switches as soon as the preset is there");
            let now = (out.light.contrast, mid(&out));
            assert!(now.0 > last.0 && now.0 - last.0 < 4.0 + 1e-9, "{t}: contrast {now:?} after {last:?}");
            assert!(now.1 >= last.1 - 1e-9 && now.1 - last.1 < 0.011, "{t}: curve mid {now:?} after {last:?}");
            last = now;
        }
    }

    /// A preset that picks another profile fades its strength in from 0 %, not from the photo's own
    /// profile amount; white balance on another scale is taken whole.
    #[test]
    fn a_new_choice_fades_its_amount_in_from_zero() {
        let mut a = DevelopSettings::default();
        a.profile.id = "lc.vivid".into();
        a.profile.amount = 80.0;
        let p = Preset::from_settings("p", "P", "User", &a, &SettingsGroup::default_copy());
        let mut base = DevelopSettings::default();
        base.profile.amount = 100.0;
        let half = p.apply(&base, 0.5);
        assert_eq!(half.profile.id, "lc.vivid");
        assert!((half.profile.amount - 40.0).abs() < 1e-6, "{}", half.profile.amount);
        let out = scale_patch(&json!({"temp": 0.1, "tint": 0.0}), &json!({"temp": 5500.0, "tint": 3.0, "scale": "kelvin"}), 0.5);
        assert_eq!(out["temp"], json!(5500.0));
    }

    fn lcc_develop_curve_mid(c: &[lcc_geom::Point]) -> f64 {
        let pts: Vec<(f64, f64)> = c.iter().map(|p| (p.x, p.y)).collect();
        curve_at(&pts, 0.5)
    }

    /// Every group with single settings is covered by them: all ticked = the whole group.
    #[test]
    fn group_items_cover_every_key() {
        let mut s = DevelopSettings::default();
        s.hdr.enabled = true;
        let full = s.to_json();
        for g in SettingsGroup::ALL {
            if g.items().is_empty() {
                continue;
            }
            let whole = extract_groups(&s, &[g]);
            let pick = SettingsPick { groups: vec![], settings: g.items().iter().map(|i| i.id.to_string()).collect() };
            assert_eq!(pick.clone().normalized().groups, vec![g], "{g:?}");
            let by_items = SettingsPick { groups: vec![], settings: pick.settings }.extract(&s);
            assert_eq!(whole, by_items, "{g:?}: the settings miss a key of the group");
            for it in g.items() {
                assert!(it.id.starts_with(&format!("{}.", g.id())), "{}", it.id);
                for p in it.paths {
                    assert!(path_in(&full, p) || ["camera_profile", "retouch"].contains(p), "{}: {p} isn't in the settings", it.id);
                }
            }
        }
        for g in SettingsGroup::ALL {
            assert_eq!(serde_json::to_value(g).unwrap(), json!(g.id()));
            assert_eq!(SettingsGroup::from_id(g.id()), Some(g));
        }
    }

    /// Client note 8a: Light without Exposure leaves each photo's exposure alone.
    #[test]
    fn single_settings_travel_alone() {
        let mut a = DevelopSettings::default();
        a.light.exposure = 1.5;
        a.light.contrast = 30.0;
        a.detail.sharpen_amount = 80.0;
        a.detail.nr_luminance = 40.0;
        let pick = SettingsPick::from_ids(&["light.contrast", "detail.sharpening"]);
        assert_eq!(pick.settings, vec!["light.contrast".to_string(), "detail.sharpening".to_string()]);
        let p = Preset { settings: pick.extract(&a), ..Preset::from_settings("p", "P", "User", &a, &[]) };
        assert_eq!(
            p.settings,
            json!({"light": {"contrast": 30.0}, "detail": {"sharpen_amount": 80.0, "sharpen_radius": 1.0, "sharpen_detail": 25.0, "sharpen_masking": 0.0}})
        );
        let mut b = DevelopSettings::default();
        b.light.exposure = -0.7;
        b.detail.nr_luminance = 12.0;
        let out = p.apply(&b, 1.0);
        assert_eq!((out.light.exposure, out.light.contrast), (-0.7, 30.0));
        assert_eq!((out.detail.sharpen_amount, out.detail.nr_luminance), (80.0, 12.0));
        // what the preset holds, as ticks again
        assert_eq!(SettingsPick::of_partial(&p.settings), pick);
        assert_eq!(pick.to_ids(), vec!["light.contrast".to_string(), "detail.sharpening".to_string()]);
        // a whole group lists every setting as a tick
        let whole = SettingsPick::from_ids(&["effects", "whiteBalance", "nonsense", "light.nope"]);
        assert_eq!(whole.groups, vec![SettingsGroup::WhiteBalance, SettingsGroup::Effects]);
        assert!(whole.settings.is_empty());
        assert_eq!(whole.to_ids(), vec!["whiteBalance", "effects.texture", "effects.clarity", "effects.dehaze"]);
        assert!(whole.has("effects.clarity") && whole.has("whiteBalance") && !whole.has("light.exposure"));
        assert_eq!(whole.intersect(&SettingsPick::from_ids(&["effects.clarity", "light"])), SettingsPick::from_ids(&["effects.clarity"]));
    }

    /// Client note 8b: a preset saved without White Balance never touches a photo's own (as shot or
    /// set by hand), and New Preset leaves it out unless it is ticked.
    #[test]
    fn preset_without_white_balance_keeps_each_photos_own() {
        assert!(!SettingsGroup::default_preset().contains(&SettingsGroup::WhiteBalance));
        assert!(SettingsGroup::default_copy().contains(&SettingsGroup::WhiteBalance));
        let mut src = DevelopSettings::for_raw(3200.0, 12.0);
        src.wb.mode = crate::WbMode::Custom;
        src.wb.scale = crate::WbScale::Kelvin;
        src.light.exposure = 0.4;
        src.calibration.red_hue = 10.0;
        for pick in [SettingsPick::of_groups(SettingsGroup::default_preset()), SettingsPick::from_ids(&["light", "color", "calibration"])] {
            let p = Preset { settings: pick.extract(&src), ..Preset::from_settings("p", "P", "User", &src, &[]) };
            assert!(p.settings.get("wb").is_none(), "{:?}", p.settings);
            for (temp, tint, mode, scale) in [
                (5600.0, 4.0, crate::WbMode::AsShot, crate::WbScale::Kelvin),
                (6500.0, 0.0, crate::WbMode::AsShot, crate::WbScale::Legacy),
                (4100.0, -8.0, crate::WbMode::Custom, crate::WbScale::Kelvin),
            ] {
                let mut photo = DevelopSettings::for_raw(temp, tint);
                photo.wb.mode = mode;
                photo.wb.scale = scale;
                let out = p.apply(&photo, 1.0);
                assert_eq!(out.wb, photo.wb);
                assert_eq!(out.light.exposure, 0.4);
                let half = p.apply(&photo, 0.5);
                assert_eq!(half.wb, photo.wb);
            }
        }
    }

    #[test]
    fn deep_merge_objects() {
        let mut a = json!({"x": {"a": 1, "b": 2}, "y": [1]});
        deep_merge(&mut a, &json!({"x": {"b": 3}, "y": [2, 3]}));
        assert_eq!(a, json!({"x": {"a": 1, "b": 3}, "y": [2, 3]}));
    }

    #[test]
    fn preset_masks_get_unused_ids_after_a_mask_id_at_the_limit() {
        let s = DevelopSettings { masks: vec![crate::Mask { id: u32::MAX, name: "Theirs".into(), ..Default::default() }], ..Default::default() };
        let p = Preset {
            id: "p".into(),
            name: "P".into(),
            group: "User".into(),
            settings: json!({"masks": [{"id": 7, "name": "A"}, {"id": 7, "name": "B"}]}),
            favorite: false,
            builtin: false,
        };
        let out = p.apply(&s, 1.0);
        let mut ids: Vec<u32> = out.masks.iter().map(|m| m.id).collect();
        assert_eq!(ids.len(), 3);
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 3, "{:?}", out.masks);
    }
}
