//! The AI Models dialog (`dialog.aiModels`): every AI mask model with its size, licence and
//! download, and which model computes Object masks (`ai.models.setObjectModel`). Each Download
//! button sits next to the size and licence it agrees to; nothing downloads by itself. The
//! downloadable AI Denoise models are listed too (`denoise::ai_models_section`).

use serde_json::json;

use crate::LccApp;
use crate::theme::Tokens;

/// The Object model choices: (command value, label).
const OBJECT_MODELS: &[(&str, &str)] = &[("auto", "Automatic"), ("sam3", "SAM 3"), ("sam2", "SAM 2.1")];

/// The Sky / Landscape / People model choices: (command value, label).
const SCENE_MODELS: &[(&str, &str)] = &[("analysis", "Photo analysis"), ("eomt", "EoMT")];

pub fn body(app: &mut LccApp, ui: &mut egui::Ui, error: &mut Option<String>) {
    let t = Tokens::get(ui.ctx());
    ui.label(crate::i18n::tr("AI masks use models that aren't part of {app}. Each is a one-time download; everything else works without them."));
    let status = app.session.segmenter.model_status();
    let mut running = false;
    for m in status.iter().filter(|m| m.offered) {
        crate::dialog_frame::group_begin(ui, Some(m.title.as_str()));
        ui.label(crate::i18n::tr(&m.purpose));
        ui.label(
            egui::RichText::new(format!(
                "{} {:.0} MB · {} {}",
                crate::i18n::tr("About"),
                m.size_bytes as f64 / 1e6,
                crate::i18n::tr("Licence:"),
                m.license
            ))
            .color(t.text_label),
        );
        if let Some(credit) = &m.credit {
            ui.label(egui::RichText::new(credit).color(t.text_dim));
        }
        let r = ui.link(crate::i18n::tr("Read the licence")).on_hover_text(&m.license_url);
        crate::widgets::register(ui.ctx(), format!("link:aiModelLicense-{}", m.id), r.rect);
        if r.clicked() {
            let _ = crate::links::open(app, &m.license_url);
        }
        let d = &m.download;
        if m.installed {
            ui.label(egui::RichText::new(crate::i18n::tr("Installed")).color(t.text_dim));
        } else if d.running {
            running = true;
            let mb = |b: u64| b as f64 / 1e6;
            let text = format!("{:.0} / {:.0} MB", mb(d.done), mb(d.total));
            crate::progress::Progress::new(crate::progress::fraction_of(d.done, d.total))
                .label(text)
                .widget(format!("progress:aiModel-{}", m.id))
                .show(ui);
            if crate::button::Button::new("Cancel Download").widget(format!("button:aiModelCancel-{}", m.id)).show(ui).clicked() {
                let _ = app.run("ai.models.cancel", json!({"id": m.id}));
            }
        } else {
            if let Some(e) = &d.error
                && !d.cancelled
            {
                ui.label(
                    egui::RichText::new(format!("{} {e}", crate::i18n::tr("The download didn't work:"))).color(egui::Color32::from_rgb(230, 90, 80)),
                );
            }
            // the button agrees to the size and licence shown right above it
            if crate::button::Button::new("Download").widget(format!("button:aiModelDownload-{}", m.id)).show(ui).clicked() {
                *error = app.run("ai.models.download", json!({"id": m.id, "acknowledged": true})).err();
            }
        }
    }
    crate::panels::denoise::ai_models_section(app, ui, &t);
    crate::dialog_frame::group_begin(ui, Some("Object masks use"));
    let current = app.session.segmenter.object_model().key();
    let items: Vec<(&str, &str)> = OBJECT_MODELS.iter().map(|(k, l)| (*l, *k)).collect();
    let active = OBJECT_MODELS.iter().position(|(k, _)| *k == current);
    if let Some(i) = crate::widgets::segmented(ui, "aiObjectModel", &items, active, OBJECT_MODELS.len())
        && let Some((k, _)) = OBJECT_MODELS.get(i)
    {
        *error = app.run("ai.models.setObjectModel", json!({"model": k})).err();
    }
    crate::dialog_frame::note(
        ui,
        egui::RichText::new(crate::i18n::tr("Automatic uses SAM 3 when it is installed, otherwise SAM 2.1. Describe masks always use SAM 3."))
            .color(t.text_dim),
    );
    crate::dialog_frame::group_end(ui);
    crate::dialog_frame::group_begin(ui, Some("Sky, Landscape and People masks use"));
    let current = app.session.segmenter.scene_model().key();
    let items: Vec<(&str, &str)> = SCENE_MODELS.iter().map(|(k, l)| (*l, *k)).collect();
    let active = SCENE_MODELS.iter().position(|(k, _)| *k == current);
    if let Some(i) = crate::widgets::segmented(ui, "aiSceneModel", &items, active, SCENE_MODELS.len())
        && let Some((k, _)) = SCENE_MODELS.get(i)
    {
        *error = app.run("ai.models.setSceneModel", json!({"model": k})).err();
    }
    crate::dialog_frame::note(
        ui,
        egui::RichText::new(crate::i18n::tr(
            "The photo analysis needs no download. EoMT is used once it is installed; until then the photo analysis stands in.",
        ))
        .color(t.text_dim),
    );
    crate::dialog_frame::group_end(ui);
    if let Some(e) = error.as_deref() {
        ui.label(egui::RichText::new(e).color(egui::Color32::from_rgb(230, 90, 80)));
    }
    if running {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
    }
}
