//! Settings ▸ search: the field over the list of sections finds a section by any text its page
//! shows (headings, labels, options, hints), in the app's language, and by the id of a control's
//! command; the open page stays open and marks the matches.

use std::time::Duration;

use serde_json::json;

use crate::headless::Headless;
use crate::state::Dialog;
use crate::{LccApp, Services};

const T: Duration = Duration::from_secs(20);

fn open(tab: &str) -> Headless {
    let services = Services { png: None, ..Default::default() };
    let mut h = Headless::new(LccApp::new(lcc_engine::Session::new(), services), [1600.0, 1400.0], 1.0);
    let r = h.request("engine.execute", json!({"command": "app.settings", "params": {"tab": tab}}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.request("ui.widgets", json!({}), T);
    steps(&mut h, 6);
    h
}

fn steps(h: &mut Headless, n: usize) {
    for _ in 0..n {
        h.step();
    }
}

/// Types `query` into the search field and lets the pages be read.
fn search(h: &mut Headless, query: &str) {
    h.app.modern_sheets.settings.search = query.to_string();
    steps(h, 12);
}

fn listed(h: &Headless, section: &str) -> bool {
    h.app.widgets.iter().any(|(w, _)| w == &format!("settings:section:{section}"))
}

fn open_tab(h: &Headless) -> String {
    match &h.app.ui.dialog {
        Some(Dialog::Settings { tab }) => tab.clone(),
        other => panic!("Settings isn't open: {}", other.is_some()),
    }
}

#[test]
fn a_control_s_label_keeps_its_section_listed() {
    let mut h = open("general");
    search(&mut h, "memory for caches");
    assert!(listed(&h, "performance"), "the row's label is on Performance's page");
    assert!(!listed(&h, "display"), "a section without the words is not listed");
}

#[test]
fn a_checkbox_s_text_and_a_hint_keep_their_section_listed() {
    let mut h = open("general");
    search(&mut h, "use the gpu for rendering");
    assert!(listed(&h, "performance"), "a checkbox's text");
    assert!(!listed(&h, "interface"), "a page without it is dropped");
    search(&mut h, "stays up for its duration");
    assert!(listed(&h, "slideshow"), "a hint's text");
    assert!(!listed(&h, "interface"), "a page without it is dropped");
    assert!(!listed(&h, "general"));
}

#[test]
fn matching_ignores_case_and_wants_every_word_in_any_order() {
    let mut h = open("general");
    search(&mut h, "CACHES MEMORY");
    assert!(listed(&h, "performance"));
    search(&mut h, "memory flicker zebra");
    assert!(!listed(&h, "performance"), "a word the page doesn't have rules it out");
}

#[test]
fn a_query_in_the_apps_language_finds_the_translated_text() {
    if lcc_engine::fonts::BUILD_LANGUAGE.is_some() {
        lcc_testkit::skip!(Fonts, "a single-language build offers only its language and English");
        return;
    }
    let mut h = open("general");
    h.app.ui.language = crate::i18n::Locale::De;
    crate::i18n::set_language(crate::i18n::Locale::De);
    search(&mut h, "Speicher für Caches");
    let found = listed(&h, "performance");
    let others = listed(&h, "interface");
    crate::i18n::set_language(crate::i18n::Locale::En);
    assert!(found, "the German label finds Performance");
    assert!(!others, "and drops a page without it");
}

#[test]
fn a_command_id_finds_the_section_of_its_control() {
    let mut h = open("general");
    search(&mut h, "settings.gpu");
    assert!(listed(&h, "performance"), "the checkbox `settings.gpu` is on Performance's page");
    assert!(!listed(&h, "display"));
}

#[test]
fn the_open_page_stays_open_when_the_search_hides_it() {
    let mut h = open("display");
    search(&mut h, "stays up for its duration");
    assert!(!listed(&h, "display"), "the open section has no match, so the list drops it");
    assert!(listed(&h, "slideshow"));
    assert_eq!(open_tab(&h), "display", "the search never switches the page");
}

#[test]
fn matches_in_the_open_page_are_marked_and_an_empty_query_marks_nothing() {
    let mut h = open("performance");
    search(&mut h, "gpu");
    assert!(crate::settings_search::highlighted(&h.view.ctx) > 0, "the page's GPU texts are marked");
    search(&mut h, "");
    assert_eq!(crate::settings_search::highlighted(&h.view.ctx), 0, "clearing the field removes the marks");
    for s in ["general", "performance", "display", "interface"] {
        assert!(listed(&h, s), "{s} is listed again");
    }
    search(&mut h, "zebra");
    assert_eq!(crate::settings_search::highlighted(&h.view.ctx), 0, "no match, no mark");
}

#[test]
fn closing_settings_drops_what_the_search_read() {
    let mut h = open("general");
    search(&mut h, "memory for caches");
    assert!(crate::settings_search::indexed_sections(&h.view.ctx) > 1, "the pages were read");
    h.request("ui.key", json!({"key": "Escape"}), T);
    steps(&mut h, 4);
    assert!(h.app.ui.dialog.is_none(), "Settings closed");
    assert_eq!(crate::settings_search::indexed_sections(&h.view.ctx), 0, "nothing of it is kept");
}

#[test]
fn reading_the_other_pages_starts_nothing() {
    let mut h = open("general");
    search(&mut h, "memory for caches");
    assert!(crate::settings_search::indexed_sections(&h.view.ctx) > 1, "the other pages were read");
    assert!(h.app.peer_sync.finding.is_none(), "reading Sync's page started no device discovery");
    assert!(!crate::settings_search::off_screen(&h.view.ctx), "and the page on screen isn't taken for one that isn't");
}

#[test]
fn the_notifications_page_is_found_by_its_text_and_reading_it_changes_nothing() {
    let mut h = open("general");
    let before = h.app.ui.toasts.len();
    search(&mut h, "stays until you dismiss it");
    assert!(listed(&h, "notifications"), "a hint on Notifications' page keeps its section listed");
    assert!(!listed(&h, "display"), "a section without the words is not listed");
    assert_eq!(h.app.ui.toasts.len(), before, "reading the page fired no test message");
}

#[test]
fn a_page_opened_on_its_own_still_starts_its_work() {
    let mut h = open("sync");
    steps(&mut h, 4);
    assert!(h.app.peer_sync.finding.is_some(), "Sync's own page looks for devices when it is open");
}

#[test]
fn reading_the_controls_page_takes_nothing_from_the_device() {
    let mut h = Headless::new(
        {
            let mut app = LccApp::new(lcc_engine::Session::new(), Services { png: None, ..Default::default() });
            app.surface.runtime.settings_mut().enabled = true;
            app
        },
        [1600.0, 1400.0],
        1.0,
    );
    h.request("engine.execute", json!({"command": "app.settings", "params": {"tab": "general"}}), T);
    steps(&mut h, 6);
    // (the frame that reads the pages: the next one's tick would drop what was armed)
    h.app.modern_sheets.settings.search = "memory for caches".into();
    h.step();
    assert!(h.app.surface.editor.touch_find.is_none(), "reading Controls' page didn't arm touch-to-find");
}

#[test]
fn a_word_without_a_dot_never_finds_a_command_id() {
    let mut h = open("general");
    // `settings.hdrDisplay` is a control's id on Performance; its page never writes the word as one
    search(&mut h, "hdrdisplay");
    assert!(!listed(&h, "performance"), "a word of an id alone finds nothing");
    search(&mut h, "settings.hdrDisplay");
    assert!(listed(&h, "performance"), "the id as a whole finds it");
}
