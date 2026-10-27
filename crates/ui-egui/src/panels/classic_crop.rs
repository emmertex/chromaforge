//! The crop tool laid out as Lightroom Classic's Crop & Straighten drawer (fork-owned): Aspect and
//! its lock, Angle with the straighten tool and Auto, Constrain to Image, and Reset / Close. The
//! rest of what upstream's Crop panel holds lives where Lightroom Classic keeps it: rotate and flip
//! in the Photo menu, the guide overlays in View ▸ Tool Overlay (cycled with ⇧O), and Upright with
//! the transform sliders in the Edit panel's Transform section ([`transform_body`]). A child module
//! of `right`, so it uses that file's helpers; upstream's `crop` stays as it is for merges.

use lcc_catalog::PhotoId;
use serde_json::json;

use super::{aspect_label, aspect_menu, cmd_icon, header, padded};
use crate::LccApp;
use crate::icons::Icon;
use crate::state::RightPanel;
use crate::theme::Tokens;
use crate::widgets::{icon_button, register, segmented_pill, slider, text_button};

pub(super) fn show(app: &mut LccApp, ui: &mut egui::Ui, id: PhotoId) {
    let d = app.session.develop_of(id).unwrap_or_default();
    header(ui, "Crop & Straighten");
    padded(ui, |ui| {
        let original = app.session.catalog.photo(id).map(|p| {
            let (w, h) = (p.width.max(1) as f64, p.height.max(1) as f64);
            if d.orientation.swaps_axes() { h / w } else { w / h }
        });
        ui.horizontal(|ui| {
            ui.label(crate::i18n::tr("Aspect"));
            let cur = aspect_label(app.crop_tool_aspect(), original, &app.session.crop_aspects.custom);
            let t = Tokens::get(ui.ctx());
            // a saved ratio's name is the user's own words: not looked up for a translation
            let saved = app.session.crop_aspects.custom.iter().any(|a| a.name == cur);
            let text = if saved { cur.as_str() } else { crate::i18n::tr(&cur) };
            let r = crate::widgets::dropdown(ui, "cropAspect", text, t.font(12.5), t.text);
            if r.clicked() {
                ui.data_mut(|d| d.insert_temp(egui::Id::new(super::ASPECT_EDITING), false));
            }
            egui::Popup::menu(&r).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| aspect_menu(app, ui));
            let locked = app.crop_tool_aspect().is_some();
            let (icon, label) = if locked { (Icon::Link, "Aspect Locked") } else { (Icon::Unlink, "Lock Aspect") };
            if cmd_icon(app, ui, "cropLock", icon, label, None, locked).clicked() {
                let _ = app.run("crop.aspect", json!({"aspect": "toggle"}));
            }
        });
    });
    // Angle: the slider (its value can be typed), then the straighten tool and Auto under it
    if let Some(spec) = lcc_develop::controls::find("crop.angle").copied() {
        let out = slider(ui, &spec, d.crop.geometry.angle, true, Some("Angle"));
        super::super::edit::apply_slider_out(app, &spec, out, |app, v| app.run("crop.straighten", json!({"angle": v})));
    }
    padded(ui, |ui| {
        ui.horizontal(|ui| {
            let on = app.ui.tool == "straighten";
            if cmd_icon(app, ui, "straightenTool", Icon::Straighten, "Straighten Tool", None, on)
                .on_hover_text(crate::i18n::tr("Drag along the horizon; double-click for Auto"))
                .clicked()
            {
                app.ui.tool = if on { String::new() } else { "straighten".into() };
            }
            if text_button(ui, "straightenAuto", crate::i18n::tr("Auto"), false).clicked() {
                app.auto_straighten(ui.ctx());
            }
        });
        ui.add_space(4.0);
        let mut c = d.geometry.constrain_crop;
        if ui.checkbox(&mut c, crate::i18n::tr("Constrain to Image")).changed() {
            let _ = app.run("develop.merge", json!({"settings": {"geometry": {"constrain_crop": c}}, "label": "Constrain to Image"}));
        }
    });
    padded(ui, |ui| {
        // one row at the right edge, as the drawer's buttons sit in Lightroom Classic
        ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), 28.0), egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if text_button(ui, "cropClose", crate::i18n::tr("Close"), false).clicked() {
                app.ui.right = RightPanel::Edit;
                app.ui.tool = String::new();
            }
            if text_button(ui, "cropReset", crate::i18n::tr("Reset"), false).clicked() {
                let _ = app.run("crop.reset", json!({}));
            }
        });
    });
}

