//! Semantic search models in Settings ▸ AI Models (`ml.models.*`, `ml.status`, `ml.enable`).

use crate::i18n::{tr, tr_format};
use egui::RichText;
use serde_json::{Value, json};

use super::settings::check;
use crate::LccApp;
use crate::theme::Tokens;
use crate::widgets::register;

fn mb(bytes: Option<u64>) -> String {
    match bytes.filter(|b| *b > 0) {
        Some(b) => tr_format!("{n} MB", n = format!("{:.0}", b as f64 / 1_000_000.0)),
        None => String::new(),
    }
}

fn licence_line(m: &Value) -> String {
    let spdx = m["licence"]["spdx"].as_str().unwrap_or("");
    let summary = m["licence"]["summary"].as_str().unwrap_or("");
    match (spdx.is_empty(), summary.is_empty()) {
        (false, false) => format!("{} ({spdx}) — {}", tr("Licence"), tr(summary)),
        (false, true) => format!("{}: {spdx}", tr("Licence")),
        (true, false) => tr(summary).into(),
        (true, true) => String::new(),
    }
}

/// The AI Models dialog's Semantic search group.
pub fn models_section(app: &mut LccApp, ui: &mut egui::Ui, t: &Tokens) {
    let list = app.session.execute("ml.models.list", &json!({})).unwrap_or(Value::Null);
    let models: Vec<Value> = list["models"].as_array().cloned().unwrap_or_default();
    if list["dir"].is_null() || models.is_empty() {
        return;
    }
    let status = app.session.execute("ml.status", &json!({})).unwrap_or(Value::Null);
    let downloads =
        app.session.execute("ml.models.downloads", &json!({})).map(|v| v["downloads"].as_array().cloned().unwrap_or_default()).unwrap_or_default();
    crate::dialog_frame::group_begin(ui, Some("Semantic search"));
    ui.label(tr(
        "Searches the library by description (for example “moody studio portraits”) using image-text embeddings computed in the background.",
    ));
    let mut enabled = status["enabled"].as_bool() == Some(true);
    if check(ui, "mlEnable", &mut enabled, "Index photos in the background for natural-language search") {
        let _ = app.run("ml.enable", json!({"enabled": enabled}));
    }
    ml_device_selector(app, ui, t, &status);
    if enabled && list["selected"].is_string() {
        let indexed = status["indexedPhotos"].as_u64().unwrap_or(0);
        let unindexed = status["unindexedPhotos"].as_u64().unwrap_or(0);
        let text = if unindexed == 0 { tr_format!("{indexed} indexed") } else { tr_format!("{indexed} indexed, {unindexed} remaining") };
        ui.label(RichText::new(text).font(t.font(11.5)).color(t.text_dim));
    }
    for m in &models {
        let dl = downloads.iter().find(|d| d["id"] == m["id"]);
        model_row(app, ui, t, m, dl);
    }
    crate::dialog_frame::group_end(ui);
}

/// The hardware device selector dropdown and active device readout for AI embeddings.
pub fn ml_device_selector(app: &mut LccApp, ui: &mut egui::Ui, t: &Tokens, status: &Value) {
    let auto = tr("Automatic").to_string();
    let cpu = tr("CPU").to_string();
    let empty_vec = Vec::new();
    let dev_list = status.get("availableDevices").and_then(Value::as_array).unwrap_or(&empty_vec);
    let gpu_cards: Vec<&Value> = dev_list.iter().filter(|d| d["kind"].as_str() == Some("gpu")).collect();

    let name_of = |setting: &str| {
        if setting.is_empty() || setting.eq_ignore_ascii_case("auto") {
            Some(auto.clone())
        } else if setting.eq_ignore_ascii_case("cpu") {
            Some(cpu.clone())
        } else {
            gpu_cards.iter().find(|d| d["id"].as_str() == Some(setting)).and_then(|d| d["name"].as_str().map(str::to_string))
        }
    };
    let current = name_of(&app.ui.settings.ml_device).unwrap_or_else(|| auto.clone());
    super::settings::row(ui, t, crate::i18n::tr("AI embedding device"), |ui| {
        let combo = crate::widgets::combo_box("settingsMlDevice").selected_text(current).width(260.0).show_ui(ui, |ui| {
            let r = ui.selectable_value(&mut app.ui.settings.ml_device, String::new(), auto.as_str());
            register(ui.ctx(), "button:settingsMlDevice-auto", r.rect);
            let r = ui.selectable_value(&mut app.ui.settings.ml_device, "cpu".to_string(), cpu.as_str());
            register(ui.ctx(), "button:settingsMlDevice-cpu", r.rect);
            for (i, d) in gpu_cards.iter().enumerate() {
                let id = d["id"].as_str().unwrap_or("");
                let name = d["name"].as_str().unwrap_or(id);
                let usable = d["usable"].as_bool().unwrap_or(true);
                let why_not = d["whyNot"].as_str();
                let r = ui.add_enabled_ui(usable, |ui| ui.selectable_value(&mut app.ui.settings.ml_device, id.to_string(), name)).inner;
                let r = match why_not {
                    Some(why) => r.on_disabled_hover_text(why),
                    None => r,
                };
                register(ui.ctx(), format!("button:settingsMlDevice-{i}"), r.rect);
            }
        });
        register(ui.ctx(), "combo:settingsMlDevice", combo.response.rect);
    });

    let readout = if let Some(dev) = status.get("device").filter(|d| !d.is_null()) {
        let kind = dev["kind"].as_str().unwrap_or("cpu");
        let name = dev["name"].as_str().unwrap_or("CPU");
        if kind == "gpu" {
            tr_format!("Active: {name} (GPU accelerated)", name = name)
        } else if let Some(why) = dev["whyCpu"].as_str().filter(|w| !w.is_empty()) {
            tr_format!("Active: CPU · {why}", why = why)
        } else {
            tr("Active: CPU").to_string()
        }
    } else {
        #[cfg(not(target_os = "macos"))]
        let note = "GPU acceleration for AI embeddings currently requires Metal on macOS; running on CPU (multithreaded)";
        #[cfg(target_os = "macos")]
        let note = "Selected device initializes on first search";
        tr_format!("Active: CPU · {why}", why = note)
    };
    crate::dialog_frame::in_control_column(ui, |ui| {
        ui.add(egui::Label::new(egui::RichText::new(&readout).size(11.0).color(t.text_dim)).truncate()).on_hover_text(&readout);
    });
}

