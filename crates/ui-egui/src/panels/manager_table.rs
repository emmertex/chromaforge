//! The table the Presets Manager draws its list as (ported from OneSlips' managers):
//! one row per group or preset, every row the same height. A group's header is a shaded band; its
//! open presets hang off an indent guide, alternate faintly and light up under the pointer. The
//! actions sit in fixed columns at the right edge, the same for groups and presets, so they form
//! columns down the list whatever the names' lengths; the count and the details are a muted column
//! just left of them. Confirmations open in the list at the row they are about (never at the top).
//!
//! Every row is exactly the list's width and every name is cut short to fit its column, so the
//! table never measures wider than the window gives it (a window that grows to fit its content
//! would otherwise widen itself frame after frame). Long lists draw only the rows in view
//! ([`virtual_list`]), so 10 000+ presets scroll smoothly.

use egui::{Rect, RichText, Sense, Ui, UiBuilder, pos2, vec2};

use crate::i18n::tr;
use crate::icons::Icon;
use crate::theme::Tokens;
use crate::widgets::{icon_button, register};

/// Height of a row (headers too).
pub const ROW_H: f32 = 30.0;
/// Height of a confirmation's row (its text and buttons), until it was drawn once.
pub const CONFIRM_H: f32 = 84.0;
/// Width of an icon action's column.
const ICON_W: f32 = 28.0;
/// Width of the Group menu's column.
const GROUP_W: f32 = 74.0;
/// Where a preset's star sits (inside the indent guide).
const INDENT: f32 = 26.0;
/// Where a preset's name starts.
pub const ITEM_NAME_LEFT: f32 = INDENT + 26.0;
/// Where a group's name starts (after its fold arrow).
pub const GROUP_NAME_LEFT: f32 = 30.0;

/// An action column, left to right as they appear at the row's right edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Col {
    /// At Import (presets only).
    ForImport,
    Rename,
    /// The Group menu (presets only).
    Group,
    Hide,
    Delete,
}

/// The columns of the table.
#[derive(Clone, Copy, Debug)]
pub struct Columns {
    /// The muted details column (count for groups, badges for presets).
    pub info_w: f32,
}

impl Columns {
    fn cols(&self) -> [(Col, f32); 5] {
        [(Col::ForImport, ICON_W), (Col::Rename, ICON_W), (Col::Group, GROUP_W), (Col::Hide, ICON_W), (Col::Delete, ICON_W)]
    }

    /// Width of all the action columns.
    pub fn actions_w(&self) -> f32 {
        self.cols().iter().map(|(_, w)| w).sum::<f32>() + 6.0
    }
}

/// A row's parts: where its name goes, its details, and each action column.
pub struct RowRects {
    pub rect: Rect,
    /// The row's own id: its columns' widgets keep their ids from frame to frame whatever rows
    /// come before (a click lands on the widget it was aimed at).
    pub id: egui::Id,
    /// The name (after the fold arrow or the star).
    pub name: Rect,
    pub info: Rect,
    cols: Vec<(Col, Rect)>,
}

impl RowRects {
    pub fn new(rect: Rect, id: egui::Id, c: &Columns, name_left: f32) -> RowRects {
        let mut x = rect.right() - 6.0;
        let mut cols: Vec<(Col, Rect)> = Vec::new();
        for (col, w) in c.cols().into_iter().rev() {
            cols.push((col, Rect::from_min_max(pos2(x - w, rect.top()), pos2(x, rect.bottom()))));
            x -= w;
        }
        cols.reverse();
        let info = Rect::from_min_max(
            pos2((x - c.info_w - 8.0).max(rect.left() + name_left), rect.top()),
            pos2((x - 8.0).max(rect.left() + name_left), rect.bottom()),
        );
        let name =
            Rect::from_min_max(pos2(rect.left() + name_left, rect.top()), pos2((info.left() - 8.0).max(rect.left() + name_left), rect.bottom()));
        RowRects { rect, id, name, info, cols }
    }

