//! Settings (fork-owned), the one Settings screen of every layout: a full-window sheet with an icon
//! rail of seven sections down its left side and the open section's rows beside it.
//!
//! - With room (a window at least [`ROOMY`] points wide: desktops, tablets) the rail always shows
//!   each section's name and a search field; Back (the ‹ button, Esc, Android's Back) closes
//!   Settings.
//! - On a narrow window the rail is icons only while a section is open. Back widens it to show the
//!   names (and the search field); Back again closes Settings. A tap on an icon switches section.
//!
//! The twelve tabs of the old dialog become seven sections (General, Library, Import, Editing,
//! Display & Speed, Sync, Shortcuts; Slideshow only in the Classic layout, which has that module).
//! Each section shows the old tabs' own rows ([`crate::panels::settings::tab_body`]), so every
//! setting keeps its control and its command.
//!
//! Own design.

use egui::{Align2, Color32, Id, Rect, Sense, Stroke, Vec2, pos2, vec2};

use crate::LccApp;
use crate::icons::{Icon, paint};
use crate::state::Dialog;
use crate::theme::Tokens;
use crate::widgets::register;

/// The screen's layer id (`Order::Middle`), which is the modal layer while Settings is open, so the
/// dialogs' barrier (`dialogs::prepare`) keeps the popups opened from its rows.
pub const LAYER: &str = "settings-screen";

/// From this width (points) the rail always shows the sections' names.
pub const ROOMY: f32 = 640.0;
const HEADER_H: f32 = 56.0;
const RAIL_WIDE: f32 = 220.0;
const RAIL_NARROW: f32 = 60.0;
const ITEM_H: f32 = 44.0;
/// The rows' widest, so a large window doesn't stretch them.
const CONTENT_MAX: f32 = 820.0;

/// A section: its key, name, icon, the old tabs it holds, and words a search finds it by.
struct Section {
    key: &'static str,
    label: &'static str,
    icon: Icon,
    tabs: &'static [&'static str],
    words: &'static str,
}

const SECTIONS: &[Section] = &[
    Section {
        key: "general",
        label: "General",
        icon: Icon::Gear,
        tabs: &["general", "interface"],
        words: "layout classic modern language library location open new startup culling auto advance confirm external editing photocraft editor appearance mode dark light font size sliders slider text pill filmstrip grid badges ratings flags wheel scroll info overlay loupe",
    },
    Section { key: "library", label: "Library", icon: Icon::Folder, tabs: &["folders"], words: "folders watch polling new photos forget unchanged" },
    Section {
        key: "import",
        label: "Import",
        icon: Icon::Plus,
        tabs: &["import"],
        words: "raw defaults non-raw metadata preset xmp sidecars names cameras cards auto import watched folder copy album lens profiles database",
    },
    Section {
        key: "editing",
        label: "Editing",
        icon: Icon::Sliders,
        tabs: &["denoise", "faces"],
        words: "ai denoise model faces people recognition",
    },
    Section {
        key: "display",
        label: "Display & Speed",
        icon: Icon::Monitor,
        tabs: &["display", "performance"],
        words: "monitor profile icc srgb screen performance rendering preview size memory cache thumbnail clear smart previews gpu",
    },
    Section {
        key: "sync",
        label: "Sync",
        icon: Icon::Cloud,
        tabs: &["sync", "connections"],
        words: "devices peers immich server api key connections keychain",
    },
    Section { key: "shortcuts", label: "Shortcuts", icon: Icon::Help, tabs: &["shortcuts"], words: "keyboard keymap keys" },
    Section { key: "slideshow", label: "Slideshow", icon: Icon::Slideshow, tabs: &["slideshow"], words: "slide duration fade playback export" },
];

/// Settings' own state between frames (kept in `LccApp::modern_sheets`).
#[derive(Default)]
pub struct SettingsScreen {
    /// Narrow window: the rail shows the sections' names (Back widened it).
    pub wide: bool,
    /// The search field's text.
    pub search: String,
    /// The tab asked for (`app.settings {tab}`) that was last brought into view, and for how many
    /// frames (a floating area's first frame is an invisible sizing pass that drops the scroll).
    scrolled_to: (String, u8),
}

fn shown(app: &LccApp, s: &Section) -> bool {
    if cfg!(target_arch = "wasm32") && s.key == "sync" {
        return false;
    }
    // Slideshow is a Classic module: its settings are Classic's
    s.key != "slideshow" || !crate::modern::on(app)
}

