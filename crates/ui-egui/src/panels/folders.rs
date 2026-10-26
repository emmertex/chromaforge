//! The Classic Folders panel's own parts: the volume rows with their free space, the header's +
//! menu, and the folder rows' disk actions (create inside, synchronise, update location, show
//! parent). The folder tree itself is `left::folders_body`.

use egui::{Align2, Rect, Sense, pos2, vec2};
use serde_json::json;

use crate::LccApp;
use crate::icons::Icon;
use crate::theme::Tokens;
use crate::widgets::{icon_button, register};

/// The + menu at the right end of the Folders header: Add Folder… (its photos join the library
/// where they are), Create Folder….
pub fn header_buttons(app: &mut LccApp, ui: &mut egui::Ui, header: Rect, viewport: Rect) {
    if app.services.pick_folder.is_none() {
        return;
    }
    let right = (header.left() + viewport.max.x).min(header.right()) - 26.0;
    let mut hdr = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(Rect::from_min_max(pos2(right - 30.0, header.top()), pos2(right, header.bottom())))
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    let plus = icon_button(&mut hdr, "folderAdd", Icon::Plus, vec2(22.0, 22.0), false, true, "Add Folder");
    egui::Popup::menu(&plus).show(|ui| {
        if ui
            .button(crate::i18n::tr("Add Folder…"))
            .on_hover_text(crate::i18n::tr("Add the photos of a folder to the library where they are"))
            .clicked()
        {
            let picked = app.services.pick_folder.as_mut().and_then(|f| f());
            if let Some(path) = picked {
                sync_and_report(app, ui.ctx(), &path);
            }
            ui.close();
        }
        if ui.button(crate::i18n::tr("Create Folder…")).clicked() {
            let picked = app.services.pick_folder.as_mut().and_then(|f| f());
            if let Some(parent) = picked {
                create_prompt(app, &parent);
            }
            ui.close();
        }
    });
}

fn create_prompt(app: &mut LccApp, parent: &str) {
    app.ui.dialog = Some(crate::state::Dialog::TextPrompt {
        title: crate::i18n::tr_format!("Create Folder in “{name}”", name = lcc_catalog::folders::folder_label(parent)),
        hint: "Folder name (created on disk)".into(),
        value: String::new(),
        command: "folder.create".into(),
        params: json!({"parent": parent}),
        key: "name".into(),
    });
}

/// Synchronise `path` (new files join the library, sidecars changed elsewhere are read) and say
/// what happened.
fn sync_and_report(app: &mut LccApp, ctx: &egui::Context, path: &str) {
    match app.run("folder.sync", json!({"path": path, "readMetadata": true})) {
        Ok(r) => {
            let added = r["imported"].as_u64().unwrap_or(0);
            let missing = r["missing"].as_array().map_or(0, Vec::len);
            let changed = r["metadataChanged"].as_array().map_or(0, Vec::len);
            app.toast(
                ctx,
                crate::i18n::tr_format!(
                    "{added} added, {missing} missing, {changed} read from changed sidecars",
                    added = added,
                    missing = missing,
                    changed = changed
                ),
            );
        }
        Err(e) => app.toast(ctx, e),
    }
}

fn human_bytes(b: u64) -> String {
    let gb = b as f64 / 1e9;
    if gb >= 1000.0 {
        format!("{:.1} TB", gb / 1000.0)
    } else if gb >= 10.0 {
        format!("{gb:.0} GB")
    } else {
        format!("{gb:.1} GB")
    }
}

/// A disk's bar, as Lightroom Classic heads each disk's folders: its disclosure triangle (`open`), a
/// status light (green: online with room, yellow: under 10 % free, red: under 2 % free, grey:
/// offline), its name and its free / total space. A click opens or closes the folders below.
/// Widget id `volume:<path>`.
pub fn volume_bar(ui: &mut egui::Ui, n: &lcc_catalog::FolderNode, open: bool) -> egui::Response {
    let t = Tokens::get(ui.ctx());
    let (path, name, count) = (&n.path, &n.name, n.count);
    let space = super::left::fs_cached(ui, "disk-space", path, 30.0, lcc_engine::cmd::folders::disk_space).flatten();
    let online = super::left::fs_cached(ui, "is-dir", path, 5.0, |p| std::path::Path::new(p).is_dir()) == Some(true);
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::click());
    register(ui.ctx(), format!("volume:{path}"), r);
    let free_share = space.map(|(f, total)| if total == 0 { 1.0 } else { f as f64 / total as f64 });
    let light = match (online, free_share) {
        (false, _) => egui::Color32::from_gray(110),
        (true, Some(s)) if s < 0.02 => egui::Color32::from_rgb(220, 70, 60),
        (true, Some(s)) if s < 0.10 => egui::Color32::from_rgb(230, 190, 60),
        _ => egui::Color32::from_rgb(90, 190, 100),
    };
    if resp.hovered() {
        ui.painter().rect_filled(r, 0.0, t.text.gamma_multiply(0.06));
    }
    let c = pos2(r.left() + 12.0, r.center().y);
    let col = if resp.hovered() { t.text } else { t.text_dim };
    let tri = if open {
        vec![c + vec2(-4.0, -2.0), c + vec2(4.0, -2.0), c + vec2(0.0, 3.0)]
    } else {
        vec![c + vec2(-2.0, -4.0), c + vec2(3.0, 0.0), c + vec2(-2.0, 4.0)]
    };
    ui.painter().add(egui::Shape::convex_polygon(tri, col, egui::Stroke::NONE));
    ui.painter().circle_filled(pos2(r.left() + 26.0, r.center().y), 4.0, light);
    let label = if path == "/" || name.is_empty() { crate::i18n::tr("This Computer").to_string() } else { name.clone() };
    ui.painter().text(pos2(r.left() + 38.0, r.center().y), Align2::LEFT_CENTER, &label, t.semibold(12.5), t.text_label);
    let right = match space {
        Some((free, total)) => format!("{} / {}", human_bytes(free), human_bytes(total)),
        None if !online => crate::i18n::tr("Offline").to_string(),
        None => String::new(),
    };
    let edge = ui.clip_rect().right().min(r.right());
    ui.painter().text(pos2(edge - 14.0, r.center().y), Align2::RIGHT_CENTER, right, t.font(11.5), t.text_dim);
    let spoken = if open { crate::i18n::tr("Collapse") } else { crate::i18n::tr("Expand") };
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::CollapsingHeader, true, open, format!("{label}, {spoken}")));
    resp.on_hover_text(crate::i18n::tr_format!("{path} · {count} photos", path = path, count = count))
}
