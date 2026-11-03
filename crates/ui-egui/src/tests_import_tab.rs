//! The Import tab (OneSlips LR-IMP-WINDOW), the default with Settings ▸ Import ▸ Import as a tab,
//! and the import review when that is off: both paths, in the Classic, Compact and Modern layouts.

use std::time::Duration;

use serde_json::{Value, json};

use crate::LccApp;
use crate::headless::Headless;
use crate::import_tab::Tab;
use crate::state::{Dialog, ViewMode};

const T: Duration = Duration::from_secs(10);
const SETTLE: Duration = Duration::from_secs(120);

fn app(size: [f32; 2], layout: &str) -> Headless {
    let services = crate::Services { png: None, ..Default::default() };
    let mut app = LccApp::new(lcc_engine::Session::new(), services);
    app.ui.view = ViewMode::PhotoGrid;
    match layout {
        "compact" => app.ui.settings.compact_layout = true,
        "modern" => app.ui.modern.enabled = true,
        _ => {}
    }
    Headless::new(app, size, 1.0)
}

fn exec(h: &mut Headless, command: &str, params: Value) -> Value {
    let r = h.request("engine.execute", json!({"command": command, "params": params}), T);
    assert_eq!(r["ok"], true, "{command} {params}: {r}");
    h.settle(SETTLE);
    r["result"].clone()
}

fn try_exec(h: &mut Headless, command: &str, params: Value) -> Value {
    let r = h.request("engine.execute", json!({"command": command, "params": params}), T);
    h.settle(SETTLE);
    r
}

/// A folder of `n` small PNGs (different bytes each) in the temporary folder.
fn folder(name: &str, n: u8) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("lc-ui-importtab-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let o = lcc_engine::export::ExportOptions { format: lcc_engine::export::ExportFormat::Png, ..Default::default() };
    for i in 0..n {
        let img = lcc_raster::Rgba8::from_fn(24, 16, |x, y| [(x * 9) as u8, (y * 12) as u8, 40 * i, 255]);
        std::fs::write(dir.join(format!("p{i}.png")), lcc_engine::export::encode_image(&img, &o).unwrap()).unwrap();
    }
    dir
}

fn has_widget(h: &mut Headless, name: &str) -> bool {
    for _ in 0..3 {
        h.step();
    }
    let found = h.app.widgets.iter().any(|(n, _)| n == name);
    found
}

