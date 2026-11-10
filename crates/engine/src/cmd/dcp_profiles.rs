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

pub(super) fn import(_s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "camera.importDcp";
    let paths: Vec<String> =
        p.get("paths").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
    if paths.is_empty() {
        return Err(bad(C, "no paths"));
    }
    let activate_new = p.get("activate").and_then(Value::as_bool);
    let (active_dir, library) = dirs(C)?;
    let (files, failed) = dcp_files(&paths);
    let how = Install { active_dir: &active_dir, library: &library, activate: activate_new, keep_existing: false };
    let done = install_in(C, files, failed, &how)?;
    // imported profiles can be chosen per photo straight away (the camera's default changes on the next start)
    lcc_raw::dcp::install_library(cp::library_dcps());
    Ok(json!({"imported": done.imported, "failed": done.failed, "restart": done.restart}))
}

/// Where and how [`install_in`] installs camera profiles.
pub(crate) struct Install<'a> {
    /// The camera-profiles folder (each camera's default `.dcp`).
    pub active_dir: &'a Path,
    /// Its `dcp/` library.
    pub library: &'a Path,
    /// `camera.importDcp`'s `activate`.
    pub activate: Option<bool>,
    /// Keep what the library has: a profile it holds already (the same bytes, or the same camera
    /// and ProfileName) is left out and named in `already`, a file name taken by another profile
    /// gets a new one, and a file that can't be written costs only itself. Off: `camera.importDcp`'s
    /// own rule (a profile imported again replaces its file).
    pub keep_existing: bool,
}

/// What [`install_in`] did.
#[derive(Default)]
pub(crate) struct Installed {
    /// As `camera.importDcp` lists them.
    pub imported: Vec<Value>,
    /// `[file name, reason]`.
    pub failed: Vec<Value>,
    /// `(file name, the library's id)` of the profiles the library held already.
    pub already: Vec<(String, String)>,
    /// `(file name, id)` of every profile installed or found in the library.
    pub ids: Vec<(String, String)>,
    /// A camera's default changed.
    pub restart: bool,
}

/// Most `.dcp` files of a library compared with by [`install_in`].
const MAX_LIBRARY: usize = 10_000;

/// The profiles in library folder `library`: `(id, bytes, UniqueCameraModel, ProfileName)` (a file
/// that doesn't read is left out).
fn library_files(library: &Path) -> Vec<(String, Vec<u8>, String, String)> {
    let Ok(entries) = std::fs::read_dir(library) else { return Vec::new() };
    let mut files: Vec<PathBuf> =
        entries.flatten().map(|e| e.path()).filter(|f| f.is_file() && f.extension().is_some_and(|x| x.eq_ignore_ascii_case("dcp"))).collect();
    files.sort();
    files
        .into_iter()
        .take(MAX_LIBRARY)
        .filter_map(|f| {
            let stem = f.file_stem()?.to_string_lossy().to_string();
            let bytes = crate::bounded_read::read_bounded(&f, lcc_raw::dcp::MAX_FILE as u64).ok()?;
            let d = lcc_raw::dcp::parse(&bytes).ok()?;
            Some((format!("dcp:{stem}"), bytes, d.model, d.name))
        })
        .collect()
}

