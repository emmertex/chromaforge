//! The Classic keymap set (the default) beside the Alternative set (the keys declared on the
//! commands), keymap files, and the Classic-only steps of [`super::handle`]: module keys, Tab
//! presses held back from focus navigation. Kept out of the shared `shortcuts.rs` (U.5); a child
//! module of it, re-exported there.

use egui::{Key, Modifiers};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{Bindable, Keymap, assign, bindable, binding, matches, parse};
use crate::LccApp;

impl Bindable {
    /// The shortcut in the active keymap set: Classic's where [`CLASSIC`] names the command,
    /// else the declared one.
    pub fn default(&self) -> Option<&'static str> {
        default_in(active_set(), self)
    }
}

/// The keymap sets: Classic (the default) and the Alternative set (the keys declared on the
/// commands, which the app used before the Classic shell).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum KeymapSet {
    #[default]
    Classic,
    Alternative,
}

impl KeymapSet {
    pub fn parse(s: &str) -> Option<KeymapSet> {
        match s {
            "classic" | "Classic" => Some(KeymapSet::Classic),
            "alternative" | "Alternative" | "legacy" => Some(KeymapSet::Alternative),
            _ => None,
        }
    }
}

thread_local! {
    static ACTIVE: std::cell::Cell<KeymapSet> = const { std::cell::Cell::new(KeymapSet::Classic) };
}

/// The set the UI thread is using (follows [`crate::state::AppSettings::keymap_set`]).
pub fn active_set() -> KeymapSet {
    ACTIVE.with(|a| a.get())
}

pub fn set_active(set: KeymapSet) {
    ACTIVE.with(|a| a.set(set));
}

pub fn default_in(set: KeymapSet, b: &Bindable) -> Option<&'static str> {
    if set == KeymapSet::Classic
        && let Some((_, sc)) = CLASSIC.iter().find(|(id, _)| *id == b.id)
    {
        return *sc;
    }
    b.default
}

