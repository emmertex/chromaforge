//! Lens Blur: the Edit section (Apply, the sliders, what stays in focus, Find Subject and Brush
//! Refinement), the Lens Blur block of a mask, and its tools on the photo (moving and resizing
//! the fixed focus shape, painting the Focus / Blur brushes).
//!
//! Adapted from upstream PR #2661 (`panels/edit/lens_blur.rs`,
//! `panels/detail/lens_blur.rs` and the Masking block), written in the fork's panel structure.

use egui::{Color32, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, pos2, vec2};
use lcc_develop::{ControlSpec, DevelopSettings, FocusShape, LocalAdjustments, Mask, Section, Track};
use lcc_geom::Point;
use serde_json::json;

use crate::json_ext::JsonExt as _;

use super::detail::{CanvasMap, Gesture};
use super::edit::{apply_slider_out, control, sub_title};
use crate::LccApp;
use crate::icons::{Icon, paint};
use crate::theme::Tokens;
use crate::widgets::{announce, register, row_margin, section_header, segmented, slider, text_button};

/// Whether `tool` is one of Lens Blur's brushes (Edit panel): `lensBlurFocus` or `lensBlurBlur`.
pub(crate) fn is_brush(tool: &str) -> bool {
    matches!(tool, "lensBlurFocus" | "lensBlurBlur")
}

/// Whether `tool` is Lens Blur's Move & Resize (the fixed focus shape on the photo).
pub(crate) fn is_shape_tool(tool: &str) -> bool {
    tool == "lensBlurShape"
}

