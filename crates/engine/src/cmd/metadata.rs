//! Metadata presets: named sets of descriptive fields (copyright, creator, place, keywords…)
//! applied to photos in one step or to every import (Settings → Import). Saved with the library.

use lcc_catalog::{CopyrightStatus, Meta};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::{CommandSpec, always, bad, cmd, str_param};
use crate::{Result, Session};

/// The fields a metadata preset can hold (photo.setMeta names).
pub const FIELDS: &[&str] = &[
    "title",
    "headline",
    "caption",
    "descriptionWriter",
    "category",
    "copyright",
    "copyrightStatus",
    "usageTerms",
    "copyrightUrl",
    "creator",
    "creatorJobTitle",
    "creatorAddress",
    "creatorCity",
    "creatorState",
    "creatorPostalCode",
    "creatorCountry",
    "creatorPhone",
    "creatorEmail",
    "creatorWebsite",
    "location",
    "city",
    "state",
    "country",
    "countryCode",
    "keywords",
    "supplementalCategories",
    "subjectCode",
];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MetadataPreset {
    pub name: String,
    /// photo.setMeta fields; `keywords` are added, not replaced.
    pub fields: Value,
}

fn text_field<'a>(m: &'a mut Meta, k: &str) -> Option<&'a mut String> {
    Some(match k {
        "title" => &mut m.title,
        "headline" => &mut m.headline,
        "caption" => &mut m.caption,
        "descriptionWriter" => &mut m.description_writer,
        "category" => &mut m.category,
        "copyright" => &mut m.copyright,
        "usageTerms" => &mut m.usage_terms,
        "copyrightUrl" => &mut m.copyright_url,
        "creator" => &mut m.creator,
        "location" => &mut m.location,
        "city" => &mut m.city,
        "state" => &mut m.state,
        "country" => &mut m.country,
        "countryCode" => &mut m.country_code,
        k => return m.contact.get_mut(k),
    })
}

fn list_field<'a>(m: &'a mut Meta, k: &str) -> Option<&'a mut Vec<String>> {
    match k {
        "supplementalCategories" => Some(&mut m.supplemental_categories),
        "subjectCode" => Some(&mut m.subject_code),
        _ => None,
    }
}

/// A preset's list of text, each once, or `None` when `v` isn't a list.
fn text_list(v: &Value) -> Option<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    for s in v.as_array()?.iter().filter_map(Value::as_str).map(str::trim).filter(|s| !s.is_empty()) {
        if !out.iter().any(|x| x == s) {
            out.push(s.to_string());
        }
    }
    Some(out)
}

/// Apply preset `fields` to `m`: text fields and lists replace, keywords are added.
pub fn apply_to(m: &mut Meta, fields: &Value) {
    for (k, v) in fields.as_object().into_iter().flatten() {
        if k == "copyrightStatus" {
            if let Some(st) = v.as_str().and_then(CopyrightStatus::parse) {
                m.copyright_status = st;
            }
        } else if k == "keywords" {
            for kw in v.as_array().into_iter().flatten().filter_map(Value::as_str) {
                if !m.keywords.iter().any(|x| x.eq_ignore_ascii_case(kw)) {
                    m.keywords.push(kw.to_string());
                }
            }
        } else if let (Some(f), Some(l)) = (list_field(m, k), text_list(v)) {
            *f = l;
        } else if let (Some(f), Some(t)) = (text_field(m, k), v.as_str()) {
            *f = t.to_string();
        }
    }
}

fn list(s: &Session) -> Value {
    json!(s.metadata_presets)
}

