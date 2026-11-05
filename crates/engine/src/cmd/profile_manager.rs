//! The Profile Manager's commands (ported from OneSlips, LR-PROF-MANAGE and LR-PROF-BODY): hide
//! profiles from the menus and the browser, and give camera bodies their own camera profile.

use std::collections::BTreeMap;

use lcc_catalog::{Op, PhotoId};
use serde_json::{Value, json};

use super::{CommandSpec, always, bad, cmd, str_param};
use crate::profile_bodies;
use crate::profile_edits::{MAX_ENTRIES, MAX_NAME};
use crate::{Result, Session};

/// Every profile with its group and whether it is hidden (the user's names, groups and order),
/// the built-in looks removed from this install, and the camera bodies.
fn manager(s: &mut Session, _p: &Value) -> Result<Value> {
    let bodies = bodies(s, &json!({}))?;
    // how many photos of this library use each look and each camera profile
    let mut looks: BTreeMap<String, u64> = BTreeMap::new();
    let mut cameras: BTreeMap<String, u64> = BTreeMap::new();
    for p in s.catalog.photos() {
        let n = looks.entry(p.develop.profile.id.clone()).or_default();
        *n = n.saturating_add(1);
        if !p.develop.profile.camera.is_empty() {
            let n = cameras.entry(p.develop.profile.camera.clone()).or_default();
            *n = n.saturating_add(1);
        }
    }
    let profiles: Vec<Value> = s
        .all_profiles()
        .into_iter()
        .map(|x| {
            let used = looks.get(&x.id).copied().unwrap_or(0);
            let mut v = json!(x);
            if let Some(o) = v.as_object_mut() {
                o.insert("usedBy".into(), json!(used));
            }
            v
        })
        .collect();
    let defaults_of = |id: &str| -> Vec<String> { s.body_profiles.iter().filter(|(_, v)| v.as_str() == id).map(|(k, _)| k.clone()).collect() };
    let hidden = |id: &str| s.profile_hidden.iter().any(|h| h == id);
    // camera profiles: the user's imported .dcp files (deletable), then the bundled calibrated looks
    let mut camera: Vec<Value> = lcc_raw::dcp::library()
        .into_iter()
        .map(|(id, d)| {
            let name = if d.name.is_empty() { id.trim_start_matches("dcp:").to_string() } else { d.name.clone() };
            json!({"id": id, "name": name, "model": d.model, "kind": "camera", "builtin": false, "hidden": hidden(&id),
                "usedBy": cameras.get(&id).copied().unwrap_or(0), "bodyDefaults": defaults_of(&id)})
        })
        .collect();
    for c in lcc_raw::dcp::calibrated() {
        for (id, d) in &c.looks {
            camera.push(json!({"id": id, "name": d.name, "model": d.model, "kind": "camera", "builtin": true, "hidden": hidden(id),
                "usedBy": cameras.get(id).copied().unwrap_or(0), "bodyDefaults": defaults_of(id)}));
        }
    }
    Ok(json!({"profiles": profiles, "cameras": camera, "removed": s.profile_edits.removed, "bodies": bodies["bodies"]}))
}

/// A camera profile id the manager knows: an imported `.dcp` (`dcp:…`) or a bundled calibrated look (`cal:…`).
fn known_camera(id: &str) -> bool {
    lcc_raw::dcp::library_get(id).is_some() || lcc_raw::dcp::calibrated_look_exists(id)
}

