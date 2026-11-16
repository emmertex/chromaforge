//! The Export module: the far-right entry of the module picker. The centre shows the
//! photos the export will write (the selection in grid order, else the active photo), like the Web
//! module's preview; every setting of the Export dialog sits in a right column the user can drag
//! wider or narrower, with the Export button at its foot.
//!
//! The settings are the Export dialog's own (`panels::dialogs::export_settings`), started from the
//! same place (`dialog.export`: the last export's settings), so the two never disagree. Touch
//! layouts (Modern on a phone or tablet) have no room for the column: there the module opens the
//! Export dialog instead and goes back to the module it came from.

use egui::{Align2, Color32, Rect, Sense, Stroke, pos2, vec2};
use serde_json::json;

use crate::LccApp;
use crate::module::{Edge, Module, ModuleId, ModuleKey, PanelId};
use crate::state::Dialog;
use crate::theme::Tokens;
use crate::widgets::register;

pub struct ExportModule;

pub static EXPORT: ExportModule = ExportModule;

/// Most thumbnails the preview draws.
const PREVIEW_MAX: usize = 200;
const COLUMN_DEFAULT_W: f32 = 500.0;
const COLUMN_MIN_W: f32 = 320.0;
/// The column never takes more than this share of the module's width.
const COLUMN_MAX_SHARE: f32 = 0.7;
const FOOTER_H: f32 = 48.0;

/// The module's form: the Export dialog's fields, kept while the app runs.
#[derive(Clone)]
struct Form {
    opts: lcc_engine::export::ExportOptions,
    full_size: bool,
    resize: lcc_engine::export::Resize,
    preset_name: String,
    limit_kb: u32,
    dir: String,
}

fn form_id() -> egui::Id {
    egui::Id::new("export-module-form")
}

fn width_id() -> egui::Id {
    egui::Id::new("export-module-column-w")
}

/// The form as `dialog.export` would open it (the last export's settings), without showing the
/// dialog.
fn fresh_form(app: &mut LccApp) -> Option<Form> {
    let before = app.ui.dialog.take();
    let opened = app.run("dialog.export", json!({})).ok().and(app.ui.dialog.take());
    app.ui.dialog = before;
    match opened {
        Some(Dialog::Export { opts, full_size, resize, preset_name, limit_kb, dir }) => {
            Some(Form { opts, full_size, resize, preset_name, limit_kb, dir })
        }
        _ => None,
    }
}

/// The photos the export writes, in the order it writes them (as `app.export` picks them).
pub(crate) fn targets(app: &mut LccApp) -> Vec<lcc_catalog::PhotoId> {
    if app.session.selection.ids.is_empty() {
        return app.session.active().into_iter().collect();
    }
    let sel: std::collections::HashSet<_> = app.session.selection.ids.iter().copied().collect();
    let mut v: Vec<_> = app.session.visible().iter().copied().filter(|id| sel.contains(id)).collect();
    let shown: std::collections::HashSet<_> = v.iter().copied().collect();
    v.extend(app.session.selection.ids.iter().filter(|id| !shown.contains(id)));
    v
}

