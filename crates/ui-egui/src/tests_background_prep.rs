//! Preparing photos in the background, in the app: the loupe prepares as many photos each way as
//! Settings ▸ Performance says, only the picks, rated and labelled photos when it says so, holds
//! background work back while the photo on screen is being made, and the Settings section changes
//! the settings.

use std::time::Duration;

use lcc_catalog::PhotoId;
use serde_json::json;

use crate::headless::Headless;
use crate::{LccApp, Services};

const T: Duration = Duration::from_secs(20);
const SETTLE: Duration = Duration::from_secs(120);

fn demo(size: [f32; 2]) -> Headless {
    let app = LccApp::new(lcc_engine::Session::with_demo(), Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, size, 1.0);
    h.settle(SETTLE);
    h
}

fn ask(h: &mut Headless, method: &str, params: serde_json::Value) -> serde_json::Value {
    let r = h.request(method, params.clone(), T);
    assert_eq!(r["ok"], true, "{method} {params}: {r}");
    r
}

fn prep(h: &mut Headless, settings: serde_json::Value) {
    ask(h, "engine.execute", json!({"command": "prefs.setBackgroundPrep", "params": {"settings": settings}}));
}

/// The loupe on photo `at`, drawn until its renders and the photos it prepares have settled.
fn loupe_on(h: &mut Headless, at: PhotoId) {
    ask(h, "engine.execute", json!({"command": "library.select", "params": {"ids": [at.0], "active": at.0}}));
    ask(h, "ui.set", json!({"view": "detail", "right": "none"}));
    for _ in 0..60 {
        h.step();
    }
    h.settle(SETTLE);
}

/// The photos prepared around photo `at` in the loupe, counted from a renderer that forgot what
/// it was asked for before (the photos around the ones on screen earlier stay in their slots).
fn asked_around(h: &mut Headless, at: PhotoId) -> Vec<u64> {
    loupe_on(h, at);
    h.app.renderer.forget_all();
    for _ in 0..60 {
        h.step();
    }
    h.settle(SETTLE);
    h.app.renderer.prefetch_keys()
}

/// "Photos ahead: 3" prepares three photos each way of the one on screen once it is shown, and a
/// step forward keeps them (nothing asked twice) and prepares the next one beyond them.
#[test]
fn the_photos_ahead_are_prepared_after_stepping() {
    let mut h = demo([1000.0, 700.0]);
    let ids = h.app.session.visible_cloned();
    assert!(ids.len() >= 9, "the demo has photos on both sides: {}", ids.len());
    let mid = ids.len() / 2;
    prep(&mut h, json!({"library": {"ahead": 3}}));
    let before = asked_around(&mut h, ids[mid]);
    assert_eq!(before.len(), 6, "three each way");
    // a step forward: the photo on screen moves on, the next one beyond is prepared
    loupe_on(&mut h, ids[mid + 1]);
    assert!(!h.app.prep_backward, "stepping forward");
    let after = asked_around(&mut h, ids[mid + 1]);
    assert_eq!(after.len(), 6, "three each way again");
    assert!(after.iter().any(|k| !before.contains(k)), "the photo three ahead of the new one is prepared");
    // off: nothing more is prepared
    prep(&mut h, json!({"library": {"ahead": 0}}));
    assert!(asked_around(&mut h, ids[mid]).is_empty(), "off: nothing prepared ahead");
}

/// "Which photos: Picks, rated and labelled" leaves the unflagged photos around the one on screen
/// alone; "Only the photo on screen" prepares nothing around it.
#[test]
fn picks_only_skips_the_unflagged_photos() {
    let mut h = demo([1000.0, 700.0]);
    let ids = h.app.session.visible_cloned();
    assert!(ids.len() >= 9);
    let mid = ids.len() / 2;
    // the six around the middle photo unmarked, then a pick ahead and a rated one behind
    let around: Vec<u64> = (mid - 3..=mid + 3).map(|i| ids[i].0).collect();
    ask(&mut h, "engine.execute", json!({"command": "library.select", "params": {"ids": around, "active": ids[mid].0}}));
    ask(&mut h, "engine.execute", json!({"command": "photo.flag", "params": {"ids": around, "flag": "none"}}));
    ask(&mut h, "engine.execute", json!({"command": "photo.rate", "params": {"ids": around, "rating": 0}}));
    ask(&mut h, "engine.execute", json!({"command": "photo.label", "params": {"ids": around, "label": "none"}}));
    ask(&mut h, "engine.execute", json!({"command": "photo.flag", "params": {"ids": [ids[mid + 2].0], "flag": "pick"}}));
    ask(&mut h, "engine.execute", json!({"command": "photo.rate", "params": {"ids": [ids[mid - 1].0], "rating": 2}}));
    prep(&mut h, json!({"library": {"ahead": 3, "scope": "marked"}}));
    let asked = asked_around(&mut h, ids[mid]);
    let marked_near = (mid - 3..=mid + 3).filter(|i| *i != mid && lcc_engine::prep::marked(&h.app.session, ids[*i])).count();
    assert!((1..6).contains(&marked_near), "some of the six around it are marked, not all: {marked_near}");
    assert_eq!(asked.len(), marked_near, "only the marked photos are prepared");
    prep(&mut h, json!({"library": {"scope": "onScreen"}}));
    assert!(asked_around(&mut h, ids[mid]).is_empty(), "only the photo on screen");
}

