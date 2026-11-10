//! The device's camera roll as an import source (Android): `file.addCameraRoll` asks the host
//! ([`crate::Services::camera_roll`]) for the photos in DCIM/Camera, which it lists through the
//! system's media store (asking for the photo permission first), and opens the import review on
//! them, as Import from Folder… does. On the first start a window asks whether to add it; the
//! answer is kept in the UI settings (`camera_roll_asked`), and Settings ▸ Folders offers it again.

use egui::{Align2, RichText, vec2};
use serde_json::Value;
use std::sync::mpsc::TryRecvError;

use crate::LccApp;
use crate::theme::Tokens;
use crate::widgets::register;

/// `file.addCameraRoll`: starts listing the camera roll; the import review opens when it answers.
pub fn start(app: &mut LccApp) -> Result<Value, String> {
    app.ui.camera_roll_asked = true;
    if app.pending_camera_roll.is_some() {
        return Ok(Value::Null);
    }
    let Some(list) = app.services.camera_roll.as_mut() else {
        return Err("there is no camera roll on this platform".into());
    };
    app.pending_camera_roll = Some(list()?);
    Ok(Value::Null)
}

/// Each frame: the host's answer opens the import review, or says why there is nothing to import.
pub fn poll(app: &mut LccApp, ctx: &egui::Context) {
    let Some(rx) = &app.pending_camera_roll else { return };
    let answer = match rx.try_recv() {
        Ok(a) => a,
        Err(TryRecvError::Empty) => {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
            return;
        }
        Err(TryRecvError::Disconnected) => Err(crate::i18n::tr("the camera roll could not be read").to_string()),
    };
    app.pending_camera_roll = None;
    match answer {
        Ok(paths) if paths.is_empty() => app.toast(ctx, crate::i18n::tr("There are no photos in the camera roll")),
        Ok(paths) => {
            if let Err(e) = crate::import::open(app, paths) {
                app.toast_error(ctx, e);
            }
        }
        Err(e) => app.toast_error(ctx, format!("{}: {e}", crate::i18n::tr("Couldn't add the camera roll"))),
    }
}

/// Whether the first-start question is due.
pub fn prompt_due(app: &LccApp) -> bool {
    app.services.camera_roll.is_some() && !app.ui.camera_roll_asked && app.library_problem.is_none() && app.pending_camera_roll.is_none()
}

