//! Export preset files: our own `.lcexport` JSON (import and export), Lightroom Classic export
//! presets (`.lrtemplate`, a Lua table with `type = "export"` whose `value` holds the Export
//! dialog's settings as `LR_*` / `export_*` keys), and `.zip` bundles of these.
//!
//! Read from the formats' public structure (Lua's table syntax) and black-box observation of files
//! the person exported; no other program's code. Settings with no counterpart here are reported by
//! name, never a reason to refuse the file.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::metadata_files::{ext_of, free_name, stem_of, strings};
use super::{CommandSpec, always, bad, cmd, str_param};
use crate::export::{ExportOptions, ExportPreset, builtin_presets, subfolder_problem};
use crate::preset_import::{Lua, delocalize, parse_lua, read_zip};
use crate::{Result, Session};

/// File extension of exported export presets (JSON).
pub const EXPORT_PRESET_EXT: &str = "lcexport";
/// `format` tag of an exported export-preset file.
const FILE_FORMAT: &str = "photo.exportPresets";
/// More presets than this in one file are not read.
const MAX_PRESETS: usize = 10_000;

#[derive(Serialize, Deserialize)]
struct PresetFile {
    format: String,
    version: u32,
    presets: Vec<ExportPreset>,
}

/// An export preset read from a file, with the settings it couldn't keep.
#[derive(Clone, Debug)]
pub struct Found {
    pub preset: ExportPreset,
    pub unmapped: Vec<String>,
    pub file: String,
}

/// A Lightroom file-naming template (`{{image_name}}-{{naming_sequenceNumber_3Digits}}`) as ours
/// (`{name}-{seq:3}`), with the tokens we have no counterpart for (left out).
pub fn lr_naming(tokens: &str) -> (String, Vec<String>) {
    let mut out = String::new();
    let mut unknown = Vec::new();
    let mut rest = tokens;
    while let Some(i) = rest.find("{{") {
        out.push_str(rest.get(..i).unwrap_or_default());
        let after = rest.get(i + 2..).unwrap_or_default();
        let Some(j) = after.find("}}") else {
            out.push_str(rest.get(i..).unwrap_or_default());
            rest = "";
            break;
        };
        let tok = after.get(..j).unwrap_or_default().trim();
        rest = after.get(j + 2..).unwrap_or_default();
        let t = tok.strip_prefix("com.adobe.").unwrap_or(tok);
        let lower = t.to_ascii_lowercase();
        // sequence numbers: `naming_sequenceNumber_3Digits`, `naming_imageNumber_5Digits`, …
        let digits = |s: &str| -> Option<usize> {
            let n = s.rsplit('_').next()?.trim_end_matches("digits").trim_end_matches("digit");
            n.parse::<usize>().ok().filter(|n| (1..=9).contains(n))
        };
        // Lightroom spells month `MM` and minutes `mm`: these are case-sensitive
        let exact = match t {
            "date_MM" => Some("{date:%m}"),
            "date_mm" => Some("{date:%M}"),
            "date_ss" => Some("{date:%S}"),
            "date_hh" | "date_HH" => Some("{date:%H}"),
            _ => None,
        };
        let mapped = if let Some(e) = exact {
            Some(e.to_string())
        } else {
            match lower.as_str() {
                "image_name" | "image_filename" | "image_originalname" => Some("{name}".to_string()),
                "image_originalnumbersuffix"
                | "image_filenumbersuffix"
                | "image_filename_number_suffix"
                | "image_originalname_number_suffix"
                | "image_original_number_suffix"
                | "image_file_number_suffix"
                | "number_suffix" => Some("{num}".into()),
                "custom_token" | "custom_text" | "naming_custom_token" => Some("{custom}".into()),
                "title" | "metadata_title" => Some("{title}".into()),
                "creator" | "artist" | "metadata_creator" => Some("{creator}".into()),
                "model" | "camera" | "cameramodel" | "exif_model" => Some("{camera}".into()),
                "lens" | "exif_lens" => Some("{lens}".into()),
                "isospeedrating" | "iso" | "exif_iso" => Some("{iso}".into()),
                "rating" | "isorating" => Some("{rating}".into()),
                "folder_name" | "foldername" | "image_foldername" => Some("{folder}".into()),
                "date_yyyy" | "date_year" | "date_fullyear" => Some("{date:%Y}".into()),
                "date_yy" | "date_shortyear" => Some("{date:%y}".into()),
                "date_dd" | "date_day" => Some("{date:%d}".into()),
                "date_month" | "date_fullmonth" => Some("{date:%B}".into()),
                "date_mon" | "date_shortmonth" => Some("{date:%b}".into()),
                "date_yyyymmdd" => Some("{date:%Y%m%d}".into()),
                "date_yymmdd" => Some("{date:%y%m%d}".into()),
                "date_local" | "date_iso" => Some("{date:%Y-%m-%d}".into()),
                "date_hour" => Some("{date:%H}".into()),
                "date_minute" => Some("{date:%M}".into()),
                "date_second" => Some("{date:%S}".into()),
                s if s.starts_with("naming_sequencenumber")
                    || s.starts_with("naming_imagenumber")
                    || s.starts_with("naming_totalnumber")
                    || s.starts_with("sequencenumber")
                    || s.starts_with("imagenumber") =>
                {
                    Some(match digits(s) {
                        Some(n) => format!("{{seq:{n}}}"),
                        None => "{seq:1}".into(),
                    })
                }
                _ => None,
            }
        };
        match mapped {
            Some(m) => out.push_str(&m),
            None => unknown.push(tok.to_string()),
        }
    }
    out.push_str(rest);
    (out, unknown)
}

