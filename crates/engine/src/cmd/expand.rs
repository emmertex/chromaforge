//! Crop ▸ Extend Past the Edge and Fill Edges (issue #861, [`crate::expand`]): a crop that may reach past the photo,
//! and filling what it shows there, content-aware on this computer or with a generative fill service.

use lcc_develop::EdgeFillKind;
use serde_json::{Value, json};

use super::{CommandSpec, bad, cmd, has_active, str_param};
use crate::{PhotoId, Result, Session};

fn active(s: &Session, c: &str) -> Result<PhotoId> {
    s.active().ok_or_else(|| bad(c, "no active photo"))
}

fn extend(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "crop.extend";
    let id = active(s, C)?;
    let mut d = (*s.develop_of(id).unwrap_or_default()).clone();
    let on = match p.get("on") {
        None | Some(Value::Null) => !d.crop.extend,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(bad(C, "`on` is true or false")),
    };
    if on == d.crop.extend {
        return Ok(json!({"extend": on}));
    }
    d.crop.extend = on;
    if !on {
        // back on the photo: the crop is fitted to it again and its fill goes (undo brings both back)
        let (w, h) = super::develop::crop_dims(s, id);
        d.crop.geometry = d.crop.geometry.constrained(w, h);
        d.edge_fill = None;
    }
    s.set_develop(id, d, if on { "Extend Crop Past the Edge" } else { "Keep Crop on the Photo" })?;
    Ok(json!({"extend": on}))
}

fn fill_edges(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "crop.fillEdges";
    let id = active(s, C)?;
    let kind = match str_param(p, "mode") {
        None | Some("contentAware") => EdgeFillKind::ContentAware,
        Some("generative") => EdgeFillKind::Generative,
        Some(m) => return Err(bad(C, format!("unknown mode `{m}` (contentAware or generative)"))),
    };
    let generative =
        (kind == EdgeFillKind::Generative).then(|| (str_param(p, "provider").unwrap_or_default(), str_param(p, "prompt").unwrap_or_default()));
    match s.edge_fill_start(id, kind, generative).map_err(|e| bad(C, e))? {
        None => Ok(json!({"started": true})),
        Some(f) => Ok(json!({"filled": {"kind": f.kind, "id": f.fill.id, "rect": f.rect, "scale": f.fill.scale, "provider": f.fill.provider}})),
    }
}

fn clear_fill(s: &mut Session, _: &Value) -> Result<Value> {
    const C: &str = "crop.clearFill";
    let id = active(s, C)?;
    let mut d = (*s.develop_of(id).unwrap_or_default()).clone();
    if d.edge_fill.take().is_none() {
        return Ok(json!({"cleared": false}));
    }
    s.set_develop(id, d, "Clear Edge Fill")?;
    Ok(json!({"cleared": true}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "crop.extend",
            "Extend Crop Past the Edge",
            [],
            None,
            "{on?: bool (default: toggle)}: let the crop reach past the photo (up to half the photo's size on each side), where the render shows blank canvas until Fill Edges fills it; off fits the crop to the photo again and drops its edge fill → {extend}",
            has_active,
            extend
        ),
        cmd!(
            query "crop.fillEdges",
            "Fill Edges",
            [],
            None,
            "{mode?: contentAware (default: from the photo itself, on this computer) | generative (a generative fill service, with YOUR key, after you accepted its terms), provider? (generative; openai), prompt? (generative; empty: continue the scene)}: fill what the extended crop shows past the photo; one undo step. In the app it runs in the background (activity row with ✕) → {started}; elsewhere it waits → {filled: {kind, id, rect, scale, provider}}. Never journaled or replayed.",
            has_active,
            fill_edges
        ),
        cmd!(
            "crop.clearFill",
            "Clear Edge Fill",
            [],
            None,
            "{}: remove the fill of the crop's area past the photo (blank canvas again) → {cleared}",
            has_active,
            clear_fill
        ),
        cmd!(query "crop.cancelFill", "Cancel Fill Edges", [], None, "{} → {cancelled}", |_| Ok(()), |s, _| Ok(json!({"cancelled": s.edge_fill_cancel()}))),
    ]
}