/// The Edit panel's Lens Blur section: Apply, Blur Amount, Boost, Transition, and what stays in
/// focus (the photo's subject, or one of its masks, e.g. an Object mask that selected it).
pub(super) fn edit_section(app: &mut LccApp, ui: &mut egui::Ui, d: &DevelopSettings) {
    let t = Tokens::get(ui.ctx());
    let dense = t.dense;
    let b = &d.lens_blur;
    egui::Frame::NONE.inner_margin(row_margin(ui, egui::Margin { left: 12, right: 16, top: 4, bottom: 4 }, 3, 3)).show(ui, |ui| {
        let mut on = b.enabled;
        let r = ui.checkbox(&mut on, crate::i18n::tr("Apply"));
        register(ui.ctx(), "check:lensBlur.enabled", r.rect);
        if r.on_hover_text(crate::i18n::tr("Blur what is out of focus, as a wide aperture would")).changed() {
            // switched on the first time: a visible starting amount, as in Lightroom
            let patch = if on && b.amount == 0.0 { json!({"enabled": on, "amount": 50.0}) } else { json!({"enabled": on}) };
            let _ = app.run("develop.merge", json!({"settings": {"lens_blur": patch}, "label": "Lens Blur"}));
        }
    });
    control(app, ui, d, "lensBlur.amount", b.enabled);
    control(app, ui, d, "lensBlur.boost", b.enabled);
    control(app, ui, d, "lensBlur.transition", b.enabled);
    // the focus controls do nothing while the blur is off: shown, but not usable
    let has_mask = b.focus_mask.and_then(|id| d.masks.iter().find(|m| m.id == id)).is_some();
    if app.ui.tool == "lensBlurShape" && (!b.enabled || has_mask || b.shape == FocusShape::Auto) {
        app.ui.tool.clear();
    }
    egui::Frame::NONE.inner_margin(row_margin(ui, egui::Margin { left: 12, right: 16, top: 6, bottom: 4 }, 2, 2)).show(ui, |ui| {
        ui.add_enabled_ui(b.enabled, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(crate::i18n::tr("In Focus")).font(t.font(if dense { 11.0 } else { 13.0 })).color(t.text_dim));
                let chosen = b.focus_mask.and_then(|id| d.masks.iter().find(|m| m.id == id));
                // without a mask: the subject estimate, or the fixed shape chosen below
                let auto = b.shape == FocusShape::Auto;
                let none = if auto { crate::i18n::tr("Subject") } else { crate::i18n::tr(b.shape.label()) };
                let shown = chosen.map_or_else(|| none.to_string(), |m| m.name.clone());
                let r = crate::widgets::dropdown(ui, "lensBlurFocus", &shown, t.font(if dense { 12.5 } else { 14.0 }), t.text_label);
                let r = r.on_hover_text(crate::i18n::tr("What stays sharp: the photo's subject, or the selection of one of its masks"));
                let mut pick: Option<Option<u32>> = None;
                egui::Popup::menu(&r).show(|ui| {
                    let item = ui.selectable_label(chosen.is_none() && auto, crate::i18n::tr("Subject"));
                    register(ui.ctx(), "lensBlurFocus:subject", item.rect);
                    if item.clicked() {
                        pick = Some(None);
                    }
                    for m in &d.masks {
                        let item = ui.selectable_label(chosen.is_some_and(|c| c.id == m.id), m.name.as_str());
                        register(ui.ctx(), format!("lensBlurFocus:mask:{}", m.id), item.rect);
                        if item.clicked() {
                            pick = Some(Some(m.id));
                        }
                    }
                });
                if let Some(focus) = pick {
                    // "Subject": the estimate again, not a fixed shape
                    let patch = match focus {
                        Some(id) => json!({"focus_mask": id}),
                        None => json!({"focus_mask": null, "shape": "auto"}),
                    };
                    let _ = app.run("develop.merge", json!({"settings": {"lens_blur": patch}, "label": "Lens Blur Focus"}));
                }
            });
            // without a focus mask: the focus area's shape (the estimate, or a fixed one), which can
            // be moved and resized on the photo
            if !has_mask {
                ui.add_space(4.0);
                focus_shape_row(app, ui, b);
                if b.shape != FocusShape::Auto {
                    let active = is_shape_tool(&app.ui.tool);
                    let r = text_button(ui, "lensBlurEditShape", crate::i18n::tr("Move & Resize"), active).on_hover_text(crate::i18n::tr(
                        "Drag the shape on the photo to move it; drag its side or bottom handle to stretch it. A shape button puts it back in the middle.",
                    ));
                    if r.clicked() {
                        if active { app.ui.tool.clear() } else { app.ui.tool = "lensBlurShape".into() }
                    }
                }
            }
            // the AI subject search runs only when asked: it takes a while on the CPU, and the
            // automatic estimate stays until then
            ui.add_space(4.0);
            let r = text_button(ui, "lensBlurFindSubject", crate::i18n::tr("Find Subject"), false).on_hover_text(crate::i18n::tr(
                "Select the subject with AI (SAM 3), starting where the camera focused when the file records it, and keep it in focus",
            ));
            if r.clicked() {
                match app.run_typed("lensBlur.findSubject", json!({})) {
                    Ok(v) => {
                        let msg = if v.field("from") == &json!("camera") { "Selecting the subject at the camera's focus point…" } else { "Selecting the subject…" };
                        app.toast(ui.ctx(), crate::i18n::tr(msg));
                    }
                    Err(e) => app.ai_error(ui.ctx(), e, None),
                }
            }
            // a Find Subject still running: say so (meanwhile the automatic estimate stays in focus)
            let waiting = b.focus_mask.and_then(|id| d.masks.iter().find(|m| m.id == id)).is_some_and(lcc_pipeline::lens_blur::awaiting_ai);
            if waiting && app.session.segmenter.busy() {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(RichText::new(crate::i18n::tr("Finding the subject… The first time on a photo takes a while.")).size(11.0).color(t.text));
                });
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
            } else if waiting {
                ui.label(
                    RichText::new(crate::i18n::tr("The subject wasn't selected. Try Find Subject again, or click it in Masking (M)."))
                        .size(11.0)
                        .color(t.text),
                );
            }
            ui.label(
                RichText::new(crate::i18n::tr("Refine it in Masking (M): + → Object adds what was missed, − → Object leaves a part out."))
                    .size(11.0)
                    .color(t.text_dim),
            );
        });
    });
    brush_refinement(app, ui, b);
}

