//! Ported from OneSlips (NOTES-1028 notes 15, 16 and 29): how soon a slider follows the pointer, ← / → through a
//! selection, and what a brush stroke on a synced selection sets off.

use std::time::Duration;

use lcc_catalog::PhotoId;
use serde_json::json;

use crate::headless::Headless;
use crate::{LccApp, Services};

const T: Duration = Duration::from_secs(20);
const SETTLE: Duration = Duration::from_secs(120);

fn detail() -> Headless {
    let app = LccApp::new(lcc_engine::Session::with_demo(), Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [1200.0, 800.0], 1.0);
    let r = h.request("ui.set", json!({"view": "detail"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h
}

fn exec(h: &mut Headless, command: &str, params: serde_json::Value) -> serde_json::Value {
    let r = h.request("engine.execute", json!({"command": command, "params": params}), T);
    assert_eq!(r["ok"], true, "{command}: {r}");
    r["result"].clone()
}

/// Selects the first `n` photos of the view (the first active), with Auto Sync on for more than one.
fn select_synced(h: &mut Headless, n: usize) -> Vec<PhotoId> {
    let ids: Vec<PhotoId> = h.app.session.visible_cloned().into_iter().take(n).collect();
    assert_eq!(ids.len(), n, "the demo has {n} photos");
    exec(h, "library.select", json!({"ids": ids.iter().map(|i| i.0).collect::<Vec<_>>()}));
    if n > 1 {
        exec(h, "develop.autoSync", json!({"on": true}));
    }
    ids
}

fn press(h: &mut Headless, pos: egui::Pos2, pressed: bool) {
    h.app.synthetic.push(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() });
}

/// Note 15: a press on a slider and a slow drag (1 point a frame) moves the value from the first frames the pointer
/// moves (it waited for egui's 6-point click distance), and the drag is one undo step.
#[test]
fn a_slider_follows_a_slow_drag_from_its_first_move() {
    for n in [1usize, 3] {
        let mut h = detail();
        let ids = select_synced(&mut h, n);
        h.settle(SETTLE);
        let id = "light.exposure";
        let rect = h.app.widgets.iter().find(|(w, _)| w == &format!("slider:{id}")).map(|(_, r)| *r).expect("the Exposure slider");
        let value = |h: &Headless, p: PhotoId| lcc_develop::controls::get(&h.app.session.develop_of(p).unwrap(), id).unwrap_or(0.0);
        let before = value(&h, ids[0]);
        let undo = h.app.session.undo.len();
        let pos = rect.center() + egui::vec2(3.0, 0.0);
        h.app.synthetic.push(egui::Event::PointerMoved(pos));
        press(&mut h, pos, true);
        h.step();
        let mut moved_at = None;
        for k in 1..=5 {
            h.app.synthetic.push(egui::Event::PointerMoved(pos + egui::vec2(k as f32, 0.0)));
            h.step();
            if moved_at.is_none() && (value(&h, ids[0]) - before).abs() > 1e-9 {
                moved_at = Some(k);
            }
        }
        assert!(moved_at.is_some_and(|k| k <= 2), "n={n}: the value moves with the first points of travel: {moved_at:?}");
        assert!(value(&h, ids[0]) > before, "n={n}: to the right, up");
        if n > 1 {
            assert!((value(&h, ids[1]) - value(&h, ids[0])).abs() < 1e-9, "Auto Sync carries it live");
        }
        press(&mut h, pos + egui::vec2(5.0, 0.0), false);
        h.step();
        h.step();
        assert_eq!(h.app.session.undo.len(), undo + 1, "n={n}: one undo step");
        assert!(h.app.session.interaction.is_none());
    }
}

/// Note 16 in the app: → and ← (keys) step through the selection in Detail and Grid, keep it, and wrap.
#[test]
fn arrow_keys_step_through_the_selection_in_detail_and_grid() {
    for view in ["detail", "grid"] {
        let mut h = detail();
        let r = h.request("ui.set", json!({"view": if view == "grid" { "photoGrid" } else { view }}), T);
        assert_eq!(r["ok"], true, "{r}");
        let vis = h.app.session.visible_cloned();
        let picked = [vis[1], vis[4], vis[7]];
        exec(&mut h, "library.select", json!({"ids": picked.iter().map(|i| i.0).collect::<Vec<_>>()}));
        exec(&mut h, "develop.autoSync", json!({"on": true}));
        let mut seen = Vec::new();
        for key in ["right", "right", "right", "left"] {
            let r = h.request("ui.key", json!({"key": key}), T);
            assert_eq!(r["ok"], true, "{r}");
            seen.push(h.app.session.active().unwrap());
            assert_eq!(h.app.session.selection.ids.len(), 3, "{view}: the selection is kept");
        }
        assert_eq!(seen, vec![picked[1], picked[2], picked[0], picked[2]], "{view}: within the selection, wrapping round");
        assert!(h.app.session.auto_sync, "{view}: Auto Sync stays on");
    }
}

/// Note 29: while the photographer paints (and for a moment after each stroke) the thumbnails of the other photos an
/// edit reached keep the one they have; then they follow.
#[test]
fn while_painting_the_other_photos_thumbnails_wait() {
    let mut h = detail();
    let ids = select_synced(&mut h, 3);
    h.settle(SETTLE);
    let key = |h: &Headless| h.app.renderer.thumb(ids[1]).map(|t| t.key);
    let before = key(&h);
    assert!(before.is_some(), "the filmstrip has its thumbnail");
    h.app.paint_hold_until = h.time() + 0.9;
    exec(&mut h, "develop.set", json!({"ids": [ids[1].0], "values": {"light.exposure": 1.0}}));
    for _ in 0..90 {
        h.app.paint_hold_until = h.time() + 0.9;
        h.step();
        assert!(!h.app.renderer.is_pending(crate::render::Slot::Thumb(ids[1])), "nothing for the other photo while painting");
    }
    assert_eq!(key(&h), before, "its thumbnail waits");
    for _ in 0..120 {
        h.step();
    }
    h.settle(SETTLE);
    assert_ne!(key(&h), before, "its thumbnail follows once the painting pauses");
}