/// Delete the imported camera profile `id` (`dcp:<file stem>`) from the `.dcp` library folder
/// `library`; when it is also its camera's active (default) profile in `active_dir`, that copy goes
/// too. Returns whether it was the active one. `id` comes from outside: only a plain file name in
/// `library` is ever removed.
pub(crate) fn delete_dcp_file(library: &std::path::Path, active_dir: &std::path::Path, id: &str) -> std::result::Result<bool, String> {
    let not_ours = || format!("`{id}` is not an imported camera profile");
    let stem = id.strip_prefix("dcp:").ok_or_else(not_ours)?;
    let plain = !stem.is_empty()
        && stem != "."
        && stem != ".."
        && !stem.contains(['/', '\\', ':'])
        && std::path::Path::new(stem).file_name().is_some_and(|n| n == stem);
    if !plain {
        return Err(not_ours());
    }
    let file = library.join(format!("{stem}.dcp"));
    if !file.is_file() {
        return Err(format!("no imported camera profile `{id}`"));
    }
    // its camera's active copy, when it is this profile (the same bytes)
    let mut was_active = false;
    if let Ok(d) = crate::camera_profiles::load_dcp(&file) {
        let active = active_dir.join(crate::camera_profiles::dcp_file_name(&d.model));
        let same = std::fs::read(&active).ok().zip(std::fs::read(&file).ok()).is_some_and(|(a, b)| a == b);
        if same {
            std::fs::remove_file(&active).map_err(|e| format!("{}: {e}", active.display()))?;
            was_active = true;
        }
    }
    std::fs::remove_file(&file).map_err(|e| format!("{}: {e}", file.display()))?;
    Ok(was_active)
}

/// Delete one of the user's own profiles from the Profile Manager: an imported LUT profile with
/// its file (as profile.deleteImported), an imported `.dcp` camera profile from the
/// camera-profiles folder. Built-in looks and bundled camera profiles are hidden, not deleted.
/// Photos using a deleted profile keep their settings and fall back safely: a gone LUT renders
/// without it, a gone camera profile gives the camera's default (`lcc_raw::dcp::resolve`).
fn delete(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "profile.delete";
    let id = str_param(p, "id").ok_or_else(|| bad(C, "missing id"))?.trim().to_string();
    if crate::presets::profile(&id).is_some() || lcc_raw::dcp::calibrated_look_exists(&id) {
        return Err(bad(C, "a built-in profile can be hidden, not deleted"));
    }
    if s.lut_profiles.iter().any(|x| x.id == id) {
        s.execute("profile.deleteImported", &json!({"id": id}))?;
        return Ok(json!({"deleted": id, "kind": "lut"}));
    }
    if !id.starts_with("dcp:") {
        return Err(bad(C, format!("unknown profile `{id}`")));
    }
    let active_dir = crate::camera_profiles::dir().ok_or_else(|| bad(C, "no camera-profiles folder (no configuration folder on this system)"))?;
    let library = crate::camera_profiles::dcp_library_dir().ok_or_else(|| bad(C, "no camera-profiles folder"))?;
    let was_active = delete_dcp_file(&library, &active_dir, &id).map_err(|e| bad(C, e))?;
    forget_camera_profile(s, &id);
    lcc_raw::dcp::install_library(crate::camera_profiles::library_dcps());
    if was_active {
        lcc_raw::dcp::install(crate::camera_profiles::active_dcps().into_iter().map(|(_, d)| d).collect());
    }
    s.save_prefs()?;
    Ok(json!({"deleted": id, "kind": "camera", "wasDefault": was_active}))
}

/// After camera profile `id` is gone: no body starts with it, and it is no longer hidden.
pub(crate) fn forget_camera_profile(s: &mut Session, id: &str) {
    s.body_profiles.retain(|_, v| v != id);
    s.profile_hidden.retain(|h| h != id);
}

/// A profile id the manager knows: a built-in look (removed ones too) or an imported LUT profile.
fn known(s: &Session, id: &str) -> bool {
    crate::presets::profile(id).is_some() || s.lut_profiles.iter().any(|x| x.id == id)
}

fn id_param(s: &Session, p: &Value, c: &str) -> Result<String> {
    let id = str_param(p, "id").ok_or_else(|| bad(c, "missing id"))?;
    if !known(s, id) {
        return Err(bad(c, format!("unknown profile `{id}`")));
    }
    Ok(id.to_string())
}

/// A name or group from `p[key]`: trimmed, not empty, at most [`MAX_NAME`] characters; `""` / null
/// clears (`None`).
fn name_param(p: &Value, key: &str, c: &str) -> Result<Option<String>> {
    match p.get(key) {
        None => Err(bad(c, format!("missing `{key}`"))),
        Some(Value::Null) => Ok(None),
        Some(Value::String(n)) => {
            let n = n.trim();
            if n.chars().count() > MAX_NAME || n.chars().any(char::is_control) {
                return Err(bad(c, format!("`{key}` must be at most {MAX_NAME} characters, without control characters")));
            }
            Ok((!n.is_empty()).then(|| n.to_string()))
        }
        Some(_) => Err(bad(c, format!("`{key}` must be a string"))),
    }
}