    /// The rectangle of column `col`.
    pub fn col(&self, col: Col) -> Rect {
        self.cols.iter().find(|(c, _)| *c == col).map_or(Rect::from_min_size(self.rect.right_top(), vec2(0.0, 0.0)), |(_, r)| *r)
    }

    /// Run `f` in a child UI filling column `col`, its contents centred.
    pub fn in_col<R>(&self, ui: &mut Ui, col: Col, f: impl FnOnce(&mut Ui) -> R) -> R {
        let r = self.col(col);
        let b = UiBuilder::new().id_salt((self.id, col as u8)).max_rect(r).layout(egui::Layout::centered_and_justified(egui::Direction::LeftToRight));
        ui.scope_builder(b, |ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            f(ui)
        })
        .inner
    }

    /// Run `f` in a child UI over the name's place, laid out left to right.
    pub fn in_name<R>(&self, ui: &mut Ui, f: impl FnOnce(&mut Ui) -> R) -> R {
        let b = UiBuilder::new().id_salt((self.id, "name")).max_rect(self.name).layout(egui::Layout::left_to_right(egui::Align::Center));
        ui.scope_builder(b, f).inner
    }

    /// Run `f` in a child UI over `r` (a preset's star, a group's fold arrow).
    pub fn in_lead<R>(&self, ui: &mut Ui, r: Rect, f: impl FnOnce(&mut Ui) -> R) -> R {
        ui.scope_builder(UiBuilder::new().id_salt((self.id, "lead")).max_rect(r), f).inner
    }
}

/// Allocate a full-width row `h` high (at the cursor) that senses clicks and drags, with an id of
/// its own (`key`: what the row shows, unique in the list).
pub fn row(ui: &mut Ui, h: f32, key: impl std::hash::Hash + std::fmt::Debug) -> (Rect, egui::Response, egui::Id) {
    let (_, rect) = ui.allocate_space(vec2(ui.available_width().max(0.0), h));
    let id = egui::Id::new(("managerRow", key));
    (rect, ui.interact(rect, id, Sense::click_and_drag()), id)
}

/// Click, Ctrl/⌘-click (add or take out) and Shift-click (a run of the list) on row `id`; `order`
/// is the rows as listed.
pub fn select_click(selected: &mut Vec<String>, anchor: &mut Option<String>, id: &str, order: &[String], m: egui::Modifiers) {
    if m.shift
        && let Some(a) = anchor.as_ref()
        && let (Some(i), Some(j)) = (order.iter().position(|x| x == a), order.iter().position(|x| x == id))
    {
        let (lo, hi) = if i <= j { (i, j) } else { (j, i) };
        *selected = order.get(lo..=hi).map(<[String]>::to_vec).unwrap_or_default();
        return;
    }
    if m.command || m.ctrl {
        if let Some(k) = selected.iter().position(|x| x == id) {
            selected.remove(k);
        } else {
            selected.push(id.to_string());
        }
    } else {
        *selected = vec![id.to_string()];
    }
    *anchor = Some(id.to_string());
}

/// A selected row's highlight.
pub fn paint_selected(ui: &Ui, t: &Tokens, rect: Rect) {
    ui.painter().rect_filled(rect.shrink2(vec2(0.0, 1.0)), 3.0, t.accent.gamma_multiply(0.22));
}

/// What is dragged in the list: presets (several at once) or a group.
#[derive(Clone, Debug, PartialEq)]
pub enum Dragged {
    Items(Vec<String>),
    Group(String),
}

fn drag_id(key: &str) -> egui::Id {
    egui::Id::new(("managerDrag", key))
}

/// Start dragging `d` in the list `key`.
pub fn drag_start(ctx: &egui::Context, key: &str, d: Dragged) {
    ctx.data_mut(|m| m.insert_temp(drag_id(key), Some(d)));
}

/// What is being dragged in the list `key`.
pub fn dragged(ctx: &egui::Context, key: &str) -> Option<Dragged> {
    ctx.data(|m| m.get_temp::<Option<Dragged>>(drag_id(key))).flatten()
}