/// The same camera or profile name, as people write it (case and outer spaces aside).
fn same_words(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

/// Install camera profiles `files` (`(file name, bytes)`) into the library `how` names: the path of
/// `camera.importDcp`, Import Profiles & Presets… and the older installs' import. `failed` are the
/// files that couldn't be read; one that doesn't parse joins them.
pub(crate) fn install_in(c: &str, files: Vec<(String, Vec<u8>)>, mut failed: Vec<Value>, how: &Install) -> Result<Installed> {
    let mut done = Installed::default();
    let mut have = if how.keep_existing { library_files(how.library) } else { Vec::new() };
    let mut parsed: Vec<(String, lcc_raw::dcp::Dcp, Vec<u8>)> = Vec::new();
    let mut twice: Vec<(String, String)> = Vec::new();
    for (name, bytes) in files {
        match lcc_raw::dcp::parse(&bytes) {
            Ok(d) => {
                let same = |b: &[u8], model: &str, pname: &str| {
                    b == bytes.as_slice() || (!d.name.trim().is_empty() && same_words(model, &d.model) && same_words(pname, &d.name))
                };
                if let Some((id, ..)) = have.iter().find(|(_, b, model, pname)| same(b, model, pname)) {
                    done.already.push((name.clone(), id.clone()));
                    done.ids.push((name, id.clone()));
                } else if let Some((first, ..)) = parsed.iter().find(|(_, q, b)| how.keep_existing && same(b, &q.model, &q.name)) {
                    // the same profile twice in one batch: once (its bindings follow the first's)
                    twice.push((name, first.clone()));
                } else {
                    parsed.push((name, d, bytes));
                }
            }
            Err(e) => failed.push(json!([name, e])),
        }
    }
    // profiles per camera model in this import: a batch that brings several for one camera (e.g.
    // Adobe's whole profile set, in file order) must not make its alphabetically first one the
    // camera's default (issue #723) — only a lone profile for a camera does, and only when the
    // camera has none yet
    let mut counts: std::collections::BTreeMap<String, u32> = Default::default();
    for (_, d, _) in &parsed {
        *counts.entry(crate::presets::slug(&d.model)).or_default() += 1;
    }
    for (name, dcp, bytes) in parsed {
        let label = if dcp.name.is_empty() { name.trim_end_matches(".dcp").to_string() } else { dcp.name.clone() };
        let base = crate::presets::slug(&format!("{}-{label}", dcp.model));
        let base = if base.is_empty() { "profile".to_string() } else { base };
        let mut slug = base.clone();
        if how.keep_existing {
            let mut n = 2u32;
            while how.library.join(format!("{slug}.dcp")).exists() && n < 10_000 {
                slug = format!("{base}-{n}");
                n += 1;
            }
        }
        let file = how.library.join(format!("{slug}.dcp"));
        let tmp = file.with_extension("dcp.tmp");
        let written = std::fs::write(&tmp, &bytes)
            .map_err(|e| format!("{}: {e}", tmp.display()))
            .and_then(|()| std::fs::rename(&tmp, &file).map_err(|e| format!("{}: {e}", file.display())));
        if let Err(e) = written {
            if !how.keep_existing {
                return Err(bad(c, e));
            }
            let _ = std::fs::remove_file(&tmp);
            failed.push(json!([name, e]));
            continue;
        }
        // the first profile for a camera becomes its default; the others are chosen per photo
        let lone = counts.get(&crate::presets::slug(&dcp.model)).copied().unwrap_or(0) == 1;
        let make_default = how.activate.unwrap_or_else(|| lone && !how.active_dir.join(cp::dcp_file_name(&dcp.model)).is_file());
        let mut active = false;
        if make_default {
            match activate(c, how.active_dir, &file) {
                Ok(_) => active = true,
                Err(e) if how.keep_existing => failed.push(json!([name, e.to_string()])),
                Err(e) => return Err(e),
            }
        }
        let id = format!("dcp:{slug}");
        done.imported.push(json!({"id": id, "model": dcp.model, "name": label, "file": file.to_string_lossy(), "active": active}));
        done.ids.push((name, id.clone()));
        have.push((id, bytes, dcp.model, dcp.name));
        done.restart |= active;
    }
    for (name, first) in twice {
        if let Some(id) = done.ids.iter().find(|(n, _)| *n == first).map(|(_, id)| id.clone()) {
            done.ids.push((name, id));
        }
    }
    done.failed = failed;
    Ok(done)
}

/// A camera profile as an older install's store lists it (`dcpProfiles` in its `shared.json`).
#[derive(Default, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct OlderDcp {
    file: String,
    camera_model: String,
    /// The bodies (serial numbers, or `file:XX`) it is for; each starts with it.
    bodies: Vec<String>,
    /// The bodies whose default it was made explicitly.
    body_defaults: Vec<String>,
    /// (Older entries: the one body.)
    serial: Option<String>,
}

/// What [`import_older`] brought.
#[derive(Default)]
pub(crate) struct OlderProfiles {
    /// Profiles installed.
    pub added: usize,
    /// The file names of those the library had already.
    pub already: Vec<String>,
    /// What was left out, by name.
    pub problems: Vec<String>,
    /// Camera bodies that now start with one of them.
    pub bound: usize,
}

/// Most profile files read from one older store.
const MAX_OLDER: usize = 2_000;