fn rename(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "profile.rename";
    let id = id_param(s, p, C)?;
    match name_param(p, "name", C)? {
        Some(n) => s.profile_edits.names.insert(id.clone(), n),
        None => s.profile_edits.names.remove(&id),
    };
    s.save_prefs()?;
    Ok(json!({"id": id, "name": s.all_profiles().into_iter().find(|x| x.id == id).map(|x| x.name)}))
}

fn set_group(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "profile.setGroup";
    let id = id_param(s, p, C)?;
    match name_param(p, "group", C)? {
        Some(g) => s.profile_edits.groups.insert(id.clone(), g),
        None => s.profile_edits.groups.remove(&id),
    };
    s.save_prefs()?;
    Ok(json!({"id": id, "group": s.all_profiles().into_iter().find(|x| x.id == id).map(|x| x.group)}))
}

fn reorder(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "profile.reorder";
    let ids = p.get("ids").and_then(Value::as_array).ok_or_else(|| bad(C, "`ids` must be a list of profile ids"))?;
    if ids.len() > MAX_ENTRIES {
        return Err(bad(C, "too many ids"));
    }
    let mut order: Vec<String> = Vec::new();
    for v in ids {
        let id = v.as_str().ok_or_else(|| bad(C, "`ids` must be a list of profile ids"))?;
        if !known(s, id) {
            return Err(bad(C, format!("unknown profile `{id}`")));
        }
        if !order.iter().any(|o| o == id) {
            order.push(id.to_string());
        }
    }
    // the ids given come first in that order, the rest keep theirs
    let rest: Vec<String> = s.profile_edits.order.iter().filter(|o| !order.contains(o)).cloned().collect();
    order.extend(rest);
    s.profile_edits.order = order;
    s.save_prefs()?;
    Ok(json!({"order": s.profile_edits.order}))
}

fn remove(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "profile.remove";
    let id = id_param(s, p, C)?;
    if crate::presets::profile(&id).is_none() {
        return Err(bad(C, "only a built-in look is removed here (an imported profile is deleted with its own command)"));
    }
    if !s.profile_edits.removed.contains(&id) {
        s.profile_edits.removed.push(id.clone());
    }
    s.profile_favorites.retain(|f| f != &id);
    s.profile_recent.retain(|f| f != &id);
    s.save_prefs()?;
    Ok(json!({"removed": id}))
}

fn restore_builtins(s: &mut Session, _p: &Value) -> Result<Value> {
    let n = s.profile_edits.removed.len();
    s.profile_edits.removed.clear();
    s.save_prefs()?;
    Ok(json!({"restored": n}))
}

/// The profiles of group `group` (as the manager shows it).
fn members(s: &Session, p: &Value, c: &str) -> Result<(String, Vec<crate::profile_edits::ShownProfile>)> {
    let g = name_param(p, "group", c)?.ok_or_else(|| bad(c, "missing `group`"))?;
    let list: Vec<_> = s.all_profiles().into_iter().filter(|x| x.group == g).collect();
    if list.is_empty() {
        return Err(bad(c, format!("no profile group `{g}`")));
    }
    Ok((g, list))
}

fn group_rename(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "profile.groupRename";
    let (g, list) = members(s, p, C)?;
    let to = name_param(p, "name", C)?.ok_or_else(|| bad(C, "missing `name`"))?;
    for x in &list {
        s.profile_edits.groups.insert(x.id.clone(), to.clone());
    }
    s.save_prefs()?;
    Ok(json!({"group": g, "name": to, "profiles": list.len()}))
}

fn group_hide(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "profile.groupHide";
    let (g, list) = members(s, p, C)?;
    let want = p.get("hidden").and_then(Value::as_bool).unwrap_or(!list.iter().all(|x| x.hidden));
    for x in &list {
        let on = s.profile_hidden.contains(&x.id);
        if want && !on {
            s.profile_hidden.push(x.id.clone());
        } else if !want {
            s.profile_hidden.retain(|h| *h != x.id);
        }
    }
    s.save_prefs()?;
    Ok(json!({"group": g, "hidden": want, "profiles": list.len()}))
}

