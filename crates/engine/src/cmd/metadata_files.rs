//! Metadata preset files: our own `.lcmeta` JSON (import and export), Lightroom Classic metadata
//! presets (`.lrtemplate`, a Lua table with `type = "Metadata"` whose `value` maps
//! `com.adobe.<field>` ids to text), XMP metadata templates (an `x:xmpmeta` packet of IPTC / Dublin
//! Core fields, as File Info templates are saved), and `.zip` bundles of these.
//!
//! Read from the formats' public structure (Lua's table syntax, the XMP and IPTC Core
//! specifications) and black-box observation of files the person exported; no other program's code.
//! Fields with no counterpart here are reported by name, never a reason to refuse the file.

use lcc_catalog::CopyrightStatus;
use lcc_meta::CreatorContact;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::metadata::{FIELDS, MetadataPreset};
use super::{CommandSpec, always, bad, cmd, str_param};
use crate::preset_import::{Lua, delocalize, parse_lua, read_zip};
use crate::{Result, Session};

/// File extension of exported metadata presets (JSON).
pub const META_EXT: &str = "lcmeta";
/// `format` tag of an exported metadata-preset file.
const FILE_FORMAT: &str = "photo.metadataPresets";
/// The Lua `type` of a Lightroom Classic metadata preset.
const LR_METADATA_TYPE: &str = "Metadata";
/// More presets than this in one file are not read.
const MAX_PRESETS: usize = 10_000;

#[derive(Serialize, Deserialize)]
struct PresetFile {
    format: String,
    version: u32,
    presets: Vec<MetadataPreset>,
}

/// A metadata preset read from a file, with the fields it couldn't keep.
#[derive(Clone, Debug)]
pub struct Found {
    pub preset: MetadataPreset,
    pub unmapped: Vec<String>,
    pub file: String,
}

pub(crate) fn ext_of(name: &str) -> String {
    std::path::Path::new(name).extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default()
}

pub(crate) fn stem_of(name: &str, fallback: &str) -> String {
    std::path::Path::new(name)
        .file_stem()
        .map(|s| s.to_string_lossy().trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| fallback.into())
}

/// Whether preset field `k` holds a list of text.
fn is_list(k: &str) -> bool {
    matches!(k, "keywords" | "supplementalCategories" | "subjectCode")
}

/// Keep the `fields` entries a metadata preset can hold (text; keywords, other categories and
/// subject codes a list of text; a known copyright status); the others are returned by name.
pub fn clean_fields(fields: &Map<String, Value>) -> (Map<String, Value>, Vec<String>) {
    let mut out = Map::new();
    let mut unmapped = Vec::new();
    for (k, v) in fields {
        let ok = FIELDS.contains(&k.as_str())
            && match (k.as_str(), v) {
                (k, Value::Array(a)) if is_list(k) => a.iter().all(Value::is_string),
                ("copyrightStatus", Value::String(s)) => CopyrightStatus::parse(s).is_some(),
                (k, _) if is_list(k) || k == "copyrightStatus" => false,
                (_, Value::String(_)) => true,
                _ => false,
            };
        if ok {
            let v = match v {
                Value::String(s) => Value::String(s.chars().take(64 * 1024).collect()),
                Value::Array(a) => Value::Array(a.iter().take(MAX_PRESETS).cloned().collect()),
                other => other.clone(),
            };
            out.insert(k.clone(), v);
        } else {
            unmapped.push(k.clone());
        }
    }
    (out, unmapped)
}

