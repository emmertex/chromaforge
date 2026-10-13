//! The right-hand panel next to the tool strip: Edit, Crop, Remove, Masking, Red Eye, Info,
//! Keywords, Versions, Activity.

use egui::{Align2, Rect, Sense, pos2, vec2};
use lcc_catalog::PhotoId;
use serde_json::json;

use crate::LccApp;
use crate::icons::{Icon, paint};
use crate::state::RightPanel;
use crate::theme::Tokens;
use crate::widgets::{divider, register, slider, text_button};

pub fn show(app: &mut LccApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let frame = egui::Frame::NONE.fill(t.chrome).stroke(egui::Stroke::new(1.0, t.divider));
    frame.show(ui, |ui| {
        let Some(id) = app.session.active() else {
            // the Keyword List is the library's, not a photo's: it is there without a selection
            if app.ui.right == RightPanel::Keywords {
                egui::ScrollArea::vertical().id_salt("right-scroll").auto_shrink([false, false]).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    header(ui, "Keywords");
                    padded(ui, |ui| {
                        ui.label(egui::RichText::new(crate::i18n::tr("Select photos to give them keywords.")).color(t.text_dim));
                    });
                    super::keyword_list::show(app, ui);
                });
                return;
            }
            let r = ui.max_rect();
            super::empty_message(ui, r, "No photo selected", "Select a photo to edit");
            return;
        };
        // Edit: Lightroom Classic's Previous / Sync… bar under the panels, outside the scrolling
        let full = ui.available_rect_before_wrap();
        let footer = (app.ui.right == RightPanel::Edit).then(|| Rect::from_min_max(pos2(full.left(), full.bottom() - SYNC_BAR_H), full.max));
        let body = footer.map_or(full, |f| Rect::from_min_max(full.min, pos2(full.right(), f.top())));
        if let Some(f) = footer {
            ui.scope_builder(egui::UiBuilder::new().max_rect(f), |ui| sync_bar(app, ui, f));
        }
        ui.scope_builder(egui::UiBuilder::new().max_rect(body), |ui| {
            egui::ScrollArea::vertical().id_salt("right-scroll").auto_shrink([false, false]).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                match app.ui.right {
                    RightPanel::Edit => super::edit::show(app, ui, id),
                    RightPanel::Profiles => super::profiles::show(app, ui, id),
                    RightPanel::Crop => crop(app, ui, id),
                    RightPanel::Remove => remove(app, ui, id),
                    RightPanel::Masking => super::masking::show(app, ui, id),
                    RightPanel::RedEye => red_eye(app, ui, id),
                    RightPanel::Info => super::metadata::show(app, ui, id),
                    RightPanel::Keywords => keywords(app, ui, id),
                    RightPanel::Versions => versions(app, ui, id),
                    RightPanel::Activity => activity(app, ui, id),
                    RightPanel::None => {}
                }
            });
        });
    });
}

/// Height of the Previous / Sync… bar under the Edit panel.
const SYNC_BAR_H: f32 = 44.0;

/// Lightroom Classic's bar under the Develop panels: the Auto Sync switch, then Previous (one photo:
/// paste the previous photo's settings), Sync… (several: Synchronize Settings) or Auto Sync (while
/// it is on: edits already go to every selected photo; a click turns it off).
fn sync_bar(app: &mut LccApp, ui: &mut egui::Ui, r: Rect) {
    let t = Tokens::get(ui.ctx());
    ui.painter().rect_filled(r, 0.0, t.chrome);
    ui.painter().hline(r.x_range(), r.top() + 0.5, egui::Stroke::new(1.0, t.divider));
    let auto = app.session.auto_sync;
    let several = app.session.selection.ids.len() > 1;
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        ui.add_space(16.0);
        ui.spacing_mut().item_spacing.x = 8.0;
        // the switch
        let (sr, resp) = ui.allocate_exact_size(vec2(30.0, 16.0), Sense::click());
        register(ui.ctx(), "button:autoSyncSwitch", sr);
        let p = ui.painter();
        p.rect_filled(sr, 8.0, if auto { t.accent } else { t.divider });
        let knob = if auto { sr.right() - 8.0 } else { sr.left() + 8.0 };
        p.circle_filled(pos2(knob, sr.center().y), 6.0, t.text);
        let tip = crate::i18n::tr(if auto { "Auto Sync is on: edits apply to every selected photo" } else { "Turn on Auto Sync" });
        if resp.on_hover_text(tip).clicked() {
            let _ = app.run("develop.autoSync", json!({"on": !auto}));
        }
        let (id, label) = match (auto, several) {
            (true, _) => ("syncBar", "Auto Sync"),
            (false, true) => ("syncBar", "Sync…"),
            (false, false) => ("previousBar", "Previous"),
        };
        if text_button(ui, id, crate::i18n::tr(label), auto).clicked() {
            let _ = match (auto, several) {
                (true, _) => app.run("develop.autoSync", json!({"on": false})),
                (false, true) => app.run("dialog.syncSettings", json!({})),
                (false, false) => app.run("develop.pastePrevious", json!({})),
            };
        }
    });
}

/// The Classic Metadata panel (Library's right column).
pub fn metadata_panel(app: &mut LccApp, ui: &mut egui::Ui) {
    match app.session.active() {
        Some(id) => super::metadata::show(app, ui, id),
        None => {
            ui.label(crate::i18n::tr("No photo selected"));
        }
    }
}

pub fn header(ui: &mut egui::Ui, title: &str) {
    let t = Tokens::get(ui.ctx());
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 46.0), Sense::hover());
    ui.painter().text(pos2(r.left() + 24.0, r.center().y + 2.0), Align2::LEFT_CENTER, crate::i18n::tr(title), t.semibold(15.0), t.text);
}

pub fn label_row(ui: &mut egui::Ui, label: &str, value: &str) {
    let t = Tokens::get(ui.ctx());
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::hover());
    ui.painter().text(pos2(r.left() + 24.0, r.center().y), Align2::LEFT_CENTER, crate::i18n::tr(label), t.font(12.5), t.text_dim);
    ui.painter().text(pos2(r.left() + 110.0, r.center().y), Align2::LEFT_CENTER, value, t.font(12.5), t.text_label);
}