/// While something is dragged in the list `key` over row `rect`: the row outlined as the place
/// it would go, and whether it was dropped this frame.
pub fn drop_target(ui: &Ui, t: &Tokens, key: &str, rect: Rect) -> Option<(Dragged, bool)> {
    let d = dragged(ui.ctx(), key)?;
    let pos = ui.input(|i| i.pointer.latest_pos())?;
    if !rect.contains(pos) || !ui.clip_rect().contains(pos) {
        return None;
    }
    ui.painter().rect_stroke(rect.shrink(1.0), 3.0, egui::Stroke::new(2.0, t.accent), egui::StrokeKind::Inside);
    let released = ui.input(|i| i.pointer.any_released());
    Some((d, released))
}

/// Once a frame after the list `key` is drawn: the label following the pointer while something is
/// dragged (`label`: what it says), and the end of the drag when the button is up (the rows act
/// on the release first).
pub fn drag_tick(ctx: &egui::Context, t: &Tokens, key: &str, label: impl FnOnce(&Dragged) -> String) {
    let Some(d) = dragged(ctx, key) else { return };
    let (released, down, pos) = ctx.input(|i| (i.pointer.any_released(), i.pointer.any_down(), i.pointer.latest_pos()));
    if released || !down {
        ctx.data_mut(|m| m.insert_temp::<Option<Dragged>>(drag_id(key), None));
        return;
    }
    let Some(pos) = pos else { return };
    ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
    let text = label(&d);
    egui::Area::new(egui::Id::new(("managerDragLabel", key))).order(egui::Order::Tooltip).interactable(false).fixed_pos(pos + vec2(14.0, 10.0)).show(
        ctx,
        |ui| {
            egui::Frame::NONE.fill(t.accent).corner_radius(10.0).inner_margin(egui::Margin::symmetric(9, 3)).show(ui, |ui| {
                let label = ui.add(egui::Label::new(RichText::new(text).color(egui::Color32::WHITE).font(t.semibold(12.0))).extend());
                register(ui.ctx(), format!("drag:{key}"), label.rect);
            });
        },
    );
}

/// Paint a group header's band.
pub fn paint_group(ui: &Ui, t: &Tokens, rect: Rect, hovered: bool) {
    let fill = if hovered { t.hover } else { t.inset };
    ui.painter().rect_filled(rect.shrink2(vec2(0.0, 1.0)), 3.0, fill);
}

/// Paint a preset's row: faint alternate shading (`n`: its place in the group), the hover
/// highlight, and the group's indent guide.
pub fn paint_item(ui: &Ui, t: &Tokens, rect: Rect, n: usize, hovered: bool) {
    if hovered {
        ui.painter().rect_filled(rect, 2.0, t.hover.gamma_multiply(0.8));
    } else if n % 2 == 1 {
        ui.painter().rect_filled(rect, 0.0, t.inset.gamma_multiply(0.45));
    }
    let x = rect.left() + 14.0;
    ui.painter().line_segment([pos2(x, rect.top()), pos2(x, rect.bottom())], egui::Stroke::new(1.0, t.divider));
    ui.painter().line_segment(
        [pos2(rect.left() + 20.0, rect.bottom()), pos2(rect.right(), rect.bottom())],
        egui::Stroke::new(0.5, t.divider.gamma_multiply(0.5)),
    );
}

/// Draw text left-aligned and cut short to fit `r`.
pub fn text_in(ui: &Ui, r: Rect, text: &str, font: egui::FontId, color: egui::Color32) {
    let g = crate::widgets::one_line(ui.painter(), text, font, color, (r.width() - 2.0).max(8.0));
    ui.painter().with_clip_rect(r.intersect(ui.clip_rect())).galley(pos2(r.left(), r.center().y - g.size().y / 2.0), g, color);
}

/// Draw muted text right-aligned in `r` (the details column).
pub fn info_in(ui: &Ui, t: &Tokens, r: Rect, text: &str) {
    let g = crate::widgets::one_line(ui.painter(), text, t.font(11.0), t.text_dim, (r.width() - 2.0).max(8.0));
    ui.painter().with_clip_rect(r.intersect(ui.clip_rect())).galley(pos2(r.right() - g.size().x, r.center().y - g.size().y / 2.0), g, t.text_dim);
}