/// A Lightroom Classic field id (`com.adobe.creatorWorkEmail`) → our preset field.
fn lr_field(id: &str) -> Option<&'static str> {
    let k = id.strip_prefix("com.adobe.").unwrap_or(id).to_ascii_lowercase();
    Some(match k.as_str() {
        "title" => "title",
        "caption" | "description" => "caption",
        "copyright" => "copyright",
        "copyrightstate" | "copyrightstatus" => "copyrightStatus",
        "rightsusageterms" | "usageterms" => "usageTerms",
        "copyrightinfourl" | "copyrighturl" => "copyrightUrl",
        "creator" | "artist" => "creator",
        "creatorjobtitle" => "creatorJobTitle",
        "creatoraddress" => "creatorAddress",
        "creatorcity" => "creatorCity",
        "creatorstate" | "creatorstateprovince" => "creatorState",
        "creatorzip" | "creatorpostalcode" => "creatorPostalCode",
        "creatorcountry" => "creatorCountry",
        "creatorworkphone" | "creatorphone" => "creatorPhone",
        "creatorworkemail" | "creatoremail" => "creatorEmail",
        "creatorworkwebsite" | "creatorwebsite" | "creatorurl" => "creatorWebsite",
        "headline" => "headline",
        "descriptionwriter" | "captionwriter" => "descriptionWriter",
        "iptccategory" | "category" => "category",
        "iptcothercategories" | "othercategories" | "supplementalcategories" => "supplementalCategories",
        "iptcsubjectcode" | "subjectcode" => "subjectCode",
        "location" | "sublocation" => "location",
        "city" => "city",
        "state" | "stateprovince" | "province" => "state",
        "country" => "country",
        "countrycode" | "isocountrycode" => "countryCode",
        "keywords" => "keywords",
        _ => return None,
    })
}

/// Lightroom's copyright states (`copyrighted`, `public domain`, `unknown`) as ours.
fn copyright_state(s: &str) -> Option<&'static str> {
    let k = s.chars().filter(char::is_ascii_alphanumeric).collect::<String>().to_ascii_lowercase();
    match k.as_str() {
        "copyrighted" => Some("copyrighted"),
        "publicdomain" => Some("publicDomain"),
        "unknown" | "" => Some("unknown"),
        _ => None,
    }
}

/// The metadata preset in a Lightroom Classic `.lrtemplate`, or `None` when the file is another
/// kind of template (develop, export, local adjustment…) or not Lua at all.
fn from_lrtemplate(file: &str, text: &str) -> Option<std::result::Result<Found, String>> {
    let root = parse_lua(text).ok()?;
    let kind = root.get("type").and_then(Lua::str)?;
    if !kind.eq_ignore_ascii_case(LR_METADATA_TYPE) {
        return None;
    }
    let name = root
        .get("title")
        .and_then(Lua::str)
        .or_else(|| root.get("internalName").and_then(Lua::str))
        .map(delocalize)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| stem_of(file, "Metadata Preset"));
    let Some(Lua::Table(_, values)) = root.get("value") else {
        return Some(Err("a metadata preset without a `value` table".into()));
    };
    let mut fields = Map::new();
    let mut unmapped = Vec::new();
    for (id, v) in values.iter().take(MAX_PRESETS) {
        let label = id.strip_prefix("com.adobe.").unwrap_or(id).to_string();
        let Some(field) = lr_field(id) else {
            unmapped.push(label);
            continue;
        };
        let text = match v {
            Lua::Str(s) => Some(s.clone()),
            Lua::Num(n) if n.is_finite() => Some(format!("{n}")),
            _ => None,
        };
        match (field, v) {
            (f, Lua::Table(items, _)) if is_list(f) => {
                let k: Vec<Value> = items.iter().filter_map(Lua::str).map(str::trim).filter(|s| !s.is_empty()).map(|s| json!(s)).collect();
                fields.insert(field.into(), Value::Array(k));
            }
            (f, Lua::Str(s)) if is_list(f) => {
                let k: Vec<Value> = s.split([',', ';']).map(str::trim).filter(|s| !s.is_empty()).map(|s| json!(s)).collect();
                fields.insert(field.into(), Value::Array(k));
            }
            ("copyrightStatus", _) => match text.as_deref().and_then(copyright_state) {
                Some(st) => {
                    fields.insert(field.into(), json!(st));
                }
                None => unmapped.push(label),
            },
            _ => match text {
                Some(t) => {
                    fields.insert(field.into(), json!(t));
                }
                // `true`, tables: nothing a text field can hold
                None => unmapped.push(label),
            },
        }
    }
    let (fields, more) = clean_fields(&fields);
    unmapped.extend(more);
    Some(Ok(Found { preset: MetadataPreset { name, fields: Value::Object(fields) }, unmapped, file: file.to_string() }))
}