fn padded(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::NONE.inner_margin(egui::Margin { left: 24, right: 22, top: 6, bottom: 6 }).show(ui, add);
}

/// The aspect presets of the Crop panel: (menu label, `crop.aspect` value).
const ASPECT_PRESETS: [(&str, &str); 10] = [
    ("Free", "free"),
    ("Original", "original"),
    ("1 × 1", "1x1"),
    ("4 × 5 / 8 × 10", "4x5"),
    ("8.5 × 11", "8.5x11"),
    ("5 × 7", "5x7"),
    ("2 × 3 / 4 × 6", "2x3"),
    ("4 × 3", "4x3"),
    ("16 × 9", "16x9"),
    ("16 × 10", "16x10"),
];

/// What the aspect button says for the stored lock (`aspect` = width × 100, height × 100):
/// a preset's name when the ratio matches one in either orientation, "Original" for the photo's own
/// shape, otherwise the ratio as `1.37 : 1`.
fn aspect_label(aspect: Option<(u32, u32)>, original: Option<f64>) -> String {
    let Some((w, h)) = aspect else { return "Free".into() };
    if w == 0 || h == 0 {
        return "Free".into();
    }
    let r = w as f64 / h as f64;
    let is = |q: f64| q.is_finite() && q > 0.0 && ((r - q).abs() / q < 0.004 || (1.0 / r - q).abs() / q < 0.004);
    if original.is_some_and(is) {
        return "Original".into();
    }
    for (label, key) in ASPECT_PRESETS {
        if let Some((x, y)) = key.split_once('x')
            && let (Ok(x), Ok(y)) = (x.parse::<f64>(), y.parse::<f64>())
            && is(x / y)
        {
            return label.to_string();
        }
    }
    format!("{:.2} : 1", r.max(1.0 / r))
}

/// "Custom" row of the aspect menu: two number fields and an Apply button (`crop.aspect` `[w, h]`).
fn custom_aspect(app: &mut LccApp, ui: &mut egui::Ui) {
    let key = egui::Id::new("crop-custom-aspect");
    let (mut w, mut h): (String, String) = ui.data_mut(|d| d.get_temp(key)).unwrap_or_else(|| ("3".into(), "2".into()));
    let mut apply = false;
    ui.horizontal(|ui| {
        ui.label(crate::i18n::tr("Custom"));
        let a = ui.add(egui::TextEdit::singleline(&mut w).desired_width(34.0));
        register(ui.ctx(), "field:cropCustomW", a.rect);
        ui.label("×");
        let b = ui.add(egui::TextEdit::singleline(&mut h).desired_width(34.0));
        register(ui.ctx(), "field:cropCustomH", b.rect);
        let go = text_button(ui, "cropCustomApply", "Apply", false);
        apply = go.clicked() || ((a.lost_focus() || b.lost_focus()) && ui.input(|i| i.key_pressed(egui::Key::Enter)));
    });
    if apply {
        // Accept `3`, `2.5` and `3,5`; anything else (zero, negative, text, huge) is ignored.
        let num = |t: &str| t.trim().replace(',', ".").parse::<f64>().ok().filter(|v| v.is_finite() && *v > 0.0 && *v <= 1000.0);
        if let (Some(x), Some(y)) = (num(&w), num(&h))
            && (1.0 / lcc_geom::MAX_RATIO..=lcc_geom::MAX_RATIO).contains(&(x / y))
        {
            let _ = app.run("crop.aspect", json!({"aspect": [x, y]}));
            ui.close();
        }
    }
    ui.data_mut(|d| d.insert_temp(key, (w, h)));
}

