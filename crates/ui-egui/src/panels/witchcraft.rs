//! The Witchcraft grading mode's palettes (DaVinci Resolve style colour grading, beyond
//! Lightroom): the colourist primaries — Lift, Gamma, Gain and Offset wheels, each with its
//! master wheel beneath — in the Color variant (wide tonal overlap) and the Log variant (narrow
//! ranges, `primaries.low`/`high`). The wheels drag the engine's own `primaries.*` controls, so
//! everything else (history, presets, XMP, MCP, the GPU) sees one edit like any other.

use egui::{Align2, Color32, Pos2, Rect, Sense, Stroke, pos2, vec2};
use lcc_develop::DevelopSettings;
use serde_json::json;

use super::edit::{
    WHEEL_CAPTURE, WheelCapture, WheelGrab, WheelLock, WheelMods, control_as, give_pointer_back, paint_wheel, wheel_pick, wheel_pointer,
};
use crate::LccApp;
use crate::theme::Tokens;
use crate::widgets::register;

/// The wheels, in the palette's order: the key names the `primaries.<key>.*` controls; the label
/// is the Color variant's (the Log variant renames the first three).
const WHEELS: [(&str, &str, &str); 4] =
    [("lift", "Lift", "Shadows"), ("gamma", "Gamma", "Midtones"), ("gain", "Gain", "Highlights"), ("offset", "Offset", "Offset")];

/// The Primaries palette: the Color | Log variant switch, the four wheels in two rows, the master
/// wheel of each under its column, and the Log variant's range boundaries.
pub(crate) fn primaries(app: &mut LccApp, ui: &mut egui::Ui, d: &DevelopSettings) {
    // Color (the default) or Log, as the palette's own segmented switch (Resolve's Primaries palette)
    egui::Frame::NONE.inner_margin(crate::widgets::row_margin(ui, egui::Margin { left: 12, right: 16, top: 6, bottom: 2 }, 3, 2)).show(ui, |ui| {
        ui.horizontal(|ui| {
            let active = usize::from(d.primaries.log);
            if let Some(i) = crate::widgets::segmented(ui, "primariesVariant", &[("Color", "color"), ("Log", "log")], Some(active), 2) {
                let _ = app.run("develop.primaries", json!({"log": i == 1}));
            }
        });
    });
    // four wheels in two rows, the master wheels under their columns (as the 3-Way grading lays
    // its wheels and luminance sliders out)
    let w = ui.available_width();
    let rad = 44.0;
    let (area, _) = ui.allocate_exact_size(vec2(w, rad * 4.0 + 64.0), Sense::hover());
    let at = [
        pos2(area.left() + w * 0.28, area.top() + rad + 8.0),
        pos2(area.left() + w * 0.72, area.top() + rad + 8.0),
        pos2(area.left() + w * 0.28, area.top() + rad * 3.0 + 36.0),
        pos2(area.left() + w * 0.72, area.top() + rad * 3.0 + 36.0),
    ];
    let labels: Vec<&str> = WHEELS.iter().map(|(_, color, log)| if d.primaries.log { *log } else { *color }).collect();
    for ((key, ..), c) in WHEELS.iter().zip(at) {
        wheel(app, ui, d, key, c, rad);
    }
    for ((key, ..), label) in WHEELS.iter().zip(&labels) {
        control_as(app, ui, d, &format!("primaries.{key}.lum"), true, Some(label));
    }
    // the Log variant's range boundaries (they move where the Shadow and Highlight wheels stop)
    if d.primaries.log {
        control_as(app, ui, d, "primaries.low", true, Some("Low Range"));
        control_as(app, ui, d, "primaries.high", true, Some("High Range"));
    }
    ui.add_space(8.0);
}

