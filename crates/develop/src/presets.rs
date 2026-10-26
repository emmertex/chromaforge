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
        let mut next = out.masks.iter().map(|m| m.id).max().unwrap_or(0);
        for m in masks {
            let Ok(mut m) = serde_json::from_value::<crate::Mask>(m.clone()) else { continue };
            if out.masks.iter().any(|q| q.name == m.name && q.components == m.components) {
                continue;
            }
            next += 1;
            m.id = next;
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

    #[test]
    fn deep_merge_objects() {
        let mut a = json!({"x": {"a": 1, "b": 2}, "y": [1]});
        deep_merge(&mut a, &json!({"x": {"b": 3}, "y": [2, 3]}));
        assert_eq!(a, json!({"x": {"a": 1, "b": 3}, "y": [2, 3]}));
    }
}
