//! The Masking panel: create masks, list them, edit components and local adjustments.

use egui::{Rect, Sense, Stroke, pos2, vec2};
use lcc_catalog::PhotoId;
use lcc_develop::{ControlSpec, LocalAdjustments, MaskShape, Section, Track};
use serde_json::json;

use super::edit::apply_slider_out;
use crate::LccApp;
use crate::icons::{Icon, paint};
use crate::theme::Tokens;
use crate::widgets::{divider, flyout_row, icon_button, one_line, register, slider, text_button};

const fn spec(id: &'static str, label: &'static str, min: f64, max: f64, step: f64, decimals: u8, track: Track) -> ControlSpec {
    ControlSpec { id, label, section: Section::Light, min, max, default: 0.0, step, decimals, track }
}

/// Local adjustment sliders (key = `LocalAdjustments` field name).
pub const LOCAL: &[ControlSpec] = &[
    spec("temp", "Temp", -100.0, 100.0, 1.0, 0, Track::Temp),
    spec("tint", "Tint", -100.0, 100.0, 1.0, 0, Track::Tint),
    spec("exposure", "Exposure", -4.0, 4.0, 0.01, 2, Track::Centered),
    spec("contrast", "Contrast", -100.0, 100.0, 1.0, 0, Track::Centered),
    spec("highlights", "Highlights", -100.0, 100.0, 1.0, 0, Track::Centered),
    spec("shadows", "Shadows", -100.0, 100.0, 1.0, 0, Track::Centered),
    spec("whites", "Whites", -100.0, 100.0, 1.0, 0, Track::Centered),
    spec("blacks", "Blacks", -100.0, 100.0, 1.0, 0, Track::Centered),
    spec("texture", "Texture", -100.0, 100.0, 1.0, 0, Track::Centered),
    spec("clarity", "Clarity", -100.0, 100.0, 1.0, 0, Track::Centered),
    spec("dehaze", "Dehaze", -100.0, 100.0, 1.0, 0, Track::Centered),
    spec("hue", "Hue", -100.0, 100.0, 1.0, 0, Track::Rainbow),
    spec("saturation", "Saturation", -100.0, 100.0, 1.0, 0, Track::Gradient { from: "#7a7a7a", to: "#e04a3a" }),
    spec("sharpness", "Sharpness", -100.0, 100.0, 1.0, 0, Track::Centered),
    spec("noise", "Noise", -100.0, 100.0, 1.0, 0, Track::Centered),
    spec("moire", "Moiré", -100.0, 100.0, 1.0, 0, Track::Centered),
    spec("defringe", "Defringe", -100.0, 100.0, 1.0, 0, Track::Centered),
];

/// The mask's slider sections (issue #642), as Lightroom Classic groups them: (fold key, title,
/// the [`LOCAL`] sliders in it). Color Grading follows Color ([`grading_section`]).
pub const GROUPS: &[(&str, &str, &[&str])] = &[
    ("light", "Light", &["exposure", "contrast", "highlights", "shadows", "whites", "blacks"]),
    ("color", "Color", &["temp", "tint", "hue", "saturation"]),
    ("effects", "Effects", &["texture", "clarity", "dehaze"]),
    ("detail", "Detail", &["sharpness", "noise", "moire", "defringe"]),
];

/// The colour-grading zones of a mask: (key in the `grade_<key>_*` fields, label).
pub const GRADE_ZONES: [(&str, &str); 4] = [("shadows", "Shadows"), ("midtones", "Midtones"), ("highlights", "Highlights"), ("global", "Global")];

/// The sliders of one colour-grading zone (ids are the `LocalAdjustments` field names).
pub fn grade_specs(zone: &str) -> [ControlSpec; 3] {
    let (h, s, l): (&'static str, &'static str, &'static str) = match zone {
        "midtones" => ("grade_midtones_hue", "grade_midtones_sat", "grade_midtones_lum"),
        "highlights" => ("grade_highlights_hue", "grade_highlights_sat", "grade_highlights_lum"),
        "global" => ("grade_global_hue", "grade_global_sat", "grade_global_lum"),
        _ => ("grade_shadows_hue", "grade_shadows_sat", "grade_shadows_lum"),
    };
    let c = |id, label, min, max, track| ControlSpec { id, label, section: Section::Grading, min, max, default: 0.0, step: 1.0, decimals: 0, track };
    [c(h, "Hue", 0.0, 360.0, Track::Rainbow), c(s, "Saturation", 0.0, 100.0, Track::Plain), c(l, "Luminance", -100.0, 100.0, Track::Centered)]
}

/// Does any slider of a section differ from 0?
fn group_changed(a: &LocalAdjustments, ids: &[&str]) -> bool {
    ids.iter().any(|k| local_get(a, k).abs() > 1e-9)
}

/// A mask slider section's title row: click folds it shut and open again (`section:mask:<key>`, kept
/// in [`crate::state::UiState::folded_sections`], so open unless folded). A dot after the title
/// marks a section whose sliders are not all at 0, so a folded one still shows it does something.
/// Says whether the section is open.
fn section_header(app: &mut LccApp, ui: &mut egui::Ui, key: &str, title: &str, changed: bool) -> bool {
    let t = Tokens::get(ui.ctx());
    let id = format!("mask:{key}");
    let open = !app.ui.section_folded(&id);
    let title = crate::i18n::tr(title);
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::CollapsingHeader, true, open, title));
    register(ui.ctx(), format!("section:{id}"), r);
    let p = ui.painter();
    if resp.hovered() {
        p.rect_filled(r, 0.0, t.chrome.gamma_multiply(1.06));
    }
    let chevron = Rect::from_center_size(pos2(r.left() + 20.0, r.center().y), vec2(12.0, 12.0));
    paint(p, chevron, if open { Icon::ChevronDown } else { Icon::ChevronRight }, t.icon);
    let g = p.layout_no_wrap(title.to_string(), t.semibold(13.0), t.text_label);
    let at = pos2(r.left() + 32.0, r.center().y - g.size().y / 2.0);
    let end = at.x + g.size().x;
    p.galley(at, g, t.text_label);
    if changed {
        p.circle_filled(pos2(end + 8.0, r.center().y), 2.5, t.accent);
    }
    if resp.clicked() {
        app.ui.toggle_folded(&id);
    }
    open
}

/// The Color Grading section inside a mask (issue #642): one zone at a time (Shadows, Midtones,
/// Highlights, Global, picked by the buttons), its Hue, Saturation and Luminance.
fn grading_section(app: &mut LccApp, ui: &mut egui::Ui, a: &LocalAdjustments) {
    let ids: Vec<&str> = GRADE_ZONES.iter().flat_map(|(z, _)| grade_specs(z).map(|s| s.id)).collect();
    if !section_header(app, ui, "grading", "Color Grading", group_changed(a, &ids)) {
        return;
    }
    let zone_id = egui::Id::new("maskGradeZone");
    let zone: &str = ui.ctx().data(|d| d.get_temp::<usize>(zone_id)).and_then(|i| GRADE_ZONES.get(i)).map_or("shadows", |(z, _)| z);
    egui::Frame::NONE.inner_margin(egui::Margin { left: 16, right: 16, top: 2, bottom: 6 }).show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            for (i, (z, label)) in GRADE_ZONES.iter().enumerate() {
                if text_button(ui, &format!("maskGradeZone:{z}"), label, *z == zone).clicked() {
                    ui.ctx().data_mut(|d| d.insert_temp(zone_id, i));
                }
            }
        });
    });
    for s in grade_specs(zone) {
        let v = local_get(a, s.id);
        let out = slider(ui, &s, v, true, None);
        apply_slider_out(app, &s, out, |app, v| app.run("mask.adjust", json!({"values": {s.id: v}})));
    }
}

pub fn local_get(a: &LocalAdjustments, key: &str) -> f64 {
    serde_json::to_value(a).ok().and_then(|v| v.get(key).and_then(|x| x.as_f64())).unwrap_or(0.0)
}

/// The icon of a mask kind, by its `mask.add` kind id: the one table the Create New Mask tiles and
/// the mask list share.
fn kind_icon(kind: &str) -> Icon {
    match kind {
        "brush" => Icon::Brush,
        "linear" => Icon::Linear,
        "radial" => Icon::Radial,
        "colorRange" => Icon::Picker,
        "luminanceRange" => Icon::Luminance,
        "depthRange" => Icon::Depth,
        "subject" => Icon::Subject,
        "sky" => Icon::Sky,
        "background" => Icon::Background,
        "object" => Icon::Object,
        "prompt" => Icon::Describe,
        "people" => Icon::People,
        "landscape" => Icon::Landscape,
        _ => Icon::Mask,
    }
}