impl Module for ExportModule {
    fn id(&self) -> ModuleId {
        ModuleId::Export
    }
    fn left_panels(&self) -> &'static [PanelId] {
        &[]
    }
    fn right_panels(&self) -> &'static [PanelId] {
        &[]
    }
    fn toolbar(&self, _ui: &mut egui::Ui, _app: &mut LccApp) {}
    fn center(&self, ui: &mut egui::Ui, app: &mut LccApp) {
        // touch layouts keep the dialog: no room for a settings column there
        if crate::modern::on(app) {
            let back = app.ui.previous_module.filter(|m| *m != ModuleId::Export).unwrap_or(ModuleId::Library);
            let _ = app.run(back.command(), json!({}));
            if let Err(e) = app.run("dialog.export", json!({})) {
                app.ui.status = e;
            }
            return;
        }
        let t = Tokens::get(ui.ctx());
        let mut area = ui.available_rect_before_wrap();
        if crate::module::edge_visible(app, Edge::Bottom) {
            let film = Rect::from_min_max(pos2(area.left(), area.bottom() - app.ui.film_height()), area.max);
            area.max.y = film.top();
            crate::panels::detail::filmstrip(app, ui, film);
        }
        app.canvas_rect = Some(area);
        register(ui.ctx(), "view:module:export", area);

        let Some(mut form) = ui.data(|d| d.get_temp::<Form>(form_id())).or_else(|| fresh_form(app)) else {
            ui.painter().text(area.center(), Align2::CENTER_CENTER, crate::i18n::tr("Export is not available"), t.font(14.0), t.text_dim);
            return;
        };

        // the column, resizable from its left edge
        let max_w = (area.width() * COLUMN_MAX_SHARE).max(COLUMN_MIN_W.min(area.width()));
        let mut col_w = ui.data_mut(|d| d.get_persisted::<f32>(width_id())).unwrap_or(COLUMN_DEFAULT_W);
        col_w = col_w.clamp(COLUMN_MIN_W.min(max_w), max_w);
        let handle = Rect::from_min_max(pos2(area.right() - col_w - 3.0, area.top()), pos2(area.right() - col_w + 3.0, area.bottom()));
        let drag = ui.interact(handle, egui::Id::new("export-module-resize"), Sense::drag());
        register(ui.ctx(), "handle:export:column", handle);
        if drag.hovered() || drag.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }
        if drag.dragged() {
            col_w = (col_w - drag.drag_delta().x).clamp(COLUMN_MIN_W.min(max_w), max_w);
        }
        ui.data_mut(|d| d.insert_persisted(width_id(), col_w));

        let preview_rect = Rect::from_min_max(area.min, pos2(area.right() - col_w, area.bottom()));
        let col_rect = Rect::from_min_max(pos2(preview_rect.right(), area.top()), area.max);
        let ids = targets(app);
        preview(ui, app, &ids, preview_rect);

        ui.painter().rect_filled(col_rect, 0.0, t.chrome);
        let line = if drag.hovered() || drag.dragged() { t.accent } else { t.divider };
        ui.painter().vline(col_rect.left(), col_rect.y_range(), Stroke::new(1.0, line));
        register(ui.ctx(), "panel:export:settings", col_rect);
        let body = Rect::from_min_max(col_rect.min, pos2(col_rect.right(), col_rect.bottom() - FOOTER_H));
        let foot = Rect::from_min_max(pos2(col_rect.left(), body.bottom()), col_rect.max);
        let mut col = ui.new_child(egui::UiBuilder::new().max_rect(body.shrink2(vec2(10.0, 8.0))).id_salt("export-module-settings"));
        egui::ScrollArea::both().id_salt("export-module-scroll").auto_shrink([false, false]).show(&mut col, |ui| {
            let label_w = (ui.available_width() * 0.4).clamp(110.0, 180.0);
            crate::dialog_frame::form(ui, label_w, |ui| {
                let f = crate::panels::dialogs::ExportForm {
                    opts: &mut form.opts,
                    full_size: &mut form.full_size,
                    resize: &mut form.resize,
                    preset_name: &mut form.preset_name,
                    limit_kb: &mut form.limit_kb,
                    dir: &mut form.dir,
                };
                crate::panels::dialogs::export_settings(app, ui, f, true);
            });
        });

        // footer: what is exported, and the button
        ui.painter().hline(foot.x_range(), foot.top(), Stroke::new(1.0, t.divider));
        let n = ids.len();
        let mut foot_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(foot.shrink2(vec2(10.0, 8.0)))
                .layout(egui::Layout::right_to_left(egui::Align::Center))
                .id_salt("export-module-foot"),
        );
        let busy = app.export.is_some();
        let label = crate::i18n::tr_format!("Export {n} Photo{}", if n == 1 { "" } else { "s" }, n = n);
        let b = foot_ui.add_enabled(n > 0 && !busy, egui::Button::new(label).min_size(vec2(140.0, 28.0)));
        register(ui.ctx(), "button:export:start", b.rect);
        if busy {
            foot_ui.label(egui::RichText::new(crate::i18n::tr("Exporting…")).color(t.text_dim));
        }
        if b.clicked() {
            match crate::panels::dialogs::start_export(app, &form.opts, form.full_size, &form.resize, form.limit_kb, &form.dir, &form.preset_name) {
                Ok(_) => form.preset_name.clear(),
                Err(e) => app.ui.status = e,
            }
        }
        ui.allocate_rect(area, Sense::hover());
        ui.data_mut(|d| d.insert_temp(form_id(), form));
    }
    fn keymap(&self) -> &'static [ModuleKey] {
        &[crate::help_overlay::KEY]
    }
    fn command_prefixes(&self) -> &'static [&'static str] {
        &["export."]
    }
    fn own_sides(&self) -> bool {
        true
    }
}