fn crop(app: &mut LccApp, ui: &mut egui::Ui, id: PhotoId) {
    let d = app.session.develop_of(id).unwrap_or_default();
    header(ui, "Crop");
    padded(ui, |ui| {
        let original = app.session.catalog.photo(id).map(|p| {
            let (w, h) = (p.width.max(1) as f64, p.height.max(1) as f64);
            if d.orientation.swaps_axes() { h / w } else { w / h }
        });
        ui.horizontal(|ui| {
            ui.label(crate::i18n::tr("Aspect Ratio"));
            let cur = aspect_label(app.crop_tool_aspect(), original);
            let t = Tokens::get(ui.ctx());
            let r = crate::widgets::dropdown(ui, "cropAspect", crate::i18n::tr(&cur), t.font(12.5), t.text);

            egui::Popup::menu(&r).show(|ui| {
                for (label, a) in ASPECT_PRESETS {
                    if ui.button(crate::i18n::tr(label)).clicked() {
                        let _ = app.run("crop.aspect", json!({"aspect": a}));
                    }
                }
                ui.separator();
                custom_aspect(app, ui);
            });
            let locked = app.crop_tool_aspect().is_some();
            let (icon, label) = if locked { (Icon::Link, "Aspect Locked") } else { (Icon::Unlink, "Lock Aspect") };
            if cmd_icon(app, ui, "cropLock", icon, label, None, locked).clicked() {
                let _ = app.run("crop.aspect", json!({"aspect": "toggle"}));
            }
        });
        ui.add_space(6.0);
        // one row of icon buttons, as in the suite's transform panels (issue #972); each command's
        // name and shortcut show on hover
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let row = [
                ("cropRotateLeft", Icon::RotateLeft, "Rotate Left", "photo.rotateLeft", false),
                ("cropRotateRight", Icon::RotateRight, "Rotate Right", "photo.rotateRight", false),
                ("cropFlipH", Icon::FlipH, "Flip Horizontal", "photo.flipHorizontal", d.crop.flip_h),
                ("cropFlipV", Icon::FlipV, "Flip Vertical", "photo.flipVertical", d.crop.flip_v),
                ("cropSwapAspect", Icon::SwapAspect, "Rotate Crop Aspect", "crop.rotateAspect", false),
                ("cropReset", Icon::Reset, "Reset Crop", "crop.reset", false),
            ];
            for (id, icon, label, command, on) in row {
                if cmd_icon(app, ui, id, icon, label, Some(command), on).clicked() {
                    let _ = app.run(command, json!({}));
                }
            }
        });
    });
    let ang = lcc_develop::controls::find("crop.angle").copied();
    if let Some(spec) = ang {
        let out = slider(ui, &spec, d.crop.geometry.angle, true, Some("Straighten"));
        super::edit::apply_slider_out(app, &spec, out, |app, v| app.run("crop.straighten", json!({"angle": v})));
        // an exact angle, typed (issue #534): a field of its own, besides the slider's value
        padded(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(crate::i18n::tr("Angle"));
                let mut angle = d.crop.geometry.angle;
                let escape = ui.input(|i| i.key_pressed(egui::Key::Escape));
                let r = ui.add(
                    egui::DragValue::new(&mut angle)
                        .range(spec.min..=spec.max)
                        .clamp_existing_to_range(false)
                        .speed(0.05)
                        .fixed_decimals(2)
                        // as the readout writes it: "+2.50", and every zero "0.00"
                        .custom_formatter(|v, _| super::detail::crop_angle_label(v).trim_end_matches('°').to_string())
                        .suffix("°")
                        // read as the Straighten value reads a typed one: "-3,25" too
                        .custom_parser(|text| crate::widgets::typed_value(&spec, text.trim().trim_end_matches('°')))
                        // a typed angle applies on Return (or leaving the field), not keystroke by
                        // keystroke: no half-typed angles, one undo step
                        .update_while_editing(false),
                );
                crate::widgets::register(ui.ctx(), "cropAngleField", r.rect);
                // a drag on the field is one undo step, like the slider's
                if r.drag_started() {
                    let _ = app.run("develop.beginInteraction", json!({"label": "Straighten"}));
                }
                // egui reports a change while text is typed even when the value isn't updated yet:
                // apply only a new angle
                // Esc keeps the old angle, but egui applies the typed text on the frame after the
                // one where Esc took the focus away: remember the Esc for that frame
                let cancel = egui::Id::new("cropAngleFieldEscaped");
                let escaped = ui.data_mut(|m| m.remove_temp::<bool>(cancel)).unwrap_or(false);
                if escape && r.lost_focus() {
                    ui.data_mut(|m| m.insert_temp(cancel, true));
                }
                if r.changed() && !escaped && angle.is_finite() && angle != d.crop.geometry.angle {
                    let _ = app.run("crop.straighten", json!({"angle": (angle.clamp(spec.min, spec.max) * 100.0).round() / 100.0}));
                }
                if r.drag_stopped() {
                    let _ = app.run("develop.endInteraction", json!({}));
                }
            });
        });
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
            if cmd_icon(app, ui, "straightenAuto", Icon::Auto, "Auto Straighten", Some("crop.autoStraighten"), false).clicked() {
                let _ = app.run("crop.autoStraighten", json!({}));
            }
        });
    });
    padded(ui, |ui| {
        ui.label(crate::i18n::tr("Overlay"));
        {
            use crate::state::CropOverlay as O;
            let opts = [
                (O::Thirds, Icon::OverlayThirds, "Thirds", "thirds"),
                (O::Grid, Icon::OverlayGrid, "Grid", "grid"),
                (O::Golden, Icon::OverlayGolden, "Golden", "golden"),
                (O::Diagonal, Icon::OverlayDiagonal, "Diagonal", "diagonal"),
                (O::Triangle, Icon::OverlayTriangle, "Triangle", "triangle"),
                (O::Spiral, Icon::OverlaySpiral, "Spiral", "spiral"),
                (O::None, Icon::OverlayNone, "None", "none"),
            ];
            let items: Vec<(Icon, &str, &str)> = opts.iter().map(|(_, i, l, k)| (*i, *l, *k)).collect();
            let active = opts.iter().position(|(o, ..)| *o == app.ui.crop_overlay);
            if let Some(i) = crate::widgets::icon_segmented(ui, "cropOverlay", &items, active)
                && let Some(o) = opts.get(i)
            {
                app.ui.crop_overlay = o.0;
            }
            if matches!(app.ui.crop_overlay, O::Triangle | O::Spiral)
                && cmd_icon(app, ui, "cropOverlayOrient", Icon::FlipH, "Flip Overlay (⇧O)", None, false).clicked()
            {
                let _ = app.run("view.cropOverlayOrientation", json!({}));
            }
        }
    });
    divider(ui);
    header(ui, "Geometry");
    // a clip takes the crop, straighten and flips only: Upright and the transform sliders are photo-only
    let video = crate::video_ui::photo_is_video(app, id);
    crate::video_ui::video_gate(ui, video, |ui| {
        padded(ui, |ui| {
            ui.label(crate::i18n::tr("Upright"));
            {
                use lcc_develop::Upright;
                let modes = [
                    ("Off", Upright::Off, "off"),
                    ("Auto", Upright::Auto, "auto"),
                    ("Guided", Upright::Guided, "guided"),
                    ("Level", Upright::Level, "level"),
                    ("Vertical", Upright::Vertical, "vertical"),
                    ("Full", Upright::Full, "full"),
                ];
                let items: Vec<(&str, &str)> = modes.iter().map(|(l, _, k)| (*l, *k)).collect();
                let active = modes.iter().position(|(_, m, _)| *m == d.geometry.upright);
                if let Some(i) = crate::widgets::segmented(ui, "upright", &items, active, 3) {
                    let (_, mode, key) = modes[i];
                    let _ = app.run("geometry.upright", json!({"mode": key}));
                    app.ui.tool = if mode == Upright::Guided { "guidedUpright".into() } else { String::new() };
                }
            }
            let guided = d.geometry.upright == lcc_develop::Upright::Guided;
            ui.horizontal_wrapped(|ui| {
                if !guided
                    && d.geometry.upright != lcc_develop::Upright::Off
                    && text_button(ui, "uprightUpdate", crate::i18n::tr("Update"), false).clicked()
                {
                    let mode = serde_json::to_value(d.geometry.upright).unwrap_or_default();
                    let _ = app.run("geometry.upright", json!({"mode": mode}));
                }
                if guided {
                    let drawing = app.ui.tool == "guidedUpright";
                    if text_button(ui, "uprightDraw", crate::i18n::tr("Draw Guides"), drawing).clicked() {
                        app.ui.tool = if drawing { String::new() } else { "guidedUpright".into() };
                    }
                    if !d.geometry.guides.is_empty() && text_button(ui, "uprightClear", crate::i18n::tr("Clear Guides"), false).clicked() {
                        let _ = app.run("geometry.guides", json!({"guides": []}));
                    }
                }
            });
            if guided {
                ui.label(
                    egui::RichText::new(crate::i18n::tr_format!(
                        "{} of 4 guides — drag along lines that should be vertical or horizontal.",
                        d.geometry.guides.len()
                    ))
                    .size(11.0)
                    .color(Tokens::get(ui.ctx()).text_dim),
                );
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
            super::edit::apply_slider_out(app, spec, out, |app, v| app.run("develop.set", json!({"control": spec.id, "value": v})));
        }
    });
}

fn remove(app: &mut LccApp, ui: &mut egui::Ui, id: PhotoId) {
    let d = app.session.develop_of(id).unwrap_or_default();
    header(ui, "Remove");
    padded(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            for (label, tool) in [("Remove", "remove"), ("Content-Aware", "fill"), ("Heal", "heal"), ("Clone", "clone")] {
                if text_button(ui, &format!("removeMode-{tool}"), label, app.ui.tool == tool).clicked() {
                    app.ui.tool = tool.into();
                }
            }
        });
        ui.add_space(8.0);
        ui.label(crate::i18n::tr_format!("{} spot(s) on this photo", d.spots.len()));
        ui.add_space(4.0);
        if text_button(ui, "findDust", crate::i18n::tr("Find Dust Spots"), false)
            .on_hover_text(crate::i18n::tr("Heal the small, soft dark spots sensor dust leaves on smooth areas"))
            .clicked()
        {
            match app.run("spot.findDust", json!({})) {
                Ok(r) => {
                    let n = r["added"].as_u64().unwrap_or(0);
                    app.toast(
                        ui.ctx(),
                        if n == 0 {
                            crate::i18n::tr("No dust spots found").to_string()
                        } else {
                            crate::i18n::tr_format!("Healed {n} dust spot{}", if n == 1 { "" } else { "s" }, n = n)
                        },
                    );
                }
                Err(e) => app.toast(ui.ctx(), e),
            }
        }
        ui.add_space(4.0);
        ui.label(egui::RichText::new(crate::i18n::tr("Paint over a distraction on the photo to remove it.")).color(Tokens::get(ui.ctx()).text_dim));
    });
    // brush settings; with a spot selected they edit that spot too
    let sel = app.session.active_spot.and_then(|i| d.spots.get(i).map(|sp| (i, sp.clone())));
    if let Some((i, sp)) = &sel {
        (app.ui.remove_size, app.ui.remove_feather, app.ui.remove_opacity) = (sp.size as f32, sp.feather as f32, sp.opacity as f32);
        divider(ui);
        let mode = match sp.mode {
            lcc_develop::SpotMode::Heal => "Heal",
            lcc_develop::SpotMode::Clone => "Clone",
            lcc_develop::SpotMode::Remove => "Remove",
            lcc_develop::SpotMode::Fill => "Content-Aware",
        };
        super::edit::sub_title(ui, &crate::i18n::tr_format!("{mode} spot {} of {}", i + 1, d.spots.len(), mode = crate::i18n::tr(mode)));
    }
    let plain = |id: &'static str, label: &'static str, min: f64, max: f64, default: f64| lcc_develop::ControlSpec {
        id,
        label,
        section: lcc_develop::Section::Detail,
        min,
        max,
        default,
        step: 1.0,
        decimals: 0,
        track: lcc_develop::Track::Plain,
    };
    let sliders = [
        (plain("ui.removeSize", "Size", 1.0, 250.0, 20.0), "size", (app.ui.remove_size * 1000.0) as f64),
        (plain("ui.removeFeather", "Feather", 0.0, 100.0, 50.0), "feather", app.ui.remove_feather as f64),
        (plain("ui.removeOpacity", "Opacity", 0.0, 100.0, 100.0), "opacity", app.ui.remove_opacity as f64),
    ];
    for (spec, key, v) in sliders {
        let out = slider(ui, &spec, v, true, None);
        let scale = if key == "size" { 1000.0 } else { 1.0 };
        if let Some(v) = out.value {
            match key {
                "size" => app.ui.remove_size = (v / scale) as f32,
                "feather" => app.ui.remove_feather = v as f32,
                _ => app.ui.remove_opacity = v as f32,
            }
        }
        if sel.is_some() {
            super::edit::apply_slider_out(app, &spec, out, |app, v| app.run("spot.update", json!({key: v / scale})));
        }
    }
    if sel.is_some() {
        padded(ui, |ui| {
            ui.horizontal(|ui| {
                if text_button(ui, "spotRefresh", crate::i18n::tr("Refresh Source (/)"), false).clicked() {
                    let _ = app.run("spot.refreshSource", json!({}));
                }
                if text_button(ui, "spotDelete", crate::i18n::tr("Delete (⌫)"), false).clicked() {
                    let _ = app.run("spot.delete", json!({}));
                }
            });
        });
    }
    // Visualize Spots (A): a black/white high-pass view that makes dust and specks stand out
    padded(ui, |ui| {
        let mut v = app.ui.visualize_spots;
        if ui.checkbox(&mut v, crate::i18n::tr("Visualize Spots (A)")).changed() {
            app.ui.visualize_spots = v;
        }
    });
    let spec = lcc_develop::ControlSpec {
        id: "ui.spotsThreshold",
        label: "Threshold",
        section: lcc_develop::Section::Detail,
        min: 0.0,
        max: 100.0,
        default: 50.0,
        step: 1.0,
        decimals: 0,
        track: lcc_develop::Track::Plain,
    };
    let out = slider(ui, &spec, app.ui.spots_threshold as f64, app.ui.visualize_spots, None);
    if let Some(v) = out.value {
        app.ui.spots_threshold = v as f32;
    }
    padded(ui, |ui| {
        if !d.spots.is_empty() && text_button(ui, "removeClear", crate::i18n::tr("Delete all spots"), false).clicked() {
            for i in (0..d.spots.len()).rev() {
                let _ = app.run("spot.delete", json!({"index": i}));
            }
        }
    });
}