fn kind_label(s: &MaskShape) -> (&'static str, Icon) {
    let (kind, label) = match s {
        MaskShape::Brush { .. } => ("brush", "Brush"),
        MaskShape::Linear { .. } => ("linear", "Linear Gradient"),
        MaskShape::Radial { .. } => ("radial", "Radial Gradient"),
        MaskShape::ColorRange { .. } => ("colorRange", "Color Range"),
        MaskShape::LuminanceRange { .. } => ("luminanceRange", "Luminance Range"),
        MaskShape::DepthRange { .. } => ("depthRange", "Depth Range"),
        MaskShape::Subject => ("subject", "Subject"),
        MaskShape::Sky => ("sky", "Sky"),
        MaskShape::Background => ("background", "Background"),
        MaskShape::Object { .. } => ("object", "Object"),
        MaskShape::Prompt { .. } => ("prompt", "Describe"),
        MaskShape::People { .. } => ("people", "People"),
        MaskShape::Landscape { .. } => ("landscape", "Landscape"),
    };
    (label, kind_icon(kind))
}

const TILE: f32 = 52.0;
const TILE_GAP: f32 = 6.0;
/// Room left and right of a tile's label.
const TILE_PAD: f32 = 3.0;

/// Columns, tile width and label size of the Create New Mask grid in `width`, `label(size)` being
/// the widest label at a font size: four tiles of up to 52 pt (at least 48), else three, else two —
/// the first that fits the labels at 10.5 pt, or at 9.5 pt before a column is dropped. A tile grows
/// past 52 pt for a long (translated) label. In a panel too narrow even for that, two tiles whose
/// labels are cut short.
fn tile_layout(width: f32, label: impl Fn(f32) -> f32) -> (usize, f32, f32) {
    let room = |n: usize| ((width - (n as f32 - 1.0) * TILE_GAP) / n as f32).floor();
    for n in [4, 3, 2] {
        let w = room(n);
        if n == 4 && w < 48.0 {
            continue;
        }
        for size in [10.5, 9.5] {
            let need = (label(size) + 2.0 * TILE_PAD).ceil();
            if need <= w {
                return (n, w.min(TILE.max(need)), size);
            }
        }
    }
    (2, room(2).max(24.0), 9.5)
}