/// The Classic keymap: where it differs from the declared shortcuts (`None` = no key in Classic).
pub const CLASSIC: &[(&str, Option<&str>)] = &[
    // modules and views
    ("view.loupe", Some("E")),
    ("panel.edit", None),
    // Lightroom Classic: ⌘⌥2 (D too, `classic_extra_keys`)
    ("module.develop", Some("Cmd+Alt+2")),
    ("view.detail", None),
    ("view.compare", Some("C")),
    ("module.library", Some("Cmd+Alt+1")),
    ("module.map", Some("Cmd+Alt+3")),
    ("module.book", Some("Cmd+Alt+4")),
    ("module.slideshow", Some("Cmd+Alt+5")),
    ("module.print", Some("Cmd+Alt+6")),
    ("module.web", Some("Cmd+Alt+7")),
    ("module.export", Some("Cmd+Alt+8")),
    ("module.previous", Some("Cmd+Alt+Up")),
    // panels and screen
    ("panel.sides", Some("Tab")),
    ("panel.all", Some("Shift+Tab")),
    ("panel.top", Some("F5")),
    ("panel.bottom", Some("F6")),
    ("panel.left", Some("F7")),
    ("panel.right", Some("F8")),
    ("panel.toolbar", Some("T")),
    ("view.lightsOut", Some("L")),
    ("view.screenMode", Some("Shift+F")),
    ("view.screenModeNormal", Some("Cmd+Alt+F")),
    ("view.filterBar", None),
    // develop tools
    ("panel.crop", Some("R")),
    ("panel.remove", Some("Q")),
    ("panel.masking", Some("Shift+W")),
    ("tool.brush", Some("K")),
    ("tool.linear", Some("M")),
    ("tool.radial", Some("Shift+M")),
    ("tool.guidedUpright", Some("Shift+T")),
    ("tool.colorRange", Some("Shift+J")),
    ("tool.depthRange", Some("Shift+D")),
    ("tool.luminanceRange", Some("Shift+Q")),
    ("tool.pointColor", Some("Cmd+Alt+P")),
    ("panel.keywords", Some("Cmd+K")),
    // flags, collections
    // ` toggles the Pick flag as in Lightroom Classic; cycling all three states (the
    // Alternative set's `) has no key here, only its menu item
    ("photo.flagToggle", Some("`")),
    ("photo.cycleFlag", None),
    ("album.toggleTarget", Some("B")),
    ("library.showQuickCollection", Some("Cmd+B")),
    ("album.clearQuick", Some("Cmd+Shift+B")),
    ("album.saveQuick", Some("Cmd+Alt+B")),
    ("photo.copyMetadata", Some("Cmd+Alt+Shift+C")),
    ("photo.pasteMetadata", Some("Cmd+Alt+Shift+V")),
    // secondary window
    ("second.grid", Some("Shift+G")),
    ("second.loupe", Some("Shift+E")),
    ("second.compare", Some("Shift+C")),
    ("second.survey", Some("Shift+N")),
    ("second.slideshow", Some("Cmd+Alt+Shift+Enter")),
    ("second.locked", Some("Cmd+Shift+Enter")),
    // Lightroom Classic's menu keys where the declared ones differ (docs/lrc-menu-gaps.md; the
    // menus follow `crate::lrc_menus`)
    ("file.export", Some("Cmd+Shift+E")),
    ("photo.editInPhotoCraft", Some("Cmd+E")),
    ("dialog.newFolder", None),
    // Library ▸ New Folder…; New Collection Set… has no key in Classic
    ("dialog.newDiskFolder", Some("Cmd+Shift+N")),
    ("dialog.pageSetup", Some("Cmd+Shift+P")),
    ("album.setTarget", Some("Cmd+Alt+Shift+B")),
    ("library.deselectActive", Some("/")),
    ("library.deselectUnflagged", Some("Cmd+Alt+Shift+D")),
    ("photo.removeFromCatalog", Some("Alt+Delete")),
    ("library.selectNone", Some("Cmd+D")),
    ("edit.redo", Some(if cfg!(target_os = "macos") { "Cmd+Shift+Z" } else { "Cmd+Y" })),
    ("develop.auto", Some("Cmd+U")),
    ("develop.copy", None),
    ("develop.paste", Some("Cmd+Shift+V")),
    ("dialog.pasteSettings", None),
    ("dialog.createPreset", None),
    ("view.toolOverlay", None),
    ("develop.hdr", Some("Shift+H")),
    ("view.histogram", Some("Cmd+0")),
    ("view.zoomFit", None),
    ("view.slideshow", Some("Cmd+Enter")),
    ("view.leftPanel", None),
    ("view.gradingMode", None),
    ("stack.ungroup", Some("Cmd+Shift+G")),
    ("masks.updateAi", Some("Cmd+Alt+U")),
    ("develop.matchExposure", Some("Cmd+Alt+Shift+M")),
    ("library.deleteRejected", Some("Cmd+Delete")),
    ("view.chooseLayoutImage", Some("Cmd+Alt+Shift+O")),
    ("app.shortcuts", None),
];

/// The largest keymap file read (a keymap is a few kilobytes).
const MAX_KEYMAP_FILE: u64 = 1 << 20;

/// `app.keymapSet {set: classic|alternative}` (no set: report the active one).
pub fn choose_set(app: &mut LccApp, p: &Value) -> Result<Value, String> {
    if let Some(name) = p.get("set").and_then(Value::as_str) {
        let set = KeymapSet::parse(name).ok_or_else(|| format!("unknown keymap set: {name} (classic|alternative)"))?;
        app.ui.settings.keymap_set = set;
        set_active(set);
    }
    Ok(json!({"set": app.ui.settings.keymap_set}))
}

/// The keymap file: the set and the user's changes over it.
pub fn keymap_file(app: &LccApp) -> Value {
    json!({"format": "keymap", "version": 1, "set": app.ui.settings.keymap_set, "keymap": app.ui.settings.keymap})
}

