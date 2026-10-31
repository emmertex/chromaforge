//! The Plug-in Manager window: a list of plug-ins and pixel filters in sections of their own, the
//! selected one's page beside it (a plug-in's with tabs, a filter's and a failed module's without),
//! installs from the file picker or a dropped `.wasm`, the plug-ins folder, and its shortcut.
//!
//! Its own test binary: the Plug-in Manager's window state is the process's, and other UI tests
//! drawing frames at the same time would race it. The tests here take turns for the same reason.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use serde_json::{Value, json};

use lcc_ui_egui::headless::Headless;
use lcc_ui_egui::{LccApp, Services};

const T: Duration = Duration::from_secs(20);

fn turn() -> MutexGuard<'static, ()> {
    static TURN: Mutex<()> = Mutex::new(());
    TURN.lock().unwrap_or_else(PoisonError::into_inner)
}

fn app() -> Headless {
    // install into memory, never into the real user's settings folder
    lcc_engine::cmd::plugins::use_in_memory_manager();
    let services = Services { png: None, ..Default::default() };
    let app = LccApp::new(lcc_engine::Session::with_demo(), services);
    let mut h = Headless::new(app, [1400.0, 900.0], 1.0);
    // a window left open by another test starts closed
    exec(&mut h, "plugins.manager", json!({"open": false}));
    h.step();
    h
}

fn exec(h: &mut Headless, command: &str, params: Value) -> Value {
    let r = h.request("engine.execute", json!({"command": command, "params": params}), T);
    assert_eq!(r["ok"], true, "{command}: {r}");
    r["result"].clone()
}

fn fails(h: &mut Headless, command: &str, params: Value) -> String {
    let r = h.request("engine.execute", json!({"command": command, "params": params}), T);
    assert_eq!(r["ok"], false, "{command} {params} should fail: {r}");
    r["error"].to_string()
}

/// Runs a request and draws until it has happened.
fn ask(h: &mut Headless, method: &str, params: Value) -> Value {
    let r = h.request(method, params.clone(), T);
    assert_eq!(r["ok"], true, "{method} {params}: {r}");
    for _ in 0..4 {
        h.step();
    }
    r
}

fn ui_cmd(h: &mut Headless, command: &str, params: Value) -> Value {
    let r = exec(h, command, params);
    for _ in 0..4 {
        h.step();
    }
    r
}

fn click(h: &mut Headless, id: &str) {
    ask(h, "ui.clickWidget", json!({"id": id}));
}

/// The widget's rectangle on screen (`[x, y, w, h]`), if it is there.
fn widget(h: &mut Headless, id: &str) -> Option<Value> {
    let w = h.request("ui.widgets", json!({}), T);
    w["result"].as_array().into_iter().flatten().find(|x| x["id"] == id).map(|x| x["rect"].clone())
}

fn has_widget(h: &mut Headless, id: &str) -> bool {
    widget(h, id).is_some()
}

fn top(h: &mut Headless, id: &str) -> f64 {
    widget(h, id).unwrap_or_else(|| panic!("{id} on screen"))[1].as_f64().unwrap()
}

fn painted(h: &Headless, text: &str) -> bool {
    h.painted_text().iter().any(|t| t.contains(text))
}

/// The screen-reader name of the node whose name starts with `start` (the access tree must be on).
fn access_name(h: &Headless, start: &str) -> Option<String> {
    let update = h.view.access.as_ref()?;
    update.nodes.iter().filter_map(|(_, n)| n.label()).find(|l| l.starts_with(start)).map(str::to_string)
}

fn listed(h: &mut Headless, id: &str) -> Option<Value> {
    exec(h, "plugin.list", json!({}))["plugins"].as_array().unwrap().iter().find(|p| p["id"] == id).cloned()
}

/// A plug-in with `manifest` that answers every call with `{}`, written to `dir/<name>.wasm`.
fn module_file(dir: &Path, name: &str, manifest: &Value) -> String {
    let manifest = manifest.to_string();
    let esc = |s: &str| s.replace('"', "\\\"");
    let src = format!(
        r#"(module (memory (export "memory") 2)
  (data (i32.const 16) "{m}") (data (i32.const 8192) "{{}}")
  (func (export "lcc_abi_version") (result i32) (i32.const 1))
  (func (export "lcc_manifest") (result i64) (i64.or (i64.shl (i64.const {ml}) (i64.const 32)) (i64.const 16)))
  (func (export "lcc_alloc") (param i32) (result i32) (i32.const 65536))
  (func (export "lcc_handle") (param i32 i32) (result i64) (i64.or (i64.shl (i64.const 2) (i64.const 32)) (i64.const 8192))))"#,
        m = esc(&manifest),
        ml = manifest.len()
    );
    let path = dir.join(format!("{name}.wasm"));
    std::fs::write(&path, wat::parse_str(&src).unwrap()).unwrap();
    path.display().to_string()
}

