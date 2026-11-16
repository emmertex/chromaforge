//! Two views of the same photos asking for different thumbnail sizes (the Slideshow and Print modules' filmstrip and
//! their own pictures) must not render them again on every frame: a larger thumbnail serves a smaller request.

use std::time::Duration;

use serde_json::json;

use crate::headless::Headless;
use crate::{LccApp, Services};

const T: Duration = Duration::from_secs(20);

fn jobs_while_idle(view: serde_json::Value) -> usize {
    let app = LccApp::new(lcc_engine::Session::with_demo(), Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [1600.0, 1000.0], 1.0);
    let r = h.request("ui.set", view, T);
    assert_eq!(r["ok"], true, "{r}");
    // long enough for every thumbnail to be asked for once
    for _ in 0..40 {
        h.step();
    }
    let before = h.app.renderer.thumb_jobs_built;
    for _ in 0..20 {
        h.step();
    }
    h.app.renderer.thumb_jobs_built - before
}

#[test]
fn the_grid_beside_the_navigator_settles() {
    let jobs = jobs_while_idle(json!({"module": "library", "view": "photoGrid", "leftPanel": true, "right": "none"}));
    assert_eq!(jobs, 0, "thumbnails rendered again on every frame");
}

#[test]
fn the_develop_filmstrip_beside_the_navigator_settles() {
    let jobs = jobs_while_idle(json!({"module": "develop", "view": "detail", "leftPanel": true}));
    assert_eq!(jobs, 0, "thumbnails rendered again on every frame");
}

/// The Slideshow and Print modules' filmstrip and their own pictures ask for different sizes of the same photos.
#[test]
fn the_slideshow_and_print_modules_settle() {
    for module in ["slideshow", "print"] {
        assert_eq!(jobs_while_idle(json!({"module": module})), 0, "{module}: thumbnails rendered again on every frame");
    }
}
