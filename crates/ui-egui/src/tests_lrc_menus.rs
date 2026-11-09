//! The menu bar follows Lightroom Classic's (`crate::lrc_menus`): the menus change with the module,
//! the job file is the Catalog, "Coming soon" items are greyed and never run, and Classic's keys
//! reach the same commands.

use serde_json::{Value, json};
use std::time::Duration;

use crate::headless::Headless;
use crate::lrc_menus::{COMING_SOON, Slot, all_layouts, parse};
use crate::menubar::{MenuNode, menu_bar, run_item};
use crate::module::ModuleId;
use crate::{LccApp, Services};

const T: Duration = Duration::from_secs(5);
const SETTLE: Duration = Duration::from_secs(10);

fn app() -> LccApp {
    let mut session = lcc_engine::Session::with_demo();
    session.selection = lcc_engine::Selection::single(session.visible_cloned()[0]);
    LccApp::new(session, Services { png: None, ..Default::default() })
}

fn titles(app: &LccApp) -> Vec<String> {
    menu_bar(app).into_iter().map(|(t, _)| t).collect()
}

/// Every item of the bar, depth first, with its submenu path.
fn items(bar: &[(String, Vec<MenuNode>)]) -> Vec<(String, MenuNode)> {
    fn walk(path: &str, nodes: &[MenuNode], out: &mut Vec<(String, MenuNode)>) {
        for n in nodes {
            match n {
                MenuNode::Submenu { label, children } => walk(&format!("{path} > {label}"), children, out),
                MenuNode::Item { .. } => out.push((path.to_string(), n.clone())),
                MenuNode::Separator => {}
            }
        }
    }
    let mut out = Vec::new();
    for (title, nodes) in bar {
        walk(title, nodes, &mut out);
    }
    out
}

fn find<'a>(all: &'a [(String, MenuNode)], want: &str) -> Option<&'a (String, MenuNode)> {
    all.iter().find(|(_, n)| matches!(n, MenuNode::Item { id, .. } if id == want))
}

fn label_of(n: &MenuNode) -> &str {
    match n {
        MenuNode::Item { label, .. } => label,
        _ => "",
    }
}

#[test]
fn the_menu_bar_changes_with_the_module() {
    let mut a = app();
    let shared = |t: &[String]| {
        t.first().map(String::as_str) == Some("File")
            && t.get(1).map(String::as_str) == Some("Edit")
            && t.ends_with(&["Window".to_string(), "Help".to_string()])
    };
    for (module, middle) in [
        (ModuleId::Library, &["Library", "Photo", "Metadata", "View"][..]),
        (ModuleId::Develop, &["Develop", "Photo", "Settings", "Tools", "View"]),
        (ModuleId::Map, &["Map", "Photo", "View"]),
        (ModuleId::Book, &["Book", "View"]),
        (ModuleId::Slideshow, &["Slideshow", "Play", "View"]),
        (ModuleId::Print, &["Print", "View"]),
        (ModuleId::Web, &["Web", "View"]),
    ] {
        a.ui.module = module;
        let t = titles(&a);
        assert!(shared(&t), "{module:?}: {t:?}");
        assert_eq!(&t[2..t.len() - 2], middle, "{module:?}");
        // every menu has rows
        for (title, nodes) in menu_bar(&a) {
            assert!(nodes.iter().any(|n| !matches!(n, MenuNode::Separator)), "{module:?} {title} is empty");
        }
    }
    // Develop's own menus hold Develop's commands; the Library menu is gone there
    a.ui.module = ModuleId::Develop;
    let develop = items(&menu_bar(&a));
    let (path, snapshot) = find(&develop, "version.create").expect("New Snapshot");
    assert_eq!((path.as_str(), label_of(snapshot)), ("Develop", "New Snapshot"));
    assert!(find(&develop, "crop.reset").is_some_and(|(p, _)| p == "Settings"), "Reset Crop in Settings");
    assert!(find(&develop, "dialog.newAlbum").is_none(), "Library's New Collection is not in Develop's menus");
    // the menu shows the module's key: ⌘N is New Snapshot in Develop, New Collection in Library
    assert!(matches!(snapshot, MenuNode::Item { shortcut: Some(s), .. } if s == "Cmd+N"), "{snapshot:?}");
    a.ui.module = ModuleId::Library;
    let library = items(&menu_bar(&a));
    assert!(matches!(&find(&library, "dialog.newAlbum").expect("New Collection").1, MenuNode::Item { shortcut: Some(s), .. } if s == "Cmd+N"));
}

