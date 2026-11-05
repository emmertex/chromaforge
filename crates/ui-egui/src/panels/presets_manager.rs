//! The Presets Manager (LR-PRE-MANAGE): every develop preset in one list, built-in ones too, by
//! group, the user's own groups first. Ported from OneSlips' Presets Manager: its table
//! look ([`super::manager_table`]), drag and drop and the way it asks before a delete.
//!
//! Per preset: Favorite, At Import (the import pickers list it first, LR-PRE-ONIMPORT), Rename
//! (double-click the name, or the pencil; built-in ones too), Group (a menu of the groups), Hide /
//! Show (hidden presets leave the Presets panel and the pickers but stay here) and, for the
//! user's own, Delete (asked in its row). Per group: fold, Rename (double-click or the pencil),
//! Hide / Show all, and Delete for groups of the user's own (asked at the group: delete its
//! presets, or move them to another group first). Click, Ctrl/⌘-click and Shift-click select
//! presets; drag them onto a group's header (or onto any preset of that group) to move them
//! there; drag a group onto another to put it inside. Import, Import Folder, Export, Export for
//! Lightroom, New Group and Restore Built-ins sit above the list. A search over names and groups
//! opens what it finds; groups start folded.
//!
//! Built for collections of 10 000+ presets: the rows are rebuilt only when the presets, their
//! marks, the search or a group's fold changed, and only the rows in view are laid out. The
//! window opens at a size that fits the screen, never grows from its content, and keeps the size
//! the user drags it to (see [`window_size`]).

use std::collections::HashSet;
use std::sync::Arc;

use serde_json::{Value, json};

use super::manager_table::{self as table, Col, Columns, Dragged, GroupAction, GroupHead};
use crate::LccApp;
use crate::i18n::{tr, tr_format};
use crate::icons::Icon;
use crate::theme::Tokens;
use crate::widgets::{icon_button, register};

/// The manager's window id (its size lives under it).
pub const WINDOW: &str = "app-presets-manager";
/// The list's key (its group headers' widgets, and its drag and drop).
const KEY: &str = "presetsManager";
/// The width the window opens at the first time (capped to the screen).
const DEFAULT_W: f32 = 920.0;
/// The smallest the window can be made.
pub const MIN_SIZE: [f32; 2] = [560.0, 360.0];
const COLS: Columns = Columns { info_w: 110.0 };

/// The window's content size: the one the user chose last (remembered across starts), else a
/// sensible first size; always inside `screen` (less its margins).
pub fn window_size(remembered: Option<[f32; 2]>, screen: egui::Rect) -> egui::Vec2 {
    let max = max_size(screen);
    let first = egui::vec2(DEFAULT_W, (screen.height() * 0.8).min(900.0));
    let size = remembered.map(|[w, h]| egui::vec2(w, h)).filter(|s| s.x.is_finite() && s.y.is_finite()).unwrap_or(first);
    egui::vec2(size.x.clamp(MIN_SIZE[0].min(max.x), max.x), size.y.clamp(MIN_SIZE[1].min(max.y), max.y))
}

/// The largest the window's content may be on `screen`: the screen less the margins and the
/// window's frame.
pub fn max_size(screen: egui::Rect) -> egui::Vec2 {
    let m = 2.0 * crate::dialog_frame::SCREEN_MARGIN;
    // (the frame's margins: 18 left and right, 14 top and bottom)
    egui::vec2((screen.width() - m - 36.0).max(200.0), (screen.height() - m - 28.0).max(200.0))
}

/// After a frame of the window: remember the size it is left at (`remembered`), once the pointer
/// is up (not in the middle of a drag). `asked`: the size it was opened at; `shown`: its
/// rectangle's. The two differ by a constant (the frame's parts the rectangle leaves out), learnt
/// on the first frames it is open, so the size remembered opens the window at the size it was left.
pub fn note_size(ctx: &egui::Context, remembered: &mut Option<[f32; 2]>, asked: egui::Vec2, shown: egui::Vec2) {
    note_size_of(ctx, WINDOW, remembered, asked, shown);
}