fn save(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "metadata.savePreset";
    let name = str_param(p, "name").map(str::trim).filter(|n| !n.is_empty()).ok_or_else(|| bad(C, "missing `name`"))?.to_string();
    let fields = match p.get("fields") {
        Some(Value::Object(o)) => {
            if let Some(k) = o.keys().find(|k| !FIELDS.contains(&k.as_str())) {
                return Err(bad(C, format!("unknown field `{k}` ({})", FIELDS.join(", "))));
            }
            if let Some(v) = o.get("copyrightStatus").filter(|v| v.as_str().and_then(CopyrightStatus::parse).is_none()) {
                return Err(bad(C, format!("copyrightStatus {v}: unknown, copyrighted or publicDomain")));
            }
            Value::Object(o.clone())
        }
        Some(_) => return Err(bad(C, "`fields` must be an object")),
        // from the active photo: its non-empty fields among `only` (default: copyright, creator, place)
        None => {
            let id = s.active().ok_or_else(|| bad(C, "no `fields` and no active photo"))?;
            let mut m = s.catalog.photo(id).ok_or_else(|| bad(C, "no photo"))?.meta.clone();
            let only: Vec<String> =
                p.get("only").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_else(
                    || {
                        ["copyright", "copyrightStatus", "usageTerms", "copyrightUrl", "creator", "location", "city", "state", "country"]
                            .into_iter()
                            .chain(lcc_meta::CreatorContact::KEYS.iter().copied())
                            .map(String::from)
                            .collect()
                    },
                );
            let mut o = Map::new();
            for k in only.iter().filter(|k| FIELDS.contains(&k.as_str())) {
                if k == "keywords" {
                    if !m.keywords.is_empty() {
                        o.insert(k.clone(), json!(m.keywords));
                    }
                } else if k == "copyrightStatus" {
                    if !m.copyright_status.is_unknown() {
                        o.insert(k.clone(), json!(m.copyright_status.id()));
                    }
                } else if let Some(v) = list_field(&mut m, k) {
                    if !v.is_empty() {
                        o.insert(k.clone(), json!(v.clone()));
                    }
                } else if let Some(v) = text_field(&mut m, k).filter(|v| !v.trim().is_empty()) {
                    o.insert(k.clone(), json!(v.clone()));
                }
            }
            Value::Object(o)
        }
    };
    let preset = MetadataPreset { name: name.clone(), fields };
    match s.metadata_presets.iter_mut().find(|m| m.name.eq_ignore_ascii_case(&name)) {
        Some(m) => *m = preset,
        None => s.metadata_presets.push(preset),
    }
    s.save_prefs()?;
    Ok(list(s))
}

fn delete(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "metadata.deletePreset";
    let name = str_param(p, "name").ok_or_else(|| bad(C, "missing `name`"))?;
    let n = s.metadata_presets.len();
    s.metadata_presets.retain(|m| !m.name.eq_ignore_ascii_case(name));
    if s.metadata_presets.len() == n {
        return Err(bad(C, format!("no metadata preset `{name}`")));
    }
    if s.import_defaults.metadata_preset.as_deref().is_some_and(|m| m.eq_ignore_ascii_case(name)) {
        s.import_defaults.metadata_preset = None;
    }
    s.save_prefs()?;
    Ok(list(s))
}

/// photo.setMeta with the preset's fields (keywords added) on `ids` / the selection.
fn apply(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "metadata.applyPreset";
    let name = str_param(p, "name").ok_or_else(|| bad(C, "missing `name`"))?;
    let preset = s
        .metadata_presets
        .iter()
        .find(|m| m.name.eq_ignore_ascii_case(name))
        .cloned()
        .ok_or_else(|| bad(C, format!("no metadata preset `{name}`")))?;
    let mut params = Map::new();
    for (k, v) in preset.fields.as_object().into_iter().flatten() {
        params.insert(if k == "keywords" { "addKeywords".into() } else { k.clone() }, v.clone());
    }
    if let Some(ids) = p.get("ids") {
        params.insert("ids".into(), ids.clone());
    }
    s.execute("photo.setMeta", &Value::Object(params))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(query "metadata.presets", "Metadata Presets", [], None, "{} → [{name, fields}]", always, |s, _| Ok(list(s))),
        cmd!(
            "metadata.savePreset",
            "Save Metadata Preset",
            [],
            None,
            "{name, fields?: {copyright?, copyrightStatus?: unknown|copyrighted|publicDomain, usageTerms?, copyrightUrl?, creator?, creatorJobTitle?, creatorAddress?, creatorCity?, creatorState?, creatorPostalCode?, creatorCountry?, creatorPhone?, creatorEmail?, creatorWebsite?, title?, headline?, caption?, descriptionWriter?, category?, location?, city?, state?, country?, keywords?: [..], supplementalCategories?: [..], subjectCode?: [..]}, only?: [field] (from the active photo when `fields` is absent; default the copyright fields, creator and contact, place)}: adds or replaces",
            always,
            save
        ),
        cmd!("metadata.deletePreset", "Delete Metadata Preset", [], None, "{name}", always, delete),
        cmd!(
            "metadata.applyPreset",
            "Apply Metadata Preset",
            [],
            None,
            "{name, ids?}: text fields and lists replace, keywords are added; one undo step",
            always,
            apply
        ),
    ]
}