pub fn show(app: &mut LccApp, ui: &mut egui::Ui, id: PhotoId) {
    let popup_open_at_start = egui::Popup::is_any_open(ui.ctx());
    let t = Tokens::get(ui.ctx());
    let d = app.session.develop_of(id).unwrap_or_default();
    egui::Frame::NONE.inner_margin(egui::Margin { left: 12, right: 16, top: 4, bottom: 10 }).show(ui, |ui| {
        ui.vertical_centered(|ui| {
            ui.label(egui::RichText::new(crate::i18n::tr("Create New Mask")).font(t.semibold(13.0)).color(t.text_label));
        });
        ui.add_space(6.0);
        let tiles: [(&str, &str); 10] = [
            ("object", "Object"),
            ("prompt", "Describe"),
            ("subject", "Subject"),
            ("sky", "Sky"),
            ("background", "Background"),
            ("brush", "Brush"),
            ("linear", "Linear"),
            ("radial", "Radial"),
            ("luminanceRange", "Luminance"),
            ("colorRange", "Color"),
        ];
        // four 52 pt tiles a row when they and their labels fit, else fewer (wider for a long label)
        let widest = |size: f32| {
            let font = t.font(size);
            tiles
                .iter()
                .map(|(_, l)| ui.painter().layout_no_wrap(crate::i18n::tr(l).to_string(), font.clone(), t.text_dim).size().x)
                .fold(0.0, f32::max)
        };
        let (cols, tile, size) = tile_layout(ui.available_width(), widest);
        let grid_w = cols as f32 * tile + (cols as f32 - 1.0) * TILE_GAP;
        let pad = ((ui.available_width() - grid_w) / 2.0).max(0.0);
        ui.horizontal(|ui| {
            ui.add_space(pad);
            egui::Grid::new("mask-tiles").spacing(vec2(TILE_GAP, TILE_GAP)).show(ui, |ui| {
                for (i, (kind, label)) in tiles.iter().enumerate() {
                    let (r, resp) = ui.allocate_exact_size(vec2(tile, 52.0), Sense::click());
                    crate::widgets::announce(&resp, crate::i18n::tr(label));
                    register(ui.ctx(), format!("maskNew:{kind}"), r);
                    ui.painter().rect_filled(r, 4.0, if resp.hovered() { t.hover } else { t.inset });
                    paint(ui.painter(), Rect::from_center_size(r.center() - vec2(0.0, 7.0), vec2(20.0, 20.0)), kind_icon(kind), t.text_label);
                    // (cut short only in a panel too narrow for the label: the full name on hover)
                    let label = crate::i18n::tr(label);
                    let g = one_line(ui.painter(), label, t.font(size), t.text_dim, tile - 2.0 * TILE_PAD);
                    let elided = g.elided;
                    let at = pos2(r.center().x, r.bottom() - 9.0) - g.size() / 2.0;
                    register(ui.ctx(), format!("maskNewLabel:{kind}"), Rect::from_min_size(at, g.size()));
                    ui.painter().galley(at, g, t.text_dim);
                    let resp = if elided { resp.on_hover_text(label) } else { resp };
                    if resp.clicked() {
                        match *kind {
                            "colorRange" => {
                                // an empty colour range; clicking the photo samples it
                                let _ = app.run("mask.add", json!({"kind": "colorRange"}));
                                app.ui.tool = "colorRange".into();
                                app.toast(ui.ctx(), crate::i18n::tr("Click the photo to pick a colour · ⇧-click adds more"));
                            }
                            "object" => start_object(app, ui.ctx(), "new"),
                            "prompt" => start_describe(app, "new"),
                            "brush" => {
                                let _ = app.run("tool.brush", json!({"new": true}));
                            }
                            "linear" | "radial" => {
                                app.ui.tool = kind.to_string();
                                let _ = app.run("mask.add", json!({"kind": kind}));
                            }
                            k => {
                                let _ = app.run("mask.add", json!({"kind": k}));
                            }
                        }
                    }
                    if i % cols == cols - 1 {
                        ui.end_row();
                    }
                }
            });
        });
        describe_field(app, ui, true);
        let seg = &app.session.segmenter;
        let status = if seg.analyzing() {
            Some("Analyzing the photo for AI masks…")
        } else if seg.busy() {
            Some("Selecting…")
        } else if seg.detail_busy() || app.ui.detail_due.is_some() {
            Some("Refining the mask's detail…")
        } else {
            None
        };
        // one line kept for the status whether or not there is one: the AI masks' background work must not move the
        // controls under it (one line, cut short when narrow, so it never grows either)
        ui.add_space(4.0);
        let line = ui.text_style_height(&egui::TextStyle::Body);
        let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), line), egui::Sense::hover());
        if let Some(status) = status {
            let label = egui::Label::new(egui::RichText::new(crate::i18n::tr(status)).color(t.text_dim)).truncate();
            ui.put(rect, |ui: &mut egui::Ui| {
                ui.horizontal(|ui| {
                    crate::progress::spinner(ui, 14.0);
                    ui.add(label)
                })
                .response
            });
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
        }
        // the model download, while its dialog is closed
        let download = seg.download_status();
        if download.running && app.ui.dialog.is_none() {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                // the bar takes what the Details button leaves
                let details = ui.spacing().interact_size.x + 30.0;
                sam_download_bar(ui, download.done, download.total, (ui.available_width() - details).max(96.0));
                if text_button(ui, "maskSamDetails", crate::i18n::tr("Details"), false).clicked() {
                    app.offer_sam_download(None);
                }
            });
        }
        // the active mask's Amount, right under the tiles (as Camera Raw / Lightroom Classic place it)
        if let Some((index, m)) = d.masks.iter().enumerate().find(|(_, m)| app.session.active_mask == Some(m.id)) {
            ui.add_space(6.0);
            amount_slider(app, ui, index, m);
        }
    });
    divider(ui);
    let mut hovered_row = None;
    // mask list
    egui::Frame::NONE.inner_margin(egui::Margin { left: 16, right: 16, top: 8, bottom: 8 }).show(ui, |ui| {
        if d.masks.is_empty() {
            ui.label(egui::RichText::new(crate::i18n::tr("No masks yet. Choose a mask type above.")).color(t.text_dim));
        }
        let count = d.masks.len();
        for (index, m) in d.masks.iter().enumerate() {
            let sel = app.session.active_mask == Some(m.id);
            if let Some((rid, name)) = app.ui.renaming_mask.as_mut().filter(|(rid, _)| *rid == m.id) {
                // inline rename: Enter (or leaving the field) commits, Escape cancels
                let rid = *rid;
                let r = ui.add(egui::TextEdit::singleline(name).desired_width(ui.available_width()).id_salt(("maskRename", rid)));
                register(ui.ctx(), format!("maskRename:{rid}"), r.rect);
                if !r.has_focus() && !r.lost_focus() {
                    r.request_focus();
                }
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    app.ui.renaming_mask = None;
                } else if r.lost_focus() {
                    let name = name.trim().to_string();
                    app.ui.renaming_mask = None;
                    if !name.is_empty() && name != m.name {
                        let _ = app.run("mask.rename", json!({"id": rid, "name": name}));
                    }
                }
                continue;
            }
            let collapsed = app.ui.is_mask_collapsed(m.id);
            let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::click());
            crate::widgets::announce_selected(&resp, m.name.clone(), Some(m.id) == app.session.active_mask);
            register(ui.ctx(), format!("mask:{}", m.id), r);
            if ui.rect_contains_pointer(r) {
                hovered_row = Some(m.id);
            }
            ui.painter().rect_filled(
                r,
                4.0,
                if sel {
                    t.tool_active
                } else if resp.hovered() {
                    t.hover.gamma_multiply(0.7)
                } else {
                    t.chrome
                },
            );

            // Collapse / expand chevron on the left
            let chev_r = Rect::from_center_size(pos2(r.left() + 10.0, r.center().y), vec2(14.0, 14.0));
            register(ui.ctx(), format!("maskCollapse:{}", m.id), chev_r);
            register(ui.ctx(), format!("button:maskCollapse:{}", m.id), chev_r);
            let chev_resp = ui.interact(chev_r, egui::Id::new(("maskCollapse", m.id)), Sense::click());
            paint(
                ui.painter(),
                chev_r.shrink(2.0),
                if collapsed { Icon::ChevronRight } else { Icon::ChevronDown },
                if chev_resp.hovered() { t.text } else { t.text_dim },
            );
            if chev_resp.clicked() {
                app.ui.toggle_mask_collapsed(m.id);
            }

            let icon = m.components.first().map(|c| kind_label(&c.shape).1).unwrap_or(Icon::Mask);
            paint(ui.painter(), Rect::from_min_size(pos2(r.left() + 22.0, r.min.y + 7.0), vec2(16.0, 16.0)), icon, t.text_label);
            // a long name is cut short (with …) before the right buttons, the full name on hover
            let color = if m.visible { t.text } else { t.text_disabled };
            let right_room = if sel || ui.rect_contains_pointer(r) { 92.0 } else { 28.0 };
            let g = one_line(ui.painter(), &m.name, t.font(13.0), color, r.width() - 44.0 - right_room);
            let elided = g.elided;
            let at = pos2(r.left() + 44.0, r.center().y - g.size().y / 2.0);
            register(ui.ctx(), format!("maskName:{}", m.id), Rect::from_min_size(at, g.size()));
            ui.painter().galley(at, g, color);
            let resp = if elided { resp.on_hover_text(&m.name) } else { resp };

            let right = r.right();
            let cy = r.center().y;

            // show / hide on hover (and always while hidden)
            let mut eye_clicked = false;
            if ui.rect_contains_pointer(r) || !m.visible {
                let er = Rect::from_center_size(pos2(right - 14.0, cy), vec2(22.0, 22.0));
                register(ui.ctx(), format!("maskVisible:{}", m.id), er);
                let eye = ui.interact(er, egui::Id::new(("maskVisible", m.id)), Sense::click());
                crate::widgets::announce_selected(&eye, crate::i18n::tr("Show mask"), m.visible);
                paint(
                    ui.painter(),
                    er.shrink(3.0),
                    if m.visible { Icon::Eye } else { Icon::EyeOff },
                    if eye.hovered() { t.text } else { t.text_dim },
                );
                if eye.clicked() {
                    eye_clicked = true;
                    let _ = app.run("mask.visible", json!({"id": m.id}));
                }
            }

            // delete only on hover over the parent mask (issue #642, #694)
            let mut bin_clicked = false;
            if ui.rect_contains_pointer(r) {
                let br = Rect::from_center_size(pos2(right - 36.0, cy), vec2(22.0, 22.0));
                register(ui.ctx(), format!("maskDeleteRow:{}", m.id), br);
                register(ui.ctx(), "button:maskDelete", br);
                register(ui.ctx(), format!("button:maskDeleteRow:{}", m.id), br);
                let bin = ui.interact(br, egui::Id::new(("maskDeleteRow", m.id)), Sense::click());
                crate::widgets::announce(&bin, crate::i18n::tr("Delete Mask"));
                paint(ui.painter(), br.shrink(3.0), Icon::Trash, if bin.hovered() { t.text } else { t.text_dim });
                let bin = bin.on_hover_text(crate::i18n::tr("Delete Mask"));
                if bin.clicked() {
                    bin_clicked = true;
                    let _ = app.run("mask.delete", json!({"id": m.id}));
                }
            }

            // minus button inline with parent mask (issue #694)
            let mut minus_clicked = false;
            if sel || ui.rect_contains_pointer(r) {
                let mr = Rect::from_center_size(pos2(right - 58.0, cy), vec2(20.0, 20.0));
                register(ui.ctx(), format!("maskMinus:{}", m.id), mr);
                register(ui.ctx(), format!("button:maskMinus:{}", m.id), mr);
                register(ui.ctx(), format!("button:maskMinus{}", m.id), mr);
                let minus = ui.interact(mr, egui::Id::new(("maskMinus", m.id)), Sense::click());
                crate::widgets::announce(&minus, crate::i18n::tr("Subtract from this mask"));
                paint(ui.painter(), mr.shrink(3.0), Icon::Minus, if minus.hovered() { t.text } else { t.text_dim });
                let minus = minus.on_hover_text(crate::i18n::tr("Subtract from this mask"));
                if minus.clicked() {
                    minus_clicked = true;
                    if !sel {
                        let _ = app.run("mask.select", json!({"id": m.id}));
                    }
                    app.ui.collapsed_masks.remove(&m.id);
                }
                egui::Popup::menu(&minus).show(|ui| component_menu(app, ui, "subtract"));
            }

            // plus button inline with parent mask (issue #694)
            let mut plus_clicked = false;
            if sel || ui.rect_contains_pointer(r) {
                let pr = Rect::from_center_size(pos2(right - 78.0, cy), vec2(20.0, 20.0));
                register(ui.ctx(), format!("maskPlus:{}", m.id), pr);
                register(ui.ctx(), format!("button:maskPlus:{}", m.id), pr);
                register(ui.ctx(), format!("button:maskPlus{}", m.id), pr);
                let plus = ui.interact(pr, egui::Id::new(("maskPlus", m.id)), Sense::click());
                crate::widgets::announce(&plus, crate::i18n::tr("Add to this mask"));
                paint(ui.painter(), pr.shrink(3.0), Icon::Plus, if plus.hovered() { t.text } else { t.text_dim });
                let plus = plus.on_hover_text(crate::i18n::tr("Add to this mask"));
                if plus.clicked() {
                    plus_clicked = true;
                    if !sel {
                        let _ = app.run("mask.select", json!({"id": m.id}));
                    }
                    app.ui.collapsed_masks.remove(&m.id);
                }
                egui::Popup::menu(&plus).show(|ui| component_menu(app, ui, "add"));
            }

            if resp.clicked() && !eye_clicked && !bin_clicked && !minus_clicked && !plus_clicked && !chev_resp.clicked() && !popup_open_at_start {
                let _ = app.run("mask.select", json!({"id": m.id}));
                if app.ui.active_mask_component.is_none() {
                    app.ui.active_mask_component = Some(0);
                }
            }
            if resp.double_clicked() {
                app.ui.renaming_mask = Some((m.id, m.name.clone()));
            }
            crate::widgets::context_menu(&resp, |ui| mask_menu(app, ui, m.id, &m.name, m.visible, index, count));

            if sel {
                describe_field(app, ui, false);
            }

            // Sub masks (components) directly under the parent mask (issue #694)
            if !collapsed {
                let cur_d = app.session.develop_of(id).unwrap_or_default();
                if let Some(cur_m) = cur_d.masks.iter().find(|x| x.id == m.id) {
                    if sel {
                        if app.ui.active_mask_component.is_none() && !cur_m.components.is_empty() {
                            app.ui.active_mask_component = Some(0);
                        } else if let Some(ci) = app.ui.active_mask_component
                            && ci >= cur_m.components.len()
                        {
                            app.ui.active_mask_component = cur_m.components.len().checked_sub(1);
                        }
                    }
                    for (i, c) in cur_m.components.iter().enumerate() {
                        let (kind, icon) = kind_label(&c.shape);
                        let label = c.name.clone().unwrap_or_else(|| match &c.shape {
                            MaskShape::Prompt { text, .. } => format!("“{text}”"),
                            _ => menu_label(kind).to_string(),
                        });
                        let comp_sel = sel && app.ui.active_mask_component == Some(i);
                        if let Some((_, _, name)) = app.ui.renaming_component.as_mut().filter(|(mid, k, _)| *mid == m.id && *k == i) {
                            let r =
                                ui.add(egui::TextEdit::singleline(name).desired_width(ui.available_width() - 30.0).id_salt(("compRename", m.id, i)));
                            register(ui.ctx(), format!("componentRename:{i}"), r.rect);
                            if !r.has_focus() && !r.lost_focus() {
                                r.request_focus();
                            }
                            if ui.input(|inp| inp.key_pressed(egui::Key::Escape)) {
                                app.ui.renaming_component = None;
                            } else if r.lost_focus() {
                                let name = name.trim().to_string();
                                app.ui.renaming_component = None;
                                if Some(&name) != c.name.as_ref() {
                                    let _ = app.run("mask.component", json!({"id": m.id, "component": i, "action": "rename", "name": name}));
                                }
                            }
                            continue;
                        }
                        ui.add_space(2.0);
                        let full_w = ui.available_width();
                        let indent = 20.0;
                        let (full_r, _) = ui.allocate_exact_size(vec2(full_w, 28.0), Sense::hover());
                        let r = Rect::from_min_size(pos2(full_r.min.x + indent, full_r.min.y), vec2(full_w - indent, 28.0));
                        let resp = ui.interact(r, egui::Id::new(("compRow", m.id, i)), Sense::click());
                        crate::widgets::announce_selected(&resp, label.clone(), comp_sel);
                        register(ui.ctx(), format!("component:{i}"), r);
                        ui.painter().rect_filled(
                            r,
                            4.0,
                            if comp_sel {
                                t.tool_active
                            } else if resp.hovered() {
                                t.hover.gamma_multiply(0.7)
                            } else {
                                t.chrome.gamma_multiply(0.9)
                            },
                        );

                        let op = match c.op {
                            lcc_develop::MaskOp::Add => "",
                            lcc_develop::MaskOp::Subtract => "− ",
                            lcc_develop::MaskOp::Intersect => "∩ ",
                        };
                        let icon_r = Rect::from_min_size(r.min + vec2(6.0, 6.0), vec2(16.0, 16.0));
                        paint(ui.painter(), icon_r, icon, if comp_sel { t.text } else { t.text_label });

                        let text = format!("{op}{label}{}", if c.invert { crate::i18n::tr(" (inverted)") } else { "" });
                        let g = one_line(ui.painter(), &text, t.font(12.5), if comp_sel { t.text } else { t.text_dim }, r.width() - 28.0 - 52.0);
                        let at = pos2(r.left() + 26.0, r.center().y - g.size().y / 2.0);
                        ui.painter().galley(at, g, if comp_sel { t.text } else { t.text_dim });

                        // Bin icon next to the three dots
                        let bin_r = Rect::from_center_size(pos2(r.right() - 36.0, r.center().y), vec2(20.0, 20.0));
                        register(ui.ctx(), format!("componentDelete:{i}"), bin_r);
                        register(ui.ctx(), format!("button:componentDelete:{i}"), bin_r);
                        register(ui.ctx(), format!("button:componentDelete{i}"), bin_r);
                        let comp_bin = ui.interact(bin_r, egui::Id::new(("componentDelete", m.id, i)), Sense::click());
                        crate::widgets::announce(&comp_bin, crate::i18n::tr("Delete component"));
                        paint(ui.painter(), bin_r.shrink(2.5), Icon::Trash, if comp_bin.hovered() { t.text } else { t.text_dim });
                        let comp_bin = comp_bin.on_hover_text(crate::i18n::tr("Delete component"));
                        if comp_bin.clicked() {
                            let _ = app.run("mask.component", json!({"id": m.id, "component": i, "action": "delete"}));
                            if app.ui.active_mask_component == Some(i) {
                                app.ui.active_mask_component = i.checked_sub(1);
                            }
                        }

                        // Options (three dots) button
                        let more_r = Rect::from_center_size(pos2(r.right() - 14.0, r.center().y), vec2(20.0, 20.0));
                        register(ui.ctx(), format!("button:componentMenu{i}"), more_r);
                        register(ui.ctx(), format!("button:componentMenu:{i}"), more_r);
                        let more = ui.interact(more_r, egui::Id::new(("componentMenu", m.id, i)), Sense::click());
                        let more = more.on_hover_text(crate::i18n::tr("Component options"));
                        let g_dots = ui.painter().layout_no_wrap("…".to_string(), t.font(13.0), if more.hovered() { t.text } else { t.text_dim });
                        let at_dots = pos2(more_r.center().x - g_dots.size().x / 2.0, more_r.center().y - g_dots.size().y / 2.0);
                        ui.painter().galley(at_dots, g_dots, if more.hovered() { t.text } else { t.text_dim });
                        egui::Popup::menu(&more).show(|ui| component_row_menu(app, ui, m.id, i, &label, cur_m.components.len()));

                        if resp.clicked() && !comp_bin.clicked() && !more.clicked() && !popup_open_at_start {
                            if !sel {
                                let _ = app.run("mask.select", json!({"id": m.id}));
                            }
                            app.ui.active_mask_component = Some(i);
                            match &c.shape {
                                MaskShape::Brush { .. } => app.ui.tool = "brush".into(),
                                MaskShape::Linear { .. } => app.ui.tool = "linear".into(),
                                MaskShape::Radial { .. } => app.ui.tool = "radial".into(),
                                MaskShape::ColorRange { .. } => app.ui.tool = "colorRange".into(),
                                MaskShape::LuminanceRange { .. } => app.ui.tool = "luminanceRange".into(),
                                MaskShape::DepthRange { .. } => app.ui.tool = "depthRange".into(),
                                MaskShape::Object { .. } => app.ui.tool = "object".into(),
                                MaskShape::Prompt { .. } => app.ui.tool = "prompt".into(),
                                _ => {}
                            }
                        }
                        if resp.double_clicked() {
                            app.ui.renaming_component = Some((m.id, i, label.clone()));
                        }
                        crate::widgets::context_menu(&resp, |ui| component_row_menu(app, ui, m.id, i, &label, cur_m.components.len()));

                        range_controls(app, ui, i, &c.shape);
                    }

                    // Core combination action bar and refinement sliders for the active mask
                    if sel {
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            let total_w = 4.0 * 32.0 + 3.0 * 4.0;
                            let pad = ((ui.available_width() - total_w) / 2.0).max(0.0);
                            ui.add_space(pad);
                            ui.spacing_mut().item_spacing.x = 4.0;
                            let add = mask_icon(ui, "maskAddComp", Icon::MaskAdd, "Add", false);
                            egui::Popup::menu(&add).show(|ui| component_menu(app, ui, "add"));
                            let sub = mask_icon(ui, "maskSubComp", Icon::MaskSubtract, "Subtract", false);
                            egui::Popup::menu(&sub).show(|ui| component_menu(app, ui, "subtract"));
                            let int = mask_icon(ui, "maskIntComp", Icon::MaskIntersect, "Intersect", false);
                            egui::Popup::menu(&int).show(|ui| component_menu(app, ui, "intersect"));
                            if mask_icon(ui, "maskInvert", Icon::MaskInvert, "Invert", cur_m.invert).clicked() {
                                let _ = app.run("mask.invert", json!({}));
                            }
                        });
                        ui.add_space(6.0);
                        let refine = ControlSpec {
                            id: "refine",
                            label: "Refine Edges",
                            section: Section::Light,
                            min: 0.0,
                            max: 100.0,
                            default: 0.0,
                            step: 1.0,
                            decimals: 0,
                            track: Track::Plain,
                        };
                        let out = slider(ui, &refine, cur_m.refine, true, None);
                        apply_slider_out(app, &refine, out, |app, v| app.run("mask.refine", json!({"value": v})));
                        let feather = ControlSpec { id: "maskFeather", label: "Feather", ..refine };
                        let out = slider(ui, &feather, cur_m.feather, true, None);
                        apply_slider_out(app, &feather, out, |app, v| app.run("mask.feather", json!({"value": v})));
                    }
                }
            }
            ui.add_space(4.0);
        }
    });
    app.ui.mask_hover = hovered_row;
    let Some(mid) = app.session.active_mask else {
        if !d.masks.is_empty() {
            divider(ui);
            egui::Frame::NONE.inner_margin(egui::Margin { left: 12, right: 16, top: 4, bottom: 4 }).show(ui, |ui| {
                show_overlay_row(app, ui);
            });
        }
        return;
    };
    let Some(m) = d.masks.iter().find(|m| m.id == mid).cloned() else { return };
    divider(ui);
    egui::Frame::NONE.inner_margin(egui::Margin { left: 12, right: 16, top: 4, bottom: 4 }).show(ui, |ui| {
        show_overlay_row(app, ui);
    });
    if app.ui.tool == "brush" {
        divider(ui);
        brush_settings(app, ui);
    }
    divider(ui);
    super::mask_effects::effect_row(app, ui, &m);
    for (key, title, ids) in GROUPS {
        if !section_header(app, ui, key, title, group_changed(&m.adjust, ids)) {
            continue;
        }
        for s in LOCAL.iter().filter(|s| ids.contains(&s.id)) {
            let v = local_get(&m.adjust, s.id);
            let out = slider(ui, s, v, true, None);
            apply_slider_out(app, s, out, |app, v| app.run("mask.adjust", json!({"values": {s.id: v}})));
        }
        if *key == "light" {
            // the mask's own point curves, after the tone sliders (the Tone Curve's editor)
            let open = app.ui.flyout_open("maskCurve");
            if flyout_row(ui, "maskCurve", "Curve", Icon::Curve, open).clicked() {
                app.ui.toggle_flyout("maskCurve");
            }
            if open {
                super::edit::curve_editor(app, ui, id, &d, super::edit::CurveTarget::Mask(m.id));
            }
        }
        if *key == "color" {
            divider(ui);
            grading_section(app, ui, &m.adjust);
        }
    }
    ui.add_space(30.0);
    let _ = Stroke::NONE;
}