/// The `.dcp` files in `dir` (top level, sorted; none when it is missing).
fn dcp_files_in(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut files: Vec<PathBuf> =
        entries.flatten().map(|e| e.path()).filter(|f| f.extension().is_some_and(|x| x.eq_ignore_ascii_case("dcp"))).collect();
    files.sort();
    files.truncate(MAX_OLDER);
    files
}

/// The body key ([`crate::profile_bodies::key`]) camera body `body` of a profile made for `model`
/// (its UniqueCameraModel) is kept under: a photo of the library from that body names it as the
/// photo knows its camera, else the profile's model names it (found through
/// [`crate::profile_bodies::body_profile`]).
fn body_key_for(s: &Session, model: &str, body: &str) -> String {
    s.catalog
        .photos()
        .find(|p| crate::profile_bodies::body(p).as_deref() == Some(body) && lcc_raw::dcp::covers_model(model, &p.meta.camera))
        .and_then(|p| crate::profile_bodies::key_of(p))
        .unwrap_or_else(|| crate::profile_bodies::key(model, body))
}

/// Make the profiles of library `library` and the defaults in `active_dir` the ones this process
/// develops with, now (those of other folders stay).
fn refresh_installed(active_dir: &Path, library: &Path) {
    let fresh = cp::library_dcps_in(library);
    let mut list: Vec<(String, lcc_raw::dcp::Dcp)> =
        lcc_raw::dcp::library().into_iter().filter(|(id, _)| !fresh.iter().any(|(f, _)| f == id)).map(|(id, d)| (id, (*d).clone())).collect();
    list.extend(fresh);
    lcc_raw::dcp::install_library(list);
    // (a later one for the same camera wins: this folder's defaults)
    let mut active: Vec<lcc_raw::dcp::Dcp> = lcc_raw::dcp::installed().iter().map(|d| (**d).clone()).collect();
    active.extend(cp::active_dcps_in(active_dir).into_iter().map(|(_, d)| d));
    lcc_raw::dcp::install(active);
}