#[test]
fn import_is_a_tab_by_default_and_browsing_shows_it() {
    let mut h = app([1400.0, 900.0], "classic");
    assert!(h.app.ui.settings.import_as_tab, "the default");
    h.settle(SETTLE);
    assert!(has_widget(&mut h, "button:tab-import"), "the Import | Library switch is in the top bar");
    exec(&mut h, "dialog.import", json!({}));
    assert_eq!(h.app.ui.tab, Tab::Import);
    assert!(!matches!(h.app.ui.dialog, Some(Dialog::Import { .. })), "no review dialog");
    assert!(has_widget(&mut h, "panel:importTab") && has_widget(&mut h, "importGo"));
    let dir = folder("default", 3);
    exec(&mut h, "library.browse", json!({"path": dir.to_string_lossy()}));
    assert_eq!(h.app.session.visible_cloned().len(), 3);
    assert_eq!(h.app.ui.tab, Tab::Import);
    // the view going back to the library brings back the Library tab
    exec(&mut h, "library.source", json!({"kind": "all"}));
    assert_eq!(h.app.ui.tab, Tab::Library);
    // a browsed folder only ever shows in the Import tab
    exec(&mut h, "library.browse", json!({"path": dir.to_string_lossy()}));
    assert_eq!(h.app.ui.tab, Tab::Import);
    exec(&mut h, "view.libraryTab", json!({}));
    assert_ne!(h.app.session.source, lcc_engine::LibrarySource::Folder, "the Library tab shows the library");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn with_the_option_off_import_opens_the_review_as_before() {
    let mut h = app([1400.0, 900.0], "classic");
    h.app.ui.settings.import_as_tab = false;
    h.settle(SETTLE);
    assert!(!has_widget(&mut h, "button:tab-import"), "no tabs");
    exec(&mut h, "dialog.import", json!({}));
    assert!(matches!(h.app.ui.dialog, Some(Dialog::Import { .. })), "the review");
    h.app.ui.dialog = None;
    let dir = folder("off", 2);
    exec(&mut h, "library.browse", json!({"path": dir.to_string_lossy()}));
    assert_eq!(h.app.ui.tab, Tab::Library, "a browsed folder shows in the library's own view");
    assert!(!has_widget(&mut h, "panel:importTab"));
    assert!(try_exec(&mut h, "view.importTab", json!({}))["ok"] == false, "no tab to switch to");
    // the review imports as it did
    exec(&mut h, "dialog.import", json!({"path": dir.to_string_lossy()}));
    h.step_until(SETTLE, |h| matches!(&h.app.ui.dialog, Some(Dialog::Import { opts }) if opts.candidates.len() == 2));
    assert!(matches!(&h.app.ui.dialog, Some(Dialog::Import { opts }) if opts.candidates.len() == 2));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_big_button_imports_the_picks_and_leaves_the_files_alone() {
    let mut h = app([1400.0, 900.0], "classic");
    let dir = folder("picks", 3);
    exec(&mut h, "library.browse", json!({"path": dir.to_string_lossy()}));
    let ids = h.app.session.visible_cloned();
    assert_eq!(ids.len(), 3);
    exec(&mut h, "photo.flag", json!({"ids": [ids[0].0], "flag": "pick"}));
    let targets = exec(&mut h, "import.targets", json!({}));
    assert_eq!(targets["picks"], 1, "{targets}");
    exec(&mut h, "import.configure", json!({"take": "picks", "keywords": "trip", "previews": "none"}));
    let r = exec(&mut h, "import.start", json!({}));
    assert_eq!(r["importing"], 1, "{r}");
    h.step_until(SETTLE, |h| h.app.import.is_none());
    assert_eq!(h.app.ui.tab, Tab::Library, "the photos come in in the Library tab");
    let p = h.app.session.catalog.photo(ids[0]).unwrap();
    assert!(!p.local, "the pick is in the library");
    assert!(p.meta.keywords.iter().any(|k| k == "trip"), "with the tab's keywords");
    assert!(h.app.session.catalog.photo(ids[1]).unwrap().local, "the others stay browsed");
    for i in 0..3 {
        assert!(dir.join(format!("p{i}.png")).is_file(), "added in place: no file moved");
    }
    // nothing left to take
    exec(&mut h, "library.browse", json!({"path": dir.to_string_lossy()}));
    assert!(try_exec(&mut h, "import.start", json!({"what": "picks"}))["ok"] == false);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn browsed_photos_cant_be_deleted_from_the_tab() {
    let mut h = app([1400.0, 900.0], "classic");
    let dir = folder("nodelete", 2);
    exec(&mut h, "library.browse", json!({"path": dir.to_string_lossy()}));
    let ids = h.app.session.visible_cloned();
    exec(&mut h, "library.select", json!({"ids": [ids[0].0]}));
    let r = try_exec(&mut h, "photo.delete", json!({}));
    assert_eq!(r["ok"], false, "{r}");
    let r = try_exec(&mut h, "photo.deleteFromDisk", json!({"confirm": true}));
    assert_eq!(r["ok"], false, "{r}");
    let p = h.app.session.catalog.photo(ids[0]).unwrap();
    assert!(p.local && !p.deleted);
    assert!(dir.join("p0.png").is_file() && dir.join("p1.png").is_file());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn import_presets_keep_the_tabs_settings() {
    let mut h = app([1400.0, 900.0], "classic");
    exec(&mut h, "import.configure", json!({"copy": true, "keywords": "a, b", "previews": "full"}));
    exec(&mut h, "import.saveSettings", json!({"name": "Card"}));
    exec(&mut h, "import.configure", json!({"copy": false, "keywords": "", "previews": "none"}));
    exec(&mut h, "import.usePreset", json!({"name": "card"}));
    let s = &h.app.ui.settings.import_tab;
    assert!(s.opts.copy && s.opts.keywords == "a, b" && s.previews == "full", "{s:?}");
    assert!(try_exec(&mut h, "import.configure", json!({"candidates": []}))["ok"] == false);
    assert!(try_exec(&mut h, "import.configure", json!({"take": "most"}))["ok"] == false);
    assert!(try_exec(&mut h, "import.usePreset", json!({"name": "none such"}))["ok"] == false);
}

/// The tab in every layout: its bar and button are drawn, and on a narrow window the sources and
/// settings are a tap away.
#[test]
fn the_tab_works_in_compact_and_modern_layouts() {
    for (layout, size) in [("compact", [1000.0, 760.0]), ("modern", [1300.0, 900.0]), ("modern", [400.0, 860.0]), ("classic", [760.0, 700.0])] {
        let mut h = app(size, layout);
        let dir = folder(&format!("{layout}-{}", size[0] as u32), 2);
        exec(&mut h, "library.browse", json!({"path": dir.to_string_lossy()}));
        assert_eq!(h.app.ui.tab, Tab::Import, "{layout}");
        h.settle(SETTLE);
        assert!(has_widget(&mut h, "panel:importTab"), "{layout} {size:?}");
        assert!(has_widget(&mut h, "importGo"), "{layout} {size:?}");
        if size[0] < 900.0 {
            assert!(has_widget(&mut h, "button:importPane-settings"), "{layout} {size:?}: the settings a tap away");
        } else {
            assert!(has_widget(&mut h, "label:importFrom"), "{layout} {size:?}: the sources beside the photos");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
