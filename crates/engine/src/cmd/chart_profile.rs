//! Camera profiling from a ColorChecker photograph (ported from OneSlips, LR-PROF-CREATE / LR-PROF-DUAL):
//! `profile.findChart` and `profile.createFromChart`. The work itself is in [`crate::chart_profiling`].

use std::path::Path;

use serde_json::{Value, json};

use super::{CommandSpec, always, bad, cmd, str_param};
use crate::chart_profiling::{self as cp, OnExists};
use crate::{Result, Session};

/// The chart photograph a command works on: `path`, else the target photo's file.
fn chart_source(s: &Session, p: &Value, c: &str) -> Result<(Option<lcc_catalog::PhotoId>, String)> {
    if let Some(path) = str_param(p, "path") {
        return Ok((None, path.to_string()));
    }
    let id = match s.targets(p)?.into_iter().next() {
        Some(id) => id,
        None => s.selection.active.ok_or_else(|| bad(c, "no photo: select the chart photo or give `id` or `path`"))?,
    };
    let ph = s.catalog.photo(id).ok_or_else(|| bad(c, format!("no photo {}", id.0)))?;
    match &ph.source {
        lcc_catalog::Source::File { path } => Ok((Some(id), path.clone())),
        _ => Err(bad(c, "the photo has no file to profile from (a demo photo?)")),
    }
}

fn corners_param(p: &Value, c: &str) -> Result<Option<[[f64; 2]; 4]>> {
    let Some(v) = p.get("corners").filter(|v| !v.is_null()) else { return Ok(None) };
    let pts: Vec<[f64; 2]> = serde_json::from_value(v.clone()).map_err(|_| bad(c, "`corners` must be four [x, y] points (0–1)"))?;
    let pts: [[f64; 2]; 4] = pts.try_into().map_err(|_| bad(c, "`corners` must be four [x, y] points (0–1)"))?;
    Ok(Some(pts))
}

fn find_chart(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "profile.findChart";
    let (id, path) = chart_source(s, p, C)?;
    let _task = s.activity.start("cameraProfile", "Finding the color chart", crate::activity::Cancel::No);
    let prep = cp::prepare_path(&path).map_err(|e| bad(C, e))?;
    let found = prep.located.map(|l| {
        let light = cp::estimate_light(&prep, &l);
        json!({
            "corners": cp::normalized_corners(&prep.frame, &l.corners),
            "turns": l.turns,
            "score": l.score,
            "estimatedCct": light.map(|e| e.cct.round()),
            "estimatedIlluminant": light.map(|e| e.illuminant),
        })
    });
    Ok(json!({
        "photo": id.map(|i| i.0),
        "path": path,
        "cameraModel": prep.camera_model,
        "body": prep.body,
        "frame": [prep.frame.width, prep.frame.height],
        "found": found.is_some(),
        "chart": found,
    }))
}

/// One chart photo's part of a request: `{corners?, turns?, refine?, illuminant?}`.
fn chart_request(p: &Value, c: &str) -> Result<cp::Request> {
    let illuminant = str_param(p, "illuminant").unwrap_or("");
    if illuminant.len() > 64 {
        return Err(bad(c, "`illuminant` is too long"));
    }
    Ok(cp::Request {
        corners: corners_param(p, c)?,
        turns: p.get("turns").and_then(Value::as_u64).map(|t| (t % 4) as u8),
        refine: p.get("refine").and_then(Value::as_bool).unwrap_or(false),
        illuminant: illuminant.to_string(),
    })
}

