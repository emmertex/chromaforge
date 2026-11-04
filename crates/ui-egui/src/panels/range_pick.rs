//! The Colour Range and Luminance Range pickers on the photo (the Masking panel's Color / Luminance
//! tools), as Lightroom Classic has them:
//!
//! - a click samples the colour (or tone) there and replaces what was picked; ⇧-click adds to it
//!   (a colour range keeps up to five colours); ⌥-click removes the nearest colour pin;
//! - a drag samples an area: up to three of its main colours (`mask.sampleColor {rect}`), or its
//!   tones (`mask.sampleLuminance {rect}`);
//! - each picked place of a colour range has a pin (a dashed box for an area): drag it to sample
//!   somewhere else, ⌥-click it to remove it.

use egui::{Color32, Pos2, Rect, Stroke, StrokeKind, vec2};
use lcc_develop::{DevelopSettings, MaskShape};
use lcc_geom::Point;
use serde_json::json;

use super::detail::{CanvasMap, Gesture};
use crate::LccApp;
use crate::widgets::register;

/// How close (points) the pointer must be to a pin to grab it.
const PIN_REACH: f32 = 9.0;

/// One pin on the photo: which picked place (`mask.colorPin`'s index), where (normalized area),
/// its first colour.
struct Pin {
    index: usize,
    area: [f64; 4],
    color: Option<[f64; 3]>,
}

/// The selected mask's colour range: its component and pins.
fn pins(app: &LccApp, d: &DevelopSettings) -> Option<(usize, Vec<Pin>)> {
    let m = d.masks.iter().find(|m| Some(m.id) == app.session.active_mask)?;
    let (k, c) = m.components.iter().enumerate().find(|(_, c)| matches!(c.shape, MaskShape::ColorRange { .. }))?;
    let MaskShape::ColorRange { samples, areas, .. } = &c.shape else { return None };
    let list = lcc_develop::color_range_pins(areas)
        .into_iter()
        .enumerate()
        .map(|(index, (area, range))| Pin { index, area, color: samples.get(range.start).copied() })
        .collect();
    Some((k, list))
}

/// A sample's colour on screen (OkLab → sRGB).
pub(crate) fn oklab_color(lab: [f64; 3]) -> Color32 {
    let lin = lcc_color::perceptual::oklab_to_2020([lab[0] as f32, lab[1] as f32, lab[2] as f32]);
    let s = lcc_color::REC2020.to_space(&lcc_color::SRGB).apply_f32(lin);
    let e = s.map(|v| (lcc_color::transfer::linear_to_srgb(if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 }) * 255.0).round() as u8);
    Color32::from_rgb(e[0], e[1], e[2])
}

fn centre(area: [f64; 4]) -> Point {
    Point::new((area[0] + area[2]) / 2.0, (area[1] + area[3]) / 2.0)
}

/// Draw a pin: a dashed box for an area, then a dot in the sampled colour with a white ring.
fn draw_pin(p: &egui::Painter, map: &CanvasMap, pin: &Pin, at: Pos2, hot: bool) {
    if pin.area[0] != pin.area[2] || pin.area[1] != pin.area[3] {
        let (a, b) = (map.screen(Point::new(pin.area[0], pin.area[1])), map.screen(Point::new(pin.area[2], pin.area[3])));
        let r = Rect::from_two_pos(a, b).translate(at - map.screen(centre(pin.area)));
        let pts = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
        p.extend(egui::Shape::dashed_line(&pts, Stroke::new(1.0, Color32::from_white_alpha(190)), 4.0, 3.0));
    }
    let fill = pin.color.map_or(Color32::GRAY, oklab_color);
    p.circle_filled(at, 5.5, fill);
    p.circle_stroke(at, 5.5, Stroke::new(if hot { 2.0 } else { 1.5 }, Color32::WHITE));
    p.circle_stroke(at, 7.0, Stroke::new(1.0, Color32::from_black_alpha(120)));
}

/// The rubber band of an area being dragged.
fn draw_band(p: &egui::Painter, a: Pos2, b: Pos2) {
    let r = Rect::from_two_pos(a, b);
    p.rect_filled(r, 0.0, Color32::from_white_alpha(18));
    p.rect_stroke(r, 0.0, Stroke::new(1.0, Color32::WHITE), StrokeKind::Middle);
}

/// Is a press at `a` and release at `b` (screen) a drag over an area, not a click?
fn is_area(a: Pos2, b: Pos2) -> bool {
    (a.x - b.x).abs() >= 4.0 && (a.y - b.y).abs() >= 4.0
}