fn model_row(app: &mut LccApp, ui: &mut egui::Ui, t: &Tokens, m: &Value, dl: Option<&Value>) {
    let id = m["id"].as_str().unwrap_or("").to_string();
    let (installed, selected) = (m["installed"].as_bool() == Some(true), m["selected"].as_bool() == Some(true));
    let dl_state = if installed { None } else { dl.and_then(|d| d["state"].as_str()) };
    let host = m["downloadHost"].as_str().unwrap_or("").to_string();
    egui::Frame::NONE.inner_margin(egui::Margin::symmetric(0, 3)).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(m["name"].as_str().unwrap_or("")).color(t.text));
                    let size = mb(m["sizeBytes"].as_u64());
                    if !size.is_empty() {
                        ui.label(RichText::new(size).color(t.text_dim));
                    }
                    if selected {
                        ui.label(RichText::new(tr("in use")).color(t.accent));
                    } else if installed {
                        ui.label(RichText::new(tr("installed")).color(t.text_dim));
                    }
                });
                let lic = licence_line(m);
                if !lic.is_empty() {
                    ui.label(RichText::new(lic).font(t.font(11.5)).color(if m["licence"]["commercial"] == "yes" { t.text_dim } else { t.caution }));
                }
                match (dl_state, dl) {
                    (Some("running"), Some(d)) => {
                        let (got, total) = (d["bytes"].as_u64().unwrap_or(0), d["total"].as_u64().unwrap_or(0));
                        let text = if total > 0 && got >= total {
                            tr("Checking…").to_string()
                        } else {
                            tr_format!("{} of {} from {host}", mb(Some(got)), mb(Some(total)), host = host)
                        };
                        crate::progress::Progress::new(crate::progress::fraction_of(got, total))
                            .label(text)
                            .width(crate::progress::ROW_WIDTH.min(ui.available_width()))
                            .widget(format!("ml:progress:{id}"))
                            .show(ui);
                        ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
                    }
                    (Some("done" | "installing"), _) => {
                        ui.horizontal(|ui| {
                            crate::progress::spinner(ui, 12.0);
                            ui.label(RichText::new(tr("Downloaded and checked. Installing…")).font(t.font(11.5)).color(t.text_dim));
                        });
                        ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
                    }
                    (Some("failed"), Some(d)) => {
                        let why = d["error"].as_str().unwrap_or("The download failed");
                        ui.add(egui::Label::new(RichText::new(why).font(t.font(11.5)).color(t.caution)).wrap());
                    }
                    _ => {}
                }
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if installed {
                    let r = ui.button(tr("Remove"));
                    register(ui.ctx(), format!("ml:remove:{id}"), r.rect);
                    if r.clicked() {
                        let _ = app.run("ml.models.remove", json!({"id": id}));
                    }
                    if !selected {
                        let r = ui.button(tr("Use"));
                        register(ui.ctx(), format!("ml:use:{id}"), r.rect);
                        if r.clicked() {
                            let _ = app.run("ml.models.select", json!({"id": id}));
                        }
                    }
                } else {
                    match dl_state {
                        Some("running") => {
                            let r = ui.button(tr("Cancel"));
                            register(ui.ctx(), format!("ml:cancelDownload:{id}"), r.rect);
                            if r.clicked() {
                                let _ = app.run("ml.models.downloadCancel", json!({"id": id}));
                            }
                        }
                        Some("done" | "installing") => {}
                        _ if !host.is_empty() => {
                            let label = if dl_state == Some("failed") { "Try again" } else { "Download" };
                            let r = ui.button(tr(label));
                            register(ui.ctx(), format!("ml:download:{id}"), r.rect);
                            if r.clicked()
                                && let Err(e) = app.run("ml.models.download", json!({"id": id, "acceptLicence": true, "acknowledged": true}))
                            {
                                app.toast_error(ui.ctx(), e);
                            }
                        }
                        _ => {}
                    }
                }
            });
        });
    });
}