/// The photos to export as a grid of thumbnails, with a caption saying how many.
fn preview(ui: &mut egui::Ui, app: &mut LccApp, ids: &[lcc_catalog::PhotoId], rect: Rect) {
    let t = Tokens::get(ui.ctx());
    register(ui.ctx(), "view:export:preview", rect);
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 0.0, t.grid_bg);
    let pad = 16.0;
    let caption = match ids.len() {
        0 => crate::i18n::tr("No photo to export: select photos in the filmstrip").to_string(),
        n if n > PREVIEW_MAX => crate::i18n::tr_format!("{n} photos to export (first {m} shown)", n = n, m = PREVIEW_MAX),
        n => crate::i18n::tr_format!("{n} photo{} to export", if n == 1 { "" } else { "s" }, n = n),
    };
    p.text(pos2(rect.left() + pad, rect.top() + pad), Align2::LEFT_TOP, caption, t.font(14.0), t.text_dim);
    let top = rect.top() + pad + 28.0;
    let inner_w = (rect.width() - 2.0 * pad).max(40.0);
    let gap = 10.0;
    let n = ids.len().min(PREVIEW_MAX);
    if n == 0 {
        return;
    }
    // big cells for a few photos, smaller as there are more (at least ~120 pt)
    let target = if n <= 1 {
        inner_w.min(rect.bottom() - top - pad)
    } else if n <= 4 {
        inner_w / 2.0
    } else {
        160.0
    };
    let cols = ((inner_w + gap) / (target.max(120.0) + gap)).floor().max(1.0) as usize;
    let cell = ((inner_w - gap * (cols as f32 - 1.0)) / cols as f32).max(20.0);
    let cell_h = if n == 1 { (rect.bottom() - top - pad).max(20.0) } else { cell };
    for (i, id) in ids.iter().take(n).enumerate() {
        let (row, col) = (i / cols, i % cols);
        let r = Rect::from_min_size(pos2(rect.left() + pad + col as f32 * (cell + gap), top + row as f32 * (cell_h + gap)), vec2(cell, cell_h));
        if r.top() > rect.bottom() {
            break;
        }
        p.rect_filled(r, 4.0, t.chrome);
        draw_photo(&p, app, *id, r.shrink(6.0), ids.len() == 1);
    }
}

/// The photo's picture fitted into `r` (a placeholder until it is rendered). `full`: the preview's
/// single photo, drawn from the loupe's own render — the thumbnail ladder stops at 512 px, and a
/// photo that fills the module's centre is soft beside Develop and the Library loupe — with the
/// thumbnail standing in until the render arrives.
fn draw_photo(p: &egui::Painter, app: &mut LccApp, id: lcc_catalog::PhotoId, r: Rect, full: bool) {
    // the cell, not a fixed size: a single photo fills the whole preview area, and a 256-px
    // thumbnail scaled up to it is the blurry preview the module used to show
    let ppp = p.ctx().pixels_per_point();
    let size = (r.width().max(r.height()).max(120.0) * ppp) as usize;
    crate::panels::grid::request_thumb(app, id, size, 8);
    if full {
        let w = (r.width() * ppp).max(8.0) as usize;
        let h = (r.height() * ppp).max(8.0) as usize;
        let job = app.session.loupe_job(id, w, h, true).map(|job| job.with_display_fit(w, h));
        if let Some(job) = job {
            app.renderer.request(crate::render::Slot::Main, job, 60);
        }
    }
    // the loupe's render of this photo when it has arrived; the thumbnail otherwise
    let shown: Option<(egui::TextureId, [usize; 2])> = if full {
        app.renderer.textures.get(&crate::render::Slot::Main).filter(|t| t.photo == id && t.quick.is_none()).map(|t| (t.id(), t.size))
    } else {
        None
    }
    .or_else(|| app.renderer.thumb(id).map(|t| (t.id(), t.size)));
    let Some((tex, [w, h])) = shown else {
        p.rect_filled(r, 0.0, Color32::from_gray(60));
        return;
    };
    let aspect = w.max(1) as f32 / h.max(1) as f32;
    let fit = if r.width() / r.height().max(1.0) > aspect { vec2(r.height() * aspect, r.height()) } else { vec2(r.width(), r.width() / aspect) };
    p.image(tex, Rect::from_center_size(r.center(), fit), Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
}