/// XMP properties a metadata template may carry that say nothing about the photo (the tool,
/// dates, document ids): never reported.
fn xmp_noise(k: &str) -> bool {
    k.starts_with("xmpMM:") || k.starts_with("rdf:") || k.starts_with("x:") || matches!(k, "xmp:CreatorTool" | "xmp:MetadataDate" | "xmp:ModifyDate")
}

/// The fields of an XMP metadata template, or `None` when the packet is a photo's sidecar or a
/// develop preset (it carries develop, rating or capture settings) or no XMP at all.
fn from_xmp(file: &str, text: &str) -> Option<std::result::Result<Found, String>> {
    let d = lcc_meta::parse_xmp(text).ok()?;
    let props = &d.properties;
    let sidecar_like = props.keys().any(|k| {
        k.starts_with("crs:")
            || k.starts_with("lc:")
            || k.starts_with("exif:")
            || k.starts_with("tiff:")
            || k.starts_with("aux:")
            || k == "xmp:Rating"
    });
    if sidecar_like {
        return None;
    }
    let m = &d.metadata;
    let mut fields = Map::new();
    let mut put = |k: &str, v: Option<&String>| {
        if let Some(v) = v.map(|v| v.trim()).filter(|v| !v.is_empty()) {
            fields.insert(k.into(), json!(v));
        }
    };
    put("title", m.title.as_ref());
    put("caption", m.caption.as_ref());
    put("copyright", m.copyright.as_ref());
    put("headline", m.headline.as_ref());
    put("descriptionWriter", m.description_writer.as_ref());
    put("category", m.category.as_ref());
    put("usageTerms", m.usage_terms.as_ref());
    put("copyrightUrl", m.copyright_url.as_ref());
    put("creator", m.artist.as_ref());
    put("location", m.sublocation.as_ref());
    put("city", m.city.as_ref());
    put("state", m.state.as_ref());
    put("country", m.country.as_ref());
    put("countryCode", m.country_code.as_ref());
    for k in CreatorContact::KEYS {
        put(k, m.contact.get(k));
    }
    if m.copyright_marked.is_some() {
        fields.insert("copyrightStatus".into(), json!(CopyrightStatus::from_marked(m.copyright_marked).id()));
    }
    if !m.keywords.is_empty() {
        fields.insert("keywords".into(), json!(m.keywords));
    }
    if !m.supplemental_categories.is_empty() {
        fields.insert("supplementalCategories".into(), json!(m.supplemental_categories));
    }
    if !m.subject_code.is_empty() {
        fields.insert("subjectCode".into(), json!(m.subject_code));
    }
    const MAPPED: &[&str] = &[
        "dc:title",
        "photoshop:Headline",
        "dc:description",
        "photoshop:CaptionWriter",
        "photoshop:Category",
        "photoshop:SupplementalCategories",
        "Iptc4xmpCore:SubjectCode",
        "dc:rights",
        "dc:creator",
        "dc:subject",
        "xmpRights:UsageTerms",
        "xmpRights:WebStatement",
        "xmpRights:Marked",
        "Iptc4xmpCore:Location",
        "photoshop:City",
        "photoshop:State",
        "photoshop:Country",
        "photoshop:AuthorsPosition",
    ];
    let mut unmapped: Vec<String> = props
        .keys()
        .filter(|k| !MAPPED.contains(&k.as_str()) && !k.starts_with("Iptc4xmpCore:CreatorContactInfo") && !xmp_noise(k))
        // a struct's members name the struct once
        .map(|k| k.split('/').next().unwrap_or(k).to_string())
        .collect();
    unmapped.dedup();
    if fields.is_empty() {
        return if unmapped.is_empty() { None } else { Some(Err(format!("no fields this app keeps ({})", unmapped.join(", ")))) };
    }
    Some(Ok(Found {
        preset: MetadataPreset { name: stem_of(file, "Metadata Preset"), fields: Value::Object(fields) },
        unmapped,
        file: file.to_string(),
    }))
}

