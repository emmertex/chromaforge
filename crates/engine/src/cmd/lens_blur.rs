//! Lens Blur (`lensBlur.*`): a shallow depth of field from the photo's estimated depth, rendered
//! by `lcc_pipeline::lens_blur`. The sliders are also develop controls (`lensBlur.amount`,
//! `lensBlur.boost`, `lensBlur.focalDistance`, `lensBlur.focalRange`).

use lcc_develop::{BokehShape, LensBlur};
use lcc_geom::Point;
use serde_json::{Value, json};

use super::{CommandSpec, bad, bool_or, cmd, f64_or, has_active, str_param};
use crate::{PhotoId, Result, Session};

/// Strokes one Lens Blur refinement brush keeps (a document can't grow without bound).
const MAX_BRUSH_STROKES: usize = 2000;

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
        "focusStrokes": lb.focus_strokes.len(),
        "blurStrokes": lb.blur_strokes.len(),
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

/// `lensBlur.brushStroke`: one stroke of the Focus or the Blur refinement brush.
fn brush_stroke(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "lensBlur.brushStroke";
    let focus = match str_param(p, "mode") {
        Some("focus") => true,
        Some("blur") => false,
        _ => return Err(bad(C, "`mode` must be \"focus\" or \"blur\"")),
    };
    let points: Vec<Point> = p
        .get("points")
        .and_then(Value::as_array)
        .ok_or_else(|| bad(C, "missing points"))?
        .iter()
        .filter_map(|q| Some(Point::new(q.get(0)?.as_f64()?, q.get(1)?.as_f64()?)))
        .filter(|q| q.x.is_finite() && q.y.is_finite())
        .collect();
    if points.is_empty() {
        return Err(bad(C, "a stroke needs at least one point"));
    }
    let d = lcc_develop::BrushStroke::default();
    let num = |key: &str, def: f64, lo: f64, hi: f64| {
        let v = f64_or(p, key, def);
        if v.is_finite() { v.clamp(lo, hi) } else { def }
    };
    let stroke = lcc_develop::BrushStroke {
        pressure: super::masks::pressure_list(p.get("pressure"), points.len())?,
        points,
        size: num("size", d.size, 0.001, 0.5),
        feather: num("feather", d.feather, 0.0, 100.0),
        flow: num("flow", d.flow, 1.0, 100.0),
        density: num("amount", d.density, 1.0, 100.0),
        erase: bool_or(p, "erase", false),
        auto_mask: bool_or(p, "autoMask", false),
        // the Masking brush's model: new strokes paint with it unless `model: 0` asks for the
        // legacy one; the Auto Mask range is fixed with the stroke
        auto_range: num("autoRange", d.auto_range, -lcc_develop::AUTO_RANGE_MAX, lcc_develop::AUTO_RANGE_MAX),
        model: if p.get("model").and_then(Value::as_u64) == Some(0) { lcc_develop::BRUSH_MODEL_LEGACY } else { lcc_develop::BRUSH_MODEL_LRC },
    };
    let id = active(s, C)?;
    let mut d = (*s.develop_of(id).ok_or_else(|| bad(C, "no such photo"))?).clone();
    let lb = &mut d.lens_blur;
    let strokes = if focus { &mut lb.focus_strokes } else { &mut lb.blur_strokes };
    if strokes.len() >= MAX_BRUSH_STROKES {
        return Err(bad(C, format!("at most {MAX_BRUSH_STROKES} strokes per brush: reset the brush first")));
    }
    strokes.push(stroke);
    let out = status_of(lb);
    s.set_develop(id, d, if focus { "Lens Blur Focus Brush" } else { "Lens Blur Blur Brush" })?;
    Ok(out)
}

/// `lensBlur.brushReset`: remove one refinement brush's strokes, or both.
fn brush_reset(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "lensBlur.brushReset";
    let (focus, blur) = match str_param(p, "mode") {
        None => (true, true),
        Some("focus") => (true, false),
        Some("blur") => (false, true),
        Some(_) => return Err(bad(C, "`mode` must be \"focus\" or \"blur\"")),
    };
    let id = active(s, C)?;
    let mut d = (*s.develop_of(id).ok_or_else(|| bad(C, "no such photo"))?).clone();
    let lb = &mut d.lens_blur;
    let before = (lb.focus_strokes.len(), lb.blur_strokes.len());
    if focus {
        lb.focus_strokes.clear();
    }
    if blur {
        lb.blur_strokes.clear();
    }
    let out = status_of(lb);
    // (nothing painted: no undo step)
    if before != (lb.focus_strokes.len(), lb.blur_strokes.len()) {
        s.set_develop(id, d, "Reset Lens Blur Brush")?;
    }
    Ok(out)
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "lensBlur.brushStroke",
            "Lens Blur Brush Stroke",
            [],
            None,
            "{mode: \"focus\"|\"blur\", points: [[x,y],…] normalized, size?: the solid core's radius, fraction of the long edge (0.03), feather?: 0..100, flow?: 1..100, amount?: 1..100 (how far the stroke refines), erase?: bool, autoMask?: bool, autoRange?, pressure?: [0..1 per point], model?: 0 for the legacy brush}: Brush Refinement, one stroke (one undo step). Focus strokes stay sharp whatever the depth says; Blur strokes get the whole blur and overrule Focus strokes → lensBlur.status",
            has_active,
            brush_stroke
        ),
        cmd!(
            "lensBlur.brushReset",
            "Reset Lens Blur Brush",
            [],
            None,
            "{mode?: \"focus\"|\"blur\"}: removes that refinement brush's strokes (both brushes without `mode`) → lensBlur.status",
            has_active,
            brush_reset
        ),
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
            "{}: the active photo's Lens Blur → {enabled, amount, boost, focalDistance, focalRange, focus, focusSize, bokeh, focusStrokes, blurStrokes (how many strokes each refinement brush holds), depth: model|subject}",
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