/// The Edit panel's Transform section, as Lightroom Classic's Transform panel: Upright, Constrain
/// Crop and the transform sliders. A clip takes the crop, straighten and flips only.
pub(crate) fn transform_body(app: &mut LccApp, ui: &mut egui::Ui, id: PhotoId) {
    let d = app.session.develop_of(id).unwrap_or_default();
    let video = crate::video_ui::photo_is_video(app, id);
    crate::video_ui::video_gate(ui, video, |ui| {
        padded(ui, |ui| {
            let t = Tokens::get(ui.ctx());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.label(egui::RichText::new(crate::i18n::tr("Upright")).font(t.semibold(13.0)).color(t.text));
                let has_upright = d.geometry.upright != lcc_develop::Upright::Off || !d.geometry.guides.is_empty();
                let reset_resp = icon_button(ui, "uprightReset", Icon::Reset, egui::vec2(20.0, 20.0), false, has_upright, "Reset Upright");
                register(ui.ctx(), "button:upright-reset".to_string(), reset_resp.rect);
                if reset_resp.clicked() {
                    let _ = app.run("geometry.upright", json!({"mode": "off"}));
                    if !d.geometry.guides.is_empty() {
                        let _ = app.run("geometry.guides", json!({"guides": []}));
                    }
                    if app.ui.tool == "guidedUpright" {
                        app.ui.tool.clear();
                    }
                }
                let is_guided = d.geometry.upright == lcc_develop::Upright::Guided || app.ui.tool == "guidedUpright";
                let guided_resp = icon_button(ui, "uprightGuided", Icon::GuidedUpright, egui::vec2(20.0, 20.0), is_guided, true, "Guided Upright");
                register(ui.ctx(), "button:upright-guided".to_string(), guided_resp.rect);
                if guided_resp.clicked() {
                    if app.ui.tool == "guidedUpright" {
                        app.ui.tool.clear();
                    } else {
                        app.ui.tool = "guidedUpright".into();
                        let _ = app.run("geometry.upright", json!({"mode": "guided"}));
                    }
                }
            });
            ui.add_space(4.0);
            {
                use lcc_develop::Upright;
                let modes = [
                    ("Off", Upright::Off, "off"),
                    ("Auto", Upright::Auto, "auto"),
                    ("Level", Upright::Level, "level"),
                    ("Vertical", Upright::Vertical, "vertical"),
                    ("Full", Upright::Full, "full"),
                ];
                let items: Vec<(&str, &str)> = modes.iter().map(|(l, _, k)| (*l, *k)).collect();
                let active = modes.iter().position(|(_, m, _)| *m == d.geometry.upright);
                if let Some(i) = segmented_pill(ui, "upright", &items, active)
                    && let Some((_, _mode, key)) = modes.get(i)
                {
                    if app.ui.tool == "guidedUpright" {
                        app.ui.tool.clear();
                    }
                    let r = app.run("geometry.upright", json!({"mode": key}));
                    super::upright_note(app, ui.ctx(), r.as_ref().ok());
                }
            }
            let guided = d.geometry.upright == lcc_develop::Upright::Guided || app.ui.tool == "guidedUpright";
            if guided {
                ui.add_space(4.0);
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(crate::i18n::tr_format!(
                            "{} of 4 guides — drag along lines that should be vertical or horizontal.",
                            d.geometry.guides.len()
                        ))
                        .size(11.0)
                        .color(Tokens::get(ui.ctx()).text_dim),
                    )
                    .wrap(),
                );
                if !d.geometry.guides.is_empty() {
                    ui.add_space(2.0);
                    if text_button(ui, "uprightClear", crate::i18n::tr("Clear Guides"), false).clicked() {
                        let _ = app.run("geometry.guides", json!({"guides": []}));
                    }
                }
            } else if d.geometry.upright != lcc_develop::Upright::Off {
                ui.add_space(2.0);
                if text_button(ui, "uprightUpdate", crate::i18n::tr("Update"), false).clicked() {
                    let mode = serde_json::to_value(d.geometry.upright).unwrap_or_default();
                    let r = app.run("geometry.upright", json!({"mode": mode}));
                    super::upright_note(app, ui.ctx(), r.as_ref().ok());
                }
            }
            ui.add_space(4.0);
            let mut c = d.geometry.constrain_crop;
            if ui.checkbox(&mut c, crate::i18n::tr("Constrain Crop")).changed() {
                let _ = app.run("develop.merge", json!({"settings": {"geometry": {"constrain_crop": c}}, "label": "Constrain Crop"}));
            }
        });
        for spec in lcc_develop::controls::in_section(lcc_develop::Section::Geometry).filter(|c| c.id != "crop.angle") {
            let v = lcc_develop::controls::get(&d, spec.id).unwrap_or(spec.default);
            let out = slider(ui, spec, v, true, None);
            super::super::edit::apply_slider_out(app, spec, out, |app, v| app.run("develop.set", json!({"control": spec.id, "value": v})));
        }
    });
}