/// The active mask's Amount slider, under a line naming the mask (its name, else "Mask N").
fn amount_slider(app: &mut LccApp, ui: &mut egui::Ui, index: usize, m: &lcc_develop::Mask) {
    let t = Tokens::get(ui.ctx());
    let name = if m.name.trim().is_empty() { format!("{} {}", crate::i18n::tr("Mask"), index + 1) } else { m.name.clone() };
    ui.vertical_centered(|ui| {
        let r = ui.add(egui::Label::new(egui::RichText::new(name).font(t.semibold(13.0)).color(t.text_label)).truncate());
        register(ui.ctx(), "maskAmountName", r.rect);
    });
    let amt = ControlSpec {
        id: "amount",
        label: "Amount",
        section: Section::Light,
        min: 0.0,
        max: 200.0,
        default: 100.0,
        step: 1.0,
        decimals: 0,
        track: Track::Plain,
    };
    let out = slider(ui, &amt, m.adjust.amount, true, None);
    apply_slider_out(app, &amt, out, |app, v| app.run("mask.adjust", json!({"values": {"amount": v}})));
}

/// Translate fixed action labels, but never mutate or translate user-entered mask names.
fn menu_label(source: &str) -> &str {
    crate::i18n::tr(source)
}

/// The right-click menu of a mask in the Masks list.
fn mask_menu(app: &mut LccApp, ui: &mut egui::Ui, id: u32, name: &str, visible: bool, index: usize, count: usize) {
    let mut run = |ui: &mut egui::Ui, label: &str, enabled: bool, cmd: &str, p: serde_json::Value| {
        if ui.add_enabled(enabled, egui::Button::new(menu_label(label))).clicked() {
            let _ = app.run(cmd, p);
            ui.close();
        }
    };
    run(ui, "Duplicate Mask", true, "mask.duplicate", json!({"id": id}));
    run(ui, "Duplicate and Invert Mask", true, "mask.duplicate", json!({"id": id, "invert": true}));
    run(ui, "Invert Mask", true, "mask.invert", json!({"id": id}));
    run(ui, if visible { "Hide Mask" } else { "Show Mask" }, true, "mask.visible", json!({"id": id}));
    ui.separator();
    run(ui, "Move Up", index > 0, "mask.move", json!({"id": id, "delta": -1}));
    run(ui, "Move Down", index + 1 < count, "mask.move", json!({"id": id, "delta": 1}));
    ui.separator();
    if ui.button(crate::i18n::tr("Rename…")).clicked() {
        app.ui.renaming_mask = Some((id, name.to_string()));
        ui.close();
    }
    if ui.button(crate::i18n::tr("Delete Mask")).clicked() {
        let _ = app.run("mask.delete", json!({"id": id}));
        ui.close();
    }
}