/// Our own `.lcmeta` file.
fn from_native(file: &str, text: &str) -> Option<std::result::Result<Vec<Found>, String>> {
    let v: Value = serde_json::from_str(text).ok()?;
    if v.get("format").and_then(Value::as_str) != Some(FILE_FORMAT) {
        return None;
    }
    let Some(list) = v.get("presets").and_then(Value::as_array) else { return Some(Err("no `presets` list".into())) };
    let mut out = Vec::new();
    let empty = Map::new();
    for p in list.iter().take(MAX_PRESETS) {
        let Some(name) = p.get("name").and_then(Value::as_str).map(str::trim).filter(|n| !n.is_empty()) else { continue };
        let (fields, unmapped) = clean_fields(p.get("fields").and_then(Value::as_object).unwrap_or(&empty));
        out.push(Found {
            preset: MetadataPreset { name: name.chars().take(256).collect(), fields: Value::Object(fields) },
            unmapped,
            file: file.to_string(),
        });
    }
    Some(Ok(out))
}

/// The metadata presets in file `file` (`bytes`), or `None` when it holds none (another kind of
/// preset, or not a preset at all). Zips are looked into (one level).
pub fn parse_bytes(file: &str, bytes: &[u8]) -> Option<std::result::Result<Vec<Found>, String>> {
    let text = || String::from_utf8_lossy(bytes).trim_start_matches('\u{feff}').to_string();
    match ext_of(file).as_str() {
        "lrtemplate" => from_lrtemplate(file, &text()).map(|r| r.map(|f| vec![f])),
        "xmp" => from_xmp(file, &text()).map(|r| r.map(|f| vec![f])),
        "zip" => {
            let entries = read_zip(bytes).ok()?;
            let mut out = Vec::new();
            for (inner, data) in entries {
                if ext_of(&inner) == "zip" {
                    continue;
                }
                if let Some(Ok(list)) = parse_bytes(&format!("{file}/{inner}"), &data) {
                    out.extend(list);
                }
            }
            (!out.is_empty()).then_some(Ok(out))
        }
        e if e == META_EXT || e == "json" => from_native(file, &text()),
        _ => None,
    }
}

/// `name`, or `name (2)`, `name (3)`… — the first `taken` doesn't know.
pub(crate) fn free_name(name: &str, taken: impl Fn(&str) -> bool) -> String {
    if !taken(name) {
        return name.to_string();
    }
    (2..100_000).map(|i| format!("{name} ({i})")).find(|n| !taken(n)).unwrap_or_else(|| format!("{name} (copy)"))
}

/// Add what was found: an identical preset (same name and fields) is skipped, a different one of
/// a name in use is added as `name (2)`. Saves the library prefs once → the report rows.
pub fn add_found(s: &mut Session, found: Vec<Found>, dry: bool) -> Result<Vec<Value>> {
    let mut rows = Vec::new();
    let mut changed = false;
    for f in found {
        let same = s.metadata_presets.iter().any(|m| m.name.eq_ignore_ascii_case(&f.preset.name) && m.fields == f.preset.fields);
        if same {
            rows.push(json!({"name": f.preset.name, "file": f.file, "unmapped": f.unmapped, "skipped": true}));
            continue;
        }
        let name = free_name(&f.preset.name, |n| s.metadata_presets.iter().any(|m| m.name.eq_ignore_ascii_case(n)));
        rows.push(json!({"name": name, "file": f.file, "unmapped": f.unmapped, "fields": f.preset.fields}));
        if !dry {
            s.metadata_presets.push(MetadataPreset { name, fields: f.preset.fields });
            changed = true;
        }
    }
    if changed {
        s.save_prefs()?;
    }
    Ok(rows)
}

pub(crate) fn strings(p: &Value, key: &str) -> Option<Vec<String>> {
    p.get(key).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
}

