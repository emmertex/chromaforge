//! The Compact layout (Settings ▸ General ▸ Layout, `view.compactLayout`): Classic's panels drawn
//! denser. Switching between Classic, Compact and Modern keeps the photo's edit and the open
//! panel; Classic draws exactly as without Compact.

use serde_json::json;

use crate::headless::Headless;
use crate::tests_classic::{T, run};
use crate::{LccApp, Services};

fn develop() -> Headless {
    let app = LccApp::new(lcc_engine::Session::with_demo(), Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [1400.0, 1000.0], 1.0);
    h.settle(std::time::Duration::from_secs(120));
    let r = h.request("ui.set", json!({"view": "detail"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    h
}

fn rect(h: &Headless, id: &str) -> Option<egui::Rect> {
    h.app.widgets.iter().find(|(w, _)| w == id).map(|(_, r)| *r)
}

fn exposure(h: &Headless) -> f64 {
    let id = h.app.session.active().expect("a photo");
    h.app.session.develop_of(id).expect("its edit").light.exposure
}

#[test]
fn compact_is_denser_and_switching_keeps_the_edit_and_the_panel() {
    let mut h = develop();
    run(&mut h, "develop.set", json!({"control": "light.exposure", "value": 0.7}));
    let panel = h.app.ui.right;
    let classic_style = h.view.ctx.global_style();
    let classic = rect(&h, "section:light").expect("the Light section header");

    assert_eq!(run(&mut h, "view.compactLayout", json!({"on": true}))["on"], true);
    h.step();
    let compact = rect(&h, "section:light").expect("the Light section header, compact");
    assert!(compact.height() < classic.height(), "compact header {compact:?} vs classic {classic:?}");
    assert!(crate::theme::compact(&h.view.ctx));
    assert_eq!(exposure(&h), 0.7, "the edit stays");
    assert_eq!(h.app.ui.right, panel, "the panel stays");

    // Modern overrides Compact; Compact again turns Modern off
    run(&mut h, "view.modernLayout", json!({"on": true}));
    assert!(!h.app.compact() && !crate::theme::compact(&h.view.ctx));
    run(&mut h, "view.compactLayout", json!({"on": true}));
    assert!(!h.app.ui.modern.enabled && h.app.compact());

    // back to Classic: the very same style and header as before
    run(&mut h, "view.compactLayout", json!({"on": false}));
    h.step();
    assert_eq!(*h.view.ctx.global_style(), *classic_style, "Classic's style is untouched");
    assert_eq!(rect(&h, "section:light").map(|r| r.height()), Some(classic.height()));
    assert_eq!(exposure(&h), 0.7);
    assert_eq!(h.app.ui.right, panel);
}