/// [`note_size`] for the manager window `window` (the Profile Manager sizes itself the same way).
pub fn note_size_of(ctx: &egui::Context, window: &str, remembered: &mut Option<[f32; 2]>, asked: egui::Vec2, shown: egui::Vec2) {
    let id = egui::Id::new(window).with("sizeDelta");
    let now = ctx.cumulative_frame_nr();
    // (the last frame it was seen, frames shown since it opened, asked less shown)
    let (last, n, delta) = ctx.data(|d| d.get_temp::<(u64, u32, Option<egui::Vec2>)>(id)).unwrap_or((0, 0, None));
    let n = if last.saturating_add(1) == now { n.saturating_add(1) } else { 0 };
    let delta = match delta {
        Some(d) => Some(d),
        // (settled after a few frames; a difference too large to be the frame's is none)
        None if n >= 3 => {
            let d = asked - shown;
            Some(if d.x.abs() < 80.0 && d.y.abs() < 80.0 && d.is_finite() { d } else { egui::Vec2::ZERO })
        }
        None => None,
    };
    ctx.data_mut(|d| d.insert_temp(id, (now, n, delta)));
    let Some(delta) = delta else { return };
    let size = shown + delta;
    if size.is_finite()
        && !ctx.input(|i| i.pointer.any_down())
        && remembered.is_none_or(|[w, h]| (w - size.x).abs() > 0.5 || (h - size.y).abs() > 0.5)
    {
        *remembered = Some([size.x.round(), size.y.round()]);
    }
}

/// The manager's state between frames that the dialog doesn't hold.
#[derive(Clone, Default)]
struct State {
    /// Presets selected for a drag (ids) and the one a Shift-click counts from.
    selected: Vec<String>,
    anchor: Option<String>,
    /// Where a deleted group's own presets would go instead.
    move_to: Option<String>,
}

fn state_id() -> egui::Id {
    egui::Id::new("presetsManagerState")
}

/// One row of the list.
#[derive(Clone, Debug)]
enum Row {
    /// A group: its name, how many presets it holds (in view), whether it is open, whether it
    /// holds built-in presets, whether all of them are hidden.
    Group { name: String, count: usize, open: bool, builtin: bool, all_hidden: bool },
    /// Asking before this group is deleted (under its header).
    Confirm(String),
    /// A preset: its index in the session's presets, its place in its group, hidden, At Import.
    Preset { ix: usize, n: usize, hidden: bool, at_import: bool },
}

/// What the rows were built from.
type RowsKey = (u64, String, usize, usize, usize, Option<String>);

fn fold_key(g: &str) -> String {
    format!("presetsManager:{g}")
}

/// The list's rows for `needle` (lowercase): the groups holding the user's own presets first,
/// then the built-in ones, each in list order; `confirm`: the group asking before a delete.
fn build_rows(app: &LccApp, needle: &str, confirm: Option<&str>) -> Vec<Row> {
    let s = &app.session;
    let hidden: HashSet<&str> = s.preset_hidden.iter().map(String::as_str).collect();
    let at_import: HashSet<&str> = s.preset_at_import.iter().map(String::as_str).collect();
    let mut groups: Vec<(String, Vec<usize>, bool)> = Vec::new();
    let mut place: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for (i, p) in s.presets.iter().enumerate() {
        if !(needle.is_empty() || p.name.to_lowercase().contains(needle) || p.group.to_lowercase().contains(needle)) {
            continue;
        }
        match place.get(&p.group).and_then(|k| groups.get_mut(*k)) {
            Some((_, list, user)) => {
                list.push(i);
                *user |= !p.builtin;
            }
            None => {
                place.insert(p.group.clone(), groups.len());
                groups.push((p.group.clone(), vec![i], !p.builtin));
            }
        }
    }
    // empty groups the user made are groups too (a place to drop presets)
    if needle.is_empty() {
        for f in &s.preset_folders {
            if !place.contains_key(f) {
                place.insert(f.clone(), groups.len());
                groups.push((f.clone(), Vec::new(), true));
            }
        }
    }
    // the user's own groups first (a stable sort keeps each part's order)
    groups.sort_by_key(|g| !g.2);
    let mut out = Vec::new();
    for (g, list, _) in groups {
        // a search opens what it finds
        let open = !needle.is_empty() || app.ui.group_is_open(&fold_key(&g), false);
        let builtin = list.iter().any(|i| s.presets.get(*i).is_some_and(|p| p.builtin));
        let all_hidden = !list.is_empty() && list.iter().all(|i| s.presets.get(*i).is_some_and(|p| hidden.contains(p.id.as_str())));
        out.push(Row::Group { name: g.clone(), count: list.len(), open, builtin, all_hidden });
        if confirm == Some(g.as_str()) {
            out.push(Row::Confirm(g.clone()));
        }
        if open {
            out.extend(list.into_iter().enumerate().filter_map(|(n, ix)| {
                let p = s.presets.get(ix)?;
                Some(Row::Preset { ix, n, hidden: hidden.contains(p.id.as_str()), at_import: at_import.contains(p.id.as_str()) })
            }));
        }
    }
    out
}