fn red_eye(app: &mut LccApp, ui: &mut egui::Ui, id: PhotoId) {
    let d = app.session.develop_of(id).unwrap_or_default();
    header(ui, "Red Eye");
    padded(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            for (label, pet) in [("Red Eye", false), ("Pet Eye", true)] {
                if text_button(ui, &format!("eyeMode-{}", if pet { "pet" } else { "red" }), label, app.ui.eye_pet == pet).clicked() {
                    app.ui.eye_pet = pet;
                }
            }
        });
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new(crate::i18n::tr("Drag over an eye on the photo; the pupil inside is found automatically."))
                .color(Tokens::get(ui.ctx()).text_dim),
        );
    });
    let n = d.red_eye.len();
    if n == 0 {
        return;
    }
    if app.ui.eye >= n {
        app.ui.eye = n - 1;
    }
    let i = app.ui.eye;
    let eye = d.red_eye[i];
    divider(ui);
    super::edit::sub_title(
        ui,
        &crate::i18n::tr_format!(
            "{label} {index} of {n}",
            label = crate::i18n::tr(if eye.pet { "Pet Eye" } else { "Red Eye" }),
            index = i + 1,
            n = n
        ),
    );
    for k in ["pupilSize", "darken"] {
        let cid = format!("redEye.{i}.{k}");
        let Some(spec) = lcc_develop::controls::find(&cid) else { continue };
        let v = lcc_develop::controls::get(&d, &cid).unwrap_or(spec.default);
        let out = slider(ui, spec, v, true, None);
        super::edit::apply_slider_out(app, spec, out, |app, v| app.run("develop.set", json!({"control": cid, "value": v})));
    }
    padded(ui, |ui| {
        if eye.pet {
            let mut on = eye.catchlight.is_some();
            if ui.checkbox(&mut on, crate::i18n::tr("Add Catchlight")).changed() {
                let _ = app.run("redeye.catchlight", json!({"index": i, "on": on}));
            }
            ui.add_space(4.0);
        }
        ui.horizontal(|ui| {
            if text_button(ui, "eyeDelete", crate::i18n::tr("Delete"), false).clicked() {
                let _ = app.run("redeye.delete", json!({"index": i}));
            }
            if n > 1 && text_button(ui, "eyeDeleteAll", crate::i18n::tr("Delete all"), false).clicked() {
                for k in (0..n).rev() {
                    let _ = app.run("redeye.delete", json!({"index": k}));
                }
            }
        });
    });
}