/// The Lens Blur focus area's shape when no mask sets it: one button per shape, each showing it
/// (drawn in the theme's colours, so it reads on the light themes too).
fn focus_shape_row(app: &mut LccApp, ui: &mut egui::Ui, b: &lcc_develop::LensBlur) {
    let t = Tokens::get(ui.ctx());
    let enabled = ui.is_enabled();
    ui.horizontal(|ui| {
        ui.label(RichText::new(crate::i18n::tr("Shape")).font(t.font(13.0)).color(t.text_dim));
        ui.add_space(4.0);
        for shape in FocusShape::ALL {
            let selected = b.shape == shape;
            let (r, resp) = ui.allocate_exact_size(vec2(36.0, 28.0), Sense::click());
            let name = serde_json::to_value(shape).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
            register(ui.ctx(), format!("lensBlurShape:{name}"), r);
            announce(&resp, crate::i18n::tr(shape.label()));
            let resp = resp.on_hover_text(crate::i18n::tr(match shape {
                FocusShape::Auto => "Auto: the photo's subject, estimated",
                FocusShape::Round => "Round",
                FocusShape::Wide => "Horizontal oval",
                FocusShape::Tall => "Vertical oval",
            }));
            let p = ui.painter();
            let fill = if selected {
                t.pressed
            } else if resp.hovered() && enabled {
                t.hover
            } else {
                t.button
            };
            p.rect_filled(r, t.radius(4.0), fill);
            if selected {
                p.rect_stroke(r, t.radius(4.0), Stroke::new(1.0, if enabled { t.accent } else { t.text_dim }), StrokeKind::Inside);
            }
            // (dimmed while the blur is off, in the theme's colours)
            let colour = match (enabled, selected) {
                (false, _) => t.icon.gamma_multiply(0.4),
                (true, true) => t.accent,
                (true, false) => t.icon,
            };
            paint_focus_shape(p, r.shrink(5.0), shape, colour);
            // a preset puts the shape back in the middle at its size (also the one in use, once moved)
            if resp.clicked() && (!selected || b.area.is_some()) {
                let _ = app.run("develop.merge", json!({"settings": {"lens_blur": {"shape": name, "area": null}}, "label": "Lens Blur Shape"}));
            }
        }
    });
}

/// A focus shape's picture in `r`: the oval in its proportions (the largest fills the height), or
/// the subject icon for Auto.
fn paint_focus_shape(p: &egui::Painter, r: Rect, shape: FocusShape, colour: Color32) {
    let Some((rx, ry)) = shape.radii() else {
        paint(p, r, Icon::Subject, colour);
        return;
    };
    // the shapes' largest radius spans the box's height (all shapes on one scale)
    let k = r.height() / 2.0 / 0.32;
    let (ax, ay) = (rx as f32 * k, ry as f32 * k);
    let c = r.center();
    let pts: Vec<Pos2> = (0..48)
        .map(|i| {
            let a = i as f32 / 48.0 * std::f32::consts::TAU;
            pos2(c.x + ax * a.cos(), c.y + ay * a.sin())
        })
        .collect();
    p.add(egui::Shape::convex_polygon(pts.clone(), colour.gamma_multiply(0.3), Stroke::NONE));
    p.add(egui::Shape::closed_line(pts, Stroke::new(1.5, colour)));
}

/// Lens Blur Brush Refinement: the Focus and Blur brushes (painted on the photo, see
/// [`brush_overlay`]), their Amount, the Masking brush's Size, Feather and Flow, its Auto Mask and
/// range, and Reset.
fn brush_refinement(app: &mut LccApp, ui: &mut egui::Ui, b: &lcc_develop::LensBlur) {
    let on = b.enabled;
    sub_title(ui, crate::i18n::tr("Brush Refinement"));
    egui::Frame::NONE.inner_margin(row_margin(ui, egui::Margin { left: 12, right: 16, top: 0, bottom: 4 }, 0, 2)).show(ui, |ui| {
        ui.add_enabled_ui(on, |ui| {
            ui.horizontal_wrapped(|ui| {
                for (tool, label, tip) in
                    [("lensBlurFocus", "Focus", "Paint what should stay sharp"), ("lensBlurBlur", "Blur", "Paint what should be blurred")]
                {
                    let active = app.ui.tool == tool;
                    let r = text_button(ui, tool, crate::i18n::tr(label), active).on_hover_text(crate::i18n::tr(tip));
                    if r.clicked() {
                        if active { app.ui.tool.clear() } else { app.ui.tool = tool.to_string() }
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let painted = !b.focus_strokes.is_empty() || !b.blur_strokes.is_empty();
                    if text_button(ui, "lensBlurBrushReset", crate::i18n::tr("Reset"), false)
                        .on_hover_text(crate::i18n::tr("Remove the strokes of both brushes"))
                        .clicked()
                        && painted
                    {
                        let _ = app.run("lensBlur.brushReset", json!({}));
                    }
                });
            });
            let mut auto = app.ui.brush_auto_mask;
            if ui
                .checkbox(&mut auto, crate::i18n::tr("Auto Mask"))
                .on_hover_text(crate::i18n::tr("Paint only areas like the one under the brush"))
                .changed()
            {
                app.ui.brush_auto_mask = auto;
            }
        });
    });
    // the brush is the Masking brush's (its size also follows [ and ]), with its own Amount
    // (a reset puts the brush's Amount back to full strength, the others to their minimum)
    let set = app.ui.brush_set(false);
    for (id, label, min, max, get) in [
        ("ui.lensBlurBrushAmount", "Amount", 1.0, 100.0, f64::from(app.ui.lens_blur_brush_amount)),
        ("ui.brushSize", "Size", 1.0, 100.0, f64::from(set.size)),
        ("ui.brushFeather", "Feather", 0.0, 100.0, f64::from(set.feather)),
        ("ui.brushFlow", "Flow", 1.0, 100.0, f64::from(set.flow)),
    ] {
        let default = if id == "ui.lensBlurBrushAmount" { 100.0 } else { min };
        let s = ControlSpec {
            id,
            label,
            section: Section::LensBlur,
            min,
            max,
            default,
            step: 1.0,
            decimals: 0,
            track: Track::Plain,
            shade: lcc_develop::Shade::Plain,
        };
        let out = slider(ui, &s, get, on, None);
        if let Some(v) = out.value {
            let v = v as f32;
            match id {
                "ui.lensBlurBrushAmount" => app.ui.lens_blur_brush_amount = v,
                _ => {
                    let s = app.ui.brush_set(false);
                    let s = match id {
                        "ui.brushSize" => crate::state::BrushPreset { size: v, ..s },
                        "ui.brushFeather" => crate::state::BrushPreset { feather: v, ..s },
                        _ => crate::state::BrushPreset { flow: v, ..s },
                    };
                    app.ui.set_brush_set(false, s);
                }
            }
        }
    }
    // the brushes paint only while the blur is on
    if !on && is_brush(&app.ui.tool) {
        app.ui.tool.clear();
    }
}