fn group_remove(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "profile.groupRemove";
    let (g, list) = members(s, p, C)?;
    // built-in looks are removed (restorable); imported LUT profiles are hidden (deleting their
    // files is the LUT import's own command)
    let (mut removed, mut hidden) = (0, 0);
    for x in &list {
        if x.kind == "look" {
            if !s.profile_edits.removed.contains(&x.id) {
                s.profile_edits.removed.push(x.id.clone());
            }
            removed += 1;
        } else if !s.profile_hidden.contains(&x.id) {
            s.profile_hidden.push(x.id.clone());
            hidden += 1;
        }
    }
    s.save_prefs()?;
    Ok(json!({"group": g, "removed": removed, "hidden": hidden}))
}

fn hide(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "profile.hide";
    let id = str_param(p, "id").ok_or_else(|| bad(C, "missing id"))?.to_string();
    if crate::presets::profile(&id).is_none() && !s.lut_profiles.iter().any(|x| x.id == id) && !known_camera(&id) {
        return Err(bad(C, format!("unknown profile `{id}`")));
    }
    let on = s.profile_hidden.contains(&id);
    let want = p.get("hidden").and_then(Value::as_bool).unwrap_or(!on);
    if want && !on {
        s.profile_hidden.push(id);
    } else if !want {
        s.profile_hidden.retain(|h| *h != id);
    }
    s.save_prefs()?;
    Ok(json!({"hidden": want}))
}

/// The camera bodies of the library's raws, each with its photo count and camera profile.
fn bodies(s: &mut Session, _p: &Value) -> Result<Value> {
    let mut found: BTreeMap<String, (String, String, usize)> = BTreeMap::new();
    for p in s.catalog.photos() {
        let Some(k) = profile_bodies::key_of(p) else { continue };
        let Some(body) = profile_bodies::body(p) else { continue };
        let e = found.entry(k).or_insert_with(|| (p.meta.camera.clone(), body, 0));
        e.2 = e.2.saturating_add(1);
    }
    // a body with a default but no photo in this library is listed too
    for k in s.body_profiles.keys() {
        if !found.contains_key(k) {
            let (model, body) = k.split_once('|').unwrap_or((k.as_str(), ""));
            found.insert(k.clone(), (model.to_string(), body.to_string(), 0));
        }
    }
    let list: Vec<Value> = found
        .into_iter()
        .map(|(k, (camera, body, photos))| json!({"body": k, "camera": camera, "serial": body, "photos": photos, "profile": s.body_profiles.get(&k)}))
        .collect();
    Ok(json!({"bodies": list}))
}

/// The body key a command names: `body` as `bodies` lists it, or the body of photo `photo` (default:
/// the active photo).
fn body_param(s: &Session, p: &Value, c: &str) -> Result<String> {
    if let Some(b) = str_param(p, "body") {
        let b = b.trim();
        if b.is_empty() || b.len() > 200 {
            return Err(bad(c, "`body` must be a body as profiles.bodies lists it"));
        }
        return Ok(b.to_string());
    }
    let id = match p.get("photo") {
        Some(v) => PhotoId(v.as_u64().ok_or_else(|| bad(c, "`photo` must be a photo id"))?),
        None => s.active().ok_or_else(|| bad(c, "no body given and no active photo"))?,
    };
    let photo = s.catalog.photo(id).ok_or_else(|| bad(c, "no such photo"))?;
    profile_bodies::key_of(photo).ok_or_else(|| bad(c, "this photo isn't a raw with a known camera body"))
}

fn set_body_default(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "profile.setBodyDefault";
    let id = str_param(p, "id").unwrap_or("").trim().to_string();
    if !id.is_empty() && !profile_bodies::known_profile(&id) {
        return Err(bad(C, format!("unknown camera profile `{id}`")));
    }
    let body = body_param(s, p, C)?;
    if id.is_empty() {
        s.body_profiles.remove(&body);
    } else {
        s.body_profiles.insert(body.clone(), id.clone());
    }
    s.save_prefs()?;
    Ok(json!({"body": body, "profile": if id.is_empty() { Value::Null } else { json!(id) }}))
}

