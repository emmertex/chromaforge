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
    hint(
        ui,
        t,
        "Hidden profiles are left out of the profile menu and browser; removed built-in looks are left out of this install until restored. Photos that use one keep it. Type a new name or group and press Return.",
    );
    let removed = m["removed"].as_array().map_or(0, Vec::len);
    if removed > 0 {
        let label = crate::i18n::tr_format!("Restore Built-in Looks ({n})", n = removed);
        if text_button(ui, "profileRestoreBuiltins", &label, false).clicked() {
            let _ = app.run("profile.restoreBuiltins", json!({}));
        }
    }
    let profiles: Vec<Value> = m["profiles"].as_array().cloned().unwrap_or_default();
    let ids: Vec<String> = profiles.iter().filter_map(|p| p["id"].as_str().map(str::to_string)).collect();
    let mut groups: Vec<String> = Vec::new();
    for p in &profiles {
        if let Some(g) = p["group"].as_str()
            && !groups.iter().any(|x| x == g)
        {
            groups.push(g.to_string());
        }
    }
    for g in &groups {
        let builtin = profiles.iter().any(|p| p["group"] == *g && p["builtinLabel"] == true);
        let title = crate::i18n::builtin_label(g, builtin).to_string();
        let all_hidden = profiles.iter().filter(|p| p["group"] == *g).all(|p| p["hidden"] == true);
        // the group's own row: rename it, hide or remove all of it
        row(ui, t, &title, |ui| {
            if let Some(to) = edit_field(ui, &format!("profileGroupName:{g}"), g, 160.0) {
                let _ = app.run("profile.groupRename", json!({"group": g, "name": to}));
            }
            let text = if all_hidden { crate::i18n::tr("Show All") } else { crate::i18n::tr("Hide All") };
            if text_button(ui, &format!("profileGroupHide:{g}"), text, all_hidden).clicked() {
                let _ = app.run("profile.groupHide", json!({"group": g, "hidden": !all_hidden}));
            }
            if text_button(ui, &format!("profileGroupRemove:{g}"), crate::i18n::tr("Remove"), false).clicked() {
                let _ = app.run("profile.groupRemove", json!({"group": g}));
            }
        });
        for p in profiles.iter().filter(|p| p["group"] == *g) {
            let (Some(id), Some(name)) = (p["id"].as_str(), p["name"].as_str()) else { continue };
            let hidden = p["hidden"].as_bool().unwrap_or(false);
            let shown_name = if p["builtinLabel"] == true { crate::i18n::profile_label(id, name).to_string() } else { name.to_string() };
            let label = format!("{title} ▸ {shown_name}");
            row(ui, t, &label, |ui| {
                if let Some(to) = edit_field(ui, &format!("profileName:{id}"), name, 140.0) {
                    let _ = app.run("profile.rename", json!({"id": id, "name": to}));
                }
                if let Some(to) = edit_field(ui, &format!("profileGroup:{id}"), g, 110.0) {
                    let _ = app.run("profile.setGroup", json!({"id": id, "group": to}));
                }
                let at = ids.iter().position(|x| x == id).unwrap_or(0);
                if at > 0 && text_button(ui, &format!("profileUp:{id}"), "↑", false).clicked() {
                    let mut order = ids.clone();
                    order.swap(at, at - 1);
                    let _ = app.run("profile.reorder", json!({"ids": order}));
                }
                if at + 1 < ids.len() && text_button(ui, &format!("profileDown:{id}"), "↓", false).clicked() {
                    let mut order = ids.clone();
                    order.swap(at, at + 1);
                    let _ = app.run("profile.reorder", json!({"ids": order}));
                }
                let text = if hidden { crate::i18n::tr("Show") } else { crate::i18n::tr("Hide") };
                if text_button(ui, &format!("profileHide:{id}"), text, hidden).clicked() {
                    let _ = app.run("profile.hide", json!({"id": id, "hidden": !hidden}));
                }
                if p["kind"] == "look" && text_button(ui, &format!("profileRemove:{id}"), crate::i18n::tr("Remove"), false).clicked() {
                    let _ = app.run("profile.remove", json!({"id": id}));
                }
            });
        }
    }
}

/// A one-line field showing `current`, kept between frames while it is edited; the new text when
/// the edit was committed (Return) and differs.
fn edit_field(ui: &mut egui::Ui, widget: &str, current: &str, width: f32) -> Option<String> {
    let key = egui::Id::new(("settingsProfilesEdit", widget));
    let mut text: String = ui.data(|d| d.get_temp::<String>(key)).unwrap_or_else(|| current.to_string());
    let r = crate::text_field::TextField::singleline(widget, &mut text).width(width).show(ui);
    if r.committed() {
        ui.data_mut(|d| d.remove::<String>(key));
        let to = text.trim().to_string();
        return (!to.is_empty() && to != current).then_some(to);
    }
    if r.cancelled() || !r.editing {
        ui.data_mut(|d| d.remove::<String>(key));
    } else {
        ui.data_mut(|d| d.insert_temp(key, text));
    }
    None
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