/// The rows, rebuilt only when what they are built from changed.
fn rows(ui: &egui::Ui, app: &LccApp, needle: &str, confirm: Option<&str>) -> Arc<Vec<Row>> {
    let open = &app.ui.group_open;
    let key: RowsKey = (
        app.session.presets.generation(),
        needle.to_string(),
        open.len(),
        open.values().filter(|v| **v).count(),
        app.session.preset_folders.len(),
        confirm.map(str::to_string),
    );
    let id = egui::Id::new("presetsManagerRows");
    if let Some((k, rows)) = ui.ctx().data(|d| d.get_temp::<(RowsKey, Arc<Vec<Row>>)>(id))
        && k == key
    {
        return rows;
    }
    let rows = Arc::new(build_rows(app, needle, confirm));
    ui.ctx().data_mut(|d| d.insert_temp(id, (key, rows.clone())));
    rows
}

/// The heights of `rows` (a confirmation is taller).
fn heights(ctx: &egui::Context, rows: &[Row]) -> Vec<f32> {
    let confirm = table::confirm_h(ctx);
    rows.iter().map(|r| if matches!(r, Row::Confirm(_)) { confirm } else { table::ROW_H }).collect()
}

/// The presets of group `g` and the groups inside it.
fn members<'a>(app: &'a LccApp, g: &'a str) -> impl Iterator<Item = &'a lcc_develop::Preset> + 'a {
    let prefix = format!("{g}/");
    app.session.presets.iter().filter(move |p| p.group == g || p.group.starts_with(&prefix))
}

/// The rows drawn last frame (for the tests: only those in view are laid out).
pub fn drawn_rows(ctx: &egui::Context) -> Option<(usize, usize)> {
    ctx.data(|d| d.get_temp::<(usize, usize)>(egui::Id::new("presetsManagerDrawn")))
}