/// Overlay colours offered as swatches (the colour of the selected one is used for the tint).
pub const OVERLAY_COLORS: [[u8; 3]; 5] = [[230, 30, 40], [40, 200, 70], [40, 110, 240], [250, 210, 30], [255, 255, 255]];

/// Color Overlay Settings popup: custom color selection swatches and opacity control.
pub(crate) fn color_overlay_popup(app: &mut LccApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    ui.set_min_width(200.0);
    ui.label(egui::RichText::new(crate::i18n::tr("Color Overlay Settings")).font(t.semibold(12.0)).color(t.text_label));
    ui.add_space(4.0);

    ui.label(egui::RichText::new(crate::i18n::tr("Custom Colors:")).color(t.text_dim));
    ui.horizontal(|ui| {
        for c in OVERLAY_COLORS {
            let (r, resp) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::click());
            crate::widgets::announce(&resp, format!("{} #{:02x}{:02x}{:02x}", crate::i18n::tr("Overlay color"), c[0], c[1], c[2]));
            register(ui.ctx(), format!("maskOverlayColor:{:02x}{:02x}{:02x}", c[0], c[1], c[2]), r);
            ui.painter().rect_filled(r.shrink(2.0), 3.0, egui::Color32::from_rgb(c[0], c[1], c[2]));
            if app.ui.mask_overlay_color == c {
                ui.painter().rect_stroke(r, 3.0, Stroke::new(1.5, t.text), egui::StrokeKind::Inside);
            }
            if resp.clicked() {
                let _ = app.run("view.maskOverlayColor", json!({"color": c}));
            }
        }
    });

    ui.add_space(6.0);
    let spec = ControlSpec {
        id: "ui.maskOverlayOpacity",
        label: "Opacity",
        section: Section::Light,
        min: 0.0,
        max: 100.0,
        default: 50.0,
        step: 1.0,
        decimals: 0,
        track: Track::Plain,
    };
    let out = slider(ui, &spec, app.ui.mask_overlay_opacity as f64, true, None);
    if let Some(v) = out.value {
        app.ui.mask_overlay_opacity = v as f32;
    }
}