/// The sliders of a mask's Lens Blur block (key = `LocalAdjustments` field name).
const MASK_LENS_BLUR: &[ControlSpec] = &[
    spec("lens_blur", "Amount", 0.0, 100.0, 1.0, 0, Track::Plain),
    spec("lens_blur_boost", "Boost", 0.0, 100.0, 1.0, 0, Track::Plain),
    spec("lens_blur_transition", "Transition", 0.0, 100.0, 1.0, 0, Track::Plain),
];

const fn spec(id: &'static str, label: &'static str, min: f64, max: f64, step: f64, decimals: u8, track: Track) -> ControlSpec {
    ControlSpec { id, label, section: Section::LensBlur, min, max, default: 0.0, step, decimals, track, shade: lcc_develop::Shade::Plain }
}

/// The value of a mask adjustment slider (key = `LocalAdjustments` field name); 0 when unset.
fn local_get(a: &LocalAdjustments, key: &str) -> f64 {
    serde_json::to_value(a).ok().and_then(|v| v.get(key).and_then(|x| x.as_f64())).unwrap_or(0.0)
}

/// The mask's Lens Blur block (`mask:lensBlur`, under the adjustment sections): a header that
/// folds it and whose eye switches it off (the values kept), which side of the mask it blurs
/// (`segmented:maskLensBlurSide`), then its Amount, Boost and Transition (`mask.adjust`).
pub(super) fn mask_block(app: &mut LccApp, ui: &mut egui::Ui, m: &Mask) {
    let a = &m.adjust;
    let id = "mask:lensBlur";
    let open = !app.ui.section_folded(id);
    let (resp, toggled) = section_header(ui, id, "Lens Blur", open, Some(!a.lens_blur_off), None);
    if let Some(on) = toggled {
        // switched on with nothing to blur: a visible amount, as the Edit section's Apply does
        let v = if on && a.lens_blur <= 0.0 { json!({"lens_blur_off": !on, "lens_blur": 50.0}) } else { json!({"lens_blur_off": !on}) };
        let _ = app.run("mask.adjust", json!({"values": v}));
    } else if resp.clicked() {
        app.ui.toggle_folded(id);
    }
    if !open {
        return;
    }
    let t = Tokens::get(ui.ctx());
    ui.add_enabled_ui(!a.lens_blur_off, |ui| {
        egui::Frame::NONE.inner_margin(row_margin(ui, egui::Margin { left: 12, right: 16, top: 0, bottom: 6 }, 0, 2)).show(ui, |ui| {
            ui.label(RichText::new(crate::i18n::tr("Blur")).color(t.text_dim).size(11.5));
            let items = [("Selection", "inside"), ("Outside", "outside")];
            let side = usize::from(a.lens_blur_outside);
            if let Some(i) = segmented(ui, "maskLensBlurSide", &items, Some(side), 2)
                && i != side
            {
                let outside = i == 1;
                let v =
                    if a.lens_blur <= 0.0 { json!({"lens_blur_outside": outside, "lens_blur": 50.0}) } else { json!({"lens_blur_outside": outside}) };
                let _ = app.run("mask.adjust", json!({"values": v}));
            }
        });
        for s in MASK_LENS_BLUR {
            let v = local_get(a, s.id);
            let out = slider(ui, s, v, true, None);
            apply_slider_out(app, s, out, |app, v| app.run("mask.adjust", json!({"values": {s.id: v}})));
        }
    });
}