/// Look up an export preset key in `v`: Lightroom Classic `.lrtemplate` files on disk store keys
/// without the `LR_` prefix (e.g. `format`, `jpeg_quality`, `size_doConstrain`, `collisionHandling`)
/// or with `export_` (`export_colorSpace`, `export_bitDepth`, `export_destinationType`), while
/// SDK / publish templates prefix them with `LR_`.
fn lget<'a>(v: &'a Lua, k: &str) -> Option<&'a Lua> {
    let base = k.strip_prefix("LR_").unwrap_or(k);
    let short = base.strip_prefix("export_").unwrap_or(base);
    let candidates = [
        k.to_string(),
        base.to_string(),
        format!("LR_{base}"),
        short.to_string(),
        format!("LR_{short}"),
        format!("export_{short}"),
        format!("LR_export_{short}"),
    ];
    for c in &candidates {
        if let Some(found) = v.get(c) {
            return Some(found);
        }
    }
    if let Lua::Table(_, m) = v {
        for c in &candidates {
            if let Some((_, found)) = m.iter().find(|(key, _)| key.eq_ignore_ascii_case(c)) {
                return Some(found);
            }
        }
    }
    None
}

fn lstr<'a>(v: &'a Lua, k: &str) -> Option<&'a str> {
    lget(v, k).and_then(Lua::str)
}

fn lnum(v: &Lua, k: &str) -> Option<f64> {
    match lget(v, k)? {
        Lua::Num(n) if n.is_finite() => Some(*n),
        Lua::Str(s) => s.trim().parse::<f64>().ok().filter(|n| n.is_finite()),
        _ => None,
    }
}