/// A group's header row as the table draws it.
pub struct GroupHead<'a> {
    /// Widget id prefix (`{key}GroupFold:{name}`, `{key}GroupHide:{name}`…) and the drag's key.
    pub key: &'a str,
    pub name: &'a str,
    /// The name as shown (a built-in group's in the UI language).
    pub shown: String,
    pub count: usize,
    pub open: bool,
    pub all_hidden: bool,
    /// Delete is offered (groups holding built-in presets are hidden, not deleted).
    pub deletable: bool,
    /// Rename is offered (a camera model's group of camera profiles keeps its name).
    pub renamable: bool,
}

/// What a group header asked for.
pub enum GroupAction {
    Fold(bool),
    Hide(bool),
    Rename(String),
    Delete,
    /// Presets dropped on the header: into this group.
    DropItems(Vec<String>),
    /// Group `group` dropped on this header: inside this one.
    DropGroup(String),
}

/// A group's header row: a shaded band with the fold arrow, its name and count; Rename, Hide /
/// Show and Delete in their columns. Clicking the band folds it; double-clicking its name renames
/// it; dragging it moves the group.
pub fn group_header(ui: &mut Ui, t: &Tokens, c: &Columns, h: &GroupHead, renaming: &mut Option<(String, String)>) -> Option<GroupAction> {
    let mut out = None;
    let sz = vec2(24.0, 24.0);
    let (rect, resp, id) = row(ui, ROW_H, (h.key, "group", h.name));
    register(ui.ctx(), format!("{}Group:{}", h.key, h.name), rect);
    paint_group(ui, t, rect, resp.contains_pointer());
    let renaming_this = renaming.as_ref().is_some_and(|(rg, _)| rg == h.name);
    if resp.drag_started() && renaming.is_none() {
        drag_start(ui.ctx(), h.key, Dragged::Group(h.name.to_string()));
    }
    if let Some((d, true)) = drop_target(ui, t, h.key, rect) {
        out = match d {
            Dragged::Items(ids) => Some(GroupAction::DropItems(ids)),
            Dragged::Group(g) if g != h.name && !h.name.starts_with(&format!("{g}/")) => Some(GroupAction::DropGroup(g)),
            Dragged::Group(_) => None,
        };
    }
    let rr = RowRects::new(rect, id, c, GROUP_NAME_LEFT);
    let fold = Rect::from_min_size(pos2(rect.left() + 3.0, rect.center().y - 12.0), sz);
    let fold_r = rr.in_lead(ui, fold, |ui| {
        icon_button(
            ui,
            &format!("{}GroupFold:{}", h.key, h.name),
            if h.open { Icon::ChevronDown } else { Icon::ChevronRight },
            sz,
            false,
            true,
            if h.open { "Collapse" } else { "Expand" },
        )
    });
    if h.renamable && resp.double_clicked() && rr.name.contains(resp.interact_pointer_pos().unwrap_or_default()) {
        *renaming = Some((h.name.to_string(), leaf(h.name).to_string()));
    } else if fold_r.clicked() || (resp.clicked() && !renaming_this) {
        out = out.or(Some(GroupAction::Fold(!h.open)));
    }
    let mut done = None;
    if let Some((rg, text)) = renaming.as_mut().filter(|(rg, _)| rg == h.name) {
        rr.in_name(ui, |ui| {
            let r = crate::text_field::TextField::singleline(&format!("field:{}GroupRename:{rg}", h.key), text)
                .width(rr.name.width().clamp(40.0, 300.0))
                .select_on_focus(true)
                .show(ui);
            if !r.editing && r.ending.is_none() {
                r.response.request_focus();
            }
            if r.committed() {
                done = Some(Some(text.trim().to_string()).filter(|n| !n.is_empty() && n != leaf(h.name)));
            } else if r.cancelled() {
                done = Some(None);
            }
        });
    } else {
        let color = if h.all_hidden { t.text_disabled } else { t.text_label };
        text_in(ui, rr.name, &h.shown, t.semibold(13.0), color);
        register(ui.ctx(), format!("{}GroupHead:{}", h.key, h.name), rr.name);
        let count = crate::panels::grid::group_digits(h.count);
        let count = if h.all_hidden { format!("{} · {count}", tr("Hidden")) } else { count };
        info_in(ui, t, rr.info, &count);
    }
    if let Some(name) = done {
        *renaming = None;
        if let Some(n) = name {
            out = Some(GroupAction::Rename(n));
        }
    }
    if rr
        .in_col(ui, Col::Rename, |ui| {
            icon_button(ui, &format!("{}GroupRename:{}", h.key, h.name), Icon::Pencil, sz, false, h.renamable, "Rename Group")
        })
        .clicked()
    {
        *renaming = Some((h.name.to_string(), leaf(h.name).to_string()));
    }
    let (icon, tip) =
        if h.all_hidden { (Icon::EyeOff, "Show the group in the Presets panel") } else { (Icon::Eye, "Hide the group from the Presets panel") };
    if rr.in_col(ui, Col::Hide, |ui| icon_button(ui, &format!("{}GroupHide:{}", h.key, h.name), icon, sz, h.all_hidden, h.count > 0, tip)).clicked() {
        out = Some(GroupAction::Hide(!h.all_hidden));
    }
    let tip = if h.deletable { "Delete Group" } else { "Groups with built-in presets can be hidden, not deleted" };
    if rr
        .in_col(ui, Col::Delete, |ui| icon_button(ui, &format!("{}GroupDelete:{}", h.key, h.name), Icon::Trash, sz, false, h.deletable, tip))
        .clicked()
    {
        out = Some(GroupAction::Delete);
    }
    out
}