#[test]
fn the_job_file_is_the_catalog_at_the_top_of_file() {
    let a = app();
    let bar = menu_bar(&a);
    let file = &bar.iter().find(|(t, _)| t == "File").expect("File").1;
    let first: Vec<&str> = file
        .iter()
        .take(4)
        .map(|n| match n {
            MenuNode::Item { label, .. } => label.as_str(),
            MenuNode::Submenu { label, .. } => label.as_str(),
            MenuNode::Separator => "---",
        })
        .collect();
    assert_eq!(first, ["New Catalog…", "Open Catalog…", "Open Recent", "Optimize Catalog"]);
    // no menu or row calls the job file a Library any more
    let all = items(&bar);
    for (path, n) in &all {
        assert!(!path.contains("Libraries"), "{path}");
        let l = label_of(n);
        assert!(!(l.contains("Library") && (l.contains("New ") || l.contains("Open ") || l.contains("Settings"))), "{path}: {l}");
    }
    assert!(find(&all, "dialog.catalogSettings").is_some_and(|(p, n)| p == "Edit" && label_of(n) == "Catalog Settings…"));
}

#[test]
fn a_coming_soon_item_is_greyed_and_does_nothing() {
    let mut a = app();
    let all = items(&menu_bar(&a));
    let (_, n) = find(&all, "soon.renameCatalog").expect("Rename Catalog… in File");
    assert!(matches!(n, MenuNode::Item { enabled: false, .. }), "{n:?}");
    assert_eq!(label_of(n), "Rename Catalog…");
    assert!(!crate::menus::ui_enabled(&a, "soon.renameCatalog"));
    // every greyed item of every module, run from the menu or by id, changes nothing
    let before = (a.session.undo.len(), a.ui.module, a.session.selection.ids.clone(), a.ui.dialog.is_some());
    for c in COMING_SOON {
        let r = run_item(&mut a, c.id, Value::Null);
        assert!(r.is_err(), "{} ran: {r:?}", c.id);
        assert!(a.run(c.id, json!({})).is_err(), "{}", c.id);
    }
    assert_eq!(before, (a.session.undo.len(), a.ui.module, a.session.selection.ids.clone(), a.ui.dialog.is_some()));
    // and none can be given a key
    assert!(crate::shortcuts::bindable().iter().all(|b| !b.id.starts_with("soon.")));
}

#[test]
fn every_slot_names_a_command_and_every_soon_item_is_placed() {
    let known: Vec<&str> = crate::menus::ui_commands().map(|c| c.0).chain(lcc_engine::command_specs().iter().map(|c| c.id)).collect();
    let mut placed = Vec::new();
    for (module, title, sub, slots) in all_layouts() {
        for slot in slots {
            if let Slot::Item(id, params, _) = parse(slot) {
                assert!(known.contains(&id), "{module:?} {title} {sub:?}: unknown command {id}");
                if let Some(p) = params {
                    assert!(serde_json::from_str::<Value>(p).is_ok_and(|v| v.is_object()), "{id}: bad parameters {p}");
                }
                placed.push(id);
            }
        }
    }
    for c in COMING_SOON {
        assert!(placed.contains(&c.id), "{} is in no menu", c.id);
        assert!(matches!(c.size, 'S' | 'M' | 'L'), "{}", c.id);
        if let Some(h) = c.hint {
            assert!(crate::shortcuts::parse(h).is_some(), "{}: bad key {h}", c.id);
        }
    }
}

