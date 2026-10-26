//! Imported `.dcp` camera profiles: one active profile per camera model replaces
//! the colour model of that model's raws (`camera_profiles::apply_dcp`). Imported files are kept
//! in the camera-profiles folder's `dcp/` library; the active one for a model is copied next to
//! the JSON profiles. Like those, the active set is read when the app starts.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::{CommandSpec, always, bad, cmd, str_param};
use crate::camera_profiles as cp;
use crate::{Result, Session};

/// `(name, bytes)` of every `.dcp` in `paths` (folders searched, zips opened), and failures.
fn dcp_files(paths: &[String]) -> (Vec<(String, Vec<u8>)>, Vec<Value>) {
    fn name(f: &Path) -> String {
        f.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
    }
    fn is_dcp(f: &Path) -> bool {
        f.extension().is_some_and(|x| x.eq_ignore_ascii_case("dcp"))
    }
    fn read(f: &Path, out: &mut Vec<(String, Vec<u8>)>, failed: &mut Vec<Value>) {
        match std::fs::metadata(f) {
            Ok(m) if m.len() > lcc_raw::dcp::MAX_FILE as u64 => failed.push(json!([name(f), "file too large for a camera profile"])),
            _ => match std::fs::read(f) {
                Ok(b) => out.push((name(f), b)),
                Err(e) => failed.push(json!([name(f), e.to_string()])),
            },
        }
    }
    let mut out = Vec::new();
    let mut failed = Vec::new();
    for p in paths {
        let path = Path::new(p);
        if path.is_dir() {
            for f in crate::walk::files_in(path, None, crate::walk::Limits::default(), is_dcp).files {
                read(&f, &mut out, &mut failed);
            }
        } else if path.extension().is_some_and(|x| x.eq_ignore_ascii_case("zip")) {
            match std::fs::read(path) {
                Ok(b) => {
                    for (inner, data) in crate::preset_import::read_zip(&b).unwrap_or_default() {
                        if is_dcp(Path::new(&inner)) {
                            out.push((name(Path::new(&inner)), data));
                        }
                    }
                }
                Err(e) => failed.push(json!([name(path), e.to_string()])),
            }
        } else {
            read(path, &mut out, &mut failed);
        }
    }
    (out, failed)
}

fn dirs(c: &str) -> Result<(PathBuf, PathBuf)> {
    let active = cp::dir().ok_or_else(|| bad(c, "no camera-profiles folder (no configuration folder on this system)"))?;
    let library = cp::dcp_library_dir().ok_or_else(|| bad(c, "no camera-profiles folder"))?;
    std::fs::create_dir_all(&library).map_err(|e| bad(c, format!("{}: {e}", library.display())))?;
    Ok((active, library))
}

/// Copy `from` to `to` through a temporary file.
fn copy_atomic(c: &str, from: &Path, to: &Path) -> Result<()> {
    let tmp = to.with_extension("dcp.tmp");
    std::fs::copy(from, &tmp).map_err(|e| bad(c, format!("{}: {e}", tmp.display())))?;
    std::fs::rename(&tmp, to).map_err(|e| bad(c, format!("{}: {e}", to.display())))?;
    Ok(())
}

/// Make the library file `file` the active profile for its camera.
fn activate(c: &str, active_dir: &Path, file: &Path) -> Result<lcc_raw::dcp::Dcp> {
    let dcp = cp::load_dcp(file).map_err(|e| bad(c, e))?;
    copy_atomic(c, file, &active_dir.join(cp::dcp_file_name(&dcp.model)))?;
    Ok(dcp)
}

fn import(_s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "camera.importDcp";
    let paths: Vec<String> =
        p.get("paths").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
    if paths.is_empty() {
        return Err(bad(C, "no paths"));
    }
    let activate_new = p.get("activate").and_then(Value::as_bool);
    let (active_dir, library) = dirs(C)?;
    let (files, mut failed) = dcp_files(&paths);
    let mut imported = Vec::new();
    let mut defaults_changed = false;
    for (name, bytes) in files {
        let dcp = match lcc_raw::dcp::parse(&bytes) {
            Ok(d) => d,
            Err(e) => {
                failed.push(json!([name, e]));
                continue;
            }
        };
        let label = if dcp.name.is_empty() { name.trim_end_matches(".dcp").to_string() } else { dcp.name.clone() };
        let slug = crate::presets::slug(&format!("{}-{label}", dcp.model));
        let file = library.join(format!("{slug}.dcp"));
        let tmp = file.with_extension("dcp.tmp");
        std::fs::write(&tmp, &bytes).map_err(|e| bad(C, format!("{}: {e}", tmp.display())))?;
        std::fs::rename(&tmp, &file).map_err(|e| bad(C, format!("{}: {e}", file.display())))?;
        // the first profile for a camera becomes its default; the others are chosen per photo
        let make_default = activate_new.unwrap_or_else(|| !active_dir.join(cp::dcp_file_name(&dcp.model)).is_file());
        if make_default {
            activate(C, &active_dir, &file)?;
        }
        imported
            .push(json!({"id": format!("dcp:{slug}"), "model": dcp.model, "name": label, "file": file.to_string_lossy(), "active": make_default}));
        defaults_changed |= make_default;
    }
    // imported profiles can be chosen per photo straight away (the camera's default changes on the next start)
    lcc_raw::dcp::install_library(cp::library_dcps());
    Ok(json!({"imported": imported, "failed": failed, "restart": defaults_changed}))
}

