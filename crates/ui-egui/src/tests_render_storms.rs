//! Renders never pile up (finishing a patch outline queued ~250
//! renders and kept 10–21 cores busy for over a minute).

use std::time::Duration;

use serde_json::json;

use crate::headless::{Headless, HeadlessView};
use crate::render::Slot;
use crate::state::{RightPanel, ViewMode};
use crate::{LccApp, Services};

const T: Duration = Duration::from_secs(30);

fn open_photo(h: &mut Headless) {
    let first = h.app.session.visible_cloned()[0];
    assert_eq!(h.request("engine.execute", json!({"command": "library.select", "params": {"ids": [first.0]}}), T)["ok"], true);
    h.app.ui.view = ViewMode::Detail;
    h.app.ui.right = RightPanel::Edit;
    h.app.ui.filmstrip = false;
    h.settle(T);
}

/// Edits faster than renders finish: each view keeps at most one render queued behind the one
/// running (latest wins), and the edits in between are never rendered.
#[test]
fn fast_edits_keep_at_most_two_renders_of_the_loupe_and_drop_the_ones_in_between() {
    let mut h = Headless::new(LccApp::new(lcc_engine::Session::with_demo(), Services::default()), [1400.0, 900.0], 1.0);
    h.settle(T);
    open_photo(&mut h);
    let done = h.app.renderer.completed;
    const EDITS: usize = 60;
    for i in 0..EDITS {
        h.app.run("develop.set", json!({"control": "light.exposure", "value": (i % 20) as f64 / 10.0 - 1.0})).unwrap();
        h.step();
        let views = h.app.renderer.pending_slots().iter().filter(|s| s.starts_with("Main") || s.starts_with("Region")).count();
        assert!(views <= 1, "one pending render per view, newest settings: {:?}", h.app.renderer.pending_slots());
        assert!(h.app.renderer.in_flight() <= 2, "renders in flight: {:?}", h.app.renderer.pending_slots());
        assert!(h.app.renderer.queued() <= 2, "queued: {}", h.app.renderer.queued());
    }
    h.settle(T);
    let rendered = h.app.renderer.completed - done;
    assert!(rendered < EDITS as u64, "{rendered} renders for {EDITS} edits: the superseded ones should have been dropped");
    // what is shown is the last edit
    let key = h.app.renderer.textures.get(&Slot::Main).map(|t| t.key);
    assert_eq!(h.app.renderer.wanted(Slot::Main), key);
}

/// The profile browser with hundreds of imported profiles: an edit re-renders the thumbnails in
/// view, not one per profile.
#[test]
fn an_edit_renders_only_the_profile_thumbnails_in_view() {
    let mut app = LccApp::new(lcc_engine::Session::with_demo(), Services::default());
    for i in 0..300 {
        app.session.lut_profiles.push(lcc_engine::cmd::lut_profiles::LutProfile {
            id: format!("lut:look-{i}"),
            name: format!("Look {i}"),
            group: format!("Pack {}", i / 50),
            file: String::new(),
        });
    }
    let photo = app.session.visible_cloned()[0];
    app.run("library.select", json!({"ids": [photo.0]})).unwrap();
    let ctx = egui::Context::default();
    crate::theme::install_fonts(&ctx);
    crate::theme::apply(&ctx);
    let frame = |app: &mut LccApp, n: u64| {
        let raw = HeadlessView::raw_input(egui::vec2(320.0, 700.0), 1.0, n as f64 / 60.0, vec![]);
        let mut out = ctx.run_ui(raw, |ui| {
            egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| crate::panels::profiles::show(app, ui, photo));
            });
        });
        out.textures_delta.clear();
    };
    let variants = |app: &LccApp| app.renderer.pending_slots().iter().filter(|s| s.starts_with("Variant")).count();
    for n in 0..3 {
        frame(&mut app, n);
    }
    let before = variants(&app);
    assert!(before > 0, "the cells in view ask for thumbnails");
    // an edit of the photo: every thumbnail is out of date
    app.run("develop.set", json!({"control": "light.exposure", "value": 0.7})).unwrap();
    for n in 3..6 {
        frame(&mut app, n);
    }
    let after = variants(&app);
    assert!(after <= 40, "{after} profile thumbnails rendering for 300+ profiles: only the ones in view should");
}

/// A render that solves a content-aware fill holds the "Filling spots" activity row while it is out.
#[test]
fn a_render_solving_a_fill_shows_in_the_activity_stack() {
    let mut h = Headless::new(LccApp::new(lcc_engine::Session::with_demo(), Services::default()), [1400.0, 900.0], 1.0);
    h.settle(T);
    open_photo(&mut h);
    let pts: Vec<[f64; 2]> = (0..200).map(|i| [0.3 + 0.002 * i as f64, 0.5 + 0.05 * ((i as f64) * 0.1).sin()]).collect();
    h.app.run("spot.add", json!({"mode": "fill", "points": pts, "size": 0.02})).unwrap();
    let mut seen = false;
    for _ in 0..600 {
        h.step();
        let rows = h.app.session.activity.list();
        if rows.iter().any(|r| r.kind == "fill") {
            assert!(h.app.renderer.filling());
            seen = true;
        }
        if seen && !h.app.renderer.is_pending(Slot::Main) {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(seen, "the fill's render held a row");
    h.settle(T);
    assert!(!h.app.session.activity.list().iter().any(|r| r.kind == "fill"), "the row is gone once the render is in");
}

/// A frame that holds the UI thread for half a second is kept with what ran in it (commands,
/// control requests, slow panels) and shown in `ui.inspect` → `perf.slowestFrame`; a quick one is not.
#[test]
fn a_slow_frame_is_kept_with_what_ran_in_it() {
    let mut app = LccApp::new(lcc_engine::Session::with_demo(), Services::default());
    app.perf.update_ms = 20.0;
    app.note_ran("develop.set");
    app.note_slow_frame(100);
    assert!(app.slowest_frame.is_none(), "a quick frame is not slow");
    app.note_ran("ui.inspect");
    let mut lap = crate::now_ms() - 400.0;
    app.lap(&mut lap, "panel.tools");
    app.perf.update_ms = 900.0;
    app.note_slow_frame(100);
    let slow = app.slowest_frame.clone().expect("the slow frame is kept");
    assert_eq!(slow.ms, 900.0);
    assert!(slow.ran.iter().any(|r| r == "ui.inspect"));
    assert!(slow.ran.iter().any(|r| r.starts_with("panel.tools")), "{:?}", slow.ran);
    let mut h = Headless::new(app, [800.0, 600.0], 1.0);
    let reply = h.request("ui.inspect", json!({}), T);
    assert_eq!(reply["result"]["perf"]["slowestFrame"]["ms"], 900.0, "{reply}");
}