fn keywords(app: &mut LccApp, ui: &mut egui::Ui, id: PhotoId) {
    // (the box is the selection's: the active photo only says that there is one)
    if app.session.catalog.photo(id).is_none() {
        return;
    }
    header(ui, "Keywords");
    keywording(app, ui);
    super::keyword_list::show(app, ui);
}

/// The keyword entry, chips, keyword set, painter and suggestions for the selected photos (the
/// Classic shell's Keywording panel; the Keywords panel adds the Keyword List below them).
pub(crate) fn keywording(app: &mut LccApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    padded(ui, |ui| {
        let kid = egui::Id::new("kw-input");
        let mut text = ui.data_mut(|d| d.get_temp::<String>(kid).unwrap_or_default());
        let r =
            crate::text_field::TextField::singleline("field:keyword", &mut text).hint(crate::i18n::tr("Add keyword")).width(f32::INFINITY).show(ui);
        // Return gives the keywords to the selected photos; Esc gives back what was there before
        // this edit (the shared field does)
        if r.ending == Some(crate::text_field::Ending::Return) && !text.trim().is_empty() {
            // a new name goes inside the default parent (Put New Keywords Inside This Keyword)
            let kws: Vec<String> = text.split(',').map(|s| app.session.catalog.typed_keyword(s)).filter(|s| !s.is_empty()).collect();
            let _ = app.run("photo.setMeta", json!({"addKeywords": kws}));
            text.clear();
        }
        ui.data_mut(|d| d.insert_temp(kid, text));
        ui.add_space(8.0);
        super::keywording::view_switch(app, ui);
        ui.add_space(4.0);
        if app.ui.keywording_view == crate::state::KeywordingView::Keywords {
            super::keywording::chip_row(app, ui);
        } else {
            super::keywording::names_row(app, ui);
        }
        ui.add_space(10.0);
        keyword_set(app, ui);
        ui.add_space(8.0);
        // the painter: click photos in the grid to give them (or take away) a keyword
        ui.horizontal(|ui| {
            let pid = egui::Id::new("kw-painter");
            let painting = app.ui.lib.painter.as_ref().filter(|p| p.kind == crate::libtools::PaintKind::Keywords).map(|p| p.value.clone());
            let mut k: String = ui.data(|d| d.get_temp(pid)).unwrap_or_else(|| painting.clone().unwrap_or_default());
            // (fixed while painting: Stop first)
            let field = ui
                .add_enabled_ui(painting.is_none(), |ui| {
                    crate::text_field::TextField::singleline("field:keywordPainter", &mut k)
                        .hint(crate::i18n::tr("Keyword to paint"))
                        .width(130.0)
                        .show(ui)
                })
                .inner;
            ui.data_mut(|d| d.insert_temp(pid, k.clone()));
            let label = if painting.is_some() { "Stop" } else { "Paint" };
            let returned = field.ending == Some(crate::text_field::Ending::Return) && painting.is_none() && !k.trim().is_empty();
            if text_button(ui, "keywordPaint", label, painting.is_some())
                .on_hover_text(crate::i18n::tr("Click photos in the grid to toggle the keyword; Esc stops"))
                .clicked()
                || returned
            {
                let _ = app.run("tool.keywordPainter", json!({"keyword": if painting.is_some() { serde_json::Value::Null } else { json!(k) }}));
            }
        });
        ui.add_space(10.0);
        // suggestions: completions of the typed text, else keywords used together with this
        // photo's keywords, else the most used ones
        let typed = ui.data(|d| d.get_temp::<String>(kid).unwrap_or_default());
        let last = typed.rsplit(',').next().unwrap_or("").trim().to_string();
        // the keywords every selected photo has: those only some have are still suggested
        let selection = app.session.targets(&serde_json::Value::Null);
        let have: Vec<String> =
            app.caches.keyword_chips(&app.session.catalog, &selection).iter().filter(|c| c.on_all()).map(|c| c.path.clone()).collect();
        let suggestions = (*app.caches.suggestions(&app.session.catalog, &have, &last, 12)).clone();
        if !suggestions.is_empty() {
            ui.label(egui::RichText::new(crate::i18n::tr("Suggestions")).color(t.text_dim));
            ui.horizontal_wrapped(|ui| {
                for k in suggestions {
                    let r = ui
                        .add(egui::Button::new(egui::RichText::new(format!("+ {}", k.replace('|', " › "))).color(t.text_label)).corner_radius(10.0));
                    register(ui.ctx(), format!("kwSuggest:{k}"), r.rect);
                    if r.clicked() {
                        let _ = app.run("photo.setMeta", json!({"addKeywords": [k]}));
                        if !last.is_empty() {
                            ui.data_mut(|d| d.insert_temp(kid, String::new()));
                        }
                    }
                }
            });
        }
    });
}

