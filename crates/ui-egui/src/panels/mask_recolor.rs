//! A mask's Recolor rows in the Masking panel: the target colour (swatch, hex field, preset
//! swatches and an eyedropper on the photo), Color to Replace (with its own eyedropper), the
//! Amount (0 = off) and the Range. The maths is `lcc_pipeline::recolor`; the commands are
//! `mask.recolor` and `mask.recolorPick`.

use egui::{Color32, Rect, Sense, Stroke, vec2};
use lcc_develop::{ControlSpec, Mask, Section, Track};
use serde_json::json;

use super::edit::apply_slider_out;
use crate::LccApp;
use crate::i18n::tr;
use crate::icons::Icon;
use crate::text_field::TextField;
use crate::theme::Tokens;
use crate::widgets::{flyout_row, icon_button, register, slider, text_button};

/// The flyout ids the rows live under (kept in [`crate::state::UiState::folded_flyouts`]).
const FLYOUT: &str = "maskRecolor";

/// A few starting points for the colour: purple, teal, amber, rose, and white.
const PRESETS: [[u8; 3]; 5] = [[122, 63, 176], [30, 140, 150], [220, 160, 40], [200, 70, 90], [240, 240, 240]];

fn spec(id: &'static str, label: &'static str, min: f64, max: f64) -> ControlSpec {
    ControlSpec {
        id,
        label,
        section: Section::Color,
        min,
        max,
        default: 0.0,
        step: 1.0,
        decimals: 0,
        track: Track::Plain,
        shade: lcc_develop::Shade::Plain,
    }
}

/// The mask's Recolor colour as an 8-bit sRGB triple (black = unset).
fn rgb8(m: &Mask) -> [u8; 3] {
    let a = &m.adjust;
    [a.recolor_r, a.recolor_g, a.recolor_b].map(|v| (if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 } * 255.0).round() as u8)
}

fn color32(rgb: [u8; 3]) -> Color32 {
    Color32::from_rgb(rgb[0], rgb[1], rgb[2])
}

/// A colour swatch of `size`, painted with a stroke when `sel`, filled dark with a diagonal line
/// when unset. Returns whether it was clicked.
fn swatch(ui: &mut egui::Ui, id: &str, rgb: Option<[u8; 3]>, size: f32) -> bool {
    let t = Tokens::get(ui.ctx());
    let (r, resp) = ui.allocate_exact_size(vec2(size, size), Sense::click());
    register(ui.ctx(), id.to_string(), r);
    match rgb {
        Some(c) => {
            ui.painter().rect_filled(r.shrink(2.0), 3.0, color32(c));
        }
        None => {
            ui.painter().rect_filled(r.shrink(2.0), 3.0, t.chrome);
            ui.painter().line_segment([r.left_bottom() + vec2(2.0, -2.0), r.right_top() - vec2(2.0, 2.0)], Stroke::new(1.0, t.text_dim));
        }
    }
    ui.painter().rect_stroke(r, 2.0, Stroke::new(1.0, t.text_dim), egui::StrokeKind::Inside);
    resp.clicked()
}

/// The rows under a mask's effect sliders: the Recolor flyout and, inside it, the colour rows.
pub(crate) fn section(app: &mut LccApp, ui: &mut egui::Ui, m: &Mask) {
    let t = Tokens::get(ui.ctx());
    let a = &m.adjust;
    let on = a.recolor.is_finite() && a.recolor > 0.0;
    let from = {
        let rgb = [a.recolor_from_r, a.recolor_from_g, a.recolor_from_b];
        rgb.iter().any(|v| v.is_finite() && *v > 0.0).then_some(rgb.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8))
    };
    let open = app.ui.flyout_open(FLYOUT);
    let row = flyout_row(ui, FLYOUT, "Recolor", Icon::Picker, open);
    let sw = Rect::from_center_size(pos2_of(row.rect.right() - 36.0, row.rect.center().y), vec2(16.0, 16.0));
    register(ui.ctx(), "maskRecolor:swatch", sw);
    ui.painter().rect_filled(sw.shrink(2.0), 2.0, color32(rgb8(m)));
    if !on {
        ui.painter().rect_stroke(sw, 2.0, Stroke::new(1.0, t.text_dim), egui::StrokeKind::Inside);
    }
    if row.clicked() {
        app.ui.toggle_flyout(FLYOUT);
    }
    if !open {
        return;
    }
    crate::widgets::slider_group(ui, FLYOUT, |ui| {
        // Amount (0 = off) and Range, through mask.recolor
        for (s, key) in [(spec("recolor", "Amount", 0.0, 100.0), "amount"), (spec("recolorRange", "Range", -100.0, 100.0), "range")] {
            let v = match key {
                "amount" => a.recolor,
                _ => a.recolor_range,
            };
            let out = slider(ui, &s, v, true, None);
            apply_slider_out(app, &s, out, |app, v| app.run("mask.recolor", json!({key: v, "id": m.id})));
        }
        // the colour: swatches, the hex field, the eyedropper
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 5.0;
            let active = app.ui.tool == "maskRecolor";
            if icon_button(ui, "maskRecolorPicker", Icon::Picker, vec2(26.0, 26.0), active, true, tr("Sample a colour on the photo")).clicked() {
                app.ui.tool = if active { String::new() } else { "maskRecolor".into() };
            }
            for c in PRESETS {
                let hex = format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2]);
                if swatch(ui, &format!("maskRecolorPreset:{hex}"), Some(c), 20.0) {
                    let _ = app.run("mask.recolor", json!({"id": m.id, "color": hex, "amount": if on { a.recolor } else { 100.0 }}));
                }
            }
        });
        ui.horizontal(|ui| {
            let cur = lcc_pipeline::recolor::to_hex([a.recolor_r, a.recolor_g, a.recolor_b]);
            let mut text = app.ui.mask_recolor_hex.clone();
            if !app.ui.mask_recolor_hex_edit {
                text = cur.clone();
            }
            let tf = TextField::singleline("maskRecolorHex", &mut text).hint("#rrggbb").width(86.0).show(ui);
            let was_editing = app.ui.mask_recolor_hex_edit;
            app.ui.mask_recolor_hex_edit = tf.editing;
            app.ui.mask_recolor_hex = text.clone();
            if tf.committed() || (was_editing && !tf.editing && tf.ending.is_some()) {
                if let Some(rgb) = lcc_pipeline::recolor::parse_hex(&text) {
                    let _ = app.run("mask.recolor", json!({"id": m.id, "color": lcc_pipeline::recolor::to_hex(rgb)}));
                } else if !text.trim().is_empty() {
                    app.ui.mask_recolor_hex = cur;
                }
            }
        });
        // Color to Replace: Automatic, or a colour with its own eyedropper
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(tr("Color to Replace")).size(12.0).color(t.text_dim));
            let active = app.ui.tool == "maskRecolorFrom";
            if icon_button(ui, "maskRecolorFromPicker", Icon::Picker, vec2(26.0, 26.0), active, true, tr("Sample the colour to replace on the photo"))
                .clicked()
            {
                app.ui.tool = if active { String::new() } else { "maskRecolorFrom".into() };
            }
            if swatch(ui, "maskRecolorFrom:swatch", from, 20.0) && from.is_some() {
                let _ = app.run("mask.recolor", json!({"id": m.id, "from": null}));
            }
            if from.is_some() && text_button(ui, "maskRecolorAuto", tr("Automatic"), false).clicked() {
                let _ = app.run("mask.recolor", json!({"id": m.id, "from": null}));
            }
        });
    });
}

use egui::pos2 as pos2_of;