/// A group path's last part (what Rename edits).
pub fn leaf(group: &str) -> &str {
    group.rsplit_once('/').map_or(group, |(_, l)| l)
}

fn confirm_h_id() -> egui::Id {
    egui::Id::new("managerTableConfirmH")
}

/// Record the height a confirmation box was drawn at (a virtual list makes its row that tall).
pub fn note_confirm_h(ctx: &egui::Context, h: f32) {
    if h.is_finite() {
        ctx.data_mut(|d| d.insert_temp(confirm_h_id(), h));
    }
}

/// The height of a group's confirmation box (as drawn last, else a guess).
pub fn confirm_h(ctx: &egui::Context) -> f32 {
    ctx.data(|d| d.get_temp::<f32>(confirm_h_id())).filter(|h| h.is_finite() && *h > 20.0).unwrap_or(CONFIRM_H).min(400.0)
}

/// Asking before one preset is deleted, in place of its row: the question at its name, Delete and
/// Cancel in the action columns' place. `Some(true)`: delete; `Some(false)`: cancelled.
pub fn confirm_item(ui: &mut Ui, t: &Tokens, c: &Columns, key: &str, question: &str) -> Option<bool> {
    let (rect, _, _) = row(ui, ROW_H, (key, "confirm"));
    ui.painter().rect_filled(rect, 3.0, t.inset);
    ui.painter().rect_stroke(rect.shrink(0.5), 3.0, egui::Stroke::new(1.0, t.caution.gamma_multiply(0.7)), egui::StrokeKind::Inside);
    let actions = Rect::from_min_max(pos2((rect.right() - c.actions_w().max(180.0)).max(rect.left()), rect.top()), rect.right_bottom());
    let text = Rect::from_min_max(pos2(rect.left() + INDENT, rect.top()), pos2((actions.left() - 8.0).max(rect.left() + INDENT), rect.bottom()));
    text_in(ui, text, question, t.font(13.0), t.text);
    let mut out = None;
    ui.scope_builder(
        UiBuilder::new()
            .id_salt((key, "confirmButtons"))
            .max_rect(actions.shrink2(vec2(4.0, 2.0)))
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
        |ui| {
            let r = ui.button(tr("Cancel"));
            register(ui.ctx(), format!("button:{key}DeleteCancel"), r.rect);
            if r.clicked() {
                out = Some(false);
            }
            let r = ui.button(tr("Delete"));
            register(ui.ctx(), format!("button:{key}DeleteConfirm"), r.rect);
            if r.clicked() {
                out = Some(true);
            }
        },
    );
    out
}