/// A pixel filter module (PhotoCraft ABI) with `manifest` that leaves pixels alone; `analyze`: it
/// also measures (exports `pc_analyze`).
fn filter_module(manifest: &Value, analyze: bool) -> Vec<u8> {
    let manifest = manifest.to_string();
    let src = format!(
        r#"(module
  (memory (export "memory") 1)
  (data (i32.const 16) "{m}")
  (func (export "pc_abi_version") (result i32) (i32.const 1))
  (func (export "pc_manifest") (result i64) (i64.or (i64.shl (i64.const {len}) (i64.const 32)) (i64.const 16)))
  (func (export "pc_alloc") (param $n i32) (result i32) (local $old i32)
    (local.set $old (memory.grow (i32.shr_u (i32.add (local.get $n) (i32.const 65535)) (i32.const 16))))
    (if (result i32) (i32.eq (local.get $old) (i32.const -1)) (then (i32.const 0)) (else (i32.mul (local.get $old) (i32.const 65536)))))
  (func (export "pc_filter") (param i32 i32 i32 i32 i32 i32 i32 i32) (result i32) (i32.const 0))
  {analyze})"#,
        m = manifest.replace('"', "\\\""),
        len = manifest.len(),
        analyze = if analyze { r#"(func (export "pc_analyze") (param i32 i32 i32 i32 i32 i32 i32 i32) (result i64) (i64.const 0))"# } else { "" }
    );
    wat::parse_str(&src).unwrap()
}