/// The section a tab (or a section's own key) belongs to: an old tab name (`app.settings
/// {tab: "performance"}`) opens the section that now holds it.
pub fn section_of(tab: &str) -> &'static str {
    SECTIONS.iter().find(|s| s.key == tab || s.tabs.contains(&tab)).map_or("general", |s| s.key)
}

/// Back on Settings: on a narrow window from a section to the widened rail (`Some`), else `None`
/// (the caller closes Settings, as Esc does).
pub(crate) fn back(app: &mut LccApp, ctx: &egui::Context) -> Option<&'static str> {
    if !matches!(app.ui.dialog, Some(Dialog::Settings { .. })) {
        return None;
    }
    if ctx.content_rect().width() < ROOMY && !app.modern_sheets.settings.wide {
        app.modern_sheets.settings.wide = true;
        return Some("settingsList");
    }
    None
}

fn close(app: &mut LccApp) {
    app.ui.dialog = None;
    app.ui.settings_section = None;
    app.modern_sheets.settings = SettingsScreen::default();
}

/// Draws Settings over the whole window (every layout).
pub(crate) fn show(app: &mut LccApp, ctx: &egui::Context) {
    let Some(Dialog::Settings { tab }) = app.ui.dialog.clone() else { return };
    let open = section_of(&tab).to_string();
    let screen = ctx.content_rect();
    let roomy = screen.width() >= ROOMY;
    if roomy {
        app.modern_sheets.settings.wide = false;
    }
    let wide = roomy || app.modern_sheets.settings.wide;
    let mut go_back = false;
    // Esc: Back (a popup or a text field being edited takes it first)
    if !egui::Popup::is_any_open(ctx)
        && !ctx.egui_wants_keyboard_input()
        && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
    {
        go_back = true;
    }
    let t = Tokens::get(ctx);
    let mut picked: Option<&'static str> = None;
    // over the layout's floating chrome (also `Middle`), under popups (dropdowns, context menus:
    // `Foreground`), which open from its rows
    let layer = egui::LayerId::new(egui::Order::Middle, Id::new(LAYER));
    ctx.move_to_top(layer);
    ctx.memory_mut(|m| m.set_modal_layer(layer));
    egui::Area::new(Id::new(LAYER)).order(egui::Order::Middle).fixed_pos(screen.min).show(ctx, |ui| {
        let (bg, _) = ui.allocate_exact_size(screen.size(), Sense::click());
        ui.painter().rect_filled(bg, 0.0, t.chrome);
        // the header: ‹ (Back), the title, what Back does
        let header = Rect::from_min_size(bg.min, vec2(bg.width(), HEADER_H));
        let b = Rect::from_min_size(pos2(header.left() + 10.0, header.center().y - 18.0), Vec2::splat(36.0));
        let resp = ui.interact(b, ui.id().with("settings-back"), Sense::click());
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, crate::i18n::tr("Back")));
        register(ui.ctx(), "settings:back", b);
        ui.painter().circle_filled(b.center(), 18.0, if resp.hovered() { t.pressed } else { t.button });
        paint(ui.painter(), Rect::from_center_size(b.center(), Vec2::splat(18.0)), Icon::ChevronLeft, t.text);
        if resp.clicked() {
            go_back = true;
        }
        let name = SECTIONS.iter().find(|s| s.key == open).map_or("General", |s| s.label);
        let title = if roomy {
            format!("{} · {}", crate::i18n::tr("Settings"), crate::i18n::tr(name))
        } else if wide {
            crate::i18n::tr("Settings").to_string()
        } else {
            crate::i18n::tr(name).to_string()
        };
        ui.painter().text(pos2(b.right() + 12.0, header.center().y), Align2::LEFT_CENTER, title, t.semibold(17.0), t.text);
        let hint = if wide { "Back closes Settings" } else { "Back shows all sections" };
        if header.width() > 520.0 {
            ui.painter().text(pos2(header.right() - 16.0, header.center().y), Align2::RIGHT_CENTER, crate::i18n::tr(hint), t.font(12.0), t.text_dim);
        }
        ui.painter().hline(bg.x_range(), header.bottom(), Stroke::new(1.0, t.divider));
        let body = Rect::from_min_max(pos2(bg.left(), header.bottom() + 1.0), bg.max);
        // narrow and widened: the rail is the whole page (the list of sections)
        // a section wider than the room beside the full rail (Slideshow's settings and preview)
        // takes the names' room: the rail shows icons
        let cramped = open == "slideshow" && body.width() - RAIL_WIDE < crate::panels::settings::SLIDESHOW_W + 80.0;
        let names = wide && !(roomy && cramped);
        let rail_w = if wide && !roomy {
            body.width()
        } else if names {
            RAIL_WIDE
        } else {
            RAIL_NARROW
        };
        let rail = Rect::from_min_size(body.min, vec2(rail_w, body.height()));
        picked = rail_ui(app, ui, rail, &open, names);
        ui.painter().vline(rail.right(), body.y_range(), Stroke::new(1.0, t.divider));
        let content = Rect::from_min_max(pos2(rail.right() + 1.0, body.top()), body.max);
        if !(wide && !roomy) {
            rows(app, ui, content, &open, &tab);
        }
    });
    // (the tab stays as it was asked for, e.g. `denoise`, until another section is picked)
    let picked_any = picked.is_some();
    let picked = picked.filter(|p| *p != open);
    // a custom thumbnail cache size still being typed is saved on the way out of its section
    if go_back || picked.is_some() {
        crate::panels::settings::save_pending_cache_size(app, ctx);
    }
    if go_back {
        if back(app, ctx).is_none() {
            close(app);
        }
        return;
    }
    if let Some(p) = picked {
        // the rows may have closed Settings or opened another dialog (e.g. a folder picker's)
        if let Some(Dialog::Settings { tab: now }) = &mut app.ui.dialog {
            *now = p.to_string();
        }
        app.ui.settings_section = None;
    }
    if picked.is_some() || app.modern_sheets.settings.wide && picked_any {
        app.modern_sheets.settings.wide = false;
    }
}

