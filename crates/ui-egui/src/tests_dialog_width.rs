//! Small dialogs are sized to their content, not to the widest dialog shown before them: egui keeps
//! a window's size and never shrinks it, and every dialog without a window of its own shared one,
//! so after the Presets Manager (or any wide dialog) Create Collection opened ~2000 points wide on a
//! 3840-point window, its Name field the whole width. Each dialog kind now has its window id, and
//! no dialog is wider than the window.

use std::time::Duration;

use serde_json::json;

use crate::headless::Headless;
use crate::{LccApp, Services};

const T: Duration = Duration::from_secs(30);

fn demo(size: [f32; 2]) -> Headless {
    let app = LccApp::new(lcc_engine::Session::new(), Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, size, 1.0);
    h.settle(T);
    h
}

fn open(h: &mut Headless, command: &str, params: serde_json::Value) {
    let r = h.request("engine.execute", json!({"command": command, "params": params}), T);
    assert_eq!(r["ok"], true, "{command}: {r}");
    assert!(h.app.ui.dialog.is_some(), "{command} opens a dialog");
    h.settle(T);
}

fn dialog_width(h: &Headless) -> f32 {
    let dialog = h.app.ui.dialog.as_ref().expect("a dialog");
    let id = crate::panels::dialogs::window_id(dialog);
    h.view.ctx.memory(|m| m.area_rect(id)).map(|r| r.width()).expect("the dialog's window")
}

#[test]
fn small_dialogs_stay_their_size_after_a_wide_one() {
    let mut h = demo([3200.0, 1400.0]);
    // a wide dialog first
    open(&mut h, "dialog.presetsManager", json!({}));
    let wide = dialog_width(&h);
    assert!(wide >= 600.0, "the Presets Manager is wide: {wide}");
    h.app.ui.dialog = None;
    h.settle(T);
    for command in ["dialog.newAlbum", "dialog.newFolder"] {
        open(&mut h, command, json!({"name": "Ceremony"}));
        let w = dialog_width(&h);
        assert!(w < 560.0, "{command}: {w} points wide after a {wide}-point dialog, on a 3200-point window");
        // a long name doesn't stretch it either
        if let Some(crate::state::Dialog::NewAlbum { name, .. }) = h.app.ui.dialog.as_mut() {
            *name = "A very long collection name for the ceremony and the speeches after it".into();
        }
        h.settle(T);
        assert!(dialog_width(&h) < 560.0, "{command}: {} after a long name", dialog_width(&h));
        h.app.ui.dialog = None;
        h.settle(T);
    }
    // the Presets Manager keeps its own (wide) size
    open(&mut h, "dialog.presetsManager", json!({}));
    assert!(dialog_width(&h) >= 600.0, "{} vs {wide}", dialog_width(&h));
}

#[test]
fn no_dialog_is_wider_than_a_small_window() {
    let mut h = demo([800.0, 600.0]);
    for command in ["dialog.presetsManager", "dialog.newAlbum"] {
        open(&mut h, command, json!({}));
        assert!(dialog_width(&h) <= 800.0, "{command}: {} fits an 800-point window", dialog_width(&h));
        h.app.ui.dialog = None;
        h.settle(T);
    }
}