fn folder(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("plugin-manager-ui-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// AC-1, AC-2: *Install from File…* asks the file picker and opens the approval for the chosen
/// module; cancelling the picker installs nothing and says nothing; the path box is gone. A single
/// `.wasm` dropped while the window is open opens the approval too; other drops don't.
#[test]
fn installs_come_from_the_file_picker_and_from_a_dropped_module() {
    let _turn = turn();
    let dir = folder("install");
    let path = module_file(&dir, "picked", &json!({"id": "test.mgr.picked", "name": "Picked", "version": "1.0.0"}));
    let mut h = app();
    ui_cmd(&mut h, "plugins.manager", json!({"open": true}));
    assert!(!painted(&h, "Module (.wasm)"), "no path box: {:?}", h.painted_text());

    // the picker answers with the module: its approval opens; Cancel installs nothing
    let answer = std::sync::Arc::new(Mutex::new(vec![path.clone()]));
    let a = answer.clone();
    h.app.services.pick_files = Some(Box::new(move || a.lock().unwrap().clone()));
    click(&mut h, "button:pluginInstallFile");
    assert!(has_widget(&mut h, "button:pluginInstall"), "the approval opened for the picked module");
    assert!(painted(&h, "Picked 1.0.0"), "{:?}", h.painted_text());
    click(&mut h, "button:pluginInstallCancel");
    assert!(listed(&mut h, "test.mgr.picked").is_none());

    // the picker cancelled: no approval, no message (nothing new on screen)
    answer.lock().unwrap().clear();
    let before: std::collections::HashSet<String> = h.painted_text().into_iter().collect();
    click(&mut h, "button:pluginInstallFile");
    assert!(!has_widget(&mut h, "button:pluginInstall"), "nothing to approve");
    let new: Vec<String> = h.painted_text().into_iter().filter(|t| !before.contains(t)).collect();
    assert!(new.is_empty(), "no message: {new:?}");

    // the command without a path asks the picker too; with one, it installs from it
    *answer.lock().unwrap() = vec![path.clone()];
    ui_cmd(&mut h, "plugins.install", json!({}));
    assert!(has_widget(&mut h, "button:pluginInstall"));
    click(&mut h, "button:pluginInstallCancel");

    // a dropped module: the overlay says it installs, the drop opens the approval, nothing imports
    let photos = h.app.session.catalog.len();
    ask(&mut h, "ui.dropFiles", json!({"paths": [path], "hover": true}));
    assert!(painted(&h, "Drop to install the plug-in"), "{:?}", h.painted_text());
    ask(&mut h, "ui.dropFiles", json!({"paths": [path]}));
    assert!(has_widget(&mut h, "button:pluginInstall"), "the approval opened for the dropped module");
    click(&mut h, "button:pluginInstall");
    assert!(listed(&mut h, "test.mgr.picked").is_some(), "installed after approval");
    assert_eq!(h.app.session.catalog.len(), photos, "nothing imported");

    // one with the window closed, or two: not an install
    let other = module_file(&dir, "other", &json!({"id": "test.mgr.other", "name": "Other", "version": "1.0.0"}));
    ui_cmd(&mut h, "plugins.manager", json!({"open": false}));
    ask(&mut h, "ui.dropFiles", json!({"paths": [other]}));
    assert!(!has_widget(&mut h, "button:pluginInstall"), "the window was closed");
    ui_cmd(&mut h, "plugins.manager", json!({"open": true}));
    ask(&mut h, "ui.dropFiles", json!({"paths": [path, other]}));
    assert!(!has_widget(&mut h, "button:pluginInstall"), "two files are not a plug-in install");
    assert!(listed(&mut h, "test.mgr.other").is_none());

    exec(&mut h, "plugin.uninstall", json!({"id": "test.mgr.picked"}));
    let _ = std::fs::remove_dir_all(&dir);
}

/// AC-3 to AC-8: the list in two sections with a page per entry; a plug-in's page with its tabs,
/// a filter's with what it is and no Enabled box, a failed module's with why and Remove.
#[test]
fn the_list_has_sections_and_each_entry_its_page() {
    let _turn = turn();
    let dir = folder("pages");
    let mut h = app();
    // nothing installed: the list says so, and no page
    let before = lcc_engine::cmd::plugins::manager().replace(lcc_plugins::Manager::in_memory());
    ui_cmd(&mut h, "plugins.manager", json!({"open": true}));
    assert!(painted(&h, "No plug-ins installed."), "{:?}", h.painted_text());
    assert!(!has_widget(&mut h, "button:pluginPage-about"), "no page");

    let full = json!({"id": "test.mgr.full", "name": "Full", "version": "2.1.0", "author": "Someone",
        "description": "Does everything.",
        "permissions": {"catalog": true, "network": ["example.com"]},
        "settings": [{"key": "name", "label": "Name", "type": "text", "default": "first"}],
        "commands": [{"id": "go", "label": "Go Now"}]});
    let plain = json!({"id": "test.mgr.plain", "name": "Plain", "version": "0.3.0"});
    exec(&mut h, "plugin.install", json!({"path": module_file(&dir, "full", &full)}));
    exec(&mut h, "plugin.install", json!({"path": module_file(&dir, "plain", &plain)}));
    exec(&mut h, "plugin.enable", json!({"id": "test.mgr.plain", "enabled": false}));
    let filter = json!({"id": "test.mgr.filter", "name": "Negative", "version": "0.11.0", "author": "nwh", "kind": "filter",
        "stage": "source", "analyzeArea": "crop", "baseTone": "neutral",
        "params": {"amount": {"type": "number", "min": 0, "max": 1, "default": 1, "label": "Amount"}}});
    let filter_bytes = filter_module(&filter, true);
    lcc_engine::filter_plugins::install_in_memory(&filter_bytes).unwrap();
    // a filter module and a plug-in that didn't load
    let broken_filter = dir.join("test.mgr.badfilter.wasm");
    // a filter module whose manifest is a string, not an object: the filter registry refuses it
    std::fs::write(&broken_filter, filter_module(&json!("not a manifest"), false)).unwrap();
    let failed = |path: &Path, id: &str, missing: Vec<String>| lcc_plugins::Failed {
        file: path.file_name().unwrap().to_string_lossy().into_owned(),
        error: format!("{id} needs a newer app"),
        missing,
        id: id.into(),
        path: Some(path.to_path_buf()),
    };
    {
        let mut g = lcc_engine::cmd::plugins::manager();
        let m = g.as_mut().unwrap();
        m.failed.push(failed(&broken_filter, "test.mgr.badfilter", vec![]));
        m.failed.push(failed(&dir.join("test.mgr.badplugin.wasm"), "test.mgr.badplugin", vec!["test-missing-feature".into()]));
    }
    h.view.ctx.enable_accesskit();
    for _ in 0..4 {
        h.step();
    }

    // AC-3: two sections, plug-ins above filters, each entry in its kind's
    assert!(painted(&h, "PLUG-INS") && painted(&h, "PIXEL FILTERS"), "{:?}", h.painted_text());
    let plugins = ["test.mgr.full", "test.mgr.plain", "test.mgr.badplugin"].map(|id| top(&mut h, &format!("pluginEntry:{id}")));
    let filters = ["test.mgr.filter", "test.mgr.badfilter"].map(|id| top(&mut h, &format!("pluginEntry:{id}")));
    let last_plugin = plugins.iter().copied().fold(f64::MIN, f64::max);
    assert!(filters.iter().all(|f| *f > last_plugin), "filters below every plug-in: {plugins:?} {filters:?}");
    assert!(painted(&h, "0.11.0") && painted(&h, "2.1.0"), "versions shown");
    // each entry's mark, as a screen reader names it
    assert_eq!(access_name(&h, "Full ").as_deref(), Some("Full 2.1.0 (enabled)"));
    assert_eq!(access_name(&h, "Plain ").as_deref(), Some("Plain 0.3.0 (disabled)"));
    for id in ["test.mgr.badplugin", "test.mgr.badfilter"] {
        let name = access_name(&h, id).unwrap_or_default();
        assert!(name.ends_with("(didn't load)"), "{id}: {name:?}");
    }
    assert!(painted(&h, "by Someone · test.mgr.full"), "the first entry's page shows: {:?}", h.painted_text());

    // AC-4, AC-5: a plug-in's page opens on its settings; Save stores them and the page stays
    click(&mut h, "pluginEntry:test.mgr.full");
    for tab in ["settings", "permissions", "about", "log"] {
        assert!(has_widget(&mut h, &format!("button:pluginPage-{tab}")), "the {tab} tab");
    }
    assert!(painted(&h, "by Someone · test.mgr.full"), "{:?}", h.painted_text());
    assert!(painted(&h, "first"), "the setting's value");
    click(&mut h, "pluginText:settings:test.mgr.full:name");
    ask(&mut h, "ui.key", json!({"key": "a", "cmd": true}));
    ask(&mut h, "ui.text", json!({"text": "second"}));
    // clicking the entry on screen keeps what was typed
    click(&mut h, "pluginEntry:test.mgr.full");
    assert!(painted(&h, "second"), "the typed value stays: {:?}", h.painted_text());
    click(&mut h, "button:pluginSettingsSave");
    assert_eq!(exec(&mut h, "plugin.settings", json!({"id": "test.mgr.full"}))["values"], json!({"name": "second"}));
    assert!(has_widget(&mut h, "button:pluginSettingsSave"), "the page stays after saving");
    // a value the host refuses: the tab keeps it and says why
    click(&mut h, "pluginText:settings:test.mgr.full:name");
    ask(&mut h, "ui.key", json!({"key": "a", "cmd": true}));
    ask(&mut h, "ui.text", json!({"text": "y".repeat(lcc_plugins::manifest::MAX_TEXT_BYTES + 1)}));
    click(&mut h, "button:pluginSettingsSave");
    assert!(
        painted(&h, "`name` must be text"),
        "says why: {:?}",
        h.painted_text().iter().map(|t| t.chars().take(80).collect::<String>()).collect::<Vec<_>>()
    );
    assert!(h.painted_text().iter().any(|t| t.starts_with("yyyy")), "the typed value stays");
    assert_eq!(exec(&mut h, "plugin.settings", json!({"id": "test.mgr.full"}))["values"], json!({"name": "second"}), "nothing saved");
    click(&mut h, "button:pluginSettingsRevert");
    assert!(painted(&h, "second"), "Revert shows what is saved");
    // plugins.page opens the window on that plug-in's Settings tab
    ui_cmd(&mut h, "plugins.manager", json!({"open": false}));
    let r = ui_cmd(&mut h, "plugins.page", json!({"id": "test.mgr.full"}));
    assert_eq!(r["tab"], "settings", "{r}");
    assert!(has_widget(&mut h, "button:pluginSettingsSave"));

    // the Permissions tab grants and revokes
    click(&mut h, "button:pluginPage-permissions");
    assert!(painted(&h, "Read the catalog") && painted(&h, "example.com"), "{:?}", h.painted_text());
    click(&mut h, "pluginPermission:test.mgr.full:0");
    assert_eq!(listed(&mut h, "test.mgr.full").unwrap()["granted"]["catalog"], true, "ticking grants it");
    click(&mut h, "pluginPermission:test.mgr.full:0");
    assert_eq!(listed(&mut h, "test.mgr.full").unwrap()["granted"]["catalog"], false, "unticking revokes it");
    // About: what it is, its commands, its size; Log
    click(&mut h, "button:pluginPage-about");
    assert!(painted(&h, "Does everything.") && painted(&h, "Go Now") && painted(&h, "KB"), "{:?}", h.painted_text());
    click(&mut h, "button:pluginPage-log");
    assert!(painted(&h, "installed version 2.1.0"), "what it logged: {:?}", h.painted_text());
    // Enabled turns it off and on
    click(&mut h, "pluginEnabled:test.mgr.full");
    assert_eq!(listed(&mut h, "test.mgr.full").unwrap()["enabled"], false);
    click(&mut h, "pluginEnabled:test.mgr.full");
    assert_eq!(listed(&mut h, "test.mgr.full").unwrap()["enabled"], true);

    // a plug-in without settings: no Settings tab, its page opens on About
    click(&mut h, "pluginEntry:test.mgr.plain");
    assert!(!has_widget(&mut h, "button:pluginPage-settings"), "no settings, no tab");
    assert!(has_widget(&mut h, "button:pluginPage-about"));
    assert!(fails(&mut h, "plugins.page", json!({"id": "test.mgr.plain", "tab": "settings"})).contains("no settings"));
    assert!(fails(&mut h, "plugins.page", json!({"id": "test.mgr.plain", "tab": "nowhere"})).contains("nowhere"));
    // Remove takes it out of the list
    click(&mut h, "button:pluginRemove:test.mgr.plain");
    assert!(listed(&mut h, "test.mgr.plain").is_none());
    assert!(!has_widget(&mut h, "pluginEntry:test.mgr.plain"));

    // AC-6: a filter's page: what it is, Remove, and no Enabled box or tabs
    click(&mut h, "pluginEntry:test.mgr.filter");
    for text in [
        "by nwh · test.mgr.filter",
        "Stage",
        "Source: on the raw, before the develop adjustments",
        "Measures",
        "Yes: the cropped photo",
        "Base tone",
        "Neutral (its own rendering)",
        "Parameters",
        "Amount",
        "KB · sha256",
    ] {
        assert!(painted(&h, text), "{text}: {:?}", h.painted_text());
    }
    assert!(!has_widget(&mut h, "pluginEnabled:test.mgr.filter"), "no Enabled box");
    for tab in ["settings", "permissions", "about", "log"] {
        assert!(!has_widget(&mut h, &format!("button:pluginPage-{tab}")), "no {tab} tab");
    }
    assert!(fails(&mut h, "plugins.page", json!({"id": "test.mgr.filter", "tab": "log"})).contains("no tabs"));
    // AC-7 shows as a real size, not 0 KB
    let kb = filter_bytes.len().div_ceil(1024);
    assert!(painted(&h, &format!("{kb} KB · sha256")), "{:?}", h.painted_text());

    // AC-8: a module that didn't load: why, what it needs, and Remove
    click(&mut h, "pluginEntry:test.mgr.badplugin");
    assert!(painted(&h, "It didn't load.") && painted(&h, "test.mgr.badplugin needs a newer app") && painted(&h, "test-missing-feature"));
    click(&mut h, "button:pluginRemoveFailed:test.mgr.badplugin");
    assert!(!has_widget(&mut h, "pluginEntry:test.mgr.badplugin"), "removed");
    click(&mut h, "pluginEntry:test.mgr.badfilter");
    let failed = exec(&mut h, "plugin.list", json!({}))["failed"].clone();
    let why = failed.as_array().unwrap().iter().find(|f| f["id"] == "test.mgr.badfilter").unwrap()["error"].as_str().unwrap().to_string();
    assert!(!why.contains("needs a newer app"), "the filter registry's reason, not the plug-in manager's: {why}");
    assert!(painted(&h, "It didn't load.") && painted(&h, &why), "{why}: {:?}", h.painted_text());
    click(&mut h, "button:pluginRemoveFailed:test.mgr.badfilter");
    assert!(!has_widget(&mut h, "pluginEntry:test.mgr.badfilter"), "removed");

    click(&mut h, "pluginEntry:test.mgr.filter");
    click(&mut h, "button:pluginRemove:test.mgr.filter");
    assert!(listed(&mut h, "test.mgr.filter").is_none());
    exec(&mut h, "plugin.uninstall", json!({"id": "test.mgr.full"}));
    *lcc_engine::cmd::plugins::manager() = before;
    let _ = std::fs::remove_dir_all(&dir);
}

/// AC-10: the footer says what the window installs; *Show Folder* opens the plug-ins folder, and
/// isn't there when plug-ins live in memory.
#[test]
fn the_footer_names_what_installs_and_shows_the_folder() {
    let _turn = turn();
    let mut h = app();
    let desktop = std::sync::Arc::new(lcc_engine::desktop::Recorder::default());
    h.app.session.desktop = desktop.clone();
    ui_cmd(&mut h, "plugins.manager", json!({"open": true}));
    assert!(painted(&h, "plug-ins and PhotoCraft pixel filters (.wasm)."), "{:?}", h.painted_text());
    assert!(!painted(&h, "{app}"), "the app's name filled in");
    assert!(!has_widget(&mut h, "button:pluginShowFolder"), "in memory: no folder to show");
    assert!(fails(&mut h, "plugins.showFolder", json!({})).contains("no folder"));
    assert!(desktop.requests().is_empty());

    // a manager with a folder
    let dir = folder("footer");
    let before = lcc_engine::cmd::plugins::manager().replace(lcc_plugins::Manager::open(&dir).unwrap());
    for _ in 0..4 {
        h.step();
    }
    assert!(painted(&h, &dir.display().to_string()), "the folder is named: {:?}", h.painted_text());
    click(&mut h, "button:pluginShowFolder");
    let opened: Vec<_> = desktop.requests();
    *lcc_engine::cmd::plugins::manager() = before;
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        matches!(opened.as_slice(), [lcc_engine::desktop::Request::Launch(line)] if line.last() == Some(&dir.display().to_string())),
        "the file manager opened the folder: {opened:?}"
    );
}

/// AC-11: ⌘⌥⇧, (Ctrl+Alt+Shift+, on Windows and Linux) opens the window in both keymap sets, and
/// it can be bound to another key like any shortcut.
#[test]
fn cmd_alt_shift_comma_opens_the_window_and_can_be_rebound() {
    let _turn = turn();
    let mut h = app();
    let press = |h: &mut Headless, key: &str| {
        ask(h, "ui.key", json!({"key": key, "cmd": true, "alt": true, "shift": true}));
    };
    let open = |h: &mut Headless| has_widget(h, "button:pluginInstallFile");
    for set in ["classic", "alternative"] {
        h.app.run("app.keymapSet", json!({"set": set})).unwrap();
        assert!(!open(&mut h));
        press(&mut h, ",");
        assert!(open(&mut h), "the chord opens the window ({set} set)");
        ui_cmd(&mut h, "plugins.manager", json!({"open": false}));
    }
    h.app.run("app.keymapSet", json!({"set": "classic"})).unwrap();
    h.app.run("app.setShortcut", json!({"id": "plugins.manager", "shortcut": "Cmd+Alt+Shift+K"})).unwrap();
    press(&mut h, ",");
    assert!(!open(&mut h), "the old key no longer opens it");
    press(&mut h, "K");
    assert!(open(&mut h), "the new key does");
    h.app.run("app.resetShortcuts", json!({})).unwrap();
    ui_cmd(&mut h, "plugins.manager", json!({"open": false}));
}

/// The Plug-in Manager window's rectangle on screen.
fn window(h: &Headless) -> egui::Rect {
    h.view.ctx.memory(|m| m.area_rect(egui::Id::new("plugin-manager"))).expect("the Plug-in Manager is on screen")
}

/// The width the window opens at (the list and a page), and the narrowest it gets.
const DEFAULT_W: f32 = 250.0 + 560.0;
const MIN_W: f32 = 250.0 + 360.0;
/// The window's frame and margins around its content.
const FRAME: f32 = 60.0;

/// The window opens at its default width whatever its page shows (a filter with many long
/// parameters, a long description, a long folder), and its edge drags it narrower as well as wider,
/// down to a width where a page is still usable; *Show Folder* stays in the window.
#[test]
fn the_window_keeps_its_width_and_drags_narrower() {
    let _turn = turn();
    let mut h = app();
    let mut params = serde_json::Map::new();
    for i in 0..30 {
        params.insert(
            format!("p{i:02}"),
            json!({"type": "number", "min": 0, "max": 1, "default": 0, "label": format!("A rather long parameter label {i:02}")}),
        );
    }
    let filter = json!({"id": "test.mgr.wide", "name": "Wide Negative", "version": "0.10.0", "kind": "filter", "stage": "source",
        "description": "Turns scanned colour negatives into positives. ".repeat(12), "params": params});
    lcc_engine::filter_plugins::install_in_memory(&filter_module(&filter, false)).unwrap();
    // a folder whose path is longer than the window
    let top_dir = folder("long");
    let dir = (0..6).fold(top_dir.clone(), |d, i| d.join(format!("a-rather-long-folder-name-{i}")));
    std::fs::create_dir_all(&dir).unwrap();
    let before = lcc_engine::cmd::plugins::manager().replace(lcc_plugins::Manager::open(&dir).unwrap());
    ui_cmd(&mut h, "plugins.page", json!({"id": "test.mgr.wide"}));
    for _ in 0..4 {
        h.step();
    }
    assert!(painted(&h, "A rather long parameter label 29"), "the filter's page shows: {:?}", h.painted_text());
    let opened = window(&h);
    assert!(opened.width() <= DEFAULT_W + FRAME, "opens at its default width, not its widest line: {opened:?}");
    let folder_button = widget(&mut h, "button:pluginShowFolder").expect("Show Folder on screen");
    let right = folder_button[0].as_f64().unwrap() + folder_button[2].as_f64().unwrap();
    assert!(right <= f64::from(opened.right()), "Show Folder stays in the window: {folder_button} {opened:?}");

    // drawing more frames doesn't widen it
    for _ in 0..10 {
        h.step();
    }
    assert!((window(&h).width() - opened.width()).abs() < 1.0, "the width holds: {opened:?} → {:?}", window(&h));

    // the right edge drags it narrower, and no narrower than a usable page
    let drag = |h: &mut Headless, by: f32| {
        let w = window(h);
        let (x, y) = (w.right() - 1.0, w.center().y);
        ask(h, "ui.drag", json!({"x": x, "y": y, "toX": x - by, "toY": y, "steps": 8}));
        for _ in 0..4 {
            h.step();
        }
        window(h)
    };
    let narrower = drag(&mut h, 150.0);
    assert!(narrower.width() < opened.width() - 100.0, "dragged narrower: {opened:?} → {narrower:?}");
    assert!(painted(&h, "A rather long parameter label 29"), "the parameters wrap to the new width");
    let narrowest = drag(&mut h, 600.0);
    assert!(narrowest.width() >= MIN_W, "stops where a page is still usable: {narrowest:?}");
    assert!(narrowest.width() <= MIN_W + FRAME, "but gets there: {narrowest:?}");
    let back = drag(&mut h, -300.0);
    assert!(back.width() > narrowest.width() + 200.0, "and wider again: {narrowest:?} → {back:?}");

    exec(&mut h, "plugin.uninstall", json!({"id": "test.mgr.wide"}));
    ui_cmd(&mut h, "plugins.manager", json!({"open": false}));
    *lcc_engine::cmd::plugins::manager() = before;
    let _ = std::fs::remove_dir_all(&top_dir);
}

/// While an install waits for approval, another (`plugins.install`, a drop, *Install from File…*)
/// leaves it as it is and says to finish it first.
#[test]
fn one_install_waits_for_the_other() {
    let _turn = turn();
    let dir = folder("one-install");
    let first = module_file(&dir, "first", &json!({"id": "test.mgr.first", "name": "First", "version": "1.0.0"}));
    let second = module_file(&dir, "second", &json!({"id": "test.mgr.second", "name": "Second", "version": "1.0.0"}));
    let mut h = app();
    ui_cmd(&mut h, "plugins.manager", json!({"open": true}));
    ui_cmd(&mut h, "plugins.install", json!({"path": first}));
    assert!(has_widget(&mut h, "button:pluginInstall") && painted(&h, "First 1.0.0"));

    let e = fails(&mut h, "plugins.install", json!({"path": second}));
    assert!(e.contains("Finish or cancel installing First first"), "{e}");
    ask(&mut h, "ui.dropFiles", json!({"paths": [second]}));
    assert!(painted(&h, "Finish or cancel installing First first"), "the drop says why: {:?}", h.painted_text());
    let picked = std::sync::Arc::new(Mutex::new(false));
    let p = picked.clone();
    let path = second.clone();
    h.app.services.pick_files = Some(Box::new(move || {
        *p.lock().unwrap() = true;
        vec![path.clone()]
    }));
    click(&mut h, "button:pluginInstallFile");
    assert!(!*picked.lock().unwrap(), "no file picker while an install waits");
    assert!(painted(&h, "First 1.0.0") && !painted(&h, "Second 1.0.0"), "the first install still waits: {:?}", h.painted_text());

    // approved, the next one goes ahead
    click(&mut h, "button:pluginInstall");
    assert!(listed(&mut h, "test.mgr.first").is_some());
    ui_cmd(&mut h, "plugins.install", json!({"path": second}));
    assert!(painted(&h, "Second 1.0.0"), "{:?}", h.painted_text());
    click(&mut h, "button:pluginInstallCancel");

    exec(&mut h, "plugin.uninstall", json!({"id": "test.mgr.first"}));
    ui_cmd(&mut h, "plugins.manager", json!({"open": false}));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Installing exactly what is installed (the same id, version and code) asks nothing: the entry
/// shows, with a note. Other code under the same version (a repair) and a newer version ask.
#[test]
fn installing_what_is_installed_asks_nothing() {
    let _turn = turn();
    let dir = folder("same");
    let manifest = json!({"id": "test.mgr.same", "name": "Same", "version": "1.0.0"});
    let installed = module_file(&dir, "same", &manifest);
    let repair = module_file(&dir, "repair", &json!({"id": "test.mgr.same", "name": "Same", "version": "1.0.0", "description": "rebuilt"}));
    let newer = module_file(&dir, "newer", &json!({"id": "test.mgr.same", "name": "Same", "version": "1.1.0"}));
    let mut h = app();
    exec(&mut h, "plugin.install", json!({"path": installed}));
    ui_cmd(&mut h, "plugins.manager", json!({"open": true}));

    let r = ui_cmd(&mut h, "plugins.install", json!({"path": installed}));
    assert_eq!(r["installed"], "test.mgr.same", "{r}");
    assert!(!has_widget(&mut h, "button:pluginInstall"), "no approval");
    assert!(painted(&h, "Same 1.0.0 is already installed"), "{:?}", h.painted_text());
    assert!(painted(&h, "test.mgr.same"), "its page shows");
    // a drop too
    ask(&mut h, "ui.dropFiles", json!({"paths": [installed]}));
    assert!(!has_widget(&mut h, "button:pluginInstall"), "no approval for a drop");

    for (what, path) in [("a repair", &repair), ("an update", &newer)] {
        ui_cmd(&mut h, "plugins.install", json!({"path": path}));
        assert!(has_widget(&mut h, "button:pluginInstall"), "{what} asks");
        click(&mut h, "button:pluginInstallCancel");
    }

    // a pixel filter the same way
    let filter = json!({"id": "test.mgr.samefilter", "name": "Same Filter", "version": "0.2.0", "kind": "filter"});
    let bytes = filter_module(&filter, false);
    lcc_engine::filter_plugins::install_in_memory(&bytes).unwrap();
    let file = dir.join("samefilter.wasm");
    std::fs::write(&file, &bytes).unwrap();
    let r = ui_cmd(&mut h, "plugins.install", json!({"path": file}));
    assert_eq!(r["installed"], "test.mgr.samefilter", "{r}");
    assert!(!has_widget(&mut h, "button:pluginInstall"), "no approval for the installed filter");

    exec(&mut h, "plugin.uninstall", json!({"id": "test.mgr.samefilter"}));
    exec(&mut h, "plugin.uninstall", json!({"id": "test.mgr.same"}));
    ui_cmd(&mut h, "plugins.manager", json!({"open": false}));
    let _ = std::fs::remove_dir_all(&dir);
}

/// An open Settings tab follows its plug-in: an update's new field shows, a value set elsewhere
/// shows, what the user typed and hasn't saved stays, and Save writes nothing the user didn't see.
#[test]
fn an_open_settings_tab_follows_its_plugin() {
    let _turn = turn();
    let dir = folder("follow");
    let field = |key: &str, default: &str| json!({"key": key, "label": key, "type": "text", "default": default});
    let v1 = module_file(&dir, "v1", &json!({"id": "test.mgr.follow", "name": "Follow", "version": "1.0.0", "settings": [field("a", "a0")]}));
    let v2 = module_file(
        &dir,
        "v2",
        &json!({"id": "test.mgr.follow", "name": "Follow", "version": "1.1.0",
        "settings": [field("a", "a0"), field("b", "b0")]}),
    );
    let mut h = app();
    exec(&mut h, "plugin.install", json!({"path": v1}));
    ui_cmd(&mut h, "plugins.page", json!({"id": "test.mgr.follow"}));
    // typed, not saved
    click(&mut h, "pluginText:settings:test.mgr.follow:a");
    ask(&mut h, "ui.key", json!({"key": "a", "cmd": true}));
    ask(&mut h, "ui.text", json!({"text": "typed"}));

    // the update adds a field: it shows on the open tab, and the typed value stays
    exec(&mut h, "plugin.install", json!({"path": v2}));
    for _ in 0..4 {
        h.step();
    }
    assert!(has_widget(&mut h, "pluginText:settings:test.mgr.follow:b"), "the new field shows");
    assert!(h.painted_text().iter().any(|t| t == "typed"), "the typed value stays: {:?}", h.painted_text());

    // a value set elsewhere (the plug-in, an agent) shows
    exec(&mut h, "plugin.setSettings", json!({"id": "test.mgr.follow", "values": {"b": "newer"}}));
    for _ in 0..4 {
        h.step();
    }
    assert!(h.painted_text().iter().any(|t| t == "newer"), "the newer value shows: {:?}", h.painted_text());
    assert!(h.painted_text().iter().any(|t| t == "typed"), "the typed value still stays");

    click(&mut h, "button:pluginSettingsSave");
    let saved = exec(&mut h, "plugin.settings", json!({"id": "test.mgr.follow"}))["values"].clone();
    assert_eq!(saved, json!({"a": "typed", "b": "newer"}), "Save keeps the newer value and writes the typed one");

    exec(&mut h, "plugin.uninstall", json!({"id": "test.mgr.follow"}));
    ui_cmd(&mut h, "plugins.manager", json!({"open": false}));
    let _ = std::fs::remove_dir_all(&dir);
}