/// The dialog's body. `query`: the search; `confirm`: the row whose Delete (or Restore) waits for
/// a yes (`preset:<id>`, `group:<group>`, `restore`); `rename`: the row renamed in place and its
/// text (`preset:<id>`, `group:<group>`, `new` for New Group).
pub fn body(app: &mut LccApp, ui: &mut egui::Ui, query: &mut String, confirm: &mut Option<String>, rename: &mut Option<(String, String)>) {
    let t = Tokens::get(ui.ctx());
    let mut st: State = ui.ctx().data(|m| m.get_temp(state_id())).unwrap_or_default();
    let mut actions: Vec<(&'static str, Value)> = Vec::new();
    toolbar(app, ui, &t, confirm, rename, &mut actions);
    // the search, and how many there are
    ui.horizontal_wrapped(|ui| {
        crate::text_field::TextField::singleline("field:presetsManagerSearch", query)
            .hint(tr("Search by name or group"))
            .width(260.0_f32.min(ui.available_width()).max(60.0))
            .show(ui);
        let n = crate::panels::grid::group_digits(app.session.presets.len());
        let h = crate::panels::grid::group_digits(app.session.preset_hidden.len());
        ui.label(egui::RichText::new(tr_format!("{n} presets, {h} hidden", n = n, h = h)).color(t.text_dim));
    });
    ui.add(
        egui::Label::new(
            egui::RichText::new(tr(
                "Hidden presets leave the Presets panel and the pickers; At Import ones come first when importing. Drag presets onto a group to move them there (Ctrl-click or Shift-click picks several); double-click a name to rename it.",
            ))
            .color(t.text_dim)
            .size(11.5),
        )
        .wrap(),
    );
    let needle = query.trim().to_lowercase();
    let confirm_group = confirm.as_deref().and_then(|c| c.strip_prefix("group:")).map(str::to_string);
    let list = rows(ui, app, &needle, confirm_group.as_deref());
    let all_groups: Vec<String> = {
        let mut g: Vec<String> = Vec::new();
        let mut seen = HashSet::new();
        for p in app.session.presets.iter() {
            if seen.insert(p.group.as_str()) {
                g.push(p.group.clone());
            }
        }
        for f in &app.session.preset_folders {
            if seen.insert(f.as_str()) {
                g.push(f.clone());
            }
        }
        g
    };
    // the rest of the window, less the footer below (measured last frame)
    let footer = ui.ctx().data(|d| d.get_temp::<f32>(egui::Id::new(WINDOW).with("dialog-footer"))).unwrap_or(48.0).clamp(0.0, 200.0);
    // (with room to spare: a body taller than the window would make the window grow to fit it, a
    // little more each frame)
    let spacing = ui.spacing().item_spacing.y;
    let height = (ui.max_rect().bottom() - ui.cursor().top() - footer - 2.0 * spacing - 12.0).max(120.0);
    let hs = heights(ui.ctx(), &list);
    let drawn = table::virtual_list(ui, "presetsManagerList", &hs, height, |ui, i| {
        let Some(row) = list.get(i) else { return };
        match row {
            Row::Group { name, count, open, builtin, all_hidden } => {
                let head = GroupHead {
                    key: KEY,
                    name,
                    shown: crate::i18n::builtin_label(name, *builtin).to_string(),
                    count: *count,
                    open: *open,
                    all_hidden: *all_hidden,
                    deletable: !*builtin,
                    renamable: true,
                };
                let mut renaming = rename.as_ref().and_then(|(k, text)| k.strip_prefix("group:").map(|g| (g.to_string(), text.clone())));
                let was = renaming.clone();
                let act = table::group_header(ui, &t, &COLS, &head, &mut renaming);
                if renaming != was {
                    *rename = renaming.map(|(g, text)| (format!("group:{g}"), text));
                }
                match act {
                    Some(GroupAction::Fold(o)) => {
                        app.ui.group_open.insert(fold_key(name), o);
                    }
                    Some(GroupAction::Hide(h)) => actions.push(("preset.hideGroup", json!({"group": name, "hidden": h}))),
                    Some(GroupAction::Rename(n)) => actions.push(("preset.renameGroup", json!({"group": name, "name": n}))),
                    Some(GroupAction::Delete) => {
                        *confirm = Some(format!("group:{name}"));
                        st.move_to = None;
                    }
                    Some(GroupAction::DropItems(ids)) => {
                        actions.extend(ids.into_iter().map(|id| ("preset.move", json!({"id": id, "group": name}))));
                    }
                    Some(GroupAction::DropGroup(g)) => actions.push(("preset.moveGroup", json!({"group": g, "parent": name}))),
                    None => {}
                }
            }
            Row::Confirm(g) => {
                let ids: Vec<String> = members(app, g).map(|p| p.id.clone()).collect();
                let others: Vec<String> = all_groups.iter().filter(|o| *o != g && !o.starts_with(&format!("{g}/"))).cloned().collect();
                match confirm_group_delete(ui, &t, g, ids.len(), &others, &mut st.move_to) {
                    Some(Some(to)) => {
                        actions.extend(ids.iter().map(|id| ("preset.move", json!({"id": id, "group": to}))));
                        actions.push(("preset.deleteGroup", json!({"group": g})));
                        *confirm = None;
                    }
                    Some(None) => {
                        actions.push(("preset.deleteGroup", json!({"group": g})));
                        *confirm = None;
                    }
                    None => {}
                }
                if ui.ctx().data(|d| d.get_temp::<bool>(egui::Id::new("presetsManagerCancelGroup"))).unwrap_or(false) {
                    ui.ctx().data_mut(|d| d.remove::<bool>(egui::Id::new("presetsManagerCancelGroup")));
                    *confirm = None;
                }
            }
            Row::Preset { ix, n, hidden, at_import } => {
                let row = PresetRow { ix: *ix, n: *n, hidden: *hidden, at_import: *at_import };
                preset_row(app, ui, &t, &mut st, row, &list, &all_groups, confirm, rename, &mut actions);
            }
        }
    });
    ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("presetsManagerDrawn"), (drawn.start, drawn.end)));
    let presets = &app.session.presets;
    table::drag_tick(ui.ctx(), &t, KEY, |d| match d {
        Dragged::Items(ids) if ids.len() == 1 => ids
            .first()
            .and_then(|id| presets.iter().find(|p| &p.id == id))
            .map(|p| crate::i18n::builtin_label(&p.name, p.builtin).to_string())
            .unwrap_or_default(),
        Dragged::Items(ids) => tr_format!("{n} presets", n = ids.len()),
        Dragged::Group(g) => g.clone(),
    });
    ui.ctx().data_mut(|m| m.insert_temp(state_id(), st));
    for (cmd, p) in actions {
        if matches!(cmd, "preset.delete" | "preset.restoreBuiltins") {
            *confirm = None;
        }
        match app.run(cmd, p) {
            Ok(r) if cmd == "preset.restoreBuiltins" => {
                let n = r.get("restored").and_then(Value::as_u64).unwrap_or(0);
                app.toast(ui.ctx(), tr_format!("Built-in presets restored ({n} changes undone)", n = n));
            }
            Ok(_) => {}
            Err(e) => {
                app.toast_error(ui.ctx(), e);
                break;
            }
        }
    }
}