fn apply_body_defaults(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "profile.applyBodyDefaults";
    let ids: Vec<PhotoId> = match p.get("ids") {
        None | Some(Value::Null) => s.catalog.photos().map(|x| x.id).collect(),
        Some(_) => s.targets(p)?,
    };
    let ops: Vec<Op> = ids
        .into_iter()
        .filter_map(|id| {
            let photo = s.catalog.photo(id)?;
            let want = s.body_default(photo)?.to_string();
            (photo.develop.profile.camera != want).then(|| {
                let mut d = (*photo.develop).clone();
                d.profile.camera = want;
                s.develop_op(id, d, "Camera Profile")
            })?
        })
        .collect();
    let n = ops.len();
    if n > 0 {
        s.commit(C, Op::Batch { ops })?;
    }
    Ok(json!({"changed": n}))
}

fn profile_plan(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "import.profilePlan";
    let paths: Vec<String> = p
        .get("paths")
        .and_then(Value::as_array)
        .ok_or_else(|| bad(C, "`paths` must be a list of files or folders"))?
        .iter()
        .map(|v| v.as_str().map(str::to_string).ok_or_else(|| bad(C, "`paths` must be a list of files or folders")))
        .collect::<Result<_>>()?;
    let choice = profile_bodies::ImportProfile::parse(p.get("cameraProfile")).map_err(|e| bad(C, e))?;
    let candidates = crate::import::scan(s, &paths);
    let rows = profile_bodies::plan(s, &candidates, &choice);
    Ok(json!({"rows": rows, "photos": rows.iter().map(|r| r.photos).sum::<usize>()}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(query "import.profilePlan", "Camera Profiles for Import", [], None, "{paths: [file or folder], cameraProfile?: as library.import} → {rows: [{body (model|serial or model|file:XX, as cameraProfile.perBody takes it), camera, serial, photos, profile (\"\": the camera's default), profileName, how: auto|chosen|all}], photos}: the camera profile each body's new raws would get", always, profile_plan),
        cmd!(query "profiles.manager", "Manage Profiles", [], None, "{} → {profiles: [{id, name, originalName, group, kind: look|lut, hidden, builtinLabel, usedBy}] (the user's names, groups and order), cameras: [{id (dcp:… or cal:…), name, model, kind: camera, builtin, hidden, usedBy, bodyDefaults: [body]}], removed: [built-in look ids], bodies (as profiles.bodies)}", always, manager),
        cmd!(
            "profile.delete",
            "Delete Profile",
            [],
            None,
            "{id} — an imported LUT profile (lut:…) or camera profile (dcp:…) is deleted with its file; built-in looks and bundled camera profiles (cal:…) can only be hidden (profile.hide). Photos using it keep their settings: a gone LUT renders without it, a gone camera profile gives the camera's default → {deleted, kind: lut|camera, wasDefault?}",
            always,
            delete
        ),
        cmd!("profile.rename", "Rename Profile", [], None, "{id, name: string (\"\" or null: its own name again)} → {id, name}", always, rename),
        cmd!(
            "profile.setGroup",
            "Move Profile to Group",
            [],
            None,
            "{id, group: string (\"\" or null: its own group again)} → {id, group}",
            always,
            set_group
        ),
        cmd!(
            "profile.reorder",
            "Reorder Profiles",
            [],
            None,
            "{ids: [profile id]} — these first, in this order; the others keep theirs → {order}",
            always,
            reorder
        ),
        cmd!(
            "profile.remove",
            "Remove Built-in Look",
            [],
            None,
            "{id: a built-in look} — out of this install's lists (photos using it keep it; profile.restoreBuiltins brings it back) → {removed}",
            always,
            remove
        ),
        cmd!(
            "profile.restoreBuiltins",
            "Restore Built-in Looks",
            [],
            None,
            "{} — every removed built-in look comes back → {restored}",
            always,
            restore_builtins
        ),
        cmd!(
            "profile.groupRename",
            "Rename Profile Group",
            [],
            None,
            "{group, name} — every profile of the group moves to `name` → {group, name, profiles}",
            always,
            group_rename
        ),
        cmd!(
            "profile.groupHide",
            "Hide Profile Group",
            [],
            None,
            "{group, hidden?: bool (toggles when omitted)} → {group, hidden, profiles}",
            always,
            group_hide
        ),
        cmd!(
            "profile.groupRemove",
            "Remove Profile Group",
            [],
            None,
            "{group} — its built-in looks are removed (restorable), its imported profiles hidden → {group, removed, hidden}",
            always,
            group_remove
        ),
        cmd!(
            "profile.hide",
            "Hide Profile",
            [],
            None,
            "{id (a look, lut:…, dcp:… or cal:…), hidden?: bool (toggles when omitted)} — out of the profile menu, the browser and the camera profile choices (still in profiles.manager; photos using it keep it) → {hidden}",
            always,
            hide
        ),
        cmd!(query "profiles.bodies", "Camera Bodies", [], None, "{} → {bodies: [{body (the key the other commands take), camera, serial (or file:XX for bodies that write none), photos, profile (its camera profile, or null)}]}", always, bodies),
        cmd!(
            "profile.setBodyDefault",
            "Set Body's Camera Profile",
            [],
            None,
            "{id: camera profile (dcp:…, cal:… or lc; \"\" clears), body? (from profiles.bodies) | photo? (default: the active photo)} — that body's raws start with it at import → {body, profile}",
            always,
            set_body_default
        ),
        cmd!(
            "profile.applyBodyDefaults",
            "Apply Body Camera Profiles",
            [],
            None,
            "{ids?} (default: every photo) — photos of bodies with a camera profile get it, one undo step → {changed}",
            always,
            apply_body_defaults
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dcp_bytes(model: &str, name: &str) -> Vec<u8> {
        let m = lcc_raw::Mat3([[0.8, -0.1, -0.05], [-0.4, 1.3, 0.2], [-0.1, 0.25, 0.7]]);
        lcc_raw::dcp::write(&lcc_raw::dcp::Dcp {
            model: model.into(),
            name: name.into(),
            copyright: String::new(),
            color: lcc_raw::ColorData { illuminant: [21, 0], color_matrix: [Some(m), None], ..Default::default() },
        })
        .unwrap()
    }

    fn temp(name: &str) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!("pm-dcp-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (active, library) = (root.join("camera-profiles"), root.join("camera-profiles").join("dcp"));
        std::fs::create_dir_all(&library).unwrap();
        (root, active, library)
    }

    /// Delete removes the imported `.dcp` and, when it is its camera's default, the default's copy;
    /// another profile for the same camera stays.
    #[test]
    fn deleting_a_camera_profile_removes_its_file_and_its_default_copy() {
        let (root, active, library) = temp("delete");
        let mine = dcp_bytes("TESTCAM PM", "Mine");
        let other = dcp_bytes("TESTCAM PM", "Other");
        std::fs::write(library.join("testcam-pm-mine.dcp"), &mine).unwrap();
        std::fs::write(library.join("testcam-pm-other.dcp"), &other).unwrap();
        let default_copy = active.join(crate::camera_profiles::dcp_file_name("TESTCAM PM"));
        std::fs::write(&default_copy, &mine).unwrap();
        assert_eq!(delete_dcp_file(&library, &active, "dcp:testcam-pm-mine"), Ok(true));
        assert!(!library.join("testcam-pm-mine.dcp").exists() && !default_copy.exists());
        assert!(library.join("testcam-pm-other.dcp").exists(), "the camera's other profile stays");
        // not the default: only its own file goes
        assert_eq!(delete_dcp_file(&library, &active, "dcp:testcam-pm-other"), Ok(false));
        assert!(delete_dcp_file(&library, &active, "dcp:testcam-pm-other").is_err(), "deleted twice is an error, not a panic");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// An id from a control-channel or MCP caller never deletes anything outside the library folder.
    #[test]
    fn hostile_camera_profile_ids_delete_nothing() {
        let (root, active, library) = temp("hostile");
        let outside = root.join("camera-profiles").join("keep.dcp");
        std::fs::write(&outside, dcp_bytes("TESTCAM PM", "Keep")).unwrap();
        for id in ["dcp:../keep", "dcp:", "dcp:.", "dcp:..", "dcp:a/b", r"dcp:a\b", "dcp:C:keep", "lut:keep", "keep", "dcp:nothing-here"] {
            assert!(delete_dcp_file(&library, &active, id).is_err(), "{id}");
        }
        assert!(outside.exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The manager lists how many photos use each profile and the bodies a camera profile starts;
    /// deleting a LUT profile a photo uses leaves the photo's settings alone and it renders without
    /// the LUT; a camera profile that is gone falls back to the camera's default; built-ins can't be
    /// deleted.
    #[test]
    fn deleting_a_profile_photos_use_keeps_them_working() {
        let mut s = Session::with_demo();
        let id = s.catalog.photos().next().map(|p| p.id).unwrap();
        s.execute("library.select", &json!({"ids": [id.0]})).unwrap();
        // a LUT profile of this session only (no file: nothing on disk is touched)
        let lut = "lut:pm-test-in-use";
        let cube = "LUT_3D_SIZE 2\n0 0 0\n1 0 0\n0 1 0\n1 1 0\n0 0 1\n1 0 1\n0 1 1\n1 1 1\n";
        lcc_pipeline::lut::register(lut, lcc_pipeline::lut::Lut3d::parse_cube(cube).unwrap());
        s.lut_profiles.push(super::super::lut_profiles::LutProfile {
            id: lut.into(),
            name: "In Use".into(),
            group: "Mine".into(),
            file: String::new(),
        });
        s.execute("develop.profile", &json!({"id": lut, "amount": 100})).unwrap();
        // the photo names an imported camera profile too (the registry is process-wide: an id of its own)
        let mut d = (*s.develop_of(id).unwrap()).clone();
        d.profile.camera = "dcp:pm-test-gone".into();
        let op = s.develop_op(id, d, "Camera Profile").unwrap();
        s.commit("test", op).unwrap();
        s.body_profiles.insert("PMTEST|1".into(), "dcp:pm-test-gone".into());
        let m = s.execute("profiles.manager", &json!({})).unwrap();
        let mine = m["profiles"].as_array().unwrap().iter().find(|p| p["id"] == lut).cloned().unwrap();
        assert_eq!(mine["usedBy"], 1, "{mine}");
        assert!(m["cameras"].is_array());
        // built-ins are hidden, not deleted
        assert!(s.execute("profile.delete", &json!({"id": "lc.vivid"})).is_err());
        assert!(s.execute("profiles.list", &json!({})).unwrap().as_array().unwrap().iter().any(|p| p["id"] == "lc.vivid"));
        // the LUT profile is deleted while the photo uses it: the photo keeps its settings and
        // renders without it
        let r = s.execute("profile.delete", &json!({"id": lut})).unwrap();
        assert_eq!(r["kind"], "lut");
        let d = s.develop_of(id).unwrap();
        assert_eq!(d.profile.id, lut, "the photo keeps its settings");
        assert!(lcc_pipeline::lut::get(lut).is_none());
        let _ = lcc_pipeline::profiles::effective(&d);
        assert!(s.profile_info(lut).is_none());
        assert!(s.execute("profiles.manager", &json!({})).is_ok());
        // a camera profile that is gone: the camera's default, no panic
        let camera = s.catalog.photo(id).unwrap().meta.camera.clone();
        let gone = lcc_raw::dcp::resolve_camera("dcp:pm-test-gone", &camera).map(|d| d.name.clone());
        assert_eq!(gone, lcc_raw::dcp::resolve_camera("", &camera).map(|d| d.name.clone()));
        forget_camera_profile(&mut s, "dcp:pm-test-gone");
        assert!(s.body_profiles.is_empty(), "no body starts with a deleted camera profile");
        // bad ids are errors, built-in camera profiles can't be deleted
        assert!(s.execute("profile.delete", &json!({"id": "nothing"})).is_err());
        assert!(s.execute("profile.delete", &json!({})).is_err());
        assert!(s.execute("profile.hide", &json!({"id": "dcp:pm-test-gone"})).is_err(), "an unknown camera profile isn't hidden");
    }
}