/// The place of a preset's star (the name follows it).
pub fn item_star(rect: Rect) -> Rect {
    Rect::from_min_size(pos2(rect.left() + INDENT - 4.0, rect.center().y - 12.0), vec2(24.0, 24.0))
}

/// The rows `first..end` of a list whose row `i` starts at `tops[i]` (with `tops[n]` its end)
/// that meet the band `y0..y1` of it.
pub fn rows_in_view(tops: &[f32], y0: f32, y1: f32) -> (usize, usize) {
    let n = tops.len().saturating_sub(1);
    let first = tops.partition_point(|t| *t <= y0).saturating_sub(1).min(n);
    let end = tops.partition_point(|t| *t < y1).min(n).max(first);
    (first, end)
}

/// A scrolling list of rows of different heights (`heights`), `height` tall and as wide as the
/// room it is given, drawing only those in view: `draw` gets each row's index and is laid out at
/// its place. Returns the rows drawn.
pub fn virtual_list(ui: &mut Ui, id: &str, heights: &[f32], height: f32, mut draw: impl FnMut(&mut Ui, usize)) -> std::ops::Range<usize> {
    let mut tops = Vec::with_capacity(heights.len() + 1);
    let mut y = 0.0;
    for h in heights {
        tops.push(y);
        y += h.max(1.0);
    }
    tops.push(y);
    let total = y;
    let mut drawn = 0..0;
    egui::ScrollArea::vertical().id_salt(id).max_height(height).min_scrolled_height(height).auto_shrink([false, false]).show_viewport(
        ui,
        |ui, view| {
            ui.set_height(total);
            let (first, end) = rows_in_view(&tops, view.top(), view.bottom());
            drawn = first..end;
            let origin = ui.min_rect().top();
            let width = ui.available_width().max(0.0);
            for i in first..end {
                let (Some(top), Some(h)) = (tops.get(i), heights.get(i)) else { continue };
                let r = Rect::from_min_size(pos2(ui.min_rect().left(), origin + top), vec2(width, *h));
                ui.scope_builder(UiBuilder::new().id_salt(("virtualRow", i)).max_rect(r).layout(egui::Layout::top_down(egui::Align::Min)), |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    draw(ui, i);
                });
            }
        },
    );
    drawn
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_in_view_finds_the_band() {
        let tops = [0.0, 30.0, 60.0, 136.0, 166.0];
        assert_eq!(rows_in_view(&tops, 0.0, 30.0), (0, 1));
        assert_eq!(rows_in_view(&tops, 35.0, 70.0), (1, 3));
        assert_eq!(rows_in_view(&tops, 140.0, 900.0), (3, 4));
        assert_eq!(rows_in_view(&tops, 900.0, 1000.0), (4, 4));
        assert_eq!(rows_in_view(&[0.0], 0.0, 100.0), (0, 0));
        assert_eq!(rows_in_view(&tops, -50.0, 10.0), (0, 1));
    }

    #[test]
    fn action_columns_sit_at_the_same_place_in_every_row() {
        let c = Columns { info_w: 120.0 };
        let id = egui::Id::new("t");
        let a = RowRects::new(Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, ROW_H)), id, &c, GROUP_NAME_LEFT);
        let b = RowRects::new(Rect::from_min_size(pos2(0.0, 30.0), vec2(800.0, ROW_H)), id, &c, ITEM_NAME_LEFT);
        for col in [Col::ForImport, Col::Rename, Col::Group, Col::Hide, Col::Delete] {
            assert_eq!(a.col(col).x_range(), b.col(col).x_range(), "{col:?}");
        }
        assert!(a.col(Col::Delete).right() <= 800.0 && a.info.right() <= a.col(Col::ForImport).left());
        assert!(b.name.right() <= b.info.left());
        // a row narrower than its columns keeps its name inside the row
        let n = RowRects::new(Rect::from_min_size(pos2(0.0, 0.0), vec2(100.0, ROW_H)), id, &c, ITEM_NAME_LEFT);
        assert!(n.name.width() >= 0.0 && n.info.width() >= 0.0);
    }
}