#[test]
fn slots_parse() {
    assert_eq!(parse("---"), Slot::Separator);
    assert_eq!(parse("@Stacking=Stack"), Slot::Submenu("Stacking", None, "Stack"));
    assert_eq!(parse("@Zoom=View>Zoom"), Slot::Submenu("Zoom", Some("View"), "Zoom"));
    assert_eq!(parse("@Sort"), Slot::Submenu("Sort", None, "Sort"));
    assert_eq!(parse("file.export"), Slot::Item("file.export", None, None));
    assert_eq!(parse("library.selectNone=Select None"), Slot::Item("library.selectNone", None, Some("Select None")));
    assert_eq!(
        parse(r#"develop.wb{"mode":"auto"}=Auto White Balance"#),
        Slot::Item("develop.wb", Some(r#"{"mode":"auto"}"#), Some("Auto White Balance"))
    );
    assert_eq!(parse(r#"x{"a":1}"#), Slot::Item("x", Some(r#"{"a":1}"#), None));
}

/// `docs/lrc-menu-gaps.md` is generated from the tables; rewrite it with
/// `cargo test -p lcc-ui-egui lrc_menu_gaps_doc -- --ignored`.
fn gaps_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/lrc-menu-gaps.md")
}

#[test]
fn the_gaps_list_is_up_to_date() {
    let on_disk = std::fs::read_to_string(gaps_path()).expect("docs/lrc-menu-gaps.md").replace("\r\n", "\n");
    assert!(
        on_disk == crate::lrc_menus::gaps_doc(),
        "docs/lrc-menu-gaps.md is stale: run `cargo test -p lcc-ui-egui lrc_menu_gaps_doc -- --ignored`"
    );
    for c in COMING_SOON {
        assert!(on_disk.contains(&format!("`{}`", c.id)), "{} missing from the list", c.id);
    }
}

#[test]
#[ignore = "writes docs/lrc-menu-gaps.md"]
fn lrc_menu_gaps_doc() {
    std::fs::write(gaps_path(), crate::lrc_menus::gaps_doc()).expect("write docs/lrc-menu-gaps.md");
}

// ---- Classic's keys

fn headless() -> Headless {
    let mut h = Headless::new(app(), [1280.0, 800.0], 1.0);
    h.settle(SETTLE);
    h
}

fn key(h: &mut Headless, key: &str, modifiers: Value) {
    let mut params = modifiers;
    params["key"] = json!(key);
    let r = h.request("ui.key", params, T);
    assert_eq!(r["ok"], true, "{key}: {r}");
    h.settle(SETTLE);
}

#[test]
fn classic_keys_reach_classic_commands() {
    let km = crate::shortcuts::Keymap::new();
    crate::shortcuts::set_active(crate::shortcuts::KeymapSet::Classic);
    let of = |id: &str| crate::shortcuts::shortcut_of(&km, id).map(str::to_string);
    assert_eq!(of("file.export").as_deref(), Some("Cmd+Shift+E"));
    assert_eq!(of("library.selectNone").as_deref(), Some("Cmd+D"));
    assert_eq!(of("photo.editInPhotoCraft").as_deref(), Some("Cmd+E"));
    assert_eq!(of("module.develop").as_deref(), Some("Cmd+Alt+2"));
    assert_eq!(of("develop.auto").as_deref(), Some("Cmd+U"));
    assert_eq!(of("develop.hdr").as_deref(), Some("Shift+H"));
    assert_eq!(of("view.slideshow").as_deref(), Some("Cmd+Enter"));
    if !cfg!(target_os = "macos") {
        assert_eq!(of("edit.redo").as_deref(), Some("Cmd+Y"));
    }
    // no two commands share a key in the Classic set (contextual pairs aside)
    let mut seen: Vec<((egui::Modifiers, egui::Key), &str)> = Vec::new();
    for b in crate::shortcuts::bindable() {
        let Some(k) = crate::shortcuts::binding(&km, b.id, b.default()).and_then(crate::shortcuts::parse) else { continue };
        if let Some((_, other)) = seen.iter().find(|(s, _)| *s == k)
            && !crate::shortcuts::CONTEXTUAL.iter().any(|(x, y)| (*x == b.id && y == other) || (*y == b.id && x == other))
        {
            panic!("{} and {other} share {k:?}", b.id);
        }
        seen.push((k, b.id));
    }

    // ⇧⌘E: Export (the Export module, or its dialog in the Modern layout)
    let mut h = headless();
    key(&mut h, "E", json!({"cmd": true, "shift": true}));
    assert!(h.app.ui.module == ModuleId::Export || h.app.ui.dialog.is_some(), "⇧⌘E opens Export");
    // ⌘D: Select None (the photo on screen stays the active one)
    let mut h = headless();
    let ids: Vec<u64> = h.app.session.visible_cloned().iter().take(3).map(|p| p.0).collect();
    h.app.run("library.select", json!({"ids": ids, "active": ids[0]})).unwrap();
    assert_eq!(h.app.session.selection.ids.len(), 3);
    key(&mut h, "D", json!({"cmd": true}));
    assert_eq!(h.app.session.selection.ids.len(), 1, "⌘D selects none but the active photo");
}