/// The keyword set: pick a set, then nine buttons (⌥1–⌥9) that toggle its keywords on the
/// selected photos; "Save as Set…" keeps the current nine under a name.
fn keyword_set(app: &mut LccApp, ui: &mut egui::Ui) {
    // on: every selected photo has it (⌥1–⌥9 then take it off them); partly on: only some do
    let selection = app.session.targets(&serde_json::Value::Null);
    let chips = app.caches.keyword_chips(&app.session.catalog, &selection);
    let t = Tokens::get(ui.ctx());
    let sets = lcc_engine::cmd::keywords::keyword_sets_json(&app.session);
    let current = sets["current"].as_str().unwrap_or_default().to_string();
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(crate::i18n::tr("Keyword Set")).color(t.text_dim));
        let combo = egui::ComboBox::from_id_salt("kw-set")
            .selected_text(crate::i18n::builtin_label(&current, current == lcc_engine::cmd::keywords::RECENT))
            .show_ui(ui, |ui| {
                for set in sets["sets"].as_array().into_iter().flatten() {
                    let name = set["name"].as_str().unwrap_or_default();
                    if ui.selectable_label(name == current, crate::i18n::builtin_label(name, name == lcc_engine::cmd::keywords::RECENT)).clicked() {
                        let _ = app.run("keyword.useSet", json!({"name": name}));
                    }
                }
                ui.separator();
                // Edit Set…: the current nine, slot by slot (named, Recent Keywords becomes a set)
                let edit = ui.button(crate::i18n::tr("Edit Set…"));
                register(ui.ctx(), "keywordSetMenu:edit", edit.rect);
                if edit.clicked() {
                    let mut slots: Vec<String> =
                        sets["keywords"].as_array().into_iter().flatten().filter_map(|k| k.as_str().map(str::to_string)).collect();
                    slots.resize(9, String::new());
                    let named = current != lcc_engine::cmd::keywords::RECENT;
                    app.ui.dialog = Some(crate::state::Dialog::KeywordSet {
                        replaces: named.then(|| current.clone()),
                        name: if named { current.clone() } else { String::new() },
                        slots,
                        as_new: false,
                    });
                }
                if ui.button(crate::i18n::tr("Save Current Keywords as Set…")).clicked() {
                    app.ui.dialog = Some(crate::state::Dialog::TextPrompt {
                        title: "Save Keyword Set".into(),
                        hint: "Set name".into(),
                        value: String::new(),
                        command: "keyword.saveSet".into(),
                        params: json!({}),
                        key: "name".into(),
                    });
                }
                if current != lcc_engine::cmd::keywords::RECENT
                    && ui.button(crate::i18n::tr_format!("Delete “{current}”", current = current)).clicked()
                {
                    let _ = app.run("keyword.deleteSet", json!({"name": current}));
                }
            });
        register(ui.ctx(), "keywordSetCombo", combo.response.rect);
    });
    let kws: Vec<String> = sets["keywords"].as_array().into_iter().flatten().filter_map(|k| k.as_str().map(str::to_string)).collect();
    if kws.is_empty() {
        ui.label(egui::RichText::new(crate::i18n::tr("Keywords you add appear here; ⌥1–⌥9 apply them.")).color(t.text_dim));
        return;
    }
    let bw = ((ui.available_width() - 8.0) / 3.0).floor().max(40.0);
    egui::Grid::new("kw-set-grid").num_columns(3).spacing([4.0, 4.0]).show(ui, |ui| {
        for (i, k) in kws.iter().enumerate() {
            // an empty slot: an idle button, so the others keep their ⌥ keys
            if k.is_empty() {
                let r = ui.add_enabled_ui(false, |ui| ui.add_sized([bw, 22.0], egui::Button::new(""))).inner;
                register(ui.ctx(), format!("kwSetEmpty:{}", i + 1), r.rect);
                if i % 3 == 2 {
                    ui.end_row();
                }
                continue;
            }
            let chip = chips.iter().find(|c| lcc_catalog::keywords::same(&c.path, &lcc_catalog::keywords::clean(k)));
            let on = chip.is_some_and(|c| c.on_all());
            let some = chip.is_some() && !on;
            let short = format!("{}{}", k.rsplit('|').next().unwrap_or(k), if some { " *" } else { "" });
            let r = ui
                .add_sized(
                    [bw, 22.0],
                    egui::Button::new(egui::RichText::new(short).size(11.5).color(if on { t.text } else { t.text_label })).selected(on).truncate(),
                )
                .on_hover_text(format!("{} — ⌥{}", k.replace('|', " › "), i + 1));
            register(ui.ctx(), format!("kwSet:{}", i + 1), r.rect);
            if on {
                register(ui.ctx(), format!("kwSetOn:{}", i + 1), r.rect);
            } else if some {
                register(ui.ctx(), format!("kwSetSome:{}", i + 1), r.rect);
            }
            if r.clicked() {
                let _ = app.run("keyword.toggleFromSet", json!({"index": i + 1}));
            }
            if i % 3 == 2 {
                ui.end_row();
            }
        }
    });
}

