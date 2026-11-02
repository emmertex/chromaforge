//! The Presets Manager (LR-PRE-MANAGE): every develop preset in one list, built-in ones too, by
//! group, the user's own groups first. Per preset: Favorite, At Import (the import pickers list it
//! first, LR-PRE-ONIMPORT), Show (hidden presets leave the Presets panel and the pickers but stay
//! here), Rename (built-in ones too) and, for the user's own, Delete (asked in its row). Per group:
//! fold, Rename, Hide / Show all, and Delete for the user's own (asked in its row). Drag a preset
//! onto a group's header to move it there, or a group onto another to put it inside. Import,
//! Export User Presets, New Group and Restore Built-ins (names, groups and hidden built-ins as
//! shipped) sit above the list. A search over names and groups opens what it finds; groups start
//! folded. Only the rows in view are built (Lightroom collections are 10 000+ presets).
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
    /// A group: its name, how many presets it shows, whether it is open, whether it holds
    /// built-in presets, whether all of them are hidden.
    Group { name: String, count: usize, open: bool, builtin: bool, all_hidden: bool },
    /// An index into the session's presets.
    Preset(usize),
}

/// What is being dragged in the list.
#[derive(Clone)]
enum Drag {
    Preset(String),
    Group(String),
}

const DRAG: &str = "presets-manager-drag";

/// The list's rows for `needle` (lowercase): the groups holding the user's own presets first,
/// then the built-in ones, each in list order.
fn rows(app: &LccApp, needle: &str) -> Vec<Row> {
    let presets = &app.session.presets;
    let mut groups: Vec<(String, Vec<usize>, bool)> = Vec::new();
    for (i, p) in presets.iter().enumerate() {
        if !(needle.is_empty() || p.name.to_lowercase().contains(needle) || p.group.to_lowercase().contains(needle)) {
            continue;
        }
        match groups.iter_mut().find(|(g, _, _)| *g == p.group) {
            Some((_, list, user)) => {
                list.push(i);
                *user |= !p.builtin;
            }
            None => groups.push((p.group.clone(), vec![i], !p.builtin)),
        }
    }
    // empty folders the user made are groups too (a place to drop presets)
    if needle.is_empty() {
        for f in &app.session.preset_folders {
            if !groups.iter().any(|(g, _, _)| g == f) {
                groups.push((f.clone(), Vec::new(), true));
            }
        }
    }
    groups.sort_by_key(|g| !g.2);
    let mut out = Vec::new();
    for (g, list, _) in groups {
        // a search opens what it finds
        let open = !needle.is_empty() || app.ui.group_is_open(&format!("presetsManager:{g}"), false);
        let builtin = list.iter().any(|i| presets.get(*i).is_some_and(|p| p.builtin));
        let all_hidden = !list.is_empty() && list.iter().all(|i| presets.get(*i).is_some_and(|p| app.session.preset_hidden.contains(&p.id)));
        out.push(Row::Group { name: g, count: list.len(), open, builtin, all_hidden });
        if open {
            out.extend(list.into_iter().map(Row::Preset));
        }
    }
    out
}

/// An inline rename field; `Some(text)` once Enter confirms it, `None` while typing; Esc or
/// leaving it ends the rename (`*done`).
fn rename_field(ui: &mut egui::Ui, id: &str, text: &mut String, width: f32, done: &mut bool) -> Option<String> {
    let r = ui.add(egui::TextEdit::singleline(text).desired_width(width));
    register(ui.ctx(), format!("field:{id}"), r.rect);
    if !r.has_focus() && !r.lost_focus() {
        r.request_focus();
    }
    if r.lost_focus() {
        *done = true;
        if ui.input(|i| i.key_pressed(egui::Key::Enter)) && !text.trim().is_empty() {
            return Some(text.trim().to_string());
        }
    }
    None
}