/// The Colour Range picker on the photo (the selected mask's colour range).
pub(crate) fn color_tool(app: &mut LccApp, ui: &egui::Ui, resp: &egui::Response, map: &CanvasMap, d: &DevelopSettings, p: &egui::Painter) {
    let (comp, list) = pins(app, d).unwrap_or((0, Vec::new()));
    let (alt, shift) = ui.input(|i| (i.modifiers.alt, i.modifiers.shift));
    let hover = resp.hover_pos();
    let near = |q: Pos2| {
        list.iter()
            .filter(|pin| map.screen(centre(pin.area)).distance(q) < PIN_REACH)
            .min_by(|a, b| map.screen(centre(a.area)).distance(q).total_cmp(&map.screen(centre(b.area)).distance(q)))
    };
    // the pins (the one dragged follows the pointer)
    let dragging = match &app.gesture {
        Some(Gesture::ColorPin { index, .. }) => Some(*index),
        _ => None,
    };
    let pointer = ui.input(|i| i.pointer.latest_pos());
    for pin in &list {
        let at = match (dragging, pointer) {
            (Some(i), Some(q)) if i == pin.index => q,
            _ => map.screen(centre(pin.area)),
        };
        let hot = hover.is_some_and(|h| near(h).is_some_and(|n| n.index == pin.index));
        draw_pin(p, map, pin, at, hot);
        register(ui.ctx(), format!("colorPin:{}", pin.index), Rect::from_center_size(at, vec2(14.0, 14.0)));
    }
    if let Some(h) = hover {
        let cursor = match near(h) {
            Some(_) if alt => egui::CursorIcon::NotAllowed,
            Some(_) => egui::CursorIcon::Move,
            None => egui::CursorIcon::Crosshair,
        };
        ui.ctx().set_cursor_icon(cursor);
    }
    let press = ui.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos());
    if resp.drag_started()
        && let Some(q) = press
    {
        app.gesture = Some(match near(q).filter(|_| !alt) {
            Some(pin) => Gesture::ColorPin { comp, index: pin.index },
            None => Gesture::RangeArea { a: map.norm(q) },
        });
    }
    if let (Some(Gesture::RangeArea { a }), Some(q)) = (&app.gesture, resp.interact_pointer_pos().or(pointer)) {
        draw_band(p, map.screen(*a), q);
    }
    let released = resp.drag_stopped();
    if released {
        let q = resp.interact_pointer_pos().or(pointer);
        match (app.gesture.take(), q) {
            (Some(Gesture::ColorPin { comp, index }), Some(q)) => {
                let n = map.norm(q);
                run(app, ui, "mask.colorPin", json!({"component": comp, "index": index, "x": n.x, "y": n.y}));
            }
            (Some(Gesture::RangeArea { a }), Some(q)) => {
                let b = map.norm(q);
                if is_area(map.screen(a), q) {
                    run(app, ui, "mask.sampleColor", json!({"component": comp, "rect": [a.x, a.y, b.x, b.y], "add": shift}));
                } else {
                    run(app, ui, "mask.sampleColor", json!({"component": comp, "x": b.x, "y": b.y, "add": shift}));
                }
            }
            (g, _) => app.gesture = g,
        }
        return;
    }
    if resp.clicked()
        && let Some(q) = resp.interact_pointer_pos()
    {
        if alt {
            // ⌥-click: the pin there, or else the nearest one, goes
            let pin = near(q)
                .or_else(|| list.iter().min_by(|a, b| map.screen(centre(a.area)).distance(q).total_cmp(&map.screen(centre(b.area)).distance(q))));
            if let Some(pin) = pin {
                run(app, ui, "mask.colorPin", json!({"component": comp, "index": pin.index, "remove": true}));
            }
            return;
        }
        let n = map.norm(q);
        run(app, ui, "mask.sampleColor", json!({"component": comp, "x": n.x, "y": n.y, "add": shift}));
    }
}

/// The Luminance Range picker on the photo: a click selects the tones round the one there, a drag
/// the tones of the area; ⇧ widens the range to take them in.
pub(crate) fn luminance_tool(app: &mut LccApp, ui: &egui::Ui, resp: &egui::Response, map: &CanvasMap, p: &egui::Painter) {
    let shift = ui.input(|i| i.modifiers.shift);
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
    }
    let press = ui.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos());
    if resp.drag_started()
        && let Some(q) = press
    {
        app.gesture = Some(Gesture::RangeArea { a: map.norm(q) });
    }
    let pointer = resp.interact_pointer_pos().or(ui.input(|i| i.pointer.latest_pos()));
    if let (Some(Gesture::RangeArea { a }), Some(q)) = (&app.gesture, pointer) {
        draw_band(p, map.screen(*a), q);
    }
    if resp.drag_stopped() {
        if let (Some(Gesture::RangeArea { a }), Some(q)) = (app.gesture.take(), pointer) {
            let b = map.norm(q);
            let p = if is_area(map.screen(a), q) {
                json!({"rect": [a.x, a.y, b.x, b.y], "add": shift})
            } else {
                json!({"x": b.x, "y": b.y, "add": shift})
            };
            run(app, ui, "mask.sampleLuminance", p);
        }
        return;
    }
    if resp.clicked()
        && let Some(q) = resp.interact_pointer_pos()
    {
        let n = map.norm(q);
        run(app, ui, "mask.sampleLuminance", json!({"x": n.x, "y": n.y, "add": shift}));
    }
}

fn run(app: &mut LccApp, ui: &egui::Ui, cmd: &str, p: serde_json::Value) {
    if let Err(e) = app.run(cmd, p) {
        app.toast(ui.ctx(), e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_drag_needs_some_width_and_height_to_be_an_area() {
        let a = egui::pos2(10.0, 10.0);
        assert!(!is_area(a, egui::pos2(12.0, 30.0)), "a vertical flick is a click");
        assert!(is_area(a, egui::pos2(30.0, 30.0)));
        assert!(is_area(a, egui::pos2(-10.0, -10.0)), "either direction");
    }

    #[test]
    fn sample_colours_show_in_srgb() {
        // OkLab white and black
        assert_eq!(oklab_color([1.0, 0.0, 0.0]), Color32::from_rgb(255, 255, 255));
        assert_eq!(oklab_color([0.0, 0.0, 0.0]), Color32::from_rgb(0, 0, 0));
        // hostile values never panic
        let _ = oklab_color([f64::NAN, f64::INFINITY, -1e300]);
    }
}