/// Dedicated Mask Overlay display options menu under the "..." button.
pub(crate) fn overlay_menu(app: &mut LccApp, ui: &mut egui::Ui) {
    use lcc_pipeline::MaskView;
    let t = Tokens::get(ui.ctx());
    let current_mode = MaskView::parse(&app.ui.mask_overlay_mode).unwrap_or_default();
    let mac = ui.ctx().os() == egui::os::OperatingSystem::Mac;

    // 1. Overlay Modes
    for v in MaskView::ALL {
        let is_current = v == current_mode;
        let b = egui::Button::new(format!("      {}", menu_label(v.label())));
        let r = ui.add(b);
        register(ui.ctx(), format!("maskOverlayMode:{}", v.name()), r.rect);
        if is_current {
            let c = egui::Rect::from_center_size(egui::pos2(r.rect.left() + 11.0, r.rect.center().y), egui::vec2(13.0, 13.0));
            crate::icons::paint(ui.painter(), c, Icon::Check, t.text);
        }
        if r.clicked() {
            let _ = app.run("view.maskOverlayMode", json!({"mode": v.name()}));
            ui.close();
        }
    }

    ui.separator();

    // 2. Quick Toggles & Pins
    // Automatically Toggle Overlay
    let auto_on = app.ui.mask_overlay_auto;
    let b_auto = egui::Button::new(format!("      {}", crate::i18n::tr("Automatically Toggle Overlay")));
    let r_auto = ui.add(b_auto);
    register(ui.ctx(), "menuitem:maskOverlayAuto", r_auto.rect);
    if auto_on {
        let c = egui::Rect::from_center_size(egui::pos2(r_auto.rect.left() + 11.0, r_auto.rect.center().y), egui::vec2(13.0, 13.0));
        crate::icons::paint(ui.painter(), c, Icon::Check, t.text);
    }
    if r_auto.clicked() {
        let _ = app.run("view.maskOverlayAuto", json!({"auto": !auto_on}));
        ui.close();
    }

    // Show Unselected Mask Pins (Alt+V / ⌥V)
    let unselected_on = app.ui.mask_unselected_pins;
    let mut b_unsel = egui::Button::new(format!("      {}", crate::i18n::tr("Show Unselected Mask Pins")));
    b_unsel = b_unsel.shortcut_text(if mac { "⌥V" } else { "Alt+V" });
    let r_unsel = ui.add(b_unsel);
    register(ui.ctx(), "menuitem:maskUnselectedPins", r_unsel.rect);
    if unselected_on {
        let c = egui::Rect::from_center_size(egui::pos2(r_unsel.rect.left() + 11.0, r_unsel.rect.center().y), egui::vec2(13.0, 13.0));
        crate::icons::paint(ui.painter(), c, Icon::Check, t.text);
    }
    if r_unsel.clicked() {
        let _ = app.run("view.maskUnselectedPins", json!({"show": !unselected_on}));
        ui.close();
    }

    // Show Pins and Tools (V)
    let pins_on = app.ui.mask_pins;
    let mut b_pins = egui::Button::new(format!("      {}", crate::i18n::tr("Show Pins and Tools")));
    b_pins = b_pins.shortcut_text("V");
    let r_pins = ui.add(b_pins);
    register(ui.ctx(), "menuitem:maskPins", r_pins.rect);
    register(ui.ctx(), "menuitem:view.maskPins", r_pins.rect);
    if pins_on {
        let c = egui::Rect::from_center_size(egui::pos2(r_pins.rect.left() + 11.0, r_pins.rect.center().y), egui::vec2(13.0, 13.0));
        crate::icons::paint(ui.painter(), c, Icon::Check, t.text);
    }
    if r_pins.clicked() {
        let _ = app.run("view.maskPins", json!({"show": !pins_on}));
        ui.close();
    }

    // Show All Overlays (⇧⌘O / Ctrl+Shift+O)
    let all_on = app.ui.mask_overlay_all;
    let mut b_all = egui::Button::new(format!("      {}", crate::i18n::tr("Show All Overlays")));
    b_all = b_all.shortcut_text(if mac { "⇧⌘O" } else { "Ctrl+Shift+O" });
    let r_all = ui.add(b_all);
    register(ui.ctx(), "menuitem:maskOverlayAll", r_all.rect);
    register(ui.ctx(), "maskOverlayAll", r_all.rect);
    if all_on {
        let c = egui::Rect::from_center_size(egui::pos2(r_all.rect.left() + 11.0, r_all.rect.center().y), egui::vec2(13.0, 13.0));
        crate::icons::paint(ui.painter(), c, Icon::Check, t.text);
    }
    if r_all.clicked() {
        let _ = app.run("view.maskOverlayAll", json!({}));
        ui.close();
    }

    ui.separator();

    // 3. Color Overlay Settings submenu
    crate::widgets::menu_button(ui, crate::i18n::tr("Color Overlay Settings…"), |ui| {
        color_overlay_popup(app, ui);
    });
}

/// The Show Overlay row with toggle checkbox, color swatch, and "..." menu.
fn show_overlay_row(app: &mut LccApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    ui.horizontal(|ui| {
        let mut show_overlay = app.ui.mask_overlay;
        let r = ui.checkbox(&mut show_overlay, crate::i18n::tr("Show Overlay"));
        register(ui.ctx(), "check:maskOverlay", r.rect);
        register(ui.ctx(), "maskOverlay", r.rect);
        if r.changed() {
            let _ = app.run("view.maskOverlay", json!({"show": show_overlay}));
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let more = ui.button("…").on_hover_text(crate::i18n::tr("Mask Overlay Options"));
            register(ui.ctx(), "button:maskOverlayMenu", more.rect);
            register(ui.ctx(), "maskOverlayMode", more.rect);
            egui::Popup::menu(&more).show(|ui| {
                overlay_menu(app, ui);
            });

            let all = app.ui.mask_overlay_all;
            let all_mac = ui.ctx().os() == egui::os::OperatingSystem::Mac;
            let all_base = crate::i18n::tr("Show all overlays");
            let all_tip = match crate::shortcuts::shortcut_of(&app.ui.settings.keymap, "view.maskOverlayAll") {
                Some(sc) => format!("{all_base} ({})", crate::menubar::shortcut_text(sc, all_mac)),
                None => all_base.to_string(),
            };
            if icon_button(ui, "maskOverlayAll", Icon::Stack, vec2(20.0, 20.0), all, true, &all_tip).clicked() {
                let _ = app.run("view.maskOverlayAll", json!({}));
            }

            let [cr, cg, cb] = app.ui.mask_overlay_color;
            let (swatch_rect, swatch_resp) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::click());
            swatch_resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, false, crate::i18n::tr("Overlay Color")));
            register(ui.ctx(), "maskOverlayColor", swatch_rect);
            register(ui.ctx(), "button:maskOverlayColor", swatch_rect);
            ui.painter().rect_filled(swatch_rect.shrink(1.0), 3.0, egui::Color32::from_rgb(cr, cg, cb));
            ui.painter().rect_stroke(swatch_rect.shrink(1.0), 3.0, Stroke::new(1.0, t.divider), egui::StrokeKind::Inside);
            let swatch_resp = swatch_resp.on_hover_text(crate::i18n::tr("Overlay Color and Opacity"));
            egui::Popup::menu(&swatch_resp).show(|ui| {
                color_overlay_popup(app, ui);
            });
        });
    });
}