/// The rail: the search field (when wide) and a button per section. Returns the one picked.
fn rail_ui(app: &mut LccApp, ui: &mut egui::Ui, rail: Rect, open: &str, wide: bool) -> Option<&'static str> {
    let t = Tokens::get(ui.ctx());
    let mut picked = None;
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rail.shrink2(vec2(8.0, 10.0))));
    egui::ScrollArea::vertical().id_salt("settings-rail").auto_shrink([false, false]).show(&mut child, |ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        let query = app.modern_sheets.settings.search.trim().to_lowercase();
        if wide {
            crate::text_field::TextField::singleline("settingsSearch", &mut app.modern_sheets.settings.search)
                .hint(crate::i18n::tr("Search settings"))
                .width(ui.available_width())
                .show(ui);
            ui.add_space(8.0);
        }
        for s in SECTIONS.iter().filter(|s| shown(app, s)) {
            let matches = query.is_empty() || crate::i18n::tr(s.label).to_lowercase().contains(&query) || s.words.contains(query.as_str());
            if wide && !matches {
                continue;
            }
            let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ITEM_H), Sense::click());
            let name = crate::i18n::tr(s.label);
            let on = s.key == open;
            resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, on, name));
            register(ui.ctx(), format!("settings:section:{}", s.key), r);
            // the old dialog's tab ids reach the section that holds them
            for tab in s.tabs {
                register(ui.ctx(), format!("button:settingsTab-{tab}"), r);
            }
            let resp = if wide { resp } else { resp.on_hover_text(name) };
            let p = ui.painter();
            if on {
                p.rect_filled(r, 12.0, Color32::from_white_alpha(30));
                p.rect_stroke(r, 12.0, Stroke::new(1.0, Color32::from_white_alpha(24)), egui::StrokeKind::Inside);
            } else if resp.hovered() || resp.is_pointer_button_down_on() {
                p.rect_filled(r, 12.0, Color32::from_white_alpha(14));
            }
            let c = if on { t.accent } else { t.text_label };
            let icon_at = if wide { pos2(r.left() + 22.0, r.center().y) } else { r.center() };
            paint(p, Rect::from_center_size(icon_at, Vec2::splat(20.0)), s.icon, c);
            if wide {
                p.text(pos2(r.left() + 44.0, r.center().y), Align2::LEFT_CENTER, name, t.font(14.0), if on { t.text } else { t.text_label });
            }
            if resp.clicked() {
                picked = Some(s.key);
            }
        }
    });
    picked
}