/// The camera profiles of an older install's store folder `store` (the `.dcp` files of its
/// `Profiles/`, with the camera bodies its `shared.json` bound them to) installed the way Import
/// Profiles & Presets… installs them ([`install_in`]): a profile the library has already (the same
/// bytes, or the same camera and ProfileName) is named in `already`, one that doesn't read in
/// `problems`. A body the older install started with one of them starts with it here too, unless it
/// has a profile of its own already; a lone profile for a camera with none becomes the camera's
/// default. Available at once. `camera`: the camera-profiles folder and its library (`None`: this
/// computer's). The older folder is only read.
pub(crate) fn import_older(s: &mut Session, store: &Path, camera: Option<(&Path, &Path)>) -> OlderProfiles {
    const C: &str = "store.importOlderInstalls";
    let mut out = OlderProfiles::default();
    // the bodies the older store bound its profiles to
    let shared_file = store.join(crate::app_store::SHARED);
    let shared: Value =
        crate::bounded_read::read_bounded(&shared_file, 64 << 20).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(Value::Null);
    let mut listed: Vec<OlderDcp> = Vec::new();
    let mut unread = 0usize;
    for v in shared.get("dcpProfiles").and_then(Value::as_array).into_iter().flatten().take(MAX_OLDER) {
        match serde_json::from_value::<OlderDcp>(v.clone()) {
            Ok(e) => listed.push(e),
            Err(_) => unread += 1,
        }
    }
    if unread > 0 {
        out.problems.push(format!("dcpProfiles: {unread} camera profile entr{} couldn't be read; left out", if unread == 1 { "y" } else { "ies" }));
    }
    let file_name = |f: &str| Path::new(f).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let dir = store.join(crate::app_store::PROFILES_DIR);
    let paths = dcp_files_in(&dir);
    for e in &listed {
        let name = file_name(&e.file);
        if !name.is_empty()
            && name.to_ascii_lowercase().ends_with(".dcp")
            && !paths.iter().any(|p| p.file_name().is_some_and(|n| n.to_string_lossy() == name))
        {
            out.problems.push(format!("{name}: the camera profile file is missing; left out"));
        }
    }
    if paths.is_empty() {
        return out;
    }
    let mut files = Vec::new();
    let mut failed = Vec::new();
    for p in &paths {
        let name = file_name(&p.to_string_lossy());
        match crate::bounded_read::read_bounded(p, lcc_raw::dcp::MAX_FILE as u64) {
            Ok(b) => files.push((name, b)),
            Err(e) => failed.push(json!([name, e.to_string()])),
        }
    }
    let own;
    let (active_dir, library) = match camera {
        Some(d) => d,
        None => match dirs(C) {
            Ok(d) => {
                own = d;
                (own.0.as_path(), own.1.as_path())
            }
            Err(e) => {
                out.problems.push(e.to_string());
                return out;
            }
        },
    };
    if let Err(e) = std::fs::create_dir_all(library) {
        out.problems.push(format!("{}: {e}", library.display()));
        return out;
    }
    let how = Install { active_dir, library, activate: None, keep_existing: true };
    let done = match install_in(C, files, failed, &how) {
        Ok(d) => d,
        Err(e) => {
            out.problems.push(e.to_string());
            return out;
        }
    };
    for f in &done.failed {
        let what = f.get(0).and_then(Value::as_str).unwrap_or_default();
        let why = f.get(1).and_then(Value::as_str).unwrap_or("couldn't be read");
        out.problems.push(format!("{what}: {why}; left out"));
    }
    out.added = done.imported.len();
    out.already = done.already.iter().map(|(n, _)| n.clone()).collect();
    refresh_installed(active_dir, library);
    // the bodies: those a profile is for, then those it was made the default of explicitly (these win)
    let mut want: Vec<(String, String)> = Vec::new();
    for explicit in [false, true] {
        for e in &listed {
            let name = file_name(&e.file);
            let Some(id) = done.ids.iter().find(|(n, _)| *n == name).map(|(_, id)| id.clone()) else { continue };
            let model =
                lcc_raw::dcp::library_get(&id).map(|d| d.model.clone()).filter(|m| !m.trim().is_empty()).unwrap_or_else(|| e.camera_model.clone());
            let bodies: Vec<&String> = if explicit { e.body_defaults.iter().collect() } else { e.serial.iter().chain(e.bodies.iter()).collect() };
            for b in bodies {
                let b = b.trim();
                if b.is_empty() || b.len() > 200 || model.trim().is_empty() {
                    continue;
                }
                let key = body_key_for(s, &model, b);
                want.retain(|(k, _)| *k != key);
                want.push((key, id.clone()));
            }
        }
    }
    for (key, id) in want {
        if !s.body_profiles.contains_key(&key) && s.body_profiles.len() < 10_000 {
            s.body_profiles.insert(key, id);
            out.bound += 1;
        }
    }
    out
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
            "{paths: [.dcp file, folder or .zip], activate?: bool}: DNG camera profiles you own, chosen per photo (develop.cameraProfile); a lone profile for a camera model (every one of the import, with activate: true) becomes that camera's default from the next start, a batch for one camera leaves the default as it is (the default is set per model with library.preferences `cameraDefault`) → {imported: [{id, model, name, file, active}], failed, restart}",
            always,
            import
        ),
        cmd!(query "camera.listDcp", "Camera Profiles (.dcp)", [], None, "{} → {profiles: [{id, model, name, copyright, file, active}], inUse: [{model, name}], calibrated: [{id (cal:…), model, name}]}", always, list),
        cmd!(
            "camera.useDcp",
            "Use Camera Profile",
            [],
            None,
            "{file: library file from camera.listDcp}: active for its model from the next start",
            always,
            use_dcp
        ),
        cmd!(
            "camera.removeDcp",
            "Stop Using Camera Profile",
            [],
            None,
            "{model}: the model's raws go back to the app's own colour from the next start",
            always,
            remove_dcp
        ),
        cmd!(
            "camera.exportDcp",
            "Export Camera Profile (.dcp)",
            [],
            None,
            "{path, ids? (one; default: the active photo), name?, camera? (UniqueCameraModel; default: the camera's usual profile name)}: the camera colour this raw renders with (its chosen .dcp or calibration look, the file's matrices and profile tables, or the colour the app fits) as a DNG camera profile for Lightroom, Camera Raw or RawTherapee → {path, model, name}",
            always,
            export_dcp
        ),
        cmd!(
            "develop.exportLut",
            "Export Look as LUT (.cube)",
            [],
            None,
            "{path, ids? (one; default: the active photo), title?}: the photo's global look (Light, tone curve, colour, mixer, colour grading, profile look; not white balance, local edits, effects or detail) as a 33³ sRGB-to-sRGB .cube LUT for Capture One, darktable, Photoshop or video tools → {path, size}",
            always,
            export_lut
        ),
    ]
}