/// Controls for a range component: the selected range (two handles over a dark → light bar),
/// Smoothness, and Show Luminance Map; colour ranges get Refine.
fn range_controls(app: &mut LccApp, ui: &mut egui::Ui, comp: usize, shape: &MaskShape) {
    let t = Tokens::get(ui.ctx());
    let update = |app: &mut LccApp, shape: MaskShape| {
        let _ = app.run("mask.update", json!({"component": comp, "shape": shape}));
    };
    match shape {
        MaskShape::LuminanceRange { lo, hi, lo_feather, hi_feather } => {
            ui.label(egui::RichText::new(crate::i18n::tr("Select Luminance Range")).color(t.text_dim));
            let (lo, hi) = (*lo, *hi);
            let w = ui.available_width().min(240.0);
            let (rect, resp) = ui.allocate_exact_size(vec2(w, 18.0), Sense::click_and_drag());
            crate::widgets::announce(&resp, crate::i18n::tr("Select Luminance Range"));
            register(ui.ctx(), format!("lumRange:{comp}"), rect);
            let p = ui.painter();
            // dark → light ramp, the selected span outlined
            let n = 24;
            for k in 0..n {
                let a = k as f32 / n as f32;
                let r = Rect::from_min_max(
                    pos2(rect.left() + a * w, rect.top() + 4.0),
                    pos2(rect.left() + (a + 1.0 / n as f32) * w + 0.5, rect.bottom() - 4.0),
                );
                let g = (a * 255.0) as u8;
                p.rect_filled(r, 0.0, egui::Color32::from_gray(g));
            }
            let x = |v: f64| rect.left() + v.clamp(0.0, 1.0) as f32 * w;
            p.rect_stroke(
                Rect::from_min_max(pos2(x(lo), rect.top() + 2.0), pos2(x(hi), rect.bottom() - 2.0)),
                2.0,
                Stroke::new(1.5, t.accent),
                egui::StrokeKind::Middle,
            );
            for v in [lo, hi] {
                p.circle_filled(pos2(x(v), rect.center().y), 5.0, egui::Color32::WHITE);
                p.circle_stroke(pos2(x(v), rect.center().y), 5.0, Stroke::new(1.0, egui::Color32::from_gray(40)));
            }
            // Choose at the press position, then keep that handle for the entire drag. Testing
            // proximity on every frame can move the opposite endpoint after a fast/crossing drag.
            let handle_id = resp.id.with("luminance-handle");
            let nearer_low = |px: f32| (px - x(lo)).abs() <= (px - x(hi)).abs();
            if resp.drag_started() {
                if let Some(origin) = ui.input(|i| i.pointer.press_origin()) {
                    ui.data_mut(|d| d.insert_temp(handle_id, nearer_low(origin.x)));
                }
                let _ = app.run("develop.beginInteraction", json!({"label": "Luminance Range"}));
            }
            if (resp.dragged() || resp.clicked())
                && let Some(pos) = resp.interact_pointer_pos()
            {
                let v = (((pos.x - rect.left()) / w) as f64).clamp(0.0, 1.0);
                let low =
                    if resp.dragged() { ui.data(|d| d.get_temp::<bool>(handle_id)).unwrap_or_else(|| nearer_low(pos.x)) } else { nearer_low(pos.x) };
                let (nlo, nhi) = if low { (v.min((hi - 0.01).max(0.0)), hi) } else { (lo, v.max((lo + 0.01).min(1.0))) };
                update(app, MaskShape::LuminanceRange { lo: nlo, hi: nhi, lo_feather: *lo_feather, hi_feather: *hi_feather });
            }
            if resp.drag_stopped() {
                ui.data_mut(|d| d.remove::<bool>(handle_id));
                let _ = app.run("develop.endInteraction", json!({}));
            }
            // Smoothness: both falloffs at once
            let smooth = ControlSpec {
                id: "smoothness",
                label: "Smoothness",
                section: Section::Light,
                min: 0.0,
                max: 100.0,
                default: 20.0,
                step: 1.0,
                decimals: 0,
                track: Track::Plain,
            };
            let cur = ((lo_feather + hi_feather) / 2.0 / 0.5 * 100.0).clamp(0.0, 100.0);
            let out = slider(ui, &smooth, cur, true, None);
            apply_slider_out(app, &smooth, out, |app, v| {
                let f = v / 100.0 * 0.5;
                app.run("mask.update", json!({"component": comp, "shape": MaskShape::LuminanceRange { lo, hi, lo_feather: f, hi_feather: f }}))
            });
            let mut map = app.ui.mask_overlay && app.ui.mask_overlay_mode == "colorOnBw";
            let r = ui.checkbox(&mut map, crate::i18n::tr("Show Luminance Map"));
            register(ui.ctx(), format!("check:lumMap{comp}"), r.rect);
            if r.changed() {
                // the luminance map: the photo in black & white with the selected range tinted
                if map {
                    app.ui.luminance_map_restore = Some((app.ui.mask_overlay, app.ui.mask_overlay_mode.clone()));
                    app.ui.mask_overlay = true;
                    app.ui.mask_overlay_mode = "colorOnBw".into();
                } else {
                    let (on, mode) = app.ui.luminance_map_restore.take().unwrap_or((false, "color".into()));
                    app.ui.mask_overlay = on;
                    app.ui.mask_overlay_mode = if mode == "colorOnBw" { "color".into() } else { mode };
                }
            }
        }
        MaskShape::ColorRange { samples, refine } => {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(crate::i18n::tr_format!("{} sample{}", samples.len(), if samples.len() == 1 { "" } else { "s" }))
                        .color(t.text_dim),
                );
                let picking = app.ui.tool == "colorRange";
                if text_button(ui, &format!("colorPick{comp}"), crate::i18n::tr("Pick"), picking)
                    .on_hover_text(crate::i18n::tr("Click the photo to pick a colour; ⇧-click adds more (up to 5)"))
                    .clicked()
                {
                    app.ui.tool = if picking { String::new() } else { "colorRange".into() };
                }
            });
            let spec = ControlSpec {
                id: "refine",
                label: "Refine",
                section: Section::Color,
                min: 0.0,
                max: 100.0,
                default: 50.0,
                step: 1.0,
                decimals: 0,
                track: Track::Plain,
            };
            let out = slider(ui, &spec, *refine, true, None);
            let samples = samples.clone();
            apply_slider_out(app, &spec, out, |app, v| {
                app.run("mask.update", json!({"component": comp, "shape": MaskShape::ColorRange { samples: samples.clone(), refine: v }}))
            });
        }
        _ => {}
    }
}

/// The right-click / "…" menu of one component of the selected mask.
fn component_row_menu(app: &mut LccApp, ui: &mut egui::Ui, mask: u32, k: usize, label: &str, count: usize) {
    let mut run = |ui: &mut egui::Ui, text: &str, p: serde_json::Value| {
        if ui.button(menu_label(text)).clicked() {
            let mut p = p;
            p["id"] = json!(mask);
            p["component"] = json!(k);
            let _ = app.run("mask.component", p);
            ui.close();
        }
    };
    run(ui, "Invert", json!({"action": "invert"}));
    run(ui, &crate::i18n::tr_format!("Duplicate \"{label}\"", label = label), json!({"action": "duplicate"}));
    if k > 0 {
        crate::widgets::menu_button(ui, crate::i18n::tr("Mode"), |ui| {
            for (op, text) in [("add", "Add"), ("subtract", "Subtract"), ("intersect", "Intersect")] {
                run(ui, text, json!({"action": "op", "op": op}));
            }
        });
    }
    crate::widgets::menu_button(ui, crate::i18n::tr("Intersect with"), |ui| component_menu(app, ui, "intersect"));
    crate::widgets::menu_button(ui, crate::i18n::tr("Subtract"), |ui| component_menu(app, ui, "subtract"));
    ui.separator();
    if ui.button(crate::i18n::tr("Rename…")).clicked() {
        app.ui.renaming_component = Some((mask, k, label.to_string()));
        ui.close();
    }
    let del = if count == 1 {
        crate::i18n::tr_format!("Delete \"{label}\" (and the mask)", label = label)
    } else {
        crate::i18n::tr_format!("Delete \"{label}\"", label = label)
    };
    if ui.button(del).clicked() {
        let _ = app.run("mask.component", json!({"id": mask, "component": k, "action": "delete"}));
        ui.close();
    }
}

/// Whether the SAM 3 model is missing in a build that could use it: then the download is
/// offered (with `then` to start afterwards) instead of starting an AI mask.
fn needs_model(app: &mut LccApp, kind: &str, op: &str) -> bool {
    let seg = &app.session.segmenter;
    let missing = lcc_engine::segment::Segmenter::AVAILABLE && seg.remote_endpoint().is_none() && seg.dir.is_some() && !seg.installed();
    if missing {
        app.offer_sam_download(Some((kind, op)));
    }
    missing
}

/// Start an Object selection (SAM 3 clicks): a new mask, or a component of the selected one
/// combined by `op`; the photo is analyzed meanwhile (in the background).
pub(crate) fn start_object(app: &mut LccApp, ctx: &egui::Context, op: &str) {
    if needs_model(app, "object", op) {
        return;
    }
    let r = if op == "new" {
        app.run_typed("mask.add", json!({"kind": "object"}))
    } else {
        app.run_typed("mask.addComponent", json!({"op": op, "kind": "object"}))
    };
    match r {
        Ok(_) => {
            app.ui.tool = "object".into();
            if let Some(d) = app.session.active().and_then(|id| app.session.develop_of(id))
                && let Some(m) = d.masks.iter().find(|m| Some(m.id) == app.session.active_mask)
            {
                app.ui.active_mask_component = Some(m.components.len().saturating_sub(1));
            }
            app.toast(ctx, crate::i18n::tr("Click the object to select it · ⌥-click leaves a part out"));
        }
        Err(e) => app.ai_error(ctx, e, Some(("object", op))),
    }
}

/// Start an AI mask of `kind` (object|prompt) combined by `op` (new|add|subtract|intersect),
/// after the model was installed.
pub(crate) fn begin_ai(app: &mut LccApp, kind: &str, op: &str) -> Result<serde_json::Value, String> {
    match kind {
        "object" => {
            let r = if op == "new" {
                app.run("mask.add", json!({"kind": "object"}))
            } else {
                app.run("mask.addComponent", json!({"op": op, "kind": "object"}))
            }?;
            app.ui.tool = "object".into();
            Ok(r)
        }
        _ => {
            app.ui.describe = Some((op.to_string(), String::new()));
            Ok(serde_json::Value::Null)
        }
    }
}

/// Open the Describe field (a new mask, or a component combined by `op`).
pub(crate) fn start_describe(app: &mut LccApp, op: &str) {
    if needs_model(app, "prompt", op) {
        return;
    }
    app.ui.describe = Some((op.to_string(), String::new()));
}

/// The SAM model's download, `width` wide. The label is a percentage once the size is known and
/// just the model's name before, so the bar keeps its height and its accessible name.
pub(crate) fn sam_download_bar(ui: &mut egui::Ui, done: u64, total: u64, width: f32) -> egui::Response {
    let fraction = crate::progress::fraction_of(done, total);
    let label = match fraction {
        Some(f) => format!("SAM 3: {} %", crate::progress::percent(f)),
        None => "SAM 3".to_string(),
    };
    crate::progress::Progress::new(fraction).label(label).width(width).widget("maskSamProgress").show(ui)
}

