//! The Profile Manager's commands (ported from OneSlips, LR-PROF-MANAGE and LR-PROF-BODY): hide
//! profiles from the menus and the browser, and give camera bodies their own camera profile.

use std::collections::BTreeMap;

use lcc_catalog::{Op, PhotoId};
use serde_json::{Value, json};

use super::{CommandSpec, always, bad, cmd, str_param};
use crate::profile_bodies;
use crate::{Result, Session};

/// Every profile with its group and whether it is hidden: the built-in looks, then imported LUTs.
fn manager(s: &mut Session, _p: &Value) -> Result<Value> {
    let hidden = |id: &str| s.profile_hidden.iter().any(|h| h == id);
    let mut profiles: Vec<Value> = crate::presets::PROFILES
        .iter()
        .map(|p| json!({"id": p.id, "name": p.name, "group": p.group, "kind": "look", "hidden": hidden(p.id)}))
        .collect();
    profiles.extend(s.lut_profiles.iter().map(|p| json!({"id": p.id, "name": p.name, "group": p.group, "kind": "lut", "hidden": hidden(&p.id)})));
    let bodies = bodies(s, &json!({}))?;
    Ok(json!({"profiles": profiles, "bodies": bodies["bodies"]}))
}

fn hide(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "profile.hide";
    let id = str_param(p, "id").ok_or_else(|| bad(C, "missing id"))?.to_string();
    if crate::presets::profile(&id).is_none() && !s.lut_profiles.iter().any(|x| x.id == id) {
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

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(query "profiles.manager", "Manage Profiles", [], None, "{} → {profiles: [{id, name, group, kind: look|lut, hidden}], bodies (as profiles.bodies)}", always, manager),
        cmd!(
            "profile.hide",
            "Hide Profile",
            [],
            None,
            "{id, hidden?: bool (toggles when omitted)} — out of the profile menu and browser (still in profiles.manager; photos using it keep it) → {hidden}",
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
