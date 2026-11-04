//! Keep recent zoomed HDR views (Settings ▸ Display & Speed): used only where the GPU has room.

use std::time::Duration;

use serde_json::json;

use crate::headless::Headless;
use crate::{LccApp, Services};

const TIMEOUT: Duration = Duration::from_secs(10);

/// An HDR display whose GPU has `memory` bytes.
struct Output {
    memory: Option<u64>,
}

impl crate::HdrOutput for Output {
    fn headroom(&mut self) -> Option<f32> {
        Some(4.0)
    }
    fn upload(&mut self, slot: u8, _: usize, _: usize, _: &[f32]) -> Result<egui::TextureId, String> {
        Ok(egui::TextureId::User(u64::from(slot)))
    }
    fn gpu_memory(&mut self) -> Option<u64> {
        self.memory
    }
}

fn app(memory: Option<u64>) -> Headless {
    let services = Services { png: None, hdr_output: Some(Box::new(Output { memory })), ..Default::default() };
    let mut h = Headless::new(LccApp::new(lcc_engine::Session::with_demo(), services), [1000.0, 700.0], 1.0);
    h.app.ui.dialog = Some(crate::state::Dialog::Settings { tab: "performance".into() });
    h.step();
    crate::settings_screen::reveal(&mut h, "check:settings.hdrZoomCache");
    h
}

fn kept(h: &mut Headless) -> u64 {
    let r = h.request("ui.inspect", json!({}), TIMEOUT);
    r["result"]["perf"]["hdrWindowsKept"].as_u64().unwrap_or_else(|| panic!("hdrWindowsKept: {r}"))
}

fn click(h: &mut Headless) {
    // (found again before each click: the section can still shift as the demo library loads)
    crate::settings_screen::reveal(h, "check:settings.hdrZoomCache");
    let reply = h.request("ui.clickWidget", json!({"id": "check:settings.hdrZoomCache"}), TIMEOUT);
    assert_eq!(reply["ok"], true, "{reply}");
    h.step();
}

// Given a GPU with room for the windows, the setting is on by default and turns the cache off
// and on again
#[test]
fn with_room_on_the_gpu_the_setting_turns_the_hdr_zoom_cache_off_and_on() {
    for memory in [Some(16 << 30), None] {
        let mut h = app(memory);
        assert!(h.app.ui.settings.hdr_zoom_cache);
        assert!(kept(&mut h) > 0, "{memory:?}: on by default");
        click(&mut h);
        assert!(!h.app.ui.settings.hdr_zoom_cache);
        assert_eq!(kept(&mut h), 0, "{memory:?}: off");
        click(&mut h);
        assert!(h.app.ui.settings.hdr_zoom_cache, "{memory:?}: the second click turns it back on");
        assert!(kept(&mut h) > 0, "{memory:?}: on again");
    }
}

// Given a GPU without room for them, the setting can't turn the cache on, from the dialog or
// the control channel: zoomed views keep only the window they show
#[test]
fn without_room_on_the_gpu_the_hdr_zoom_cache_stays_off() {
    let mut h = app(Some(256 << 20));
    assert!(h.app.ui.settings.hdr_zoom_cache, "the saved choice is left as it is");
    assert_eq!(kept(&mut h), 0);
    click(&mut h);
    assert!(h.app.ui.settings.hdr_zoom_cache, "the greyed-out box changes nothing");
    assert_eq!(kept(&mut h), 0);
    let reply = h.request("ui.set", json!({"settings": {"hdrZoomCache": true}}), TIMEOUT);
    assert_eq!(reply["ok"], true, "{reply}");
    h.step();
    assert_eq!(kept(&mut h), 0, "nor through the control channel");
}
