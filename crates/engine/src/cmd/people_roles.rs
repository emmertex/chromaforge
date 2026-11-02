//! `people.*` roles: who has a role in a shoot ("Bride" is Sam in this folder), so masks and
//! presets that say "the Bride" find that person on each photo. Kept in the library's
//! `people.json` ([`crate::people_store`]), never in sidecars, exports or on the network;
//! `people.forget` / `people.forgetAll` delete it.
//!
//! Ported from OneSlips' fork (own design there).

use lcc_catalog::PhotoId;
use serde_json::{Value, json};

use super::{CommandSpec, bad, cmd, str_param};
use crate::people_store::Scope;
use crate::{Result, Session};

fn always(_: &Session) -> std::result::Result<(), String> {
    Ok(())
}

fn name_param(p: &Value, key: &str, c: &str) -> Result<String> {
    let n = str_param(p, key).map(str::trim).filter(|n| !n.is_empty()).ok_or_else(|| bad(c, format!("missing `{key}`")))?;
    if n.chars().count() > 200 {
        return Err(bad(c, format!("`{key}` is too long")));
    }
    Ok(n.to_string())
}

fn scope_param(s: &Session, p: &Value, photo: Option<PhotoId>, c: &str) -> Result<Scope> {
    let v = p.get("scope").cloned().unwrap_or(Value::Null);
    let folder_of = |id: PhotoId| s.photo_context(id).folder;
    Ok(match &v {
        Value::Null => Scope::Folder(photo.and_then(folder_of).ok_or_else(|| bad(c, "the photo has no folder: give `scope`"))?),
        Value::String(k) if matches!(k.as_str(), "folder" | "current") => {
            Scope::Folder(photo.and_then(folder_of).ok_or_else(|| bad(c, "the photo has no folder: give `scope`"))?)
        }
        Value::Object(o) => {
            if let Some(f) = o.get("folder").and_then(Value::as_str) {
                Scope::Folder(crate::people_store::norm_folder(f))
            } else if let Some(a) = o.get("album").and_then(Value::as_u64) {
                Scope::Album(a)
            } else if let (Some(f), Some(t)) = (o.get("from").and_then(Value::as_str), o.get("to").and_then(Value::as_str)) {
                let d = |x: &str| x.get(..10).unwrap_or(x).to_string();
                Scope::Dates { from: d(f), to: d(t) }
            } else if let Some(ids) = o.get("ids").and_then(Value::as_array) {
                Scope::Photos(ids.iter().filter_map(Value::as_u64).take(100_000).collect())
            } else {
                return Err(bad(c, "`scope` must be \"folder\", {folder}, {album}, {from, to} or {ids}"));
            }
        }
        _ => return Err(bad(c, "`scope` must be \"folder\", {folder}, {album}, {from, to} or {ids}")),
    })
}

fn scope_json(sc: &Scope) -> Value {
    serde_json::to_value(sc).unwrap_or(Value::Null)
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "people.setRole",
            "Set Role",
            [],
            None,
            "{role: \"Bride\", person: \"Sam\", id?, scope?: \"folder\" (the photo's folder, default) | {folder} | {album} | {from, to} (dates) | {ids}} — who has the role in this shoot; masks and presets saying \"Bride\" find that person → {role, person, scope}",
            always,
            |s, p| {
                let c = "people.setRole";
                let (role, person) = (name_param(p, "role", c)?, name_param(p, "person", c)?);
                let photo = p.get("id").and_then(Value::as_u64).map(PhotoId).or_else(|| s.active());
                let scope = scope_param(s, p, photo, c)?;
                let sj = scope_json(&scope);
                s.with_people(|st| st.set_role(&role, &person, scope)).map_err(crate::EngineError::Other)?.map_err(|e| bad(c, e))?;
                Ok(json!({"role": role, "person": person, "scope": sj}))
            }
        ),
        cmd!(
            query "people.roles",
            "Roles",
            [],
            None,
            "{id?} → {here: [{role, person}] (for the photo), roles: [{role, person, scope}] (all)}",
            always,
            |s, p| {
                let photo = p.get("id").and_then(Value::as_u64).map(PhotoId).or_else(|| s.active());
                let ctx = photo.map(|id| s.photo_context(id));
                let store = s.people_store().clone();
                let here: Vec<Value> =
                    ctx.map(|c| store.roles_for(&c).into_iter().map(|(r, n)| json!({"role": r, "person": n})).collect()).unwrap_or_default();
                let all: Vec<Value> =
                    store.roles.iter().map(|b| json!({"role": b.role, "person": b.person, "scope": scope_json(&b.scope)})).collect();
                Ok(json!({"here": here, "roles": all}))
            }
        ),
        cmd!("people.clearRole", "Clear Role", [], None, "{role, scope?: as people.setRole (omitted: everywhere)} → {removed}", always, |s, p| {
            let c = "people.clearRole";
            let role = name_param(p, "role", c)?;
            let scope = if p.get("scope").is_some() { Some(scope_param(s, p, s.active(), c)?) } else { None };
            let n = s.with_people(|st| st.clear_role(&role, scope.as_ref())).map_err(crate::EngineError::Other)?;
            Ok(json!({"removed": n}))
        }),
        cmd!(
            "people.forget",
            "Forget This Person",
            [],
            None,
            "{person} — delete the roles and appearance examples kept for them in people.json (biometric data; their name on the photos stays) → {forgotten}",
            always,
            |s, p| {
                let person = name_param(p, "person", "people.forget")?;
                let forgotten = s.with_people(|st| st.forget(&person)).map_err(crate::EngineError::Other)?;
                Ok(json!({"forgotten": forgotten}))
            }
        ),
        cmd!(
            "people.forgetAll",
            "Forget All Roles and Appearance Data",
            [],
            None,
            "{} — empty people.json: every role and appearance example (names on the photos and face recognition stay; see faces.* for those) → {deleted: [files]}",
            always,
            |s, _| {
                s.with_people(|st| *st = crate::people_store::PeopleStore::default()).map_err(crate::EngineError::Other)?;
                Ok(json!({"deleted": [crate::people_store::FILE]}))
            }
        ),
    ]
}
