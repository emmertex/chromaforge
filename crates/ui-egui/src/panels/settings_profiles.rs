//! Settings ▸ Profiles: the Profile Manager (ported from OneSlips, LR-PROF-MANAGE / LR-PROF-BODY):
//! hide profiles from the menu and the browser, and give each camera body its own camera profile.

use serde_json::{Value, json};

use super::settings::{heading, hint, row};
use crate::LccApp;
use crate::theme::Tokens;
use crate::widgets::{register, text_button};

pub fn settings_tab(app: &mut LccApp, ui: &mut egui::Ui, t: &Tokens) {
    let m = app.session.execute("profiles.manager", &json!({})).unwrap_or_default();
    heading(ui, t, "Camera bodies");
    hint(
        ui,
        t,
        "Raws from a body start with its camera profile when they are imported. Apply to Photos gives it to the body's photos already in the library.",
    );
    let bodies: Vec<Value> = m["bodies"].as_array().cloned().unwrap_or_default();
    if bodies.is_empty() {
        hint(ui, t, "No raw photos with a known camera body in this library yet.");
    }
    for (i, b) in bodies.iter().enumerate() {
        let Some(key) = b["body"].as_str() else { continue };
        let label = format!("{} · {}", b["camera"].as_str().unwrap_or(""), b["serial"].as_str().unwrap_or(""));
        let current = b["profile"].as_str().unwrap_or("").to_string();
        let options = options_for(b["camera"].as_str().unwrap_or(""));
        row(ui, t, &label, |ui| {
            let shown = options.iter().find(|(id, _)| *id == current).map(|(_, n)| n.clone()).unwrap_or_else(|| current.clone());
            let mut chosen = current.clone();
            let combo = crate::widgets::combo_box(format!("bodyProfile{i}")).width(240.0).selected_text(shown).show_ui(ui, |ui| {
                for (id, name) in &options {
                    ui.selectable_value(&mut chosen, id.clone(), name);
                }
            });
            register(ui.ctx(), format!("combo:bodyProfile:{key}"), combo.response.rect);
            if chosen != current {
                let _ = app.run("profile.setBodyDefault", json!({"id": chosen, "body": key}));
            }
            let photos = b["photos"].as_u64().unwrap_or(0);
            if photos > 0 && !current.is_empty() && text_button(ui, &format!("bodyApply:{key}"), crate::i18n::tr("Apply to Photos"), false).clicked()
            {
                let _ = app.run("profile.applyBodyDefaults", json!({}));
            }
        });
    }
    heading(ui, t, "Profiles");
    hint(ui, t, "Hidden profiles are left out of the profile menu and browser. Photos that use one keep it.");
    let profiles: Vec<Value> = m["profiles"].as_array().cloned().unwrap_or_default();
    let mut groups: Vec<String> = Vec::new();
    for p in &profiles {
        if let Some(g) = p["group"].as_str()
            && !groups.iter().any(|x| x == g)
        {
            groups.push(g.to_string());
        }
    }
    for g in &groups {
        let builtin = profiles.iter().any(|p| p["group"] == *g && p["kind"] == "look");
        let title = crate::i18n::builtin_label(g, builtin);
        for p in profiles.iter().filter(|p| p["group"] == *g) {
            let (Some(id), Some(name)) = (p["id"].as_str(), p["name"].as_str()) else { continue };
            let hidden = p["hidden"].as_bool().unwrap_or(false);
            let label = format!("{title} ▸ {}", crate::i18n::profile_label(id, name));
            row(ui, t, &label, |ui| {
                let text = if hidden { crate::i18n::tr("Show") } else { crate::i18n::tr("Hide") };
                if text_button(ui, &format!("profileHide:{id}"), text, hidden).clicked() {
                    let _ = app.run("profile.hide", json!({"id": id, "hidden": !hidden}));
                }
            });
        }
    }
}

/// The camera profiles a body of `camera` can start with: the camera's default, the built-in colour,
/// and the imported (.dcp) and calibrated profiles for that camera (as the Profiles panel offers them).
fn options_for(camera: &str) -> Vec<(String, String)> {
    let mut options: Vec<(String, String)> =
        vec![(String::new(), crate::i18n::tr("Camera default").to_string()), ("lc".into(), crate::i18n::tr("Built-in colour").to_string())];
    for (pid, p) in lcc_raw::dcp::library() {
        if lcc_raw::dcp::covers_model(&p.model, camera) {
            let name = if p.name.is_empty() { pid.trim_start_matches("dcp:").to_string() } else { p.name.clone() };
            options.push((pid, name));
        }
    }
    if let Some(cal) = lcc_raw::dcp::calibrated_for_camera(camera) {
        options.extend(cal.looks.iter().map(|(pid, p)| (pid.clone(), p.name.clone())));
    }
    options
}