/// `app.keymapExport {path}`.
pub fn export_file(app: &mut LccApp, path: &str) -> Result<Value, String> {
    app.session.check_write_target(path)?;
    let bytes = serde_json::to_vec_pretty(&keymap_file(app)).map_err(|e| e.to_string())?;
    std::fs::write(path, bytes).map_err(|e| format!("can't write {path}: {e}"))?;
    Ok(json!({"path": path, "count": app.ui.settings.keymap.len()}))
}

/// Apply a keymap file's contents: the set, then each entry through [`assign`] (so conflicts
/// move keys as in the editor). Everything is checked first; a file with any bad part is an
/// error naming it and changes nothing (no half-applied set or keymap).
pub fn import_value(app: &mut LccApp, v: &Value) -> Result<Value, String> {
    let o = v.as_object().ok_or("a keymap file is a JSON object")?;
    let set = match o.get("set") {
        None | Some(Value::Null) => None,
        Some(Value::String(name)) => Some(KeymapSet::parse(name).ok_or_else(|| format!("unknown keymap set: {name}"))?),
        Some(other) => return Err(format!("`set` must be a keymap set name, got {other}")),
    };
    let entries = o.get("keymap").and_then(Value::as_object).ok_or("missing `keymap` object")?;
    if entries.len() > 10_000 {
        return Err(format!("too many keymap entries ({})", entries.len()));
    }
    // Build against the file's set (assign resolves conflicts with that set's defaults), then
    // restore the active set unless everything checked out.
    let previous = active_set();
    if let Some(set) = set {
        set_active(set);
    }
    let mut keymap = Keymap::new();
    let mut bad = Vec::new();
    for (id, sc) in entries {
        let r = match sc {
            Value::String(s) if s.is_empty() => assign(&mut keymap, id, None),
            Value::String(s) => assign(&mut keymap, id, Some(s)),
            Value::Null => assign(&mut keymap, id, None),
            _ => Err("not a string".into()),
        };
        if let Err(e) = r {
            bad.push(format!("{id}: {e}"));
        }
    }
    if !bad.is_empty() {
        set_active(previous);
        return Err(format!("keymap not imported: {}", bad.join("; ")));
    }
    if let Some(set) = set {
        app.ui.settings.keymap_set = set;
    }
    app.ui.settings.keymap = keymap;
    Ok(json!({"set": app.ui.settings.keymap_set, "count": app.ui.settings.keymap.len()}))
}

/// `app.keymapImport {path}`.
pub fn import_file(app: &mut LccApp, path: &str) -> Result<Value, String> {
    let len = std::fs::metadata(path).map_err(|e| format!("can't read {path}: {e}"))?.len();
    if len > MAX_KEYMAP_FILE {
        return Err(format!("{path} is too large for a keymap ({len} bytes)"));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("can't read {path}: {e}"))?;
    let v: Value = serde_json::from_slice(&bytes).map_err(|e| format!("{path} is not a keymap file: {e}"))?;
    import_value(app, &v)
}

/// Whether `dialog` hosts the shortcut editor, so a row may stay recording: Help ▸ Keyboard
/// Shortcuts, or the Shortcuts tab of Settings (#155).
pub(super) fn records_shortcuts(dialog: Option<&crate::state::Dialog>) -> bool {
    use crate::state::Dialog;
    matches!(dialog, Some(Dialog::Shortcuts)) || matches!(dialog, Some(Dialog::Settings { tab }) if tab == "shortcuts")
}

/// Keys the active set gives a command: the fixed bindings (aliases, ratings) yield to them
/// (Classic's ⇧E is the secondary loupe, not export).
pub(super) fn taken() -> Vec<(Modifiers, Key)> {
    if active_set() == KeymapSet::Classic { CLASSIC.iter().filter_map(|(_, sc)| sc.and_then(parse)).collect() } else { Vec::new() }
}