/// Whether the segmenter has work (loading, analyzing, a request, a detail pass running or waiting
/// to start): nothing new starts then.
fn segmenter_working(analyzing: bool, busy: bool, detail_busy: bool, detail_due: bool) -> bool {
    analyzing || busy || detail_busy || detail_due
}

/// Enter in the Describe field is the Select button: it starts nothing while the segmenter works.
fn enter_submits(working: bool, lost_focus: bool, enter_pressed: bool) -> bool {
    !working && lost_focus && enter_pressed
}

/// The Describe field: type what to select ("sky", "the red car") and press Return.
fn describe_field(app: &mut LccApp, ui: &mut egui::Ui, new_mask: bool) {
    let Some((op, mut text)) = app.ui.describe.clone() else { return };
    // a new mask's field sits under the tiles; one that combines, under the selected mask
    if (op == "new") != new_mask {
        return;
    }
    ui.add_space(8.0);
    let prompt = match op.as_str() {
        "subtract" => "Describe what to leave out",
        "intersect" => "Describe what to keep",
        _ => "Describe what to select",
    };
    ui.label(crate::i18n::tr(prompt));
    let mut submit = false;
    // the (translated) Select button is placed first, at the right; the field takes what is left
    ui.allocate_ui_with_layout(vec2(ui.available_width(), 24.0), egui::Layout::right_to_left(egui::Align::Center), |ui| {
        let seg = &app.session.segmenter;
        let working = segmenter_working(seg.analyzing(), seg.busy(), seg.detail_busy(), app.ui.detail_due.is_some());
        if crate::button::Button::new("Select").small().busy(working).widget("button:maskDescribeGo").show(ui).clicked() {
            submit = true;
        }
        let r = ui.add(
            egui::TextEdit::singleline(&mut text)
                .hint_text(crate::i18n::tr("e.g. sky · the red car · car, road"))
                .desired_width(ui.available_width()),
        );
        register(ui.ctx(), "maskDescribe", r.rect);
        if !r.has_focus() && !r.lost_focus() && text.is_empty() {
            r.request_focus();
        }
        // Enter is the Select button: nothing starts while the segmenter is working
        if enter_submits(working, r.lost_focus(), ui.input(|i| i.key_pressed(egui::Key::Enter))) {
            submit = true;
        }
    });
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        app.ui.describe = None;
        return;
    }
    app.ui.describe = Some((op.clone(), text.clone()));
    if submit && !text.trim().is_empty() {
        let r = if op == "new" {
            app.run_typed("mask.add", json!({"kind": "prompt", "text": text}))
        } else {
            app.run_typed("mask.addComponent", json!({"op": op, "kind": "prompt", "text": text}))
        };
        match r {
            // the mask appears when the model has found it (a detail pass follows)
            Ok(_) => app.ui.describe = None,
            Err(e) => app.ai_error(ui.ctx(), e, Some(("prompt", op.as_str()))),
        }
    }
}

fn component_menu(app: &mut LccApp, ui: &mut egui::Ui, op: &str) {
    let b = ui.button(crate::i18n::tr("Object"));
    register(ui.ctx(), format!("maskComp:{op}:object"), b.rect);
    if b.clicked() {
        start_object(app, ui.ctx(), op);
    }
    let b = ui.button(crate::i18n::tr("Describe…"));
    register(ui.ctx(), format!("maskComp:{op}:prompt"), b.rect);
    if b.clicked() {
        start_describe(app, op);
    }
    for (kind, label) in [
        ("brush", "Brush"),
        ("linear", "Linear Gradient"),
        ("radial", "Radial Gradient"),
        ("sky", "Sky"),
        ("subject", "Subject"),
        ("luminanceRange", "Luminance Range"),
    ] {
        // painting only adds or erases
        if kind == "brush" && op == "intersect" {
            continue;
        }
        let b = ui.button(menu_label(label));
        register(ui.ctx(), format!("maskComp:{op}:{kind}"), b.rect);
        if b.clicked() {
            if kind == "brush" {
                let _ = app.run("tool.brush", json!({"op": op}));
            } else {
                let _ = app.run("mask.addComponent", json!({"op": op, "kind": kind}));
                if matches!(kind, "linear" | "radial" | "colorRange" | "luminanceRange" | "depthRange") {
                    app.ui.tool = kind.to_string();
                }
            }
            if let Some(d) = app.session.active().and_then(|id| app.session.develop_of(id))
                && let Some(m) = d.masks.iter().find(|m| Some(m.id) == app.session.active_mask)
            {
                app.ui.active_mask_component = Some(m.components.len().saturating_sub(1));
            }
        }
    }
}

fn brush_settings(app: &mut LccApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    egui::Frame::NONE.inner_margin(egui::Margin { left: 12, right: 16, top: 6, bottom: 0 }).show(ui, |ui| {
        // wraps at narrow panel widths (and long translations) rather than widening the panel
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new(crate::i18n::tr("Brush")).font(t.semibold(13.0)));
            if text_button(ui, "brushAdd", crate::i18n::tr("Add"), !app.ui.brush_erase).clicked() {
                app.ui.brush_erase = false;
            }
            if text_button(ui, "brushErase", crate::i18n::tr("Erase"), app.ui.brush_erase).clicked() {
                app.ui.brush_erase = true;
            }
            ui.add_space(8.0);
            // Lightroom's brush A / B: two presets, each with its own size, feather, flow, density
            for (slot, id, label) in [(0u8, "brushA", "A"), (1, "brushB", "B")] {
                if text_button(ui, id, label, app.ui.brush_slot == slot).clicked() && app.ui.brush_slot != slot {
                    let _ = app.run("brush.preset", json!({"slot": label.to_ascii_lowercase()}));
                }
            }
        });
        let mut auto = app.ui.brush_auto_mask;
        if ui
            .checkbox(&mut auto, crate::i18n::tr("Auto Mask"))
            .on_hover_text(crate::i18n::tr("Paint only areas like the one under the brush"))
            .changed()
        {
            app.ui.brush_auto_mask = auto;
        }
        let mut pressure = app.ui.brush_pressure;
        if ui
            .checkbox(&mut pressure, crate::i18n::tr("Pen Pressure"))
            .on_hover_text(crate::i18n::tr("A pen's pressure sets the size and flow of the brush"))
            .changed()
        {
            app.ui.brush_pressure = pressure;
        }
    });
    for (id, label, min, max, get) in [
        ("ui.brushSize", "Size", 1.0, 100.0, (app.ui.brush_size * 400.0) as f64),
        ("ui.brushFeather", "Feather", 0.0, 100.0, app.ui.brush_feather as f64),
        ("ui.brushFlow", "Flow", 1.0, 100.0, app.ui.brush_flow as f64),
        ("ui.brushDensity", "Density", 0.0, 100.0, app.ui.brush_density as f64),
    ] {
        let s = ControlSpec { id, label, section: Section::Light, min, max, default: min, step: 1.0, decimals: 0, track: Track::Plain };
        let out = slider(ui, &s, get, true, None);
        if let Some(v) = out.value {
            match id {
                "ui.brushSize" => app.ui.brush_size = (v / 400.0) as f32,
                "ui.brushFeather" => app.ui.brush_feather = v as f32,
                "ui.brushDensity" => app.ui.brush_density = v as f32,
                _ => app.ui.brush_flow = v as f32,
            }
        }
    }
}

/// A masks-panel icon button named `label` (translated; also its hover text), addressable as
/// `button:{id}` like the text button it replaced, and `icon:{id}`.
fn mask_icon(ui: &mut egui::Ui, id: &str, icon: Icon, label: &str, active: bool) -> egui::Response {
    let name = crate::i18n::tr(label);
    let r = crate::widgets::icon_button_named(ui, id, icon, vec2(32.0, 28.0), active, true, name, name);
    crate::widgets::register(ui.ctx(), format!("button:{id}"), r.rect);
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_detail_pass_counts_as_the_segmenter_working() {
        assert!(segmenter_working(false, false, true, false));
        assert!(segmenter_working(true, false, false, false));
        assert!(segmenter_working(false, true, false, false));
        assert!(!segmenter_working(false, false, false, false));
    }

    #[test]
    fn a_pending_detail_pass_counts_as_the_segmenter_working() {
        // a new prompt finishing before it starts would replace its target, and that mask never refines
        assert!(segmenter_working(false, false, false, true));
    }

    #[test]
    fn enter_in_the_describe_field_starts_nothing_while_the_segmenter_works() {
        assert!(enter_submits(false, true, true));
        assert!(!enter_submits(true, true, true));
        assert!(!enter_submits(false, false, true));
        assert!(!enter_submits(false, true, false));
    }
}