fn versions(app: &mut LccApp, ui: &mut egui::Ui, id: PhotoId) {
    header(ui, "Versions");
    versions_body(app, ui, id);
}

/// The Versions rows without the right column's own header: the fork's Develop column draws them
/// under its Classic "Versions" header (see `panels::classic_left`).
pub(crate) fn versions_body(app: &mut LccApp, ui: &mut egui::Ui, id: PhotoId) {
    let Some(p) = app.session.catalog.photo(id).cloned() else { return };
    let t = Tokens::get(ui.ctx());
    padded(ui, |ui| {
        if text_button(ui, "versionCreate", crate::i18n::tr("Create Version"), false).clicked() {
            let _ = app.run("version.create", json!({}));
        }
        ui.add_space(8.0);
        // Named (made by you) and Auto (made by the app) versions
        let tab_id = egui::Id::new("versions-tab");
        let mut auto: bool = ui.data(|d| d.get_temp(tab_id)).unwrap_or(false);
        let named_n = p.versions.iter().filter(|v| !v.auto).count();
        let items = [("Named", "named"), ("Auto", "auto")];
        let labels =
            [crate::i18n::tr_format!("Named ({named_n})", named_n = named_n), crate::i18n::tr_format!("Auto ({})", p.versions.len() - named_n)];
        let items: Vec<(&str, &str)> = items.iter().zip(&labels).map(|((_, k), l)| (l.as_str(), *k)).collect();
        if let Some(i) = crate::widgets::segmented(ui, "versionsTab", &items, Some(auto as usize), 2) {
            auto = i == 1;
            ui.data_mut(|d| d.insert_temp(tab_id, auto));
        }
        ui.add_space(6.0);
        let shown: Vec<(usize, &lcc_catalog::Version)> = p.versions.iter().enumerate().filter(|(_, v)| v.auto == auto).collect();
        if shown.is_empty() {
            let msg = if auto { "No automatic versions." } else { "No versions yet. Create one to keep this look." };
            ui.label(egui::RichText::new(crate::i18n::tr(msg)).color(t.text_dim));
        }
        // newest first
        for (i, v) in shown.into_iter().rev() {
            let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 58.0), Sense::click());
            register(ui.ctx(), format!("version:{i}"), r);
            let current = *v.settings == *p.develop;
            if resp.hovered() {
                ui.painter().rect_filled(r, 4.0, t.hover.gamma_multiply(0.7));
            }
            // thumbnail of the version's look
            let tr = Rect::from_min_size(r.min + vec2(4.0, 5.0), vec2(72.0, 48.0));
            ui.painter().rect_filled(tr, 2.0, t.canvas);
            if let Some(job) = app.session.variant_job(id, &v.settings, 160)
                && let Some(tex) = app.renderer.variant(job)
            {
                ui.painter().image(tex.tex.id(), tr, crate::panels::presets::cover_uv(tr, tex.size), egui::Color32::WHITE);
            }
            let x = tr.right() + 10.0;
            ui.painter().with_clip_rect(r.with_max_x(r.right() - 24.0)).text(
                pos2(x, r.top() + 18.0),
                Align2::LEFT_CENTER,
                &v.name,
                t.semibold(13.0),
                t.text,
            );
            let painter = ui.painter().with_clip_rect(r);
            painter.text(pos2(x, r.top() + 38.0), Align2::LEFT_CENTER, short_time(&v.created), t.font(11.0), t.text_dim);
            if current {
                paint(ui.painter(), Rect::from_center_size(pos2(r.right() - 14.0, r.center().y), vec2(14.0, 14.0)), Icon::Check, t.accent);
            }
            // resting on a version shows it in the loupe (nothing is changed)
            if resp.hovered() && !current {
                app.hover_preview =
                    Some(crate::HoverPreview { label: crate::i18n::tr_format!("Version: {}", v.name), settings: (*v.settings).clone() });
            }
            if resp.double_clicked() {
                crate::panels::dialogs::prompt(app, "Rename Version", "Version name", &v.name, "version.rename", json!({"index": i}), "name");
            } else if resp.clicked() && !current {
                let _ = app.run("version.restore", json!({"index": i}));
            }
            resp.on_hover_text(crate::i18n::tr(if current { "The photo has these settings" } else { "Click to restore · double-click to rename" }))
                .context_menu(|ui| {
                    if ui.button(crate::i18n::tr("Restore")).clicked() {
                        let _ = app.run("version.restore", json!({"index": i}));
                    }
                    if ui.button(crate::i18n::tr("Update with Current Settings")).clicked() {
                        let _ = app.run("version.update", json!({"index": i}));
                    }
                    if ui.button(crate::i18n::tr("Rename…")).clicked() {
                        crate::panels::dialogs::prompt(app, "Rename Version", "Version name", &v.name, "version.rename", json!({"index": i}), "name");
                    }
                    if ui.button(crate::i18n::tr("Set as Before")).clicked() {
                        let _ = app.run("beforeAfter.setBefore", json!({"source": "version", "name": v.name}));
                    }
                    ui.separator();
                    if ui.button(crate::i18n::tr("Delete")).clicked() {
                        let _ = app.run("version.delete", json!({"index": i}));
                    }
                });
        }
    });
}