/// The module's own keys (Classic set): they come first and take their key from the global keymap.
pub(super) fn module_keys(module: crate::module::ModuleId) -> &'static [crate::module::ModuleKey] {
    module_keys_of(module)
}

/// The keys `module` binds over the global keymap in the active set (none in the Alternative set);
/// the menus show them beside their items.
pub fn module_keys_of(module: crate::module::ModuleId) -> &'static [crate::module::ModuleKey] {
    if active_set() == KeymapSet::Classic { crate::module::get(module).keymap() } else { &[] }
}

/// Second keys the Classic set gives commands in every module, as Lightroom Classic does (`D` goes
/// to Develop besides ⌘⌥2; ⇧⌘L dims the lights). Like module keys they come before
/// the global keymap and yield to a key the user gave a command. None in the Alternative set.
pub fn classic_extra_keys() -> &'static [crate::module::ModuleKey] {
    const EXTRA: &[crate::module::ModuleKey] = &[("D", "module.develop", "{}"), ("Cmd+Shift+L", "view.lightsOut", r#"{"mode": "dim"}"#)];
    if active_set() == KeymapSet::Classic { EXTRA } else { &[] }
}

/// Whether a Tab pressed with `m` is the binding's (`bm`). On macOS Control is a key of its own
/// (Command is the shortcut key), so Ctrl+Tab (Settings' next section) is not a bare Tab.
fn same_tab_modifiers(bm: Modifiers, m: Modifiers) -> bool {
    bm.shift == m.shift && bm.alt == m.alt && bm.command == m.command && (!cfg!(target_os = "macos") || bm.ctrl == m.ctrl)
}

/// Whether `keymap` (in the active set) binds Tab with modifiers `m` to a command: only such a
/// Tab is held back from egui's focus navigation ([`deferred_tabs`]); an unbound one moves the focus.
pub(crate) fn tab_bound(keymap: &Keymap, m: Modifiers) -> bool {
    bindable().iter().any(|b| binding(keymap, b.id, b.default()).and_then(parse).is_some_and(|(bm, k)| k == Key::Tab && same_tab_modifiers(bm, m)))
}

/// Tab presses held back from egui's focus navigation fire the commands bound to them.
pub(super) fn deferred_tabs(deferred: &mut Vec<Modifiers>, keymap: &Keymap, fire: &mut Vec<String>) {
    for m in std::mem::take(deferred) {
        for b in bindable() {
            if let Some(sc) = binding(keymap, b.id, b.default())
                && let Some((bm, Key::Tab)) = parse(sc)
                && same_tab_modifiers(bm, m)
            {
                fire.push(b.id.to_string());
            }
        }
    }
}

/// Module keys pressed this frame: their keys are `consumed` (the global keymap skips them). A
/// key the user gave a command in `keymap` is theirs: the module key yields to it.
pub(super) fn module_key_presses(
    i: &egui::InputState,
    keys: &[crate::module::ModuleKey],
    keymap: &Keymap,
    consumed: &mut Vec<(Modifiers, Key)>,
    aliased: &mut Vec<(&str, Value)>,
) {
    for (sc, id, params) in keys {
        if let Some((m, k)) = parse(sc)
            && matches(i, m, k)
            && !keymap.values().any(|user| parse(user) == Some((m, k)))
        {
            consumed.push((m, k));
            aliased.push((id, serde_json::from_str(params).unwrap_or_default()));
        }
    }
}

/// Tab / ⇧Tab toggle panels: they must not also move keyboard focus into a field.
pub(super) fn keep_focus_on_panel_toggle(ctx: &egui::Context, fire: &[String]) {
    if fire.iter().any(|f| f == "panel.sides" || f == "panel.all") {
        ctx.memory_mut(|m| {
            if let Some(id) = m.focused() {
                m.surrender_focus(id);
            }
        });
    }
}
