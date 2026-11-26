//! Settings ▸ Notifications: what the on-screen overlay pill says about edits to the images and the library, and how
//! the messages at the bottom of the window look and how long they stay. The pill's settings are saved with the
//! control surfaces and the messages' with the app's; the page reads and writes both through
//! [`NotificationSettings`], and each kind has a test button (`notifications.testFire`).

use serde_json::json;

use super::settings::{check, choices, heading_help, hint, row};
use super::surface_osd::{MODES, look_editor};
use crate::LccApp;
use crate::i18n::{tr, tr_format};
use crate::notifications::NotificationSettings;
use crate::theme::Tokens;
use crate::toast::{self, ToastKind};
use lcc_surface::settings::{OsdArea, OsdMode};

/// A button that fires a sample of `kind`; true when clicked.
fn test_button(ui: &mut egui::Ui, kind: &str) -> bool {
    test_button_if(ui, kind, true)
}

/// [`test_button`], greyed out when `usable` is false (the pill would show nothing).
fn test_button_if(ui: &mut egui::Ui, kind: &str, usable: bool) -> bool {
    ui.add_enabled_ui(usable, |ui| crate::widgets::text_button(ui, &format!("testFire.{kind}"), "Test", false).clicked()).inner
}

pub fn settings_tab(app: &mut LccApp, ui: &mut egui::Ui, t: &Tokens) {
    let mut s = NotificationSettings::read(app);
    let before = s.clone();
    let mut fire: Option<&'static str> = None;

    heading_help(ui, t, "Image and Library edits", "changeOverlay");
    let mut mode = s.pill.mode;
    row(ui, t, "Show changes", |ui| {
        if choices(ui, "osdMode", MODES, &mut mode) {
            s.pill.set_mode(mode);
        }
    });
    hint(
        ui,
        t,
        &format!(
            "{}\n{}",
            tr("Flash a message for each change: what changed, and how many photos were affected."),
            tr("Control surfaces are defined in Settings ▸ Controls.")
        ),
    );
    // with Show changes Off nothing below can be used: there is nothing to look at
    ui.add_enabled_ui(s.pill.mode != OsdMode::Off, |ui| {
        let p = &s.pill;
        let (mut in_develop, mut elsewhere) = (p.in_develop, p.elsewhere);
        // a sample shows as a control surface's would, so only the kind switched off can't be fired
        let (can_develop, can_elsewhere) = (p.allows(OsdArea::Develop, true), p.allows(OsdArea::Elsewhere, true));
        row(ui, t, "In Develop", |ui| {
            // the last one on can't be turned off
            ui.add_enabled_ui(!in_develop || elsewhere, |ui| check(ui, "notifications.inDevelop", &mut in_develop, "On"));
            if test_button_if(ui, "develop", can_develop) {
                fire = Some("develop");
            }
        });
        hint(ui, t, "This short message appears during and after each change to a develop setting.");
        row(ui, t, "Elsewhere", |ui| {
            ui.add_enabled_ui(!elsewhere || in_develop, |ui| check(ui, "notifications.elsewhere", &mut elsewhere, "On"));
            if test_button_if(ui, "elsewhere", can_elsewhere) {
                fire = Some("elsewhere");
            }
        });
        hint(ui, t, "This short message appears after each change to image metadata, or to the image library.");
        s.pill.set_areas(in_develop, elsewhere);
        let p = &mut s.pill;
        look_editor(ui, t, false, (&mut p.size, &mut p.opacity, &mut p.display_ms, &mut p.position, &mut p.background));
    });

    heading_help(ui, t, "Messages", "notifications");
    hint(ui, t, "Messages displayed after various changes in the application.");
    for (label, kind, toast_kind, text) in [
        ("Information", "information", ToastKind::Message, "Various updates, such as a completed background process."),
        ("Confirmation", "confirmation", ToastKind::Action, "A message with a button, such as a background action you can Undo."),
        ("Warnings", "warning", ToastKind::Error, "Warnings and errors."),
        ("Persistent", "persistent", ToastKind::Sticky, "A warning that stays until you dismiss it, such as changes not written to disk."),
    ] {
        row(ui, t, label, |ui| {
            if test_button(ui, kind) {
                fire = Some(kind);
            }
        });
        // the time follows the Shown for slider below, without any reading time a long text adds; persistent has none
        let text = match toast_kind {
            ToastKind::Sticky => tr(text).to_string(),
            k => {
                let ms = (toast::stay_seconds(k, "", s.messages.info_seconds()) * 1000.0).round();
                tr_format!("{text} ({ms} ms)", text = tr(text), ms = ms).to_string()
            }
        };
        hint(ui, t, &text);
    }
    let m = &mut s.messages;
    look_editor(ui, t, true, (&mut m.size, &mut m.opacity, &mut m.display_ms, &mut m.position, &mut m.background));

    if s != before {
        s.write(app);
    }
    if let Some(kind) = fire
        && let Err(e) = app.run("notifications.testFire", json!({"kind": kind}))
    {
        app.toast_as(&ui.ctx().clone(), crate::ToastKind::Error, e);
    }
}
