//! The Settings window (OneSlips, LR-PREF-WINDOW): in the Classic layout on a desktop-sized window,
//! Settings is a window with one row of tabs. Every tab is reachable from the row or its More
//! menu, the window keeps its size and place on every tab, its controls stay inside it on a small
//! screen, Esc closes it, and it opens again on the tab it was left at.

use std::time::Duration;

use serde_json::json;

use crate::headless::Headless;
use crate::state::Dialog;
use crate::{LccApp, Services};

const T: Duration = Duration::from_secs(20);

fn demo(size: [f32; 2]) -> Headless {
    let services = Services { png: None, write_shared: Some(std::sync::Arc::new(|_: &str, _: &[u8]| Ok(()))), ..Default::default() };
    let app = LccApp::new(lcc_engine::Session::with_demo(), services);
    let mut h = Headless::new(app, size, 1.0);
    h.settle(T);
    h
}

fn rect(h: &Headless, id: &str) -> Option<egui::Rect> {
    h.app.widgets.iter().rev().find(|(w, _)| w == id).map(|(_, r)| *r)
}

fn steps(h: &mut Headless, n: usize) {
    for _ in 0..n {
        h.step();
    }
}

fn settings(h: &mut Headless, params: serde_json::Value) {
    let r = h.request("engine.execute", json!({"command": "app.settings", "params": params}), T);
    assert_eq!(r["ok"], true, "{r}");
    steps(h, 6);
}

fn open_tab(h: &Headless) -> String {
    match &h.app.ui.dialog {
        Some(Dialog::Settings { tab }) => tab.clone(),
        other => panic!("Settings is not open: {other:?}"),
    }
}

fn shown_tabs() -> Vec<&'static str> {
    crate::panels::settings::shown_tabs().map(|(id, _)| *id).collect()
}

/// Clicks tab `id` in the row, or through More when it is in the menu.
fn pick(h: &mut Headless, id: &str) {
    let widget = format!("button:settingsTab-{id}");
    if rect(h, &widget).is_none() {
        let r = h.request("ui.clickWidget", json!({"id": "button:settingsTabMore"}), T);
        assert_eq!(r["ok"], true, "{id}: More: {r}");
        steps(h, 4);
    }
    let r = h.request("ui.clickWidget", json!({"id": widget}), T);
    assert_eq!(r["ok"], true, "{id}: {r}");
    steps(h, 6);
}

#[test]
fn classic_settings_is_a_window_with_one_row_of_tabs() {
    let mut h = demo([1300.0, 860.0]);
    settings(&mut h, json!({"tab": "general"}));
    assert!(rect(&h, "button:settingsTab-general").is_some(), "the General tab");
    assert!(rect(&h, "settings:section:general").is_none(), "no section rail in the window");
    let page = rect(&h, "settings:rows").expect("the page");
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, h.size);
    assert!(screen.contains_rect(page), "{page:?}");
    assert!(page.width() < screen.width() - 100.0, "a window, not the whole screen: {page:?}");
    // one line: every tab in the row at the same height
    let tops: Vec<f32> = shown_tabs().iter().filter_map(|id| rect(&h, &format!("button:settingsTab-{id}"))).map(|r| r.top()).collect();
    assert!(tops.len() > 4 && tops.iter().all(|t| (t - tops[0]).abs() < 0.5), "one row: {tops:?}");
}

#[test]
fn every_tab_is_reachable_and_the_window_keeps_its_size() {
    let mut h = demo([1300.0, 860.0]);
    settings(&mut h, json!({"tab": "general"}));
    let page = rect(&h, "settings:rows").expect("the page");
    let close = rect(&h, "settings:back").expect("Close");
    for id in shown_tabs() {
        pick(&mut h, id);
        assert_eq!(open_tab(&h), id, "{id} opened");
        assert_eq!(rect(&h, "settings:rows"), Some(page), "{id}: the page moved or changed size");
        assert_eq!(rect(&h, "settings:back"), Some(close), "{id}: Close moved");
    }
}

#[test]
fn a_small_window_keeps_the_tabs_and_close_on_screen() {
    let mut h = demo([700.0, 480.0]);
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, h.size);
    for id in shown_tabs() {
        settings(&mut h, json!({"tab": id}));
        assert_eq!(open_tab(&h), id);
        for w in ["settings:rows".to_string(), "settings:back".to_string(), format!("button:settingsTab-{id}")] {
            let r = rect(&h, &w).unwrap_or_else(|| panic!("{id}: no {w}"));
            assert!(screen.contains_rect(r), "{id}: {w} {r:?} runs off the {screen:?} window");
        }
        // the tabs that don't fit are under More, not on a second row
        assert!(rect(&h, "button:settingsTabMore").is_some(), "{id}: More");
    }
}

