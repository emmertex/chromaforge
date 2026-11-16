//! Contexts: what the app shows, as a dotted key the profiles map to pages.
//!
//! Portions © RAD Films - Roman Alurkoff (from TactileBridge), MIT: TactileBridge switched the
//! pad and dial layout with Capture One's active cursor tool and dialogs; here the app reports its
//! state directly.
//!
//! Keys, most specific first (a key falls back to its parents, then `default`):
//!
//! | Key | When |
//! |---|---|
//! | `dialog.<name>` | a dialog is open (`dialog.export`, `dialog.settings`, …) |
//! | `library` | the photo grid (Library) |
//! | `develop`, `develop.<section>` | Detail view with the Edit panel; the section last opened (`develop.light`, `develop.color`, `develop.detail`, `develop.effects`, `develop.optics`) |
//! | `crop` | Crop & Rotate |
//! | `remove` | Remove (heal / clone) |
//! | `redeye` | Red Eye |
//! | `masking` | the Masks panel (a mask selected or not: one page, test notes 2026-10-24 note 24) |
//! | `masking.mask.brush` | the Masks panel, painting with the Brush |
//!
//! (Format 3 profiles also mapped `masking.mask` and `masking.mask.gradient`; they are merged into `masking` on
//! load, see `defaults::merge_mask_pages`.)

use serde::Serialize;

/// The app state a context is made from (the host fills it from its own state).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AppState<'a> {
    /// The open dialog, as a lowerCamel name (`export`, `settings`), if any.
    pub dialog: Option<&'a str>,
    /// The grid is showing (no photo open in Detail).
    pub library: bool,
    /// The right-hand tool panel: `edit`, `crop`, `remove`, `masking`, `redeye`, … (`None` = none).
    pub panel: Option<&'a str>,
    /// The Edit panel's focused section (`light`, `color`, …).
    pub section: Option<&'a str>,
    /// A mask is selected (Masking).
    pub mask_selected: bool,
    /// The active painting tool: `brush`, `linear`, `radial`.
    pub tool: Option<&'a str>,
}

/// The current context: its key and an English label for the OSD and the editor.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Context {
    pub key: String,
}

impl Context {
    pub fn new(key: impl Into<String>) -> Context {
        Context { key: key.into() }
    }
}

/// The context key of an app state.
pub fn context_key(s: &AppState) -> String {
    if let Some(d) = s.dialog.filter(|d| !d.is_empty()) {
        return format!("dialog.{d}");
    }
    if s.library {
        return "library".into();
    }
    match s.panel {
        Some("crop") => "crop".into(),
        Some("remove") => "remove".into(),
        Some("redeye") => "redeye".into(),
        // one page for the panel, whether a mask is selected or not (its controls then set the next mask's
        // settings); the Brush has its own (size, feather, flow while painting)
        Some("masking") => if s.tool == Some("brush") { "masking.mask.brush" } else { "masking" }.into(),
        _ => match s.section.filter(|s| !s.is_empty()) {
            Some(sec) => format!("develop.{sec}"),
            None => "develop".into(),
        },
    }
}

/// `key`, then each parent, then `default` (`masking.mask.brush` → `masking.mask` → `masking` →
/// `default`).
pub fn fallbacks(key: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut k = key.trim();
    while !k.is_empty() {
        out.push(k.to_string());
        match k.rfind('.') {
            Some(i) => k = k.get(..i).unwrap_or(""),
            None => break,
        }
    }
    if out.last().map(String::as_str) != Some("default") {
        out.push("default".into());
    }
    out
}

/// Contexts the editor offers for mapping pages (key, English label), most general first.
pub const KNOWN: &[(&str, &str)] = &[
    ("default", "Anywhere"),
    ("library", "Library (grid)"),
    ("develop", "Edit"),
    ("develop.light", "Edit: Light"),
    ("develop.color", "Edit: Color"),
    ("develop.effects", "Edit: Effects"),
    ("develop.detail", "Edit: Detail"),
    ("develop.optics", "Edit: Optics"),
    ("develop.geometry", "Edit: Geometry"),
    ("crop", "Crop & Rotate"),
    ("remove", "Remove"),
    ("redeye", "Red Eye"),
    ("masking", "Masks panel"),
    ("masking.mask.brush", "Masks panel: painting with the Brush"),
    ("dialog", "Any dialog"),
    ("dialog.export", "Export dialog"),
    ("dialog.import", "Import dialog"),
    ("dialog.settings", "Settings"),
];

/// The English label of a context key (the key itself when unknown).
pub fn label(key: &str) -> &str {
    KNOWN.iter().find(|(k, _)| *k == key).map_or(key, |(_, l)| l)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_from_app_state() {
        let s = AppState::default();
        assert_eq!(context_key(&s), "develop");
        assert_eq!(context_key(&AppState { library: true, ..s }), "library");
        assert_eq!(context_key(&AppState { dialog: Some("export"), library: true, ..s }), "dialog.export", "a dialog wins");
        assert_eq!(context_key(&AppState { panel: Some("edit"), section: Some("color"), ..s }), "develop.color");
        assert_eq!(context_key(&AppState { panel: Some("crop"), section: Some("color"), ..s }), "crop");
        assert_eq!(context_key(&AppState { panel: Some("masking"), ..s }), "masking");
        // one Masks page, a mask selected or not (note 24); the Brush its own
        assert_eq!(context_key(&AppState { panel: Some("masking"), mask_selected: true, ..s }), "masking");
        assert_eq!(context_key(&AppState { panel: Some("masking"), mask_selected: true, tool: Some("brush"), ..s }), "masking.mask.brush");
        assert_eq!(context_key(&AppState { panel: Some("masking"), mask_selected: true, tool: Some("radial"), ..s }), "masking");
        assert_eq!(
            context_key(&AppState { panel: Some("masking"), tool: Some("brush"), ..s }),
            "masking.mask.brush",
            "the Brush armed: its page, before the first stroke too"
        );
    }

    #[test]
    fn fallback_chain() {
        assert_eq!(fallbacks("masking.mask.brush"), ["masking.mask.brush", "masking.mask", "masking", "default"]);
        assert_eq!(fallbacks("default"), ["default"]);
        assert_eq!(fallbacks(""), ["default"]);
        assert_eq!(fallbacks("a..b"), ["a..b", "a.", "a", "default"]);
        assert_eq!(label("crop"), "Crop & Rotate");
        assert_eq!(label("x.y"), "x.y");
    }
}