fn lbool(v: &Lua, k: &str) -> Option<bool> {
    match lget(v, k)? {
        Lua::Bool(b) => Some(*b),
        Lua::Num(n) => Some(*n != 0.0),
        Lua::Str(s) => match s.trim().to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" => Some(true),
            "false" | "0" | "no" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

/// The settings of a Lightroom Classic export preset's `value` table as `app.export` params, with
/// what couldn't be carried over.
pub fn lr_export_params(v: &Lua) -> (Map<String, Value>, Vec<String>) {
    let mut o = Map::new();
    let mut unmapped: Vec<String> = Vec::new();
    // ---- where to
    if let Some(service) = lstr(v, "exportServiceProvider").filter(|s| !s.eq_ignore_ascii_case("com.adobe.ag.export.file")) {
        unmapped.push(format!("export service `{service}` (exported to files instead)"));
    }
    let folder = lstr(v, "export_destinationPathPrefix").map(str::trim).filter(|p| !p.is_empty());
    match lstr(v, "export_destinationType").map(str::to_ascii_lowercase).as_deref() {
        Some("sourcefolder" | "sameasoriginal") => {
            o.insert("destinationMode".into(), json!("sameAsOriginal"));
        }
        Some("pickfolderlater" | "picklater" | "chooselater") => {
            o.insert("destinationMode".into(), json!("chooseLater"));
        }
        Some("specificfolder") | None => {
            if let Some(f) = folder {
                o.insert("destinationMode".into(), json!("specificFolder"));
                o.insert("dir".into(), json!(f));
                if !std::path::Path::new(f).is_dir() {
                    unmapped.push(format!("folder `{f}` (not on this computer: made when exporting)"));
                }
            }
        }
        Some(other) => unmapped.push(format!("destination `{other}` (choose the folder in the Export dialog)")),
    }
    if lbool(v, "export_useParentFolder") == Some(true) {
        unmapped.push("put in parent folder".into());
    }
    if lbool(v, "export_useSubfolder") == Some(true)
        && let Some(sub) = lstr(v, "export_destinationPathSuffix").map(str::trim).filter(|s| !s.is_empty())
    {
        match subfolder_problem(sub) {
            None => {
                o.insert("subfolder".into(), json!(sub));
            }
            Some(why) => unmapped.push(format!("subfolder `{sub}` ({why})")),
        }
    }
    match lstr(v, "export_collisionHandling").map(str::to_ascii_lowercase).as_deref() {
        Some("overwrite" | "overwritewithoutwarning") => {
            o.insert("conflict".into(), json!("overwrite"));
        }
        Some("skip") => {
            o.insert("conflict".into(), json!("skip"));
        }
        Some("rename" | "ask") | None => {}
        Some(other) => unmapped.push(format!("existing files `{other}`")),
    }
    // ---- file naming
    if lbool(v, "renamingTokensOn") == Some(true) {
        let tokens =
            lstr(v, "tokens").filter(|t| !t.trim().is_empty()).or_else(|| lstr(v, "tokensArchivedToString2").filter(|t| !t.trim().is_empty()));
        if let Some(tokens) = tokens {
            let (naming, unknown) = lr_naming(tokens);
            o.insert("naming".into(), json!(naming));
            unmapped.extend(unknown.into_iter().map(|t| format!("file name token `{t}`")));
        }
        if let Some(c) = lstr(v, "tokenCustomString") {
            o.insert("customText".into(), json!(c));
        }
        if let Some(n) = lnum(v, "initialSequenceNumber").filter(|n| (0.0..=999_999.0).contains(n)) {
            o.insert("startNumber".into(), json!(n.round() as i64));
        }
    }
    if lstr(v, "extensionCase").is_some_and(|c| c.eq_ignore_ascii_case("uppercase") || c.eq_ignore_ascii_case("ucase")) {
        unmapped.push("uppercase file extension (lowercase written instead)".into());
    }
    // ---- file settings
    let format = lstr(v, "format").unwrap_or("JPEG");
    let ours = match format.trim().to_ascii_uppercase().as_str() {
        "JPEG" | "JPG" => Some("jpeg"),
        "TIFF" | "TIF" => Some("tiff"),
        "PSD" => Some("psd"),
        "PNG" => Some("png"),
        "DNG" => {
            if lstr(v, "dng_conversionMethod").is_some_and(|m| m.eq_ignore_ascii_case("convertToLinear")) {
                Some("renderedDng")
            } else {
                Some("dng")
            }
        }
        "ORIGINAL" => Some("original"),
        "AVIF" => Some("avif"),
        "JXL" | "JPEGXL" | "JPEG_XL" | "JPEG XL" => Some("jxl"),
        "WEBP" => Some("webp"),
        _ => None,
    };
    match ours {
        Some(f) => {
            o.insert("format".into(), json!(f));
        }
        None => unmapped.push(format!("format `{format}` (JPEG instead)")),
    }
    let quality_keys: &[&str] = match ours {
        Some("avif") => &["avif_quality", "jpeg_quality", "jxl_quality", "heic_quality", "quality"],
        Some("jxl") => &["jxl_quality", "jpeg_quality", "avif_quality", "heic_quality", "quality"],
        _ => &["jpeg_quality", "avif_quality", "jxl_quality", "heic_quality", "quality"],
    };
    let quality = quality_keys.iter().find_map(|k| lnum(v, k));
    if let Some(q) = quality.filter(|q| *q >= 0.0) {
        // 0..1 in Lightroom; tolerate 0..100
        let q = if q <= 1.0 { q * 100.0 } else { q };
        o.insert("quality".into(), json!(q.round().clamp(1.0, 100.0) as i64));
    }
    if lbool(v, "jpeg_useLimitSize") == Some(true)
        && let Some(kb) = lnum(v, "jpeg_limitSize").filter(|k| *k >= 1.0 && *k <= u32::MAX as f64)
    {
        o.insert("limitKb".into(), json!(kb.round() as i64));
    }
    match lstr(v, "export_colorSpace").map(|s| s.to_ascii_lowercase().replace([' ', '_', '-', '(', ')'], "")).as_deref() {
        Some("srgb" | "srgbiec6196621") => {
            o.insert("colorSpace".into(), json!("srgb"));
        }
        Some("adobergb" | "adobergb1998") => {
            o.insert("colorSpace".into(), json!("adobeRgb"));
        }
        Some("prophotorgb" | "prophoto") => {
            o.insert("colorSpace".into(), json!("proPhoto"));
        }
        Some("displayp3" | "p3" | "p3d65") => {
            o.insert("colorSpace".into(), json!("displayP3"));
        }
        Some("rec2020" | "hdrrec2020" | "bt2020") => {
            o.insert("colorSpace".into(), json!("rec2020"));
        }
        None => {}
        Some(other) => unmapped.push(format!("colour space `{other}` (sRGB instead)")),
    }
    if let Some(b) = lnum(v, "export_bitDepth").map(|b| b.round() as i64).filter(|b| matches!(b, 8 | 10 | 16 | 32)) {
        o.insert("bitDepth".into(), json!(b));
    }
    match lstr(v, "tiff_compressionMethod").map(str::to_ascii_lowercase).as_deref() {
        Some(m) if m.ends_with("none") => {
            o.insert("tiffCompression".into(), json!("none"));
        }
        Some(m) if m.ends_with("lzw") => {
            o.insert("tiffCompression".into(), json!("lzw"));
        }
        Some(m) if m.ends_with("zip") || m.ends_with("deflate") => {
            o.insert("tiffCompression".into(), json!("zip"));
        }
        _ => {}
    }
    if ours == Some("dng") {
        if lbool(v, "dng_lossyCompression") == Some(true) {
            o.insert("dngCompression".into(), json!("lossy"));
        } else if lbool(v, "dng_compressed") == Some(false) {
            o.insert("dngCompression".into(), json!("uncompressed"));
        } else if lbool(v, "dng_compressed") == Some(true) {
            o.insert("dngCompression".into(), json!("lossless"));
        }
        if lbool(v, "dng_embedRAW") == Some(true) {
            unmapped.push("embed original raw file in DNG".into());
        }
    }
    if lbool(v, "hdrOutput") == Some(true) {
        o.insert("hdr".into(), json!(true));
    }
    if lbool(v, "jxl_lossless") == Some(true) {
        o.insert("jxlLossless".into(), json!(true));
    }
    // ---- image sizing: pixels from the units and resolution
    let res = lnum(v, "size_resolution").filter(|r| *r > 0.0 && *r < 1.0e6);
    let per_cm = lstr(v, "size_resolutionUnits").is_some_and(|u| {
        let u = u.to_ascii_lowercase();
        u.starts_with("cm") || u == "centimeters" || u == "pixelspercentimeter" || u == "ppc"
    });
    let ppi = res.map(|r| if per_cm { r * 2.54 } else { r });
    if let Some(p) = ppi {
        o.insert("ppi".into(), json!(p.round().clamp(1.0, u16::MAX as f64) as i64));
    }
    let constrain = lbool(v, "size_doConstrain").or_else(|| lbool(v, "size_userWantsConstrain"));
    if constrain == Some(true) {
        let units = lstr(v, "size_units").map(str::to_ascii_lowercase).unwrap_or_else(|| "pixels".into());
        let px = |n: f64| -> f64 {
            let r = ppi.unwrap_or(240.0);
            match units.as_str() {
                "inch" | "inches" | "in" => n * r,
                "cm" | "centimeters" => n * r / 2.54,
                _ => n,
            }
        };
        let w = lnum(v, "size_maxWidth").or_else(|| lnum(v, "size_dimension")).filter(|n| *n > 0.0).map(px);
        let h = lnum(v, "size_maxHeight").or_else(|| lnum(v, "size_dimension")).filter(|n| *n > 0.0).map(px);
        let whole = |n: f64| json!(n.round().clamp(1.0, 1.0e6) as i64);
        match lstr(v, "size_resizeType").map(str::to_ascii_lowercase).as_deref() {
            Some("longedge" | "long" | "l") => {
                if let Some(n) = w.into_iter().chain(h).reduce(f64::max) {
                    o.insert("longEdge".into(), whole(n));
                }
            }
            Some("shortedge" | "short" | "s") => {
                if let Some(n) = w.into_iter().chain(h).reduce(f64::max) {
                    o.insert("shortEdge".into(), whole(n));
                }
            }
            Some("width" | "w") => {
                if let Some(n) = w.or(h) {
                    o.insert("width".into(), whole(n));
                }
            }
            Some("height" | "h") => {
                if let Some(n) = h.or(w) {
                    o.insert("height".into(), whole(n));
                }
            }
            Some("megapixels" | "mp") => {
                if let Some(mp) = lnum(v, "size_megapixels").filter(|m| *m > 0.0 && *m < 1.0e4) {
                    o.insert("megapixels".into(), json!(mp));
                }
            }
            Some("percentage" | "percent" | "pct") => {
                if let Some(pc) = lnum(v, "size_percentage").filter(|m| *m > 0.0 && *m <= 1.0e4) {
                    o.insert("percent".into(), json!(pc));
                }
            }
            Some("dimensions" | "dim") => match (w, h) {
                (Some(w), Some(h)) => {
                    o.insert("width".into(), whole(w));
                    o.insert("height".into(), whole(h));
                    o.insert("fit".into(), json!("dimensions"));
                }
                (Some(w), None) => {
                    o.insert("width".into(), whole(w));
                }
                (None, Some(h)) => {
                    o.insert("height".into(), whole(h));
                }
                (None, None) => {}
            },
            // width & height: fit inside W × H
            Some("wh" | "widthheight" | "widthandheight") | None => match (w, h) {
                (Some(w), Some(h)) => {
                    o.insert("width".into(), whole(w));
                    o.insert("height".into(), whole(h));
                    o.insert("fit".into(), json!("widthHeight"));
                }
                (Some(w), None) => {
                    o.insert("width".into(), whole(w));
                }
                (None, Some(h)) => {
                    o.insert("height".into(), whole(h));
                }
                (None, None) => {}
            },
            Some(other) => unmapped.push(format!("resize to `{other}` (full size instead)")),
        }
        if let Some(b) = lbool(v, "size_doNotEnlarge") {
            o.insert("dontEnlarge".into(), json!(b));
        }
    } else if constrain == Some(false) || lstr(v, "format").is_some() {
        o.insert("longEdge".into(), json!(0)); // full size
    }
    // ---- output sharpening
    if lbool(v, "outputSharpeningOn") == Some(true) {
        let media = lstr(v, "outputSharpeningMedia").map(str::to_ascii_lowercase).unwrap_or_else(|| "screen".into());
        let media = match media.as_str() {
            "matte" | "mattepaper" | "matte_paper" => "matte",
            "glossy" | "glossypaper" | "glossy_paper" => "glossy",
            _ => "screen",
        };
        o.insert("sharpen".into(), json!(media));
        let level = match lnum(v, "outputSharpeningLevel").map(|l| l.round() as i64) {
            Some(1) => "low",
            Some(3) => "high",
            Some(_) => "standard",
            None => match lstr(v, "outputSharpeningLevel").map(str::to_ascii_lowercase).as_deref() {
                Some("low") => "low",
                Some("high") => "high",
                _ => "standard",
            },
        };
        o.insert("sharpenAmount".into(), json!(level));
    } else if lbool(v, "outputSharpeningOn") == Some(false) {
        o.insert("sharpen".into(), json!("none"));
    }
    // ---- metadata
    let policy = lstr(v, "embeddedMetadataOption").map(|s| s.to_ascii_lowercase());
    match policy.as_deref() {
        Some("all") => {
            o.insert("metadata".into(), json!("all"));
        }
        Some("allexceptcamerainfo" | "allexceptcameraandcamerarawinfo" | "allexceptcamerarawinfo" | "allexceptcamera") => {
            o.insert("metadata".into(), json!(if policy.as_deref() == Some("allexceptcamerarawinfo") { "all" } else { "allExceptCamera" }));
        }
        Some("copyrightonly" | "copyright") => {
            o.insert("metadata".into(), json!("copyright"));
        }
        Some("copyrightandcontactonly" | "copyrightandcontact") => {
            o.insert("metadata".into(), json!("copyrightContact"));
        }
        Some("none") => {
            o.insert("metadata".into(), json!("none"));
        }
        Some(other) => unmapped.push(format!("metadata `{other}`")),
        None => {}
    }
    if lbool(v, "minimizeEmbeddedMetadata") == Some(true)
        && !matches!(policy.as_deref(), Some("copyrightonly" | "copyright" | "copyrightandcontactonly" | "copyrightandcontact" | "none"))
    {
        o.insert("metadata".into(), json!("copyright"));
    }
    if let Some(b) = lbool(v, "removeLocationMetadata") {
        o.insert("removeLocation".into(), json!(b));
    }
    // ---- what the app can't do (reported when it is on)
    if lbool(v, "useWatermark") == Some(true) {
        unmapped.push("watermark (pick one in the Export dialog)".into());
    }
    if matches!(lget(v, "exportFiltersFromPlugins"), Some(Lua::Table(a, m)) if !a.is_empty() || !m.is_empty()) {
        unmapped.push("post-processing plug-in actions".into());
    }
    if lstr(v, "export_postProcessing")
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .is_some_and(|p| !p.eq_ignore_ascii_case("doNothing") && !p.eq_ignore_ascii_case("none"))
    {
        unmapped.push("after export action".into());
    }
    if lbool(v, "reimportExportedPhoto") == Some(true) {
        unmapped.push("add to this catalog".into());
    }
    if lbool(v, "removeFaceMetadata") == Some(false) {
        unmapped.push("person info (never written)".into());
    }
    if lbool(v, "includeVideoFiles") == Some(false) || lstr(v, "export_videoFileHandling").is_some_and(|s| s.eq_ignore_ascii_case("exclude")) {
        unmapped.push("exclude video files".into());
    }
    let mut params = Value::Object(o);
    // Lightroom Classic stores `export_bitDepth = 16` in the table even when `format = "JPEG"`
    ExportOptions::drop_unfit_depth(&mut params, Default::default());
    let o = match params {
        Value::Object(m) => m,
        _ => Map::new(),
    };
    (o, unmapped)
}

/// The export preset in a Lightroom Classic `.lrtemplate`, or `None` when the file is another
/// kind of template or not Lua at all.
fn from_lrtemplate(file: &str, text: &str) -> Option<std::result::Result<Found, String>> {
    let root = parse_lua(text).ok()?;
    let value = root.get("value");
    let kind = root.get("type").and_then(Lua::str).map(str::trim);
    let typed =
        kind.is_some_and(|t| t.eq_ignore_ascii_case("export") || t.eq_ignore_ascii_case("exportpreset") || t.eq_ignore_ascii_case("export_preset"));
    let other_kind = kind.is_some_and(|t| !t.is_empty() && !typed);
    let looks = !other_kind
        && value.is_some_and(|v| {
            v.get("settings").is_none()
                && (lget(v, "format").is_some()
                    || lget(v, "exportServiceProvider").is_some()
                    || lget(v, "export_destinationType").is_some()
                    || lget(v, "jpeg_quality").is_some()
                    || lget(v, "size_doConstrain").is_some())
        });
    if !typed && !looks {
        return None;
    }
    let Some(v @ Lua::Table(..)) = value else { return Some(Err("an export preset without a `value` table".into())) };
    let name = root
        .get("title")
        .and_then(Lua::str)
        .or_else(|| root.get("internalName").and_then(Lua::str))
        .map(delocalize)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| stem_of(file, "Export Preset"));
    let (params, unmapped) = lr_export_params(v);
    let params = Value::Object(params);
    if let Err(e) = ExportOptions::validate("export.importPresets", &params) {
        return Some(Err(e.to_string()));
    }
    Some(Ok(Found { preset: ExportPreset { name, params, unmapped: unmapped.clone() }, unmapped, file: file.to_string() }))
}

/// Our own `.lcexport` file: unknown keys (a newer version's) are dropped and reported, a preset
/// the exporter would refuse is skipped with its reason.
fn from_native(file: &str, text: &str) -> Option<std::result::Result<Vec<Found>, String>> {
    let v: Value = serde_json::from_str(text).ok()?;
    if v.get("format").and_then(Value::as_str) != Some(FILE_FORMAT) {
        return None;
    }
    let Some(list) = v.get("presets").and_then(Value::as_array) else { return Some(Err("no `presets` list".into())) };
    let mut out = Vec::new();
    for p in list.iter().take(MAX_PRESETS) {
        let Some(name) = p.get("name").and_then(Value::as_str).map(str::trim).filter(|n| !n.is_empty()) else { continue };
        let Some(raw) = p.get("params").filter(|x| x.is_object()) else { continue };
        let params = ExportOptions::known_keys_only(raw);
        let mut unmapped: Vec<String> = raw.as_object().into_iter().flatten().map(|(k, _)| k.clone()).filter(|k| params.get(k).is_none()).collect();
        let mut params = params;
        if let Some(o) = params.as_object_mut() {
            o.retain(|k, _| !matches!(k.as_str(), "ids" | "id" | "path" | "preset" | "background" | "pluginDestination"));
        }
        if let Err(e) = ExportOptions::validate("export.importPresets", &params) {
            unmapped.push(format!("preset skipped: {e}"));
            continue;
        }
        out.push(Found {
            preset: ExportPreset { name: name.chars().take(256).collect(), params, unmapped: unmapped.clone() },
            unmapped,
            file: file.to_string(),
        });
    }
    Some(Ok(out))
}

/// The export presets in file `file` (`bytes`), or `None` when it holds none. Zips are looked
/// into (one level).
pub fn parse_bytes(file: &str, bytes: &[u8]) -> Option<std::result::Result<Vec<Found>, String>> {
    let text = || String::from_utf8_lossy(bytes).trim_start_matches('\u{feff}').to_string();
    match ext_of(file).as_str() {
        "lrtemplate" => from_lrtemplate(file, &text()).map(|r| r.map(|f| vec![f])),
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
        e if e == EXPORT_PRESET_EXT || e == "json" => from_native(file, &text()),
        _ => None,
    }
}

/// Add what was found: identical (name and settings) presets are skipped, a name in use (or a
/// built-in's) gets ` (2)`. Saves the library prefs once → the report rows.
pub fn add_found(s: &mut Session, found: Vec<Found>, dry: bool) -> Result<Vec<Value>> {
    let builtins: Vec<String> = builtin_presets().into_iter().map(|b| b.name).collect();
    let mut rows = Vec::new();
    let mut changed = false;
    for f in found {
        let same = s.export_presets.iter().any(|x| x.name.eq_ignore_ascii_case(&f.preset.name) && x.params == f.preset.params);
        if same {
            rows.push(json!({"name": f.preset.name, "file": f.file, "unmapped": f.unmapped, "skipped": true}));
            continue;
        }
        let name = free_name(&f.preset.name, |n| {
            builtins.iter().any(|b| b.eq_ignore_ascii_case(n)) || s.export_presets.iter().any(|x| x.name.eq_ignore_ascii_case(n))
        });
        rows.push(json!({"name": name, "file": f.file, "unmapped": f.unmapped, "params": f.preset.params}));
        if !dry {
            s.export_presets.push(ExportPreset { name, params: f.preset.params, unmapped: f.unmapped });
            changed = true;
        }
    }
    if changed {
        s.save_prefs()?;
    }
    Ok(rows)
}

fn import(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "export.importPresets";
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
            None => failed.push(json!([f, "not an export preset"])),
        }
    }
    let imported = add_found(s, found, dry)?;
    Ok(json!({"imported": imported, "failed": failed}))
}