/// The open section's rows: its old tabs' own, one after another, finger-sized; labels above their
/// controls when narrow.
/// `asked`: the tab Settings was opened on; when it is a later part of the section, the rows scroll
/// to it once.
fn rows(app: &mut LccApp, ui: &mut egui::Ui, area: Rect, open: &str, asked: &str) {
    let t = Tokens::get(ui.ctx());
    let Some(section) = SECTIONS.iter().find(|s| s.key == open) else { return };
    let stacked = area.width() < 520.0;
    // (Slideshow: its settings and their live preview side by side)
    let max = if open == "slideshow" { crate::panels::settings::SLIDESHOW_W + 2.0 * crate::dialog_frame::GROUP_PAD + 40.0 } else { CONTENT_MAX };
    let w = area.width().min(max + 32.0);
    let column = Rect::from_min_size(pos2(area.left() + ((area.width() - w) / 2.0).max(0.0), area.top()), vec2(w, area.height()));
    let mut inner = ui.new_child(egui::UiBuilder::new().max_rect(column.shrink2(vec2(16.0, 0.0))));
    // (the scrolling column: where a pointer scrolls the rows)
    register(ui.ctx(), "settings:rows", column);
    let was = crate::dialog_frame::set_finger_sized(ui.ctx(), true);
    egui::ScrollArea::vertical().id_salt(("settings-rows", open)).auto_shrink([false, false]).show(&mut inner, |ui| {
        let s = ui.spacing_mut();
        s.item_spacing = vec2(10.0, 10.0);
        s.interact_size = vec2(44.0, 36.0);
        s.button_padding = vec2(12.0, 6.0);
        s.icon_width = 22.0;
        s.icon_width_inner = 13.0;
        s.icon_spacing = 10.0;
        s.combo_height = 480.0;
        for (style, size) in [(egui::TextStyle::Body, 14.5), (egui::TextStyle::Button, 14.5)] {
            if let Some(f) = ui.style_mut().text_styles.get_mut(&style) {
                f.size = size;
            }
        }
        ui.add_space(14.0);
        ui.set_width((ui.available_width() - 12.0).max(120.0));
        for tab in section.tabs {
            if *tab == asked {
                let s = &mut app.modern_sheets.settings.scrolled_to;
                if s.0 != asked {
                    *s = (asked.to_string(), 0);
                }
                if s.1 < 3 {
                    s.1 += 1;
                    if section.tabs.first() != Some(tab) {
                        ui.scroll_to_cursor(Some(egui::Align::TOP));
                    }
                }
            }
            let body = |ui: &mut egui::Ui| {
                crate::dialog_frame::group_begin(ui, None);
                if stacked && *tab == "shortcuts" {
                    ui.label(egui::RichText::new(crate::i18n::tr("Keyboard shortcuts are edited on a wider screen.")).color(t.text_label));
                    return;
                }
                crate::panels::settings::tab_body(app, ui, &t, tab);
            };
            if stacked {
                crate::dialog_frame::stacked_form(ui, 120.0, body);
            } else {
                crate::dialog_frame::form(ui, 190.0, body);
            }
            ui.add_space(6.0);
        }
        ui.add_space(48.0);
    });
    crate::dialog_frame::set_finger_sized(ui.ctx(), was);
}

/// Tests: scrolls Settings' rows until widget `id` is in view (the sections are taller than a small
/// window), as a person scrolling to it would.
#[cfg(test)]
pub(crate) fn reveal(h: &mut crate::headless::Headless, id: &str) {
    use std::time::Duration;
    let t = Duration::from_secs(10);
    let find = |h: &crate::headless::Headless, w: &str| h.app.widgets.iter().rev().find(|(i, _)| i == w).map(|(_, r)| *r);
    for _ in 0..40 {
        h.step();
        let Some(rows) = find(h, "settings:rows") else { panic!("Settings' rows are not on screen") };
        let dy = match find(h, id) {
            Some(w) if w.top() >= rows.top() + 4.0 && w.bottom() <= rows.bottom() - 4.0 => {
                // in view and settled (a scroll glides on for a few frames)
                for _ in 0..6 {
                    h.step();
                }
                if find(h, id) == Some(w) {
                    return;
                }
                continue;
            }
            Some(w) => (rows.top() + 120.0 - w.center().y).clamp(-500.0, 500.0),
            None => -300.0,
        };
        let r = h.request("ui.hoverWidget", serde_json::json!({"id": "settings:rows", "fx": 0.98, "fy": 0.5}), t);
        assert_eq!(r["ok"], true, "{r}");
        let r = h.request("ui.scroll", serde_json::json!({"dy": dy}), t);
        assert_eq!(r["ok"], true, "{r}");
        for _ in 0..8 {
            h.step();
        }
    }
    panic!("{id} never came into view in Settings");
}
