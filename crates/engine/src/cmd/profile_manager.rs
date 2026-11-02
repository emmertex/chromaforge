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
    Ok(json!({"profiles": s.all_profiles(), "removed": s.profile_edits.removed, "bodies": bodies["bodies"]}))
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
        cmd!(query "profiles.manager", "Manage Profiles", [], None, "{} → {profiles: [{id, name, originalName, group, kind: look|lut, hidden, builtinLabel}] (the user's names, groups and order), removed: [built-in look ids], bodies (as profiles.bodies)}", always, manager),
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