/// The dialog's body. `query`: the search; `confirm`: the row whose Delete (or Restore) waits for
/// a yes; `rename`: the row renamed in place and its text.
pub fn body(app: &mut LccApp, ui: &mut egui::Ui, query: &mut String, confirm: &mut Option<String>, rename: &mut Option<(String, String)>) {
    let t = Tokens::get(ui.ctx());
    ui.set_min_width(600.0);
    let mut action: Option<(&'static str, serde_json::Value)> = None;
    // the toolbar
    ui.horizontal(|ui| {
        let b = ui.button(crate::i18n::tr("Import…"));
        register(ui.ctx(), "button:presetsManagerImport", b.rect);
        if b.clicked() {
            action = Some(("file.importPresets", json!({})));
        }
        let any_user = app.session.presets.iter().any(|p| !p.builtin);
        let b = ui.add_enabled(any_user, egui::Button::new(crate::i18n::tr("Export User Presets…")));
        register(ui.ctx(), "button:presetsManagerExport", b.rect);
        if b.clicked() {
            action = Some(("file.exportPresets", json!({})));
        }
        let b = ui.button(crate::i18n::tr("New Group…"));
        register(ui.ctx(), "button:presetsManagerNewGroup", b.rect);
        if b.clicked() {
            *rename = Some(("new".into(), String::new()));
        }
        if confirm.as_deref() == Some("restore") {
            let b = ui.button(crate::i18n::tr("Restore"));
            register(ui.ctx(), "button:presetsManagerRestoreConfirm", b.rect);
            if b.clicked() {
                action = Some(("preset.restoreBuiltins", json!({})));
            }
            if ui.button(crate::i18n::tr("Cancel")).clicked() {
                *confirm = None;
            }
        } else {
            let b = ui
                .button(crate::i18n::tr("Restore Built-ins"))
                .on_hover_text(crate::i18n::tr("Built-in presets get their own names and groups back and are shown again"));
            register(ui.ctx(), "button:presetsManagerRestore", b.rect);
            if b.clicked() {
                *confirm = Some("restore".into());
            }
        }
    });
    let mut done = false;
    if let Some((_, text)) = rename.as_mut().filter(|(k, _)| k == "new") {
        ui.horizontal(|ui| {
            ui.label(crate::i18n::tr("New group name"));
            if let Some(name) = rename_field(ui, "presetsManagerNewGroupName", text, 220.0, &mut done) {
                action = Some(("preset.createGroup", json!({"name": name})));
            }
        });
    }
    if done {
        *rename = None;
    }
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
    let mut fold: Option<(String, bool)> = None;
    let drag_id = egui::Id::new(DRAG);
    let dragging: Option<Drag> = ui.ctx().data(|d| d.get_temp(drag_id));
    let released = ui.input(|i| i.pointer.any_released());
    let mut drop: Option<(Drag, String)> = None;
    let mut start_drag: Option<Drag> = None;
    egui::ScrollArea::vertical().id_salt("presetsManager").max_height(420.0).auto_shrink([false, true]).show_rows(
        ui,
        row_h,
        list.len(),
        |ui, range| {
            for row in list.get(range).unwrap_or_default() {
                match row {
                    Row::Group { name: g, count: n, open, builtin, all_hidden } => {
                        let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::click_and_drag());
                        register(ui.ctx(), format!("presetsManagerGroup:{g}"), r);
                        let target = match &dragging {
                            Some(Drag::Preset(_)) => true,
                            Some(Drag::Group(d)) => d != g && !g.starts_with(&format!("{d}/")),
                            None => false,
                        };
                        let pointer_in = ui.input(|i| i.pointer.hover_pos()).is_some_and(|p| r.contains(p));
                        if target && pointer_in {
                            ui.painter().rect_stroke(r.shrink(1.0), 3.0, egui::Stroke::new(1.5, t.tool_active), egui::StrokeKind::Inside);
                            if released && let Some(d) = dragging.clone() {
                                drop = Some((d, g.clone()));
                            }
                        } else if resp.hovered() {
                            ui.painter().rect_filled(r, 3.0, t.hover);
                        }
                        if resp.drag_started() {
                            start_drag = Some(Drag::Group(g.clone()));
                        }
                        let chevron = egui::Rect::from_center_size(egui::pos2(r.left() + 9.0, r.center().y), vec2(9.0, 9.0));
                        let icon = if *open { crate::icons::Icon::ChevronDown } else { crate::icons::Icon::ChevronRight };
                        crate::icons::paint(ui.painter(), chevron, icon, t.text_dim);
                        let mut child = ui.new_child(
                            egui::UiBuilder::new().max_rect(r.with_min_x(r.left() + 20.0)).layout(egui::Layout::left_to_right(egui::Align::Center)),
                        );
                        let row_key = format!("group:{g}");
                        if let Some((_, text)) = rename.as_mut().filter(|(k, _)| *k == row_key) {
                            let mut done = false;
                            let leaf = g.rsplit_once('/').map(|(_, l)| l).unwrap_or(g);
                            if text.is_empty() {
                                *text = leaf.to_string();
                            }
                            if let Some(name) = rename_field(&mut child, &format!("presetsManagerGroupName:{g}"), text, 220.0, &mut done) {
                                action = Some(("preset.renameGroup", json!({"group": g, "name": name})));
                            }
                            if done {
                                *rename = None;
                            }
                        } else {
                            let label = crate::i18n::builtin_label(g, *builtin);
                            let color = if *all_hidden { t.text_dim } else { t.text_label };
                            child.label(egui::RichText::new(format!("{label}  ({n})")).font(t.semibold(12.0)).color(color));
                        }
                        // the group's actions at the right
                        let mut acts = ui.new_child(
                            egui::UiBuilder::new().max_rect(r.shrink2(vec2(4.0, 0.0))).layout(egui::Layout::right_to_left(egui::Align::Center)),
                        );
                        if confirm.as_deref() == Some(row_key.as_str()) {
                            if acts.small_button(crate::i18n::tr("Cancel")).clicked() {
                                *confirm = None;
                            }
                            let d = acts.small_button(crate::i18n::tr_format!("Delete {n} presets", n = n));
                            register(acts.ctx(), format!("button:presetsManagerGroupDeleteConfirm:{g}"), d.rect);
                            if d.clicked() {
                                action = Some(("preset.deleteGroup", json!({"group": g})));
                            }
                        } else {
                            if !*builtin {
                                let d = acts.small_button(crate::i18n::tr("Delete"));
                                register(acts.ctx(), format!("button:presetsManagerGroupDelete:{g}"), d.rect);
                                if d.clicked() {
                                    *confirm = Some(row_key.clone());
                                }
                            }
                            if *n > 0 {
                                let h = acts.small_button(crate::i18n::tr(if *all_hidden { "Show All" } else { "Hide All" }));
                                register(acts.ctx(), format!("button:presetsManagerGroupHide:{g}"), h.rect);
                                if h.clicked() {
                                    action = Some(("preset.hideGroup", json!({"group": g, "hidden": !all_hidden})));
                                }
                            }
                            let rn = acts.small_button(crate::i18n::tr("Rename…"));
                            register(acts.ctx(), format!("button:presetsManagerGroupRename:{g}"), rn.rect);
                            if rn.clicked() {
                                *rename = Some((row_key.clone(), String::new()));
                            }
                        }
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
                        let row_key = format!("preset:{pid}");
                        ui.horizontal(|ui| {
                            ui.set_height(row_h);
                            ui.add_space(18.0);
                            if crate::rating::favorite_button(ui, format!("presetsManagerStar:{pid}"), fav, crate::i18n::tr("Favorite")).clicked() {
                                action = Some(("preset.favorite", json!({"id": pid, "favorite": !fav})));
                            }
                            let name_w = (ui.available_width() - 300.0).max(120.0);
                            if let Some((_, text)) = rename.as_mut().filter(|(k, _)| *k == row_key) {
                                let mut done = false;
                                if text.is_empty() {
                                    *text = label.clone();
                                }
                                if let Some(name) = rename_field(ui, &format!("presetsManagerName:{pid}"), text, name_w - 4.0, &mut done) {
                                    action = Some(("preset.rename", json!({"id": pid, "name": name})));
                                }
                                if done {
                                    *rename = None;
                                }
                            } else {
                                let (r, resp) = ui.allocate_exact_size(vec2(name_w, row_h), Sense::click_and_drag());
                                register(ui.ctx(), format!("presetsManagerRow:{pid}"), r);
                                if resp.drag_started() {
                                    start_drag = Some(Drag::Preset(pid.clone()));
                                }
                                if resp.double_clicked() {
                                    *rename = Some((row_key.clone(), String::new()));
                                }
                                let color = if shown { t.text } else { t.text_dim };
                                let g = crate::widgets::one_line(ui.painter(), &label, t.font(12.5), color, name_w - 4.0);
                                ui.painter().galley(r.left_center() + vec2(2.0, -g.size().y / 2.0), g, color);
                            }
                            if confirm.as_deref() == Some(row_key.as_str()) {
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
                            let rn = ui.small_button(crate::i18n::tr("Rename…"));
                            register(ui.ctx(), format!("button:presetsManagerRename:{pid}"), rn.rect);
                            if rn.clicked() {
                                *rename = Some((row_key.clone(), String::new()));
                            }
                            if !builtin {
                                let d = ui.small_button(crate::i18n::tr("Delete"));
                                register(ui.ctx(), format!("button:presetsManagerDelete:{pid}"), d.rect);
                                if d.clicked() {
                                    *confirm = Some(row_key.clone());
                                }
                            }
                        });
                    }
                }
            }
        },
    );
    // the drag: started on a row, shown at the pointer, dropped on a group's header
    if let Some(d) = start_drag {
        ui.ctx().data_mut(|m| m.insert_temp(drag_id, d));
    }
    if let Some(d) = ui.ctx().data(|m| m.get_temp::<Drag>(drag_id))
        && let Some(pos) = ui.ctx().input(|i| i.pointer.hover_pos())
    {
        let text = match &d {
            Drag::Preset(id) => app
                .session
                .presets
                .iter()
                .find(|p| &p.id == id)
                .map(|p| crate::i18n::builtin_label(&p.name, p.builtin).to_string())
                .unwrap_or_default(),
            Drag::Group(g) => g.clone(),
        };
        egui::Area::new(egui::Id::new("presets-manager-drag-label"))
            .order(egui::Order::Tooltip)
            .interactable(false)
            .fixed_pos(pos + vec2(14.0, 10.0))
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).fill(t.chrome).show(ui, |ui| {
                    ui.label(text);
                });
            });
    }
    if released {
        ui.ctx().data_mut(|m| m.remove::<Drag>(drag_id));
    }
    if let Some((d, group)) = drop {
        action = Some(match d {
            Drag::Preset(id) => ("preset.move", json!({"id": id, "group": group})),
            Drag::Group(g) => ("preset.moveGroup", json!({"group": g, "parent": group})),
        });
    }
    if let Some((g, open)) = fold {
        app.ui.group_open.insert(format!("presetsManager:{g}"), open);
    }
    if let Some((cmd, p)) = action {
        if matches!(cmd, "preset.delete" | "preset.deleteGroup" | "preset.restoreBuiltins") {
            *confirm = None;
        }
        match app.run(cmd, p) {
            Ok(r) if cmd == "preset.restoreBuiltins" => {
                let n = r.get("restored").and_then(serde_json::Value::as_u64).unwrap_or(0);
                app.toast(ui.ctx(), crate::i18n::tr_format!("Built-in presets restored ({n} changes undone)", n = n));
            }
            Ok(_) => {}
            Err(e) => app.toast_error(ui.ctx(), e),
        }
    }
    crate::dialog_frame::note(
        ui,
        egui::RichText::new(crate::i18n::tr(
            "Hidden presets leave the Presets panel and the pickers; At Import ones come first when importing. Drag a preset onto a group to move it; double-click a name to rename it.",
        ))
        .color(t.text_dim),
    );
}