/// One colourist wheel (`key` of [`WHEELS`]) centred on `c`: drag to pick hue and saturation,
/// double-click to reset the puck — the same holds and readouts as the Color Grading wheels.
fn wheel(app: &mut LccApp, ui: &mut egui::Ui, d: &DevelopSettings, key: &str, c: Pos2, rad: f32) {
    let t = Tokens::get(ui.ctx());
    let wh = match key {
        "lift" => d.primaries.lift,
        "gamma" => d.primaries.gamma,
        "gain" => d.primaries.gain,
        _ => d.primaries.offset,
    };
    let label = WHEELS.iter().find(|(k, ..)| *k == key).map_or(key, |(_, l, _)| *l);
    let log_label = WHEELS.iter().find(|(k, ..)| *k == key).map_or(key, |(_, _, l)| *l);
    let r = Rect::from_center_size(c, vec2(rad * 2.0, rad * 2.0));
    let resp = ui.interact(r, egui::Id::new(("primariesWheel", key)), Sense::click_and_drag());
    crate::widgets::announce(&resp, format!("{} {}", crate::i18n::tr(label), crate::i18n::tr("Color wheel")));
    let resp = resp.on_hover_text(crate::i18n::tr(
        "Drag to set hue and saturation. Hold Shift to keep the hue, Cmd/Ctrl to keep the saturation, Alt/Option to fine-adjust. Double-click to reset.",
    ));
    register(ui.ctx(), format!("wheel:primaries-{key}"), r);
    paint_wheel(ui.painter(), c, rad);
    let a = (wh.hue as f32).to_radians();
    let dot = c + vec2(a.cos(), -a.sin()) * rad * (wh.sat as f32 / 100.0);
    // the puck's modifiers and mid-drag re-grab, as the Color Grading wheels
    let grab_id = egui::Id::new(("primariesWheelGrab", key));
    let mods = ui.input(|i| WheelMods::from_modifiers(i.modifiers));
    let pointer = wheel_pointer(ui, &resp);
    let grab = match pointer {
        Some(q) if resp.dragged() || resp.clicked() => {
            let held = ui.data(|m| m.get_temp::<WheelGrab>(grab_id)).filter(|g| g.mods == mods && !resp.drag_started());
            Some(held.unwrap_or_else(|| {
                let origin = if resp.drag_started() { ui.input(|i| i.pointer.press_origin()).unwrap_or(q) } else { q };
                let g = WheelGrab::new(mods, wh.hue as f32, wh.sat as f32, origin - c, rad);
                ui.data_mut(|m| m.insert_temp(grab_id, g));
                g
            }))
        }
        _ => {
            ui.data_mut(|m| m.remove::<WheelGrab>(grab_id));
            None
        }
    };
    let guide = Stroke::new(1.5, Color32::from_white_alpha(170));
    match grab.as_ref().and_then(|g| g.lock()) {
        Some(WheelLock::Hue(h)) => {
            let a = h.to_radians();
            ui.painter().line_segment([c, c + vec2(a.cos(), -a.sin()) * rad], guide);
        }
        Some(WheelLock::Sat(s)) => {
            ui.painter().circle_stroke(c, rad * s / 100.0, guide);
        }
        None => {}
    }
    ui.painter().circle_filled(c, 2.0, Color32::WHITE);
    ui.painter().circle_filled(dot, 5.0, Color32::WHITE);
    ui.painter().circle_stroke(dot, 5.0, Stroke::new(1.0, Color32::BLACK));
    ui.painter().text(
        pos2(c.x, c.y + rad + 12.0),
        Align2::CENTER_CENTER,
        crate::i18n::tr(if d.primaries.log { log_label } else { label }),
        t.font(12.0),
        t.text_label,
    );
    if resp.drag_started() {
        let _ = app.run("develop.beginInteraction", json!({"label": crate::i18n::tr(if d.primaries.log { log_label } else { label })}));
    }
    if (resp.dragged() || resp.clicked())
        && let Some(q) = pointer
    {
        let (hue, sat) = grab.map_or_else(|| wheel_pick(q - c, rad, None), |g| g.pick(q - c, rad));
        let _ = app.run("develop.set", json!({"values": {format!("primaries.{key}.hue"): hue.round(), format!("primaries.{key}.sat"): sat.round()}}));
    }
    if resp.drag_stopped() || resp.clicked() {
        let _ = app.run("develop.endInteraction", json!({}));
    }
    if resp.drag_stopped()
        && let Some(held) = ui.ctx().data_mut(|m| m.remove_temp::<WheelCapture>(egui::Id::new(WHEEL_CAPTURE)))
        && !held.dropped
    {
        give_pointer_back(ui.ctx(), held.origin);
    }
    if resp.double_clicked() {
        let _ = app.run("develop.set", json!({"values": {format!("primaries.{key}.hue"): 0, format!("primaries.{key}.sat"): 0}}));
    }
}
