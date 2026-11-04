//! `concepts.*`: remembered objects. Select a thing on a photo (any mask: an Object click, a
//! brush, People ▸ Clothes…) and name it ("Wedding dress"); `concepts.find` then selects it on
//! other photos. Learning and finding run on SAM 2.1 (open model) on the AI worker; what is
//! learned stays in the library (`concepts.json`).
//!
//! Ported from OneSlips' fork (own design there).

use lcc_catalog::PhotoId;
use serde_json::{Value, json};

use super::{CommandSpec, bad, cmd, str_param};
use crate::{Result, Session};

fn always(_: &Session) -> std::result::Result<(), String> {
    Ok(())
}

fn name_param(p: &Value, key: &str, c: &str) -> Result<String> {
    let n = str_param(p, key).map(str::trim).filter(|n| !n.is_empty()).ok_or_else(|| bad(c, format!("missing `{key}`")))?;
    if n.chars().count() > 120 {
        return Err(bad(c, format!("`{key}` is too long")));
    }
    Ok(n.to_string())
}

fn photo_param(s: &Session, p: &Value, c: &str) -> Result<PhotoId> {
    p.get("id").and_then(Value::as_u64).map(PhotoId).or_else(|| s.active()).ok_or_else(|| bad(c, "no photo"))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(query "concepts.list", "Remembered Objects", [], None, "→ {concepts: [{name, examples, photos}]}", always, |s, _| {
            let list: Vec<Value> =
                s.concept_store().concepts.iter().map(|c| json!({"name": c.name, "examples": c.examples.len(), "photos": c.photos})).collect();
            Ok(json!({"concepts": list}))
        }),
        cmd!(
            "concepts.teach",
            "Remember as…",
            [],
            None,
            "{name: \"Wedding dress\", id?: photo, mask?: maskId (the selected mask)} — learn what the mask selects (SAM 2.1), adding an example to the remembered object (made when new); teach it on a few photos (worn and hanging, light and dark) → {name, examples} (the app: {pending: true}, a notice follows)",
            always,
            |s, p| {
                let c = "concepts.teach";
                let name = name_param(p, "name", c)?;
                let id = photo_param(s, p, c)?;
                let d = s.develop_of(id).ok_or_else(|| bad(c, "no such photo"))?;
                let mid = p
                    .get("mask")
                    .and_then(Value::as_u64)
                    .and_then(|m| u32::try_from(m).ok())
                    .or(s.active_mask)
                    .ok_or_else(|| bad(c, "select the mask to learn from (give `mask`)"))?;
                let mask = d.masks.iter().find(|m| m.id == mid).cloned().ok_or_else(|| bad(c, format!("no mask {mid}")))?;
                if mask.components.is_empty() {
                    return Err(bad(c, "the mask selects nothing"));
                }
                match s.concept_teach(id, mask, &name).map_err(|e| bad(c, e))? {
                    Some(n) => Ok(json!({"name": name, "examples": n})),
                    None => Ok(json!({"pending": true, "name": name})),
                }
            }
        ),
        cmd!(
            "concepts.find",
            "Find Remembered Object",
            [],
            None,
            "{name, id?: photo (the active one), mask?: maskId (add to it as a component), op?: add|subtract|intersect (default add)} — select a remembered object on the photo (SAM 2.1): an Object selection named after it, grown from the clicks its look picked → the new mask (the app: {pending: true})",
            always,
            |s, p| {
                let c = "concepts.find";
                let name = name_param(p, "name", c)?;
                let id = photo_param(s, p, c)?;
                if s.active() != Some(id) {
                    s.execute("library.select", &json!({"ids": [id.0]}))?;
                }
                let mask = p.get("mask").and_then(Value::as_u64).and_then(|m| u32::try_from(m).ok());
                let op = str_param(p, "op").unwrap_or("add");
                if !matches!(op, "add" | "subtract" | "intersect") {
                    return Err(bad(c, format!("unknown op `{op}` (add|subtract|intersect)")));
                }
                match s.concept_find(id, &name, mask, op).map_err(|e| bad(c, e))? {
                    Some(r) => Ok(r),
                    None => Ok(json!({"pending": true, "name": name})),
                }
            }
        ),
        cmd!("concepts.rename", "Rename Remembered Object", [], None, "{from, to}", always, |s, p| {
            let c = "concepts.rename";
            let (from, to) = (name_param(p, "from", c)?, name_param(p, "to", c)?);
            s.with_concepts(|st| st.rename(&from, &to)).map_err(|e| bad(c, e))?.map_err(|e| bad(c, e))?;
            Ok(json!({"name": to}))
        }),
        cmd!("concepts.forget", "Forget Remembered Object", [], None, "{name} → {forgotten}", always, |s, p| {
            let c = "concepts.forget";
            let name = name_param(p, "name", c)?;
            let f = s.with_concepts(|st| st.forget(&name)).map_err(|e| bad(c, e))?;
            Ok(json!({"forgotten": f}))
        }),
    ]
}