// ---------------------------------------------------------------------------- on the photo

/// Lens Blur's fixed focus shape on the photo (Move & Resize): its outline, a centre pin that
/// moves it (or drag anywhere inside), and handles on its right and bottom edges that stretch it
/// sideways or up and down. One drag is one undo step.
pub(crate) fn shape_overlay(app: &mut LccApp, ui: &mut egui::Ui, resp: &egui::Response, map: &CanvasMap, d: &DevelopSettings) {
    let Some(area) = d.lens_blur.focus_area(d.crop_center()) else { return };
    let clip = app.canvas_rect.unwrap_or(map.rect);
    let p = &ui.painter_at(clip);
    let l = super::detail::frame_long_norm(map);
    let at = |dx: f64, dy: f64| map.screen(Point::new(area.center.x + dx * l.0, area.center.y + dy * l.1));
    let outline: Vec<Pos2> = (0..72)
        .map(|i| {
            let a = i as f64 / 72.0 * std::f64::consts::TAU;
            at(area.rx * a.cos(), area.ry * a.sin())
        })
        .collect();
    // a dark keyline under a light line: visible over bright and dark photos alike
    p.add(egui::Shape::closed_line(outline.clone(), Stroke::new(3.0, Color32::from_black_alpha(140))));
    p.add(egui::Shape::closed_line(outline, Stroke::new(1.5, Color32::from_white_alpha(235))));
    let (c, hx, hy) = (at(0.0, 0.0), at(area.rx, 0.0), at(0.0, area.ry));
    super::detail::pin(p, c, true);
    for h in [hx, hy] {
        p.circle_filled(h, 5.5, Color32::WHITE);
        p.circle_stroke(h, 5.5, Stroke::new(1.5, Color32::from_black_alpha(160)));
    }
    for (name, q) in [("center", c), ("width", hx), ("height", hy)] {
        register(ui.ctx(), format!("lensBlurArea:{name}"), Rect::from_center_size(q, vec2(14.0, 14.0)));
    }
    // which part a point grabs: a handle, else the inside
    let hit = |q: Pos2| -> Option<u8> {
        if q.distance(hx) < 10.0 {
            return Some(1);
        }
        if q.distance(hy) < 10.0 {
            return Some(2);
        }
        let n = map.norm(q);
        let (ex, ey) = ((n.x - area.center.x) / (area.rx * l.0).max(1e-9), (n.y - area.center.y) / (area.ry * l.1).max(1e-9));
        (q.distance(c) < 10.0 || (ex.is_finite() && ey.is_finite() && ex * ex + ey * ey <= 1.0)).then_some(0)
    };
    if let Some(q) = resp.hover_pos() {
        let icon = match hit(q) {
            Some(1) => Some(egui::CursorIcon::ResizeHorizontal),
            Some(2) => Some(egui::CursorIcon::ResizeVertical),
            Some(_) => Some(egui::CursorIcon::Move),
            None => None,
        };
        if let Some(icon) = icon {
            ui.ctx().set_cursor_icon(icon);
        }
    }
    if resp.drag_started()
        && let Some(q) = ui.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos())
        && let Some(handle) = hit(q)
    {
        let _ = app.run("develop.beginInteraction", json!({"label": "Lens Blur Shape"}));
        app.gesture = Some(Gesture::LensBlurArea { handle, original: area });
    }
    if resp.dragged()
        && let (Some(Gesture::LensBlurArea { handle, original }), Some(q)) = (app.gesture.clone(), resp.interact_pointer_pos())
    {
        let n = map.norm(q);
        let n0 = map.norm(ui.input(|i| i.pointer.press_origin()).unwrap_or(q));
        let moved = match handle {
            0 => lcc_develop::FocusArea { center: Point::new(original.center.x + n.x - n0.x, original.center.y + n.y - n0.y), ..original },
            1 => lcc_develop::FocusArea { rx: (n.x - original.center.x).abs() / l.0.max(1e-9), ..original },
            _ => lcc_develop::FocusArea { ry: (n.y - original.center.y).abs() / l.1.max(1e-9), ..original },
        }
        .sanitized();
        let _ = app.run("develop.merge", json!({"settings": {"lens_blur": {"area": moved}}, "label": "Lens Blur Shape"}));
    }
    if resp.drag_stopped() && matches!(app.gesture, Some(Gesture::LensBlurArea { .. })) {
        app.gesture = None;
        let _ = app.run("develop.endInteraction", json!({}));
    }
}