/// Import, Export, New Group and Restore Built-ins, wrapped to the window's width.
fn toolbar(
    app: &mut LccApp,
    ui: &mut egui::Ui,
    t: &Tokens,
    confirm: &mut Option<String>,
    rename: &mut Option<(String, String)>,
    actions: &mut Vec<(&'static str, Value)>,
) {
    ui.horizontal_wrapped(|ui| {
        let b = ui.button(tr("Import…"));
        register(ui.ctx(), "button:presetsManagerImport", b.rect);
        if b.clicked() {
            actions.push(("file.importPresets", json!({})));
        }
        let b = ui.add_enabled(app.services.pick_folder.is_some(), egui::Button::new(tr("Import Folder…")));
        register(ui.ctx(), "button:presetsManagerImportFolder", b.rect);
        if b.clicked()
            && let Some(dir) = app.services.pick_folder.as_mut().and_then(|f| f())
        {
            actions.push(("file.importPresets", json!({"paths": [dir]})));
        }
        let any_user = app.session.presets.iter().any(|p| !p.builtin);
        let b = ui.add_enabled(any_user, egui::Button::new(tr("Export User Presets…")));
        register(ui.ctx(), "button:presetsManagerExport", b.rect);
        if b.clicked() {
            actions.push(("file.exportPresets", json!({})));
        }
        let b = ui.add_enabled(any_user && app.services.pick_folder.is_some(), egui::Button::new(tr("Export for Lightroom…")));
        register(ui.ctx(), "button:presetsManagerExportXmp", b.rect);
        if b.clicked()
            && let Some(dir) = app.services.pick_folder.as_mut().and_then(|f| f())
        {
            let ids: Vec<String> = app.session.presets.iter().filter(|p| !p.builtin).map(|p| p.id.clone()).collect();
            actions.push(("preset.exportXmp", json!({"ids": ids, "folder": dir})));
        }
        if let Some((_, text)) = rename.as_mut().filter(|(k, _)| k == "new") {
            ui.label(egui::RichText::new(tr("New group:")).color(t.text_label));
            let r = crate::text_field::TextField::singleline("field:presetsManagerNewGroupName", text).width(220.0).show(ui);
            if !r.editing && r.ending.is_none() {
                r.response.request_focus();
            }
            if r.committed() {
                let name = text.trim().to_string();
                if !name.is_empty() {
                    actions.push(("preset.createGroup", json!({"name": name})));
                }
                *rename = None;
            } else if r.cancelled() {
                *rename = None;
            }
        } else {
            let b = ui.button(tr("New Group…"));
            register(ui.ctx(), "button:presetsManagerNewGroup", b.rect);
            if b.clicked() {
                *rename = Some(("new".into(), String::new()));
            }
        }
        if confirm.as_deref() == Some("restore") {
            let b = ui.button(tr("Restore"));
            register(ui.ctx(), "button:presetsManagerRestoreConfirm", b.rect);
            if b.clicked() {
                actions.push(("preset.restoreBuiltins", json!({})));
            }
            if ui.button(tr("Cancel")).clicked() {
                *confirm = None;
            }
        } else {
            let b = ui.button(tr("Restore Built-ins")).on_hover_text(tr("Built-in presets get their own names and groups back and are shown again"));
            register(ui.ctx(), "button:presetsManagerRestore", b.rect);
            if b.clicked() {
                *confirm = Some("restore".into());
            }
        }
    });
}

/// Asking before group `g` is deleted, in a box under its header: its `count` presets are deleted
/// too, or moved to another group first. `Some(Some(group))`: move them there, then delete;
/// `Some(None)`: delete everything. Cancel sets a flag the caller reads.
fn confirm_group_delete(
    ui: &mut egui::Ui,
    t: &Tokens,
    g: &str,
    count: usize,
    others: &[String],
    move_to: &mut Option<String>,
) -> Option<Option<String>> {
    let mut out = None;
    let framed = egui::Frame::NONE
        .fill(t.inset)
        .stroke(egui::Stroke::new(1.0, t.caution.gamma_multiply(0.7)))
        .corner_radius(4.0)
        .inner_margin(egui::Margin { left: 30, right: 8, top: 6, bottom: 6 })
        .show(ui, |ui| {
            ui.set_width(ui.available_width().max(0.0));
            ui.spacing_mut().item_spacing.y = 6.0;
            let text = if count == 0 {
                tr_format!("Delete the group “{name}”?", name = g)
            } else {
                tr_format!("Delete the group “{name}” and its {n} presets? Or move them to another group first.", name = g, n = count)
            };
            let r = ui.add(egui::Label::new(egui::RichText::new(text).color(t.text)).wrap());
            register(ui.ctx(), format!("label:presetsManagerGroupDeleteAsk:{g}"), r.rect);
            ui.horizontal_wrapped(|ui| {
                if count > 0 && !others.is_empty() {
                    let label = move_to.clone().unwrap_or_else(|| tr("Move to…").to_string());
                    let r =
                        crate::widgets::combo_box(("presetsManagerMoveTo", g)).width(200.0).selected_text(label).height(300.0).show_ui(ui, |ui| {
                            for o in others {
                                if ui.selectable_label(move_to.as_deref() == Some(o.as_str()), o.as_str()).clicked() {
                                    *move_to = Some(o.clone());
                                }
                            }
                        });
                    register(ui.ctx(), format!("combo:presetsManagerMoveTo:{g}"), r.response.rect);
                    if let Some(to) = move_to.clone() {
                        let r = ui.button(tr("Move Them and Delete the Group"));
                        register(ui.ctx(), format!("button:presetsManagerGroupMove:{g}"), r.rect);
                        if r.clicked() {
                            out = Some(Some(to));
                        }
                    }
                }
                let r = ui.button(if count > 0 { tr("Delete Everything") } else { tr("Delete") });
                register(ui.ctx(), format!("button:presetsManagerGroupDeleteConfirm:{g}"), r.rect);
                if r.clicked() {
                    out = Some(None);
                }
                let r = ui.button(tr("Cancel"));
                register(ui.ctx(), format!("button:presetsManagerGroupDeleteCancel:{g}"), r.rect);
                if r.clicked() {
                    ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("presetsManagerCancelGroup"), true));
                }
            });
        });
    table::note_confirm_h(ui.ctx(), framed.response.rect.height() + 4.0);
    out
}