#[test]
fn esc_closes_and_settings_reopens_on_the_last_tab() {
    let mut h = demo([1300.0, 860.0]);
    settings(&mut h, json!({"tab": "general"}));
    pick(&mut h, "performance");
    let r = h.request("ui.key", json!({"key": "Escape"}), T);
    assert_eq!(r["ok"], true, "{r}");
    steps(&mut h, 4);
    assert!(h.app.ui.dialog.is_none(), "Esc closes Settings");
    settings(&mut h, json!({}));
    assert_eq!(open_tab(&h), "performance", "Settings… opens on the tab shown last");
    let r = h.request("ui.clickWidget", json!({"id": "settings:back"}), T);
    assert_eq!(r["ok"], true, "{r}");
    steps(&mut h, 2);
    assert!(h.app.ui.dialog.is_none(), "Close closes");
    let kept = h.app.ui.settings.settings_size;
    assert!(kept.is_some(), "the window's size is kept");
    // and it opens at that size again: it doesn't grow by its frame each time
    settings(&mut h, json!({}));
    steps(&mut h, 4);
    assert_eq!(h.app.ui.settings.settings_size, kept, "reopened at the size it was left at");
    // the touch screen's section names still open Settings, on the first tab they hold
    settings(&mut h, json!({"tab": "library"}));
    assert_eq!(open_tab(&h), "folders");
}

#[test]
fn the_modern_layout_keeps_the_full_window_settings() {
    let mut h = demo([1300.0, 860.0]);
    h.app.ui.modern.enabled = true;
    settings(&mut h, json!({"tab": "general"}));
    assert!(rect(&h, "settings:section:general").is_some(), "the section rail");
}

#[test]
fn window_sizes_fit_the_screen_and_ignore_nonsense() {
    use crate::panels::settings::{DEFAULT_SIZE, MIN_SIZE, window_size};
    let big = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1920.0, 1080.0));
    assert_eq!(window_size(big, None).default, egui::vec2(DEFAULT_SIZE[0], DEFAULT_SIZE[1]));
    assert_eq!(window_size(big, Some([1000.0, 700.0])).default, egui::vec2(1000.0, 700.0));
    assert_eq!(window_size(big, Some([f32::NAN, -3.0])).default, egui::vec2(DEFAULT_SIZE[0], DEFAULT_SIZE[1]));
    assert_eq!(window_size(big, Some([10.0, 10.0])).default, egui::vec2(MIN_SIZE[0], MIN_SIZE[1]));
    let small = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(500.0, 400.0));
    let s = window_size(small, Some([5000.0, 5000.0]));
    assert!(s.default.x <= 500.0 && s.default.y <= 400.0 && s.min.x <= s.max.x && s.min.y <= s.max.y, "{s:?}");
    let nan = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(f32::NAN, f32::INFINITY));
    let s = window_size(nan, None);
    assert!(s.default.x.is_finite() && s.default.y.is_finite(), "{s:?}");
}

#[test]
fn tabs_that_dont_fit_go_under_more_and_the_chosen_one_stays() {
    use crate::panels::settings::fit_tabs;
    let widths = [80.0; 6];
    assert_eq!(fit_tabs(&widths, 0, 1000.0, 50.0), ((0..6).collect(), vec![]));
    let (shown, hidden) = fit_tabs(&widths, 5, 300.0, 50.0);
    assert!(shown.contains(&5) && !hidden.contains(&5), "{shown:?} {hidden:?}");
    assert_eq!(shown.len() + hidden.len(), 6);
    assert!(shown.iter().map(|i| widths[*i]).sum::<f32>() <= 250.0);
    // no room at all: nothing panics, the chosen tab is still shown
    let (shown, _) = fit_tabs(&widths, 2, 0.0, 50.0);
    assert_eq!(shown, vec![2]);
    assert_eq!(fit_tabs(&[], 0, 100.0, 10.0), (vec![], vec![]));
}

#[test]
fn every_tab_and_section_name_opens_a_tab() {
    use crate::panels::settings::window_tab;
    for id in shown_tabs() {
        assert_eq!(window_tab(id), id);
    }
    for section in ["general", "library", "import", "editing", "ai", "display", "sync", "shortcuts", "controls", "slideshow"] {
        assert!(shown_tabs().contains(&window_tab(section)), "{section}");
    }
    assert_eq!(window_tab("nonsense"), "general");
}