/// "Sep 30, 2026, 12:00 PM" from an ISO time.
fn short_time(iso: &str) -> String {
    // Other languages use their own date patterns (`i18n::display_time`).
    if crate::i18n::language() != crate::i18n::Locale::En {
        return crate::i18n::display_time(iso);
    }
    let long = lcc_catalog::dates::display_time(iso);
    // "September 30, 2026 at 12:00:00 PM" → month abbreviated, seconds dropped
    let (date, time) = long.split_once(" at ").map_or((long.as_str(), None), |(d, t)| (d, Some(t)));
    let date = match date.split_once(' ') {
        Some((m, rest)) => format!("{} {rest}", m.chars().take(3).collect::<String>()),
        None => date.to_string(),
    };
    match time.and_then(|t| t.rsplit_once(' ')) {
        Some((hms, ampm)) => format!("{date}, {} {ampm}", hms.rsplit_once(':').map_or(hms, |x| x.0)),
        None => date,
    }
}

/// How a History row is drawn: the step the settings are at is highlighted, the steps after it
/// (still there to return to until a new edit) are greyed out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HistoryRow {
    Done,
    Current,
    Undone,
}

fn history_row(i: usize, cursor: Option<usize>) -> HistoryRow {
    match cursor {
        Some(c) if i == c => HistoryRow::Current,
        Some(c) if i > c => HistoryRow::Undone,
        _ => HistoryRow::Done,
    }
}

fn activity(app: &mut LccApp, ui: &mut egui::Ui, id: PhotoId) {
    header(ui, "History");
    history_body(app, ui, id);
}

/// The History rows without the right column's own header: the fork's Develop column draws them
/// under its Classic "History" header (see `panels::classic_left`).
pub(crate) fn history_body(app: &mut LccApp, ui: &mut egui::Ui, id: PhotoId) {
    let Some(p) = app.session.catalog.photo(id).cloned() else { return };
    let t = Tokens::get(ui.ctx());
    padded(ui, |ui| {
        if p.history.is_empty() {
            ui.label(egui::RichText::new(crate::i18n::tr("No edits yet.")).color(t.text_dim));
        }
        let cursor = p.history_index();
        for (i, h) in p.history.iter().enumerate().rev() {
            let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::click());
            register(ui.ctx(), format!("history:{i}"), r);
            let row = history_row(i, cursor);
            if row == HistoryRow::Current {
                ui.painter().rect_filled(r, 3.0, t.pressed);
            } else if resp.hovered() {
                ui.painter().rect_filled(r, 3.0, t.hover);
            }
            let (icon, text) = if row == HistoryRow::Undone { (t.text_disabled, t.text_disabled) } else { (t.icon, t.text_label) };
            paint(ui.painter(), Rect::from_min_size(r.min + vec2(0.0, 4.0), vec2(16.0, 16.0)), Icon::Clock, icon);
            ui.painter().text(
                pos2(r.left() + 24.0, r.center().y),
                Align2::LEFT_CENTER,
                crate::i18n::history_label(&h.label, &app.session.presets),
                t.font(12.5),
                text,
            );
            if resp.clicked() {
                let _ = app.run("history.restore", json!({"index": i}));
            }
            resp.context_menu(|ui| {
                if ui.button(crate::i18n::tr("Copy History Step Settings to Before")).clicked() {
                    let _ = app.run("beforeAfter.setBefore", json!({"source": "history", "index": i}));
                }
                if ui.button(crate::i18n::tr("Create Version from Step")).clicked() {
                    let _ = app.run("history.restore", json!({"index": i}));
                    let _ = app.run("version.create", json!({"name": h.label}));
                }
                ui.separator();
                if ui.button(crate::i18n::tr("Clear History")).clicked() {
                    let _ = app.run("history.clear", json!({}));
                }
            });
        }
    });
}

/// A panel icon button for `label` (translated: the accessible name), its hover text the label plus
/// `command`'s current shortcut. Addressable as `button:{id}` (as the text button it replaced was)
/// and `icon:{id}`.
fn cmd_icon(app: &LccApp, ui: &mut egui::Ui, id: &str, icon: Icon, label: &str, command: Option<&str>, active: bool) -> egui::Response {
    let sc = command.and_then(|c| crate::shortcuts::shortcut_of(&app.ui.settings.keymap, c));
    let tip = crate::widgets::command_tooltip(ui, label, sc);
    let r = crate::widgets::icon_button_named(ui, id, icon, vec2(28.0, 24.0), active, true, crate::i18n::tr(label), &tip);
    register(ui.ctx(), format!("button:{id}"), r.rect);
    r
}

#[cfg(test)]
mod tests {
    // Feature: the History panel greys out the steps after the cursor
    #[test]
    fn history_rows_after_the_cursor_are_greyed() {
        use super::{HistoryRow, history_row};
        let rows: Vec<_> = (0..4).map(|i| history_row(i, Some(1))).collect();
        assert_eq!(rows, [HistoryRow::Done, HistoryRow::Current, HistoryRow::Undone, HistoryRow::Undone]);
        assert_eq!(history_row(3, Some(3)), HistoryRow::Current, "at the latest step nothing is greyed");
        assert_eq!(history_row(0, None), HistoryRow::Done);
    }

    // Feature: the aspect button tells what is locked
    #[test]
    fn aspect_label_names_presets_in_either_orientation() {
        assert_eq!(super::aspect_label(None, Some(1.5)), "Free");
        assert_eq!(super::aspect_label(Some((1600, 900)), None), "16 × 9");
        assert_eq!(super::aspect_label(Some((900, 1600)), None), "16 × 9", "portrait 16:9");
        assert_eq!(super::aspect_label(Some((400, 500)), None), "4 × 5 / 8 × 10");
    }

    #[test]
    fn aspect_label_prefers_original_over_a_coinciding_preset_and_never_shows_pixel_sizes() {
        assert_eq!(super::aspect_label(Some((300, 200)), Some(1.5)), "Original");
        assert_eq!(super::aspect_label(Some((600_000, 400_000)), Some(1.5)), "Original");
        assert_eq!(super::aspect_label(Some((137, 100)), Some(1.5)), "1.37 : 1");
        assert_eq!(super::aspect_label(Some((0, 5)), None), "Free");
    }

    #[test]
    fn short_times() {
        assert_eq!(super::short_time("2026-09-30T12:00:05"), "Sep 30, 2026, 12:00 PM");
        assert_eq!(super::short_time("2026-01-02"), "Jan 2, 2026");
    }
}