fn export(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "export.exportPresets";
    let path = str_param(p, "path").filter(|x| !x.trim().is_empty()).ok_or_else(|| bad(C, "missing `path`"))?;
    let mut path = std::path::PathBuf::from(path);
    if path.extension().is_none() {
        path.set_extension(EXPORT_PRESET_EXT);
    }
    let names = strings(p, "names");
    let chosen: Vec<ExportPreset> = s
        .export_presets
        .iter()
        .filter(|x| names.as_ref().is_none_or(|n| n.iter().any(|y| y.eq_ignore_ascii_case(&x.name))))
        .map(|x| ExportPreset { name: x.name.clone(), params: x.params.clone(), unmapped: Vec::new() })
        .collect();
    if chosen.is_empty() {
        return Err(bad(C, "no user export presets to export (save one first, or check `names`)"));
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
    const C: &str = "export.renamePreset";
    let name = str_param(p, "name").map(str::trim).ok_or_else(|| bad(C, "missing `name`"))?.to_string();
    let new = str_param(p, "newName").map(str::trim).filter(|n| !n.is_empty()).ok_or_else(|| bad(C, "missing `newName`"))?.to_string();
    if builtin_presets().iter().any(|b| b.name.eq_ignore_ascii_case(&new)) {
        return Err(bad(C, format!("`{new}` is a built-in preset; choose another name")));
    }
    if !new.eq_ignore_ascii_case(&name) && s.export_presets.iter().any(|x| x.name.eq_ignore_ascii_case(&new)) {
        return Err(bad(C, format!("an export preset `{new}` already exists")));
    }
    let x =
        s.export_presets.iter_mut().find(|x| x.name.eq_ignore_ascii_case(&name)).ok_or_else(|| bad(C, format!("no user export preset `{name}`")))?;
    x.name = new;
    s.save_prefs()?;
    s.execute("export.presets", &json!({}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "export.importPresets",
            "Import Export Presets",
            [],
            None,
            "{paths: [file or folder], dryRun?}: .lcexport, Lightroom Classic export presets (.lrtemplate) and .zip bundles of these: format, quality, file-size limit, colour space, bit depth, sizing, sharpening, metadata, file naming, destination folder and subfolder; a name in use by a different preset (or a built-in) becomes `name (2)`, an identical preset is skipped; preset.import (Import Profiles & Presets…) routes these files here too → {imported: [{name, file, unmapped: [settings not carried over], params?, skipped?}], failed: [[file, reason]]}",
            always,
            import
        ),
        cmd!(
            "export.exportPresets",
            "Export Export Presets",
            [],
            None,
            "{path, names?: [user preset name] (default all user presets)} → {path, count}; writes a .lcexport JSON file",
            always,
            export
        ),
        cmd!("export.renamePreset", "Rename Export Preset", [], None, "{name, newName}: a user preset → presets", always, rename),
    ]
}