/// The library, each entry with whether it is the active profile for its model.
fn list(_s: &mut Session, _p: &Value) -> Result<Value> {
    let active: Vec<(PathBuf, lcc_raw::dcp::Dcp)> = cp::active_dcps();
    let same = |a: &Path, b: &Path| std::fs::read(a).ok().zip(std::fs::read(b).ok()).is_some_and(|(x, y)| x == y);
    let mut out = Vec::new();
    if let Some(lib) = cp::dcp_library_dir()
        && let Ok(entries) = std::fs::read_dir(&lib)
    {
        let mut files: Vec<PathBuf> =
            entries.flatten().map(|e| e.path()).filter(|f| f.extension().is_some_and(|x| x.eq_ignore_ascii_case("dcp"))).collect();
        files.sort();
        for f in files {
            match cp::load_dcp(&f) {
                Ok(d) => {
                    let is_active = active.iter().any(|(a, ad)| ad.model == d.model && same(a, &f));
                    let id = format!("dcp:{}", f.file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_default());
                    out.push(json!({"id": id, "model": d.model, "name": d.name, "copyright": d.copyright, "file": f.to_string_lossy(), "active": is_active}));
                }
                Err(e) => out.push(json!({"file": f.to_string_lossy(), "error": e})),
            }
        }
    }
    let in_use: Vec<Value> = lcc_raw::dcp::installed().iter().map(|d| json!({"model": d.model, "name": d.name})).collect();
    // the bundled Lightroom-matched calibrations, chosen per photo as `cal:<id>`
    let calibrated: Vec<Value> = lcc_raw::dcp::calibrated()
        .iter()
        .flat_map(|c| c.looks.iter().map(|(id, d)| json!({"id": id, "model": d.model, "name": d.name})).collect::<Vec<_>>())
        .collect();
    Ok(json!({"profiles": out, "inUse": in_use, "calibrated": calibrated}))
}

fn use_dcp(_s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "camera.useDcp";
    let file = str_param(p, "file").ok_or_else(|| bad(C, "missing file"))?;
    let (active_dir, library) = dirs(C)?;
    let path = PathBuf::from(file);
    // only files in the library (what `camera.listDcp` names)
    if path.parent().map(Path::to_path_buf) != Some(library.clone()) {
        return Err(bad(C, format!("`{file}` is not an imported camera profile")));
    }
    let dcp = activate(C, &active_dir, &path)?;
    Ok(json!({"model": dcp.model, "name": dcp.name, "restart": true}))
}

fn remove_dcp(_s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "camera.removeDcp";
    let model = str_param(p, "model").ok_or_else(|| bad(C, "missing model"))?;
    let dir = cp::dir().ok_or_else(|| bad(C, "no camera-profiles folder"))?;
    let f = dir.join(cp::dcp_file_name(model));
    if !f.is_file() {
        return Err(bad(C, format!("no active camera profile for `{model}`")));
    }
    std::fs::remove_file(&f).map_err(|e| bad(C, format!("{}: {e}", f.display())))?;
    Ok(json!({"model": model, "restart": true}))
}

/// The one photo a command acts on (`ids` with one id, else the active photo), with its original's bytes.
fn one_photo(s: &Session, p: &Value, c: &str) -> Result<(lcc_catalog::PhotoId, Vec<u8>)> {
    let ids = s.targets(p)?;
    let id = match ids.as_slice() {
        [] => s.selection.active.ok_or_else(|| bad(c, "no photo"))?,
        [id] => *id,
        _ => return Err(bad(c, "one photo at a time")),
    };
    let ph = s.catalog.photo(id).ok_or_else(|| bad(c, format!("no photo {}", id.0)))?;
    let lcc_catalog::Source::File { path } = &ph.source else { return Err(bad(c, "a demo photo has no original file")) };
    let bytes = match &s.media.file_bytes {
        Some(read) => read(path).map_err(|e| bad(c, e))?,
        None => std::fs::read(path).map_err(|e| bad(c, format!("{path}: {e}")))?,
    };
    Ok((id, bytes))
}

