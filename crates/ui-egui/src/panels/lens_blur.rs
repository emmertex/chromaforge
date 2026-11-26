//! Lens Blur's Brush Refinement (the Focus and Blur brushes of the Edit panel's Lens Blur section
//! and their painting on the photo) and the Lens Blur block of a mask.
//!
//! Adapted from upstream PR #2661 (`panels/edit/lens_blur.rs`, `panels/detail/lens_blur.rs` and
//! the Masking block), written in the fork's panel structure.

use egui::{Color32, Pos2, RichText, Stroke};
use lcc_develop::{ControlSpec, LocalAdjustments, Mask, Section, Track};
use serde_json::json;

use super::detail::{CanvasMap, Gesture};
use super::edit::{apply_slider_out, sub_title};
use crate::LccApp;
use crate::theme::Tokens;
use crate::widgets::{row_margin, section_header, segmented, slider, text_button};

/// Whether `tool` is one of Lens Blur's brushes (Edit panel): `lensBlurFocus` or `lensBlurBlur`.
pub(crate) fn is_brush(tool: &str) -> bool {
    matches!(tool, "lensBlurFocus" | "lensBlurBlur")
}

/// Brush Refinement in the Lens Blur section: the Focus and Blur brushes (armed like a tool),
/// Reset, Auto Mask, and the brush's Amount, Size, Feather and Flow.
pub(super) fn brush_refinement(app: &mut LccApp, ui: &mut egui::Ui, b: &lcc_develop::LensBlur) {
    let on = b.enabled;
    sub_title(ui, crate::i18n::tr("Brush Refinement"));
    egui::Frame::NONE.inner_margin(row_margin(ui, egui::Margin { left: 12, right: 16, top: 0, bottom: 4 }, 0, 2)).show(ui, |ui| {
        ui.add_enabled_ui(on, |ui| {
            ui.horizontal_wrapped(|ui| {
                for (tool, label, tip) in
                    [("lensBlurFocus", "Focus", "Paint what should stay sharp"), ("lensBlurBlur", "Blur", "Paint what should be blurred")]
                {
                    let active = app.ui.tool == tool;
                    let r = text_button(ui, tool, label, active).on_hover_text(crate::i18n::tr(tip));
                    if r.clicked() {
                        if active { app.ui.tool.clear() } else { app.ui.tool = tool.to_string() }
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let painted = !b.focus_strokes.is_empty() || !b.blur_strokes.is_empty();
                    if text_button(ui, "lensBlurBrushReset", "Reset", false)
                        .on_hover_text(crate::i18n::tr("Remove the strokes of both brushes"))
                        .clicked()
                        && painted
                        && let Err(e) = app.run("lensBlur.brushReset", json!({}))
                    {
                        app.toast_error(ui.ctx(), e);
                    }
                });
            });
            let mut auto = app.ui.brush_auto_mask;
            if super::settings::check(ui, "lensBlurAutoMask", &mut auto, "Auto Mask") {
                app.ui.brush_auto_mask = auto;
            }
        });
    });
    // the brush is the Masking brush's (its size also follows [ and ]), with its own Amount
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
                "ui.lensBlurBrushAmount" => app.ui.lens_blur_brush_amount = v.clamp(1.0, 100.0),
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
const MASK_LENS_BLUR: &[ControlSpec] =
    &[spec("lens_blur", "Blur Amount", 0.0, 100.0, 1.0, 0, Track::Plain), spec("lens_blur_boost", "Boost", 0.0, 100.0, 1.0, 0, Track::Plain)];

const fn spec(id: &'static str, label: &'static str, min: f64, max: f64, step: f64, decimals: u8, track: Track) -> ControlSpec {
    ControlSpec { id, label, section: Section::LensBlur, min, max, default: 0.0, step, decimals, track, shade: lcc_develop::Shade::Plain }
}

/// The value of a mask's Lens Blur slider.
fn local_get(a: &LocalAdjustments, key: &str) -> f64 {
    if key == "lens_blur_boost" { a.lens_blur_boost } else { a.lens_blur }
}

/// The mask's Lens Blur block (`mask:lensBlur`, under the adjustment sections): a header that
/// folds it and whose eye switches it off (the values kept), which side of the mask it blurs
/// (`segmented:maskLensBlurSide`), then its Blur Amount and Boost (`mask.adjust`).
pub(super) fn mask_block(app: &mut LccApp, ui: &mut egui::Ui, m: &Mask) {
    let a = &m.adjust;
    let id = "mask:lensBlur";
    let open = !app.ui.section_folded(id);
    let (resp, toggled) = section_header(ui, id, "Lens Blur", open, Some(!a.lens_blur_off), None);
    if let Some(on) = toggled {
        // switched on with nothing to blur: a visible amount
        let v = if on && a.lens_blur <= 0.0 { json!({"lens_blur_off": !on, "lens_blur": 50.0}) } else { json!({"lens_blur_off": !on}) };
        if let Err(e) = app.run("mask.adjust", json!({"values": v})) {
            app.toast_error(ui.ctx(), e);
        }
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
                if let Err(e) = app.run("mask.adjust", json!({"values": v})) {
                    app.toast_error(ui.ctx(), e);
                }
            }
        });
        for s in MASK_LENS_BLUR {
            let v = local_get(a, s.id);
            let out = slider(ui, s, v, true, None);
            apply_slider_out(app, s, out, |app, v| app.run("mask.adjust", json!({"values": {s.id: v}})));
        }
    });
}

/// Lens Blur Brush Refinement on the photo: the brush circle (size and feather), the stroke being
/// painted (green: Focus, red: Blur), committed as `lensBlur.brushStroke` when the drag ends.
/// ⌥ (Alt) erases what the brush painted.
pub(crate) fn brush_overlay(app: &mut LccApp, ui: &mut egui::Ui, resp: &egui::Response, map: &CanvasMap) {
    let long = f64::from(map.per_long());
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
            app.toast_error(ui.ctx(), e);
        }
    }
}