/// What a preset's row shows, from the cached rows.
#[derive(Clone, Copy)]
struct PresetRow {
    ix: usize,
    n: usize,
    hidden: bool,
    at_import: bool,
}

/// One preset's row: favourite, name, details, then At Import, Rename, Group, Hide and Delete in
/// their columns.
#[allow(clippy::too_many_arguments)]
fn preset_row(
    app: &LccApp,
    ui: &mut egui::Ui,
    t: &Tokens,
    st: &mut State,
    row: PresetRow,
    rows: &[Row],
    groups: &[String],
    confirm: &mut Option<String>,
    rename: &mut Option<(String, String)>,
    actions: &mut Vec<(&'static str, Value)>,
) {
    let Some(p) = app.session.presets.get(row.ix) else { return };
    let (id, builtin, hidden) = (p.id.clone(), p.builtin, row.hidden);
    let name = crate::i18n::builtin_label(&p.name, builtin).to_string();
    let row_key = format!("preset:{id}");
    let sz = egui::vec2(24.0, 24.0);
    // asking before a delete takes the row
    if confirm.as_deref() == Some(row_key.as_str()) {
        match table::confirm_item(ui, t, &COLS, &format!("presetsManager:{id}"), &tr_format!("Delete “{name}”?", name = name)) {
            Some(true) => actions.push(("preset.delete", json!({"id": id}))),
            Some(false) => *confirm = None,
            None => {}
        }
        return;
    }
    let (rect, resp, rid) = table::row(ui, table::ROW_H, ("preset", &id));
    table::paint_item(ui, t, rect, row.n, resp.contains_pointer());
    let selected = st.selected.contains(&id);
    if selected {
        table::paint_selected(ui, t, rect);
    }
    let renaming = rename.as_ref().is_some_and(|(k, _)| *k == row_key);
    let rr = table::RowRects::new(rect, rid, &COLS, table::ITEM_NAME_LEFT);
    if resp.double_clicked() && !renaming {
        *rename = Some((row_key.clone(), name.clone()));
    } else if resp.clicked() {
        let order: Vec<String> = rows
            .iter()
            .filter_map(|r| if let Row::Preset { ix, .. } = r { app.session.presets.get(*ix).map(|q| q.id.clone()) } else { None })
            .collect();
        let m = ui.input(|i| i.modifiers);
        table::select_click(&mut st.selected, &mut st.anchor, &id, &order, m);
    }
    if resp.drag_started() && !renaming {
        let ids = if selected { st.selected.clone() } else { vec![id.clone()] };
        table::drag_start(ui.ctx(), KEY, Dragged::Items(ids));
    }
    // dropped on a preset of another group: into that group
    if let Some((Dragged::Items(ids), true)) = table::drop_target(ui, t, KEY, rect)
        && !ids.contains(&id)
    {
        actions.extend(ids.into_iter().map(|i| ("preset.move", json!({"id": i, "group": p.group}))));
    }
    let fav = rr.in_lead(ui, table::item_star(rect), |ui| {
        icon_button(ui, &format!("presetsManagerStar:{id}"), if p.favorite { Icon::StarFilled } else { Icon::Star }, sz, p.favorite, true, "Favorite")
    });
    if fav.clicked() {
        actions.push(("preset.favorite", json!({"id": id, "favorite": !p.favorite})));
    }
    if renaming {
        let mut done = None;
        if let Some((_, text)) = rename.as_mut() {
            rr.in_name(ui, |ui| {
                let r = crate::text_field::TextField::singleline(&format!("field:presetsManagerName:{id}"), text)
                    .width(rr.name.width().clamp(40.0, 380.0))
                    .select_on_focus(true)
                    .show(ui);
                if !r.editing && r.ending.is_none() {
                    r.response.request_focus();
                }
                if r.committed() {
                    done = Some(Some(text.trim().to_string()));
                } else if r.cancelled() {
                    done = Some(None);
                }
            });
        }
        if let Some(n) = done {
            *rename = None;
            if let Some(n) = n.filter(|n| !n.is_empty() && *n != name) {
                actions.push(("preset.rename", json!({"id": id, "name": n})));
            }
        }
    } else {
        table::text_in(ui, rr.name, &name, t.font(13.0), if hidden { t.text_disabled } else { t.text });
        register(ui.ctx(), format!("presetsManagerRow:{id}"), rr.name);
    }
    let mut info: Vec<String> = Vec::new();
    if builtin {
        info.push(tr("Built-in").to_string());
    }
    if hidden {
        info.push(tr("Hidden").to_string());
    }
    table::info_in(ui, t, rr.info, &info.join(" · "));
    let tip = if row.at_import { "Take out of At Import" } else { "At Import: shown first when importing" };
    if rr
        .in_col(ui, Col::ForImport, |ui| icon_button(ui, &format!("presetsManagerAtImport:{id}"), Icon::Check, sz, row.at_import, true, tip))
        .clicked()
    {
        actions.push(("preset.forImport", json!({"id": id, "on": !row.at_import})));
    }
    if rr.in_col(ui, Col::Rename, |ui| icon_button(ui, &format!("presetsManagerRename:{id}"), Icon::Pencil, sz, false, true, "Rename")).clicked() {
        *rename = Some((row_key.clone(), name.clone()));
    }
    let r = rr.in_col(ui, Col::Group, |ui| {
        crate::widgets::menu_button(ui, tr("Group"), |ui| {
            ui.set_min_width(200.0);
            egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                for g in groups {
                    if ui.selectable_label(*g == p.group, g.as_str()).clicked() {
                        actions.push(("preset.move", json!({"id": id, "group": g})));
                        ui.close();
                    }
                }
            });
        })
    });
    register(ui.ctx(), format!("presetsManagerGroupMenu:{id}"), r.response.rect);
    let (icon, tip) = if hidden { (Icon::EyeOff, "Show in the Presets panel") } else { (Icon::Eye, "Hide from the Presets panel") };
    if rr.in_col(ui, Col::Hide, |ui| icon_button(ui, &format!("presetsManagerHide:{id}"), icon, sz, hidden, true, tip)).clicked() {
        actions.push(("preset.hide", json!({"id": id, "hidden": !hidden})));
    }
    let tip = if builtin { "Built-in presets can be hidden, not deleted" } else { "Delete" };
    if rr.in_col(ui, Col::Delete, |ui| icon_button(ui, &format!("presetsManagerDelete:{id}"), Icon::Trash, sz, false, !builtin, tip)).clicked() {
        *confirm = Some(row_key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_fits_the_screen() {
        let small = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        let big = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(3840.0, 2160.0));
        let m = max_size(small);
        let s = window_size(None, small);
        assert!(s.x <= m.x && s.y <= m.y, "{s:?} inside {m:?}");
        // a remembered size larger than this screen is cut to it
        let s = window_size(Some([5000.0, 4000.0]), small);
        assert!(s.x <= m.x && s.y <= m.y, "{s:?}");
        // a sensible first size on a 4K screen, not the screen's width
        let s = window_size(None, big);
        assert_eq!(s.x, DEFAULT_W);
        assert!(s.y <= 900.0);
        // the size the user chose is kept
        assert_eq!(window_size(Some([1200.0, 700.0]), big), egui::vec2(1200.0, 700.0));
        // nonsense reads as none
        assert_eq!(window_size(Some([f32::NAN, 700.0]), big), window_size(None, big));
    }
}