/// Write `bytes` to the command's `path`, never over a photo's original.
fn write_out(s: &Session, c: &str, path: &str, bytes: &[u8]) -> Result<()> {
    if path.trim().is_empty() {
        return Err(bad(c, "needs `path`"));
    }
    s.original_guard().check(Path::new(path)).map_err(|e| bad(c, e.to_string()))?;
    crate::export::write_file(path, bytes).map_err(|e| bad(c, e))
}

fn export_dcp(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "camera.exportDcp";
    let path = str_param(p, "path").ok_or_else(|| bad(C, "needs `path` (the .dcp to write)"))?.to_string();
    let (id, bytes) = one_photo(s, p, C)?;
    let choice = s.develop_of(id).map(|d| d.profile.camera.clone()).unwrap_or_default();
    let dcp =
        crate::colour_export::photo_dcp(&bytes, &choice, str_param(p, "name").unwrap_or_default(), str_param(p, "camera")).map_err(|e| bad(C, e))?;
    let parsed = lcc_raw::dcp::parse(&dcp).map_err(|e| bad(C, format!("the profile written doesn't read back: {e}")))?;
    write_out(s, C, &path, &dcp)?;
    Ok(json!({"path": path, "model": parsed.model, "name": parsed.name}))
}

fn export_lut(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "develop.exportLut";
    let path = str_param(p, "path").ok_or_else(|| bad(C, "needs `path` (the .cube to write)"))?.to_string();
    let ids = s.targets(p)?;
    let id = match ids.as_slice() {
        [] => s.selection.active.ok_or_else(|| bad(C, "no photo"))?,
        [id] => *id,
        _ => return Err(bad(C, "one photo at a time")),
    };
    let settings = s.develop_of(id).ok_or_else(|| bad(C, format!("no photo {}", id.0)))?;
    let title = str_param(p, "title")
        .map(str::to_string)
        .unwrap_or_else(|| s.catalog.photo(id).map(|ph| format!("{} look: {}", lcc_brand::DISPLAY_NAME, ph.file_name)).unwrap_or_default());
    let cube = crate::colour_export::look_cube(&settings, &title).map_err(|e| bad(C, e))?;
    write_out(s, C, &path, cube.as_bytes())?;
    Ok(json!({"path": path, "size": crate::colour_export::CUBE_SIZE}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "camera.importDcp",
            "Import Camera Profiles (.dcp)",
            [],
            None,
            "{paths: [.dcp file, folder or .zip], activate?: bool} — DNG camera profiles you own, chosen per photo (develop.cameraProfile); the first for a camera model (or each, with activate: true) also becomes that camera's default from the next start → {imported: [{id, model, name, file, active}], failed, restart}",
            always,
            import
        ),
        cmd!(query "camera.listDcp", "Camera Profiles (.dcp)", [], None, "{} → {profiles: [{id, model, name, copyright, file, active}], inUse: [{model, name}], calibrated: [{id (cal:…), model, name}]}", always, list),
        cmd!(
            "camera.useDcp",
            "Use Camera Profile",
            [],
            None,
            "{file: library file from camera.listDcp} — active for its model from the next start",
            always,
            use_dcp
        ),
        cmd!(
            "camera.removeDcp",
            "Stop Using Camera Profile",
            [],
            None,
            "{model} — the model's raws go back to the app's own colour from the next start",
            always,
            remove_dcp
        ),
        cmd!(
            "camera.exportDcp",
            "Export Camera Profile (.dcp)",
            [],
            None,
            "{path, ids? (one; default: the active photo), name?, camera? (UniqueCameraModel; default: the camera's usual profile name)} — the camera colour this raw renders with (its chosen .dcp or calibration look, the file's matrices and profile tables, or the colour the app fits) as a DNG camera profile for Lightroom, Camera Raw or RawTherapee → {path, model, name}",
            always,
            export_dcp
        ),
        cmd!(
            "develop.exportLut",
            "Export Look as LUT (.cube)",
            [],
            None,
            "{path, ids? (one; default: the active photo), title?} — the photo's global look (Light, tone curve, colour, mixer, colour grading, profile look; not white balance, local edits, effects or detail) as a 33³ sRGB-to-sRGB .cube LUT for Capture One, darktable, Photoshop or video tools → {path, size}",
            always,
            export_lut
        ),
    ]
}
