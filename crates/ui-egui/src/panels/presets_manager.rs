//! The Presets Manager (LR-PRE-MANAGE): every develop preset in one list, built-in ones too, by
//! group. Per preset: Favorite, At Import (the import pickers list it first, LR-PRE-ONIMPORT),
//! Show (hidden presets leave the Presets panel and the pickers but stay here) and, for the user's
//! own, Delete (asked in its row). A search over names and groups opens what it finds; groups
//! start folded. Only the rows in view are built (Lightroom collections are 10 000+ presets).
//!
//! Ported from OneSlips' Presets Manager (behaviour), drawn in the app's own dialog look.

use std::sync::Arc;

use egui::{Sense, vec2};
use serde_json::json;

use crate::LccApp;
use crate::theme::Tokens;
use crate::widgets::register;

#[derive(Clone)]
enum Row {
    /// A group: its name, how many presets it shows, whether it is open.
    Group(String, usize, bool),
    /// An index into the session's presets.
    Preset(usize),
}

/// The list's rows for `needle` (lowercase) with the groups `open` says are open.
fn rows(app: &LccApp, needle: &str) -> Vec<Row> {
    let presets = &app.session.presets;
    let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
    for (i, p) in presets.iter().enumerate() {
        if !(needle.is_empty() || p.name.to_lowercase().contains(needle) || p.group.to_lowercase().contains(needle)) {
            continue;
        }
        match groups.iter_mut().find(|(g, _)| *g == p.group) {
            Some((_, list)) => list.push(i),
            None => groups.push((p.group.clone(), vec![i])),
        }
    }
    let mut out = Vec::new();
    for (g, list) in groups {
        // a search opens what it finds
        let open = !needle.is_empty() || app.ui.group_is_open(&format!("presetsManager:{g}"), false);
        out.push(Row::Group(g, list.len(), open));
        if open {
            out.extend(list.into_iter().map(Row::Preset));
        }
    }
    out
}