/// While the loupe's photo is being made, background work is held back (the background
/// preparation's build waits for it: `prep::wait_for_foreground`).
#[test]
fn background_work_waits_while_the_loupe_photo_renders() {
    let mut h = demo([1000.0, 700.0]);
    let ids = h.app.session.visible_cloned();
    ask(&mut h, "ui.set", json!({"view": "detail", "right": "none"}));
    ask(&mut h, "engine.execute", json!({"command": "library.select", "params": {"ids": [ids[1].0], "active": ids[1].0}}));
    let mut held = false;
    for _ in 0..200 {
        h.step();
        if h.app.renderer.wanted(crate::render::Slot::Main).is_some() && lcc_engine::prep::foreground_busy() {
            held = true;
            break;
        }
    }
    assert!(held, "the photo on screen being made holds background work back");
    h.settle(SETTLE);
}

/// A step onto the photo the background preparation is making waits for that preview: the loupe asks
/// for no render of its own meanwhile (that decoded the same raw twice, and held the build back while it
/// ran), and renders the photo once the build lets go of it.
#[test]
fn the_loupe_waits_for_the_photo_the_preparation_is_making() {
    let mut h = demo([1000.0, 700.0]);
    let ids = h.app.session.visible_cloned();
    loupe_on(&mut h, ids[1]);
    let held = lcc_engine::cmd::previews::hold_as_building(&h.app.session, ids[2]).expect("no build holds it");
    ask(&mut h, "engine.execute", json!({"command": "library.select", "params": {"ids": [ids[2].0], "active": ids[2].0}}));
    for _ in 0..40 {
        h.step();
        assert!(!h.app.renderer.is_pending(crate::render::Slot::Main), "no render of its own while the build makes it");
        assert!(h.app.renderer.textures.get(&crate::render::Slot::Main).is_none_or(|t| t.photo != ids[2]));
    }
    drop(held);
    h.settle(SETTLE);
    assert!(h.app.renderer.textures.get(&crate::render::Slot::Main).is_some_and(|t| t.photo == ids[2]), "rendered once the build let go");
}

/// The loupe waits for the preparation's render of the photo on screen only briefly
/// ([`crate::panels::detail::PREP_WAIT_MAX_S`]): a build held up (lowest priority on a busy
/// machine) doesn't keep the photo unrendered.
#[test]
fn the_loupe_renders_itself_when_the_preparation_takes_too_long() {
    let mut h = demo([1000.0, 700.0]);
    let ids = h.app.session.visible_cloned();
    loupe_on(&mut h, ids[1]);
    let _held = lcc_engine::cmd::previews::hold_as_building(&h.app.session, ids[2]).expect("no build holds it");
    ask(&mut h, "engine.execute", json!({"command": "library.select", "params": {"ids": [ids[2].0], "active": ids[2].0}}));
    let t0 = h.time;
    assert!(
        h.step_until(T, |h| h.app.renderer.textures.get(&crate::render::Slot::Main).is_some_and(|t| t.photo == ids[2])),
        "rendered by the loupe while the build still holds it"
    );
    let waited = h.time - t0;
    assert!(waited >= crate::panels::detail::PREP_WAIT_MAX_S * 0.9, "it waited first ({waited:.2} s)");
    assert!(waited < 3.5, "not much longer than the wait ({waited:.2} s)");
}

/// A held step key outruns the photos made ahead: their thumbnails go a second's worth of steps
/// ahead instead of five photos, so every step shows something.
#[test]
fn holding_a_step_key_makes_thumbnails_further_ahead() {
    let mut h = demo([1000.0, 700.0]);
    let ids = h.app.session.visible_cloned();
    assert!(ids.len() > 12, "the demo has enough photos ({})", ids.len());
    loupe_on(&mut h, ids[0]);
    // (full screen: no filmstrip asks for thumbnails; and none made yet)
    ask(&mut h, "ui.set", json!({"fullscreen": true}));
    h.app.renderer.forget_all();
    for _ in 0..30 {
        h.step();
    }
    let wanted = |h: &Headless, id: PhotoId| h.app.renderer.thumb(id).is_some() || h.app.renderer.is_pending(crate::render::Slot::Thumb(id));
    ask(&mut h, "ui.key", json!({"key": "ArrowRight"}));
    for _ in 0..30 {
        h.step();
    }
    assert!(!wanted(&h, ids[1 + 9]), "not asked for after a slow step");
    for _ in 0..3 {
        ask(&mut h, "ui.key", json!({"key": "ArrowRight"}));
        h.step();
    }
    let at = ids.iter().position(|id| Some(*id) == h.app.session.active()).unwrap();
    assert!(h.app.loupe_timing.holding(), "steps two frames apart are a held key");
    let far = (at + 9).min(ids.len() - 1);
    assert!(far > at + 5, "a photo further than five ahead");
    assert!(wanted(&h, ids[far]), "the thumbnail {} ahead is asked for while the key is held", far - at);
}

/// Settings ▸ Performance draws the section: for culling and the library, Previews, Photos ahead,
/// Eye focus ahead and Which photos; for editing, Photos ahead and Which photos; each choice shows
/// the setting in use.
#[test]
fn the_settings_section_is_drawn() {
    let mut h = demo([1600.0, 2600.0]);
    prep(&mut h, json!({"culling": {"ahead": 10}}));
    ask(&mut h, "engine.execute", json!({"command": "app.settings", "params": {"tab": "performance"}}));
    h.settle(T);
    for _ in 0..4 {
        h.step();
    }
    for key in ["culling", "library"] {
        for id in
            [format!("button:settingsPrepAhead-{key}-4"), format!("button:settingsPrepEyes-{key}-5"), format!("button:settingsPrepScope-{key}-2")]
        {
            assert!(h.app.widgets.iter().any(|(w, _)| *w == id), "{id} drawn");
        }
    }
    assert!(h.app.widgets.iter().any(|(w, _)| w == "button:settingsPrepAhead-develop-4"));
    assert!(h.app.widgets.iter().any(|(w, _)| w == "button:settingsPrepScope-develop-0"));
    assert_eq!(h.app.session.prep_settings().culling.ahead, 10);
}