fn file_name(path: &str) -> String {
    Path::new(path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
}

/// The body keys (`model|body`) of the chart photos in the catalog, for their bodies' defaults.
fn body_keys(s: &Session, ids: &[lcc_catalog::PhotoId]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for id in ids {
        if let Some(k) = s.catalog.photo(*id).and_then(|ph| crate::profile_bodies::key_of(ph))
            && !out.contains(&k)
        {
            out.push(k);
        }
    }
    out
}

fn create_from_chart(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "profile.createFromChart";
    let (id, path) = chart_source(s, p, C)?;
    // (natural / hueSaturation and accurate / hueSaturationLightness: earlier names)
    let natural = match str_param(p, "rendering").unwrap_or("photographic") {
        "photographic" | "natural" | "hueSaturation" => true,
        "reproduction" | "accurate" | "hueSaturationLightness" => false,
        other => return Err(bad(C, format!("unknown rendering `{other}` (photographic or reproduction)"))),
    };
    let name = str_param(p, "name").unwrap_or("");
    let camera_model = str_param(p, "cameraModel").unwrap_or("");
    if name.chars().count() > 200 || camera_model.chars().count() > 200 {
        return Err(bad(C, "`name` and `cameraModel` are at most 200 characters"));
    }
    let options = lcc_camprof::BuildOptions {
        illuminant: String::new(),
        natural,
        correction_table: p.get("correctionTable").and_then(Value::as_bool).unwrap_or(true),
        camera_model: camera_model.to_string(),
        name: name.to_string(),
    };
    let first = chart_request(p, C)?;
    let on_exists = OnExists::parse(str_param(p, "onExists")).ok_or_else(|| bad(C, "`onExists` is ask, replace, keepBoth or fail"))?;
    // the second photo, under another light: named explicitly (never the selection)
    let second = match p.get("second").filter(|v| !v.is_null()) {
        None => None,
        Some(q) if q.is_object() => {
            let source = if let Some(path) = str_param(q, "path") {
                (None, path.to_string())
            } else if let Some(n) = q.get("id").and_then(Value::as_u64) {
                chart_source(s, &json!({"ids": [n]}), C)?
            } else {
                return Err(bad(C, "`second` needs the other chart photo's `id` or `path`"));
            };
            Some((source, chart_request(q, C)?))
        }
        Some(_) => return Err(bad(C, "`second` must be an object: {id? | path?, corners?, turns?, illuminant?}")),
    };
    let library =
        crate::camera_profiles::dcp_library_dir().ok_or_else(|| bad(C, "no camera-profiles folder (no configuration folder on this system)"))?;
    let task = s.activity.start("cameraProfile", "Creating camera profile", crate::activity::Cancel::No);
    task.detail(&file_name(&path));
    let prep_a = cp::prepare_path(&path).map_err(|e| bad(C, e))?;
    let prep_b = match &second {
        Some(((_, path_b), _)) => {
            task.detail(&file_name(path_b));
            Some(cp::prepare_path(path_b).map_err(|e| bad(C, e))?)
        }
        None => None,
    };
    let out =
        cp::build((&prep_a, &first), prep_b.as_ref().zip(second.as_ref()).map(|(prep, (_, req))| (prep, req)), &options).map_err(|e| bad(C, e))?;
    drop(task);
    let photos: Vec<lcc_catalog::PhotoId> = id.into_iter().chain(second.as_ref().and_then(|((i, _), _)| *i)).collect();
    let apply_to = if p.get("apply").and_then(Value::as_bool).unwrap_or(true) { photos.clone() } else { Vec::new() };
    let bodies = if p.get("bodyDefault").and_then(Value::as_bool).unwrap_or(true) { body_keys(s, &photos) } else { Vec::new() };
    let installed = cp::install(s, &library, &out.dcp, &bodies, on_exists, &apply_to)?;
    let preps: Vec<&cp::Prepared> = std::iter::once(&prep_a).chain(prep_b.as_ref()).collect();
    let mut report = cp::report_json(&preps, &out);
    if let (Some(o), Some(i)) = (report.as_object_mut(), installed.as_object()) {
        for (k, v) in i {
            o.insert(k.clone(), v.clone());
        }
    }
    Ok(report)
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            query "profile.findChart",
            "Find Color Chart",
            [],
            None,
            "{ids? | path?: a raw photo of a ColorChecker 24 (default: the active photo)} → {photo, path, cameraModel, body, frame: [w, h], found, chart: {corners: [[x, y] ×4], turns, score, estimatedCct, estimatedIlluminant}}: estimatedCct is what the chart's greys measure through the file's colour matrix, for information; corners are 0–1 in the developed raw frame (default crop) as the sensor recorded it (not rotated), top left, top right, bottom right, bottom left; turns 0–3 = patch A01 at the top left, top right, bottom right, bottom left",
            always,
            find_chart
        ),
        cmd!(
            "profile.createFromChart",
            "Create Camera Profile from Color Chart",
            [],
            None,
            "{ids? | path?, corners?: [[x, y] ×4] (0–1, as profile.findChart; found automatically when omitted), turns?: 0–3, refine?: bool, illuminant?: D55 (default; also daylight, flash) | A (also tungsten, StdA) | D50 | D65 | D75, second?: {id? | path?, corners?, turns?, refine?, illuminant?} (a second chart photo under another light: a dual-illuminant profile; the same light twice is refused), rendering?: photographic (default: hue and saturation match the chart, lightness as the camera recorded) | reproduction (lightness too; for artwork and copy work), correctionTable?: bool (default true), name? (default: '<camera> <body> · <light(s)> · <Photo|Repro> · <capture date>'), onExists?: fail (default) | replace (write over it, same id: the photos using it follow) | keepBoth (save as '… 2') | ask (save nothing, answer {exists: {id, name, usedBy}, keepBothName}), cameraModel? (default: the file's), apply?: bool (default true: the chart photos develop with the new profile), bodyDefault?: bool (default true: the chart photos' camera bodies start with it)} — fits a DNG camera profile (ColorMatrix, ForwardMatrix, HueSatMap per light) to a ColorChecker 24 and adds it to the imported camera profiles (dcp: id) → {id, name, cameraModel, file, bodies, applied, replaced, dual, meanDeltaE, maxDeltaE, matrixMeanDeltaE, illuminant, corners, turns, patches, calibrations} (ΔE = CIEDE2000)",
            always,
            create_from_chart
        ),
    ]
}