/// The first-start question: add the camera roll to the library?
pub fn prompt(app: &mut LccApp, ctx: &egui::Context) {
    if !prompt_due(app) {
        return;
    }
    let t = Tokens::get(ctx);
    let mut choice = None;
    let frame = crate::dialog_frame::card_frame(ctx);
    egui::Window::new(crate::i18n::tr("Add Your Camera Roll?"))
        .id(egui::Id::new("camera-roll-prompt"))
        .title_bar(false)
        .order(egui::Order::Foreground)
        .collapsible(false)
        .resizable(false)
        .frame(frame)
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .default_width(420.0)
        .show(ctx, |ui| {
            ui.set_width((ctx.content_rect().width() - 64.0).clamp(240.0, 400.0));
            ui.spacing_mut().item_spacing.y = 8.0;
            crate::dialog_frame::card_header(ui, crate::i18n::tr("Add Your Camera Roll?"), None);
            ui.add(
                egui::Label::new(
                    RichText::new(crate::i18n::tr(
                        "{app} can add the photos in your camera roll (DCIM/Camera) to the catalog. Android asks you to allow access to your photos first.",
                    ))
                    .color(t.text),
                )
                .wrap(),
            );
            ui.label(RichText::new(crate::i18n::tr("You can add it later from Settings ▸ Folders.")).color(t.text_dim));
            ui.horizontal(|ui| {
                for (id, label, add) in [("cameraRollAdd", "Add Camera Roll", true), ("cameraRollNotNow", "Not Now", false)] {
                    let r = ui.add(egui::Button::new(crate::i18n::tr(label)).min_size(vec2(0.0, 26.0)));
                    register(ui.ctx(), format!("button:{id}"), r.rect);
                    if r.clicked() {
                        choice = Some(add);
                    }
                }
            });
        });
    match choice {
        Some(true) => {
            if let Err(e) = app.run("file.addCameraRoll", Value::Null) {
                app.toast_error(ctx, e);
            }
        }
        Some(false) => app.ui.camera_roll_asked = true,
        None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::mpsc::{Sender, channel};
    use std::sync::{Arc, Mutex};

    type Answer = Arc<Mutex<Option<Sender<Result<Vec<String>, String>>>>>;

    fn app_with_roll() -> (LccApp, Answer) {
        let answer: Answer = Default::default();
        let a = answer.clone();
        let services = crate::Services {
            camera_roll: Some(Box::new(move || {
                let (tx, rx) = channel();
                *a.lock().unwrap() = Some(tx);
                Ok(rx)
            })),
            ..Default::default()
        };
        (LccApp::new(lcc_engine::Session::new(), services), answer)
    }

    #[test]
    fn without_a_camera_roll_nothing_is_asked_and_the_command_says_why() {
        let mut app = LccApp::new(lcc_engine::Session::new(), crate::Services::default());
        assert!(!prompt_due(&app));
        assert!(app.run("file.addCameraRoll", json!({})).is_err());
    }

    #[test]
    fn the_first_start_asks_once_and_the_answer_is_remembered() {
        let (mut app, _) = app_with_roll();
        assert!(prompt_due(&app));
        app.run("file.addCameraRoll", json!({})).unwrap();
        assert!(app.ui.camera_roll_asked);
        assert!(!prompt_due(&app));
        // the answer survives the settings file
        let saved: crate::UiState = serde_json::from_value(serde_json::to_value(&app.ui).unwrap()).unwrap();
        assert!(saved.camera_roll_asked);
        // an old settings file without it asks
        let old: crate::UiState = serde_json::from_value(json!({})).unwrap();
        assert!(!old.camera_roll_asked);
    }

    #[test]
    fn a_refused_permission_is_reported_and_the_roll_can_be_asked_for_again() {
        let ctx = egui::Context::default();
        let (mut app, answer) = app_with_roll();
        app.run("file.addCameraRoll", json!({})).unwrap();
        answer.lock().unwrap().take().unwrap().send(Err("permission denied".into())).unwrap();
        poll(&mut app, &ctx);
        assert!(app.pending_camera_roll.is_none());
        // asking again starts a new listing
        app.run("file.addCameraRoll", json!({})).unwrap();
        assert!(app.pending_camera_roll.is_some());
        // a host that went away without answering is an error too, never a hang
        drop(answer.lock().unwrap().take());
        poll(&mut app, &ctx);
        assert!(app.pending_camera_roll.is_none());
    }

    #[test]
    fn the_camera_roll_opens_the_import_review() {
        let ctx = egui::Context::default();
        let (mut app, answer) = app_with_roll();
        app.run("file.addCameraRoll", json!({})).unwrap();
        let dir = std::env::temp_dir().join(format!("lc-camera-roll-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let img = lcc_raster::Rgba8::from_fn(8, 6, |x, y| [(x * 20) as u8, (y * 30) as u8, 9, 255]);
        let o = lcc_engine::export::ExportOptions { format: lcc_engine::export::ExportFormat::Png, ..Default::default() };
        let photo = dir.join("IMG_1.png");
        std::fs::write(&photo, lcc_engine::export::encode_image(&img, &o).unwrap()).unwrap();
        answer.lock().unwrap().take().unwrap().send(Ok(vec![photo.to_string_lossy().to_string()])).unwrap();
        poll(&mut app, &ctx);
        assert!(app.pending_camera_roll.is_none());
        assert!(app.scan.is_some(), "the import review's scan started");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