/// Lens Blur Brush Refinement on the photo: the brush circle (size and feather), the stroke being
/// painted (green: Focus, red: Blur), committed as `lensBlur.brushStroke` when the drag ends.
/// ⌥ (Alt) erases what the brush painted.
pub(crate) fn brush_overlay(app: &mut LccApp, ui: &mut egui::Ui, resp: &egui::Response, map: &CanvasMap) {
    let long = f64::from(map.rect.width().max(map.rect.height()));
    let clip = app.canvas_rect.unwrap_or(map.rect);
    let p = &ui.painter_at(clip);
    let focus = app.ui.tool == "lensBlurFocus";
    // the Masking brush's Size and Feather: the solid core, and where its halo ends
    let set = app.ui.brush_set(false);
    let r0 = crate::state::brush_core_points(set.size);
    let r = r0 * lcc_develop::brush_model::outer_ratio(f64::from(set.feather)) as f32;
    if let Some(h) = resp.hover_pos() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::None);
        p.circle_stroke(h, r0, Stroke::new(2.0, Color32::WHITE));
        if r > r0 + 0.5 {
            p.circle_stroke(h, r, Stroke::new(1.0, Color32::from_white_alpha(150)));
        }
    }
    if let Some(q) = resp.interact_pointer_pos()
        && (resp.drag_started() || resp.dragged() || resp.clicked())
    {
        let n = map.norm(q);
        let force = if app.ui.brush_pressure { ui.input(super::detail::pen_force) } else { None };
        match &mut app.gesture {
            // a held, unmoving pointer records nothing
            Some(Gesture::Brush { points, .. }) if points.last() == Some(&n) => {}
            Some(Gesture::Brush { points, pressure, .. }) => {
                super::detail::record_pressure(pressure, points.len(), force);
                points.push(n);
            }
            // a stroke starts with a press, not part-way through a drag
            _ if !(resp.drag_started() || resp.clicked()) => {}
            _ => {
                let mut pressure = Vec::new();
                super::detail::record_pressure(&mut pressure, 0, force);
                app.gesture = Some(Gesture::Brush { points: vec![n], pressure, erase: vec![], sent: 0 });
            }
        }
    }
    if let Some(Gesture::Brush { points, pressure, sent: 0, .. }) = &app.gesture {
        let screen: Vec<Pos2> = points.iter().map(|q| map.screen(*q)).collect();
        let params = crate::brush_live::LiveParams {
            clip,
            scale: ui.ctx().pixels_per_point(),
            r,
            hard: 0.0,
            lrc: Some({
                let (a, pw) = lcc_develop::brush_model::profile(f64::from(set.feather));
                (a as f32 * r0, pw as f32)
            }),
            flow: (set.flow / 100.0).clamp(0.0, 1.0),
            density: (app.ui.lens_blur_brush_amount / 100.0).clamp(0.0, 1.0),
            color: if focus { [70, 200, 120] } else { [230, 70, 70] },
            opacity: 0.45,
        };
        crate::brush_live::paint(&mut app.brush_live, ui.ctx(), p, params, &screen, pressure);
    }
    if (resp.drag_stopped() || resp.clicked())
        && let Some(Gesture::Brush { points, pressure, .. }) = app.gesture.take()
    {
        app.brush_live = None;
        let pts: Vec<[f64; 2]> = points.iter().map(|q| [q.x, q.y]).collect();
        let erase = resp.ctx.input(|i| i.modifiers.alt);
        let r = app.run(
            "lensBlur.brushStroke",
            json!({"mode": if focus { "focus" } else { "blur" }, "points": pts, "size": f64::from(r0) / long.max(1.0), "feather": set.feather,
                   "flow": set.flow, "amount": app.ui.lens_blur_brush_amount, "erase": erase, "autoMask": app.ui.brush_auto_mask,
                   "autoRange": app.ui.brush_auto_range, "model": 1, "pressure": pressure}),
        );
        if let Err(e) = r {
            app.toast(ui.ctx(), e);
        }
    }
}
