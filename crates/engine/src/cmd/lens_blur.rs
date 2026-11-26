//! Lens Blur (`lensBlur.*`): a shallow depth of field from the photo's estimated depth, rendered
//! by `lcc_pipeline::lens_blur`. The sliders are also develop controls (`lensBlur.amount`,
//! `lensBlur.boost`, `lensBlur.focalDistance`, `lensBlur.focalRange`).

use lcc_develop::{BokehShape, LensBlur};
use lcc_geom::Point;
use serde_json::{Value, json};

use super::{CommandSpec, bad, cmd, has_active, str_param};
use crate::{PhotoId, Result, Session};

fn active(s: &Session, c: &str) -> Result<PhotoId> {
    s.active().ok_or_else(|| bad(c, "no active photo"))
}

/// Where the depth comes from now: `model` (the depth model) or `subject` (estimated from the subject).
pub fn depth_source() -> &'static str {
    if lcc_pipeline::depth_model::has_depth_model() { "model" } else { "subject" }
}

fn status_of(lb: &LensBlur) -> Value {
    json!({
        "enabled": lb.enabled,
        "amount": lb.amount,
        "boost": lb.boost,
        "focalDistance": lb.focal_distance,
        "focalRange": lb.focal_range,
        "focus": lb.focus.map(|p| json!({"x": p.x, "y": p.y})),
        "focusSize": lb.focus_size,
        "bokeh": lb.bokeh.name(),
        "depth": depth_source(),
    })
}

fn num(c: &str, p: &Value, key: &str, lo: f64, hi: f64) -> Result<Option<f64>> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => {
            let x = v.as_f64().filter(|x| x.is_finite()).ok_or_else(|| bad(c, format!("`{key}` must be a number")))?;
            Ok(Some(x.clamp(lo, hi)))
        }
    }
}

fn set(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "lensBlur.set";
    let id = active(s, c)?;
    let bokeh = match str_param(p, "bokeh") {
        Some(b) => Some(BokehShape::parse(b).ok_or_else(|| bad(c, format!("unknown bokeh `{b}` (circle, pentagon, hexagon or ring)")))?),
        None => None,
    };
    let focus = match p.get("focus") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(v) => {
            let g = |k: &str| v.get(k).and_then(Value::as_f64).filter(|x| x.is_finite());
            let (Some(x), Some(y)) = (g("x"), g("y")) else { return Err(bad(c, "`focus` is {x, y} in 0..1 (or null to clear it)")) };
            Some(Some(Point::new(x.clamp(0.0, 1.0), y.clamp(0.0, 1.0))))
        }
    };
    let enabled = match p.get("enabled") {
        None | Some(Value::Null) => None,
        Some(v) => Some(v.as_bool().ok_or_else(|| bad(c, "`enabled` must be true or false"))?),
    };
    let amount = num(c, p, "amount", 0.0, 100.0)?;
    let boost = num(c, p, "boost", 0.0, 100.0)?;
    let dist = num(c, p, "focalDistance", 0.0, 100.0)?;
    let range = num(c, p, "focalRange", 0.0, 100.0)?;
    let size = num(c, p, "focusSize", 0.0, 0.5)?;
    let mut d = (*s.develop_of(id).ok_or_else(|| bad(c, "no such photo"))?).clone();
    let lb = &mut d.lens_blur;
    // (the sliders or the picker switch it on, as in Lightroom)
    let touched = amount.is_some() || boost.is_some() || dist.is_some() || range.is_some() || focus.is_some() || bokeh.is_some() || size.is_some();
    lb.enabled = enabled.unwrap_or(lb.enabled || touched);
    if let Some(v) = amount {
        lb.amount = v;
    }
    if let Some(v) = boost {
        lb.boost = v;
    }
    if let Some(v) = dist {
        lb.focal_distance = v;
        // a focal distance typed in replaces the picked point
        if focus.is_none() {
            lb.focus = None;
        }
    }
    if let Some(v) = range {
        lb.focal_range = v;
    }
    if let Some(f) = focus {
        lb.focus = f;
    }
    if let Some(v) = size {
        lb.focus_size = v;
    }
    if let Some(b) = bokeh {
        lb.bokeh = b;
    }
    let out = status_of(lb);
    s.set_develop(id, d, "Lens Blur")?;
    Ok(out)
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "lensBlur.set",
            "Set Lens Blur",
            [],
            None,
            "{enabled?: bool, amount?: 0..100 (Blur Amount), boost?: 0..100 (brighter out-of-focus highlights), focalDistance?: 0..100 (0 nearest; clears the picked focus), focalRange?: 0..100 (depth kept sharp), focus?: {x, y} (normalized point of the uncropped photo whose distance is kept sharp) | null, focusSize?: 0..0.5 (half the picked area's side; 0 = a point), bokeh?: circle|pentagon|hexagon|ring}: change the active photo's Lens Blur (one undo step; any change but `enabled: false` switches it on). Depth comes from the depth model (`ai.models.download {id: \"depth\"}`), else it is estimated from the subject → lensBlur.status",
            has_active,
            set
        ),
        cmd!(
            query "lensBlur.status",
            "Lens Blur Status",
            [],
            None,
            "{}: the active photo's Lens Blur → {enabled, amount, boost, focalDistance, focalRange, focus, focusSize, bokeh, depth: model|subject}",
            has_active,
            |s, _| {
                let c = "lensBlur.status";
                let id = active(s, c)?;
                let d = s.develop_of(id).ok_or_else(|| bad(c, "no such photo"))?;
                Ok(status_of(&d.lens_blur))
            }
        ),
    ]
}