fn import(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "metadata.importPresets";
    let paths = strings(p, "paths").filter(|v| !v.is_empty()).ok_or_else(|| bad(C, "missing `paths`"))?;
    let dry = p.get("dryRun").and_then(Value::as_bool).unwrap_or(false);
    let mut found = Vec::new();
    let mut failed = Vec::new();
    for f in crate::presets::expand_preset_paths(&paths) {
        let bytes = match super::mask_effect_files::read_capped(&f) {
            Ok(b) => b,
            Err(e) => {
                failed.push(json!([f, e]));
                continue;
            }
        };
        match parse_bytes(&f, &bytes) {
            Some(Ok(list)) => found.extend(list),
            Some(Err(e)) => failed.push(json!([f, e])),
            None => failed.push(json!([f, "not a metadata preset"])),
        }
    }
    let imported = add_found(s, found, dry)?;
    Ok(json!({"imported": imported, "failed": failed}))
}

fn export(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "metadata.exportPresets";
    let path = str_param(p, "path").filter(|x| !x.trim().is_empty()).ok_or_else(|| bad(C, "missing `path`"))?;
    let mut path = std::path::PathBuf::from(path);
    if path.extension().is_none() {
        path.set_extension(META_EXT);
    }
    let names = strings(p, "names");
    let chosen: Vec<MetadataPreset> =
        s.metadata_presets.iter().filter(|m| names.as_ref().is_none_or(|n| n.iter().any(|x| x.eq_ignore_ascii_case(&m.name)))).cloned().collect();
    if chosen.is_empty() {
        return Err(bad(C, "no metadata presets to export (save one first, or check `names`)"));
    }
    let count = chosen.len();
    let text =
        serde_json::to_string_pretty(&PresetFile { format: FILE_FORMAT.into(), version: 1, presets: chosen }).map_err(|e| bad(C, e.to_string()))?;
    let target = path.to_string_lossy().to_string();
    s.check_write_target(&target).map_err(|e| bad(C, e))?;
    crate::export::write_file_durable(&target, text.as_bytes()).map_err(crate::EngineError::Other)?;
    Ok(json!({"path": target, "count": count}))
}

fn rename(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "metadata.renamePreset";
    let name = str_param(p, "name").map(str::trim).ok_or_else(|| bad(C, "missing `name`"))?.to_string();
    let new = str_param(p, "newName").map(str::trim).filter(|n| !n.is_empty()).ok_or_else(|| bad(C, "missing `newName`"))?.to_string();
    if !new.eq_ignore_ascii_case(&name) && s.metadata_presets.iter().any(|m| m.name.eq_ignore_ascii_case(&new)) {
        return Err(bad(C, format!("a metadata preset `{new}` already exists")));
    }
    let m =
        s.metadata_presets.iter_mut().find(|m| m.name.eq_ignore_ascii_case(&name)).ok_or_else(|| bad(C, format!("no metadata preset `{name}`")))?;
    m.name.clone_from(&new);
    if s.import_defaults.metadata_preset.as_deref().is_some_and(|m| m.eq_ignore_ascii_case(&name)) {
        s.import_defaults.metadata_preset = Some(new);
    }
    s.save_prefs()?;
    Ok(json!(s.metadata_presets))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "metadata.importPresets",
            "Import Metadata Presets",
            [],
            None,
            "{paths: [file or folder], dryRun?}: .lcmeta, Lightroom Classic metadata presets (.lrtemplate), XMP metadata templates and .zip bundles of these; a preset whose name is taken by a different one is added as `name (2)`, an identical one skipped; preset.import (Import Profiles & Presets…) routes these files here too → {imported: [{name, file, unmapped: [fields not carried over], fields?, skipped?}], failed: [[file, reason]]}",
            always,
            import
        ),
        cmd!(
            "metadata.exportPresets",
            "Export Metadata Presets",
            [],
            None,
            "{path, names?: [preset name] (default all)} → {path, count}; writes a .lcmeta JSON file",
            always,
            export
        ),
        cmd!("metadata.renamePreset", "Rename Metadata Preset", [], None, "{name, newName} → presets", always, rename),
    ]
}