/// The dialog's body. `query`: the search; `confirm`: the user preset whose Delete waits for a yes.
pub fn body(app: &mut LccApp, ui: &mut egui::Ui, query: &mut String, confirm: &mut Option<String>) {
    let t = Tokens::get(ui.ctx());
    ui.set_min_width(560.0);
    ui.horizontal(|ui| {
        crate::text_field::TextField::singleline("field:presetsManagerSearch", query)
            .hint(crate::i18n::tr("Search presets"))
            .width(ui.available_width() - 130.0)
            .show(ui);
        let n = app.session.presets.len();
        let hidden = app.session.preset_hidden.len();
        ui.label(egui::RichText::new(crate::i18n::tr_format!("{n} presets, {h} hidden", n = n, h = hidden)).color(t.text_dim));
    });
    let needle = query.trim().to_lowercase();
    // rebuilt when the presets, their marks, the search or a group's fold changed
    let key = (app.session.presets.generation(), needle.clone(), app.ui.group_open.len(), app.ui.group_open.values().filter(|v| **v).count());
    let cache = egui::Id::new("presets-manager-rows");
    let list = match ui.ctx().data(|d| d.get_temp::<((u64, String, usize, usize), Arc<Vec<Row>>)>(cache)) {
        Some((k, list)) if k == key => list,
        _ => {
            let list = Arc::new(rows(app, &needle));
            ui.ctx().data_mut(|d| d.insert_temp(cache, (key, list.clone())));
            list
        }
    };
    let row_h = 26.0;
    let mut action: Option<(&'static str, serde_json::Value)> = None;
    let mut fold: Option<(String, bool)> = None;
    egui::ScrollArea::vertical().id_salt("presetsManager").max_height(420.0).auto_shrink([false, true]).show_rows(
        ui,
        row_h,
        list.len(),
        |ui, range| {
            for row in list.get(range).unwrap_or_default() {
                match row {
                    Row::Group(g, n, open) => {
                        let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::click());
                        register(ui.ctx(), format!("presetsManagerGroup:{g}"), r);
                        if resp.hovered() {
                            ui.painter().rect_filled(r, 3.0, t.hover);
                        }
                        let chevron = egui::Rect::from_center_size(egui::pos2(r.left() + 9.0, r.center().y), vec2(9.0, 9.0));
                        let icon = if *open { crate::icons::Icon::ChevronDown } else { crate::icons::Icon::ChevronRight };
                        crate::icons::paint(ui.painter(), chevron, icon, t.text_dim);
                        let builtin = app.session.presets.iter().any(|p| &p.group == g && p.builtin);
                        let name = crate::i18n::builtin_label(g, builtin);
                        let text = format!("{name}  ({n})");
                        ui.painter().text(egui::pos2(r.left() + 20.0, r.center().y), egui::Align2::LEFT_CENTER, text, t.semibold(12.0), t.text_label);
                        if resp.clicked() && needle.is_empty() {
                            fold = Some((g.clone(), !open));
                        }
                    }
                    Row::Preset(i) => {
                        let Some(p) = app.session.presets.get(*i) else { continue };
                        let (pid, fav, builtin) = (p.id.clone(), p.favorite, p.builtin);
                        let label = crate::i18n::builtin_label(&p.name, p.builtin).to_string();
                        let at_import = app.session.preset_at_import.contains(&pid);
                        let shown = !app.session.preset_hidden.contains(&pid);
                        ui.horizontal(|ui| {
                            ui.set_height(row_h);
                            ui.add_space(18.0);
                            if crate::rating::favorite_button(ui, format!("presetsManagerStar:{pid}"), fav, crate::i18n::tr("Favorite")).clicked() {
                                action = Some(("preset.favorite", json!({"id": pid, "favorite": !fav})));
                            }
                            let name_w = (ui.available_width() - 230.0).max(120.0);
                            let (r, _) = ui.allocate_exact_size(vec2(name_w, row_h), Sense::hover());
                            let color = if shown { t.text } else { t.text_dim };
                            let g = crate::widgets::one_line(ui.painter(), &label, t.font(12.5), color, name_w - 4.0);
                            ui.painter().galley(r.left_center() + vec2(2.0, -g.size().y / 2.0), g, color);
                            if confirm.as_deref() == Some(pid.as_str()) {
                                let d = ui.button(crate::i18n::tr("Delete"));
                                register(ui.ctx(), format!("button:presetsManagerDeleteConfirm:{pid}"), d.rect);
                                if d.clicked() {
                                    action = Some(("preset.delete", json!({"id": pid})));
                                }
                                if ui.button(crate::i18n::tr("Cancel")).clicked() {
                                    *confirm = None;
                                }
                                return;
                            }
                            let mut on = at_import;
                            let c = ui.checkbox(&mut on, crate::i18n::tr("At Import"));
                            register(ui.ctx(), format!("check:presetsManagerAtImport:{pid}"), c.rect);
                            if c.changed() {
                                action = Some(("preset.forImport", json!({"id": pid, "on": on})));
                            }
                            let mut show = shown;
                            let c = ui.checkbox(&mut show, crate::i18n::tr("Show"));
                            register(ui.ctx(), format!("check:presetsManagerShow:{pid}"), c.rect);
                            if c.changed() {
                                action = Some(("preset.hide", json!({"id": pid, "hidden": !show})));
                            }
                            if !builtin {
                                let d = ui.small_button(crate::i18n::tr("Delete"));
                                register(ui.ctx(), format!("button:presetsManagerDelete:{pid}"), d.rect);
                                if d.clicked() {
                                    *confirm = Some(pid.clone());
                                }
                            }
                        });
                    }
                }
            }
        },
    );
    if let Some((g, open)) = fold {
        app.ui.group_open.insert(format!("presetsManager:{g}"), open);
    }
    if let Some((cmd, p)) = action {
        if cmd == "preset.delete" {
            *confirm = None;
        }
        if let Err(e) = app.run(cmd, p) {
            app.toast_error(ui.ctx(), e);
        }
    }
    crate::dialog_frame::note(
        ui,
        egui::RichText::new(crate::i18n::tr("Hidden presets leave the Presets panel and the pickers; At Import ones come first when importing."))
            .color(t.text_dim),
    );
}
