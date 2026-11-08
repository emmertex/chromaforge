//! P6.3 localisation coverage: every complete catalog carries the same messages (a message missing
//! from one language fails, it doesn't quietly show English), and every text the UI passes to
//! `tr` (directly, or through a helper that translates; read by `i18n_scan`) is in every complete catalog.
//!
//! The languages in [`PARTIAL`] are catching up: what they lack is reported, not failed, as
//! upstream's `display_label_gaps_are_reported` does for every language.

use std::collections::{BTreeMap, BTreeSet};

use crate::i18n::Locale;

fn catalog(code: &str, formats: bool) -> BTreeMap<String, String> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("locales");
    let name = if formats { format!("{code}-formats.json") } else { format!("{code}.json") };
    let text = std::fs::read_to_string(dir.join(&name)).unwrap_or_else(|e| panic!("{name}: {e}"));
    let mut messages: BTreeMap<String, String> = serde_json::from_str(&text).unwrap_or_else(|e| panic!("{name}: {e}"));
    // the fork's rows (`locales/fork/`), merged over upstream's as the app does
    if let Ok(text) = std::fs::read_to_string(dir.join("fork").join(&name)) {
        messages.extend(serde_json::from_str::<BTreeMap<String, String>>(&text).unwrap_or_else(|e| panic!("fork/{name}: {e}")));
    }
    messages
}

/// Languages upstream added after the fork's own UI: their catalogs (upstream's rows plus the
/// fork overlay's few rows) don't yet carry the fork's messages. Their gaps are reported; the
/// messages upstream's own tests require in every language (the smart-album rule words) are in
/// their overlays. A language leaves this list once its overlay catches up.
const PARTIAL: &[Locale] = &[Locale::Pl, Locale::Cs, Locale::It, Locale::Ka, Locale::Sv, Locale::Fi, Locale::Id, Locale::Vi];

/// Every message a fork overlay (`locales/fork/`) carries, in any language.
fn fork_keys(formats: bool) -> BTreeSet<String> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("locales").join("fork");
    translated()
        .filter_map(|l| {
            let name = if formats { format!("{}-formats.json", l.code()) } else { format!("{}.json", l.code()) };
            std::fs::read_to_string(dir.join(name)).ok()
        })
        .filter_map(|text| serde_json::from_str::<BTreeMap<String, String>>(&text).ok())
        .flat_map(BTreeMap::into_keys)
        .collect()
}

/// Help ▸ What's New, line by line as the dialog looks each one up.
fn whats_new_lines() -> BTreeSet<&'static str> {
    crate::panels::dialogs::WHATS_NEW
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with("# "))
        .map(|line| line.strip_prefix("### ").or_else(|| line.strip_prefix("## ")).or_else(|| line.strip_prefix("- ")).unwrap_or(line))
        .collect()
}

fn translated() -> impl Iterator<Item = Locale> {
    Locale::ALL.iter().copied().filter(|l| *l != Locale::En)
}

fn complete() -> impl Iterator<Item = Locale> {
    translated().filter(|l| !PARTIAL.contains(l))
}

/// Print what a partial language lacks (a report, not a failure).
fn report(what: &str, gaps: &[String]) {
    if !gaps.is_empty() {
        eprintln!(
            "{} {what} (reported only: partial languages, files outside STRICT); first ones:\n{}",
            gaps.len(),
            gaps.iter().take(20).cloned().collect::<Vec<_>>().join("\n")
        );
    }
}

#[test]
fn every_catalog_carries_every_message() {
    for formats in [false, true] {
        let all: Vec<(Locale, BTreeMap<String, String>)> = complete().map(|l| (l, catalog(l.code(), formats))).collect();
        // Help ▸ What's New's lines are upstream's release notes, translated as each language gets
        // to them (a language with them all, another with none): the translation gap report counts
        // them (`i18n`), they don't fail here
        let notes = whats_new_lines();
        let union: BTreeSet<&String> = all.iter().flat_map(|(_, c)| c.keys()).filter(|k| !notes.contains(k.as_str())).collect();
        // a message only upstream's catalogs carry, in some languages, is upstream's translation
        // gap (its own report counts it): reported here; the fork's messages must be everywhere
        let fork = fork_keys(formats);
        let mut missing = Vec::new();
        let mut upstream_gaps = Vec::new();
        for (l, c) in &all {
            for key in &union {
                if !c.contains_key(*key) {
                    let line = format!("{} lacks {key:?}", l.code());
                    if fork.contains(key.as_str()) { missing.push(line) } else { upstream_gaps.push(line) }
                }
            }
        }
        assert!(missing.is_empty(), "{} message(s) missing (formats: {formats}):\n{}", missing.len(), missing.join("\n"));
        report("upstream messages missing", &upstream_gaps);
        // the partial languages: every runtime format (a missing one would show English text with
        // values in it), the rest reported
        let mut gaps = Vec::new();
        for l in PARTIAL {
            let c = catalog(l.code(), formats);
            for key in &union {
                if !c.contains_key(*key) {
                    if formats {
                        missing.push(format!("{} lacks {key:?}", l.code()));
                    } else {
                        gaps.push(format!("{} lacks {key:?}", l.code()));
                    }
                }
            }
        }
        assert!(missing.is_empty(), "{} format message(s) missing:\n{}", missing.len(), missing.join("\n"));
        report("messages missing", &gaps);
    }
}

/// The files whose text every complete catalog must carry today. The scan reads every file; the
/// rest of the UI's text is reported until the catalogs catch up, and then this list goes.
const STRICT: &[&str] = &[
    "map/mod.rs",
    "map/side.rs",
    "map/view.rs",
    "book.rs",
    "slideshow_ui.rs",
    "print_ui.rs",
    "web_module.rs",
    "creations_ui.rs",
    "edit_in.rs",
    "module.rs",
    "plate.rs",
    "help_overlay.rs",
    "access.rs",
    "panels/publish.rs",
    "panels/ai_models.rs",
    "panels/retouch.rs",
    "panels/tether_bar.rs",
    "panels/connections.rs",
    "panels/peer_sync.rs",
    "folder_poll.rs",
    "panels/plugins.rs",
    "panels/classic.rs",
    "panels/navigator.rs",
    "panels/metadata.rs",
    "panels/cells.rs",
    "video_ui.rs",
    "video_view.rs",
];

/// Every text the UI hands to `tr` (directly, or through a helper that translates), in every
/// non-test source file (`i18n_scan`), is in every complete catalog: a failure for the [`STRICT`]
/// files, a report for the others.
#[test]
fn ui_text_is_in_every_catalog() {
    let texts = crate::i18n_scan::translated_texts();
    assert!(texts.len() > 2000, "the scan finds the UI's text ({} messages)", texts.len());
    assert!(texts.contains_key("Map Style") && texts.contains_key("Zoom In"), "helpers and access names are scanned");
    let (mut missing, mut gaps) = (Vec::new(), Vec::new());
    for l in translated() {
        let (plain, formats) = (catalog(l.code(), false), catalog(l.code(), true));
        for (text, files) in &texts {
            if !plain.contains_key(text) && !formats.contains_key(text) {
                let files: Vec<&str> = files.iter().map(String::as_str).collect();
                let gap = format!("{}: {text:?} ({})", l.code(), files.join(", "));
                if PARTIAL.contains(&l) || !files.iter().any(|f| STRICT.contains(f)) { gaps.push(gap) } else { missing.push(gap) }
            }
        }
    }
    assert!(missing.is_empty(), "{} untranslated message(s):\n{}", missing.len(), missing.join("\n"));
    report("untranslated messages", &gaps);
}

/// `build.rs` writes each format message's macro arm as JSON spells it, and `macro_rules!` matches a
/// literal as written in the source, so a call spelled another way (`\u{201c}` for `“`, a raw string)
/// matches no arm and shows English in every language. JSON and Rust spell only `\n`, `\t`, `\"` and
/// `\\` the same way.
#[test]
fn format_messages_are_spelled_as_their_macro_arms() {
    let mut odd = Vec::new();
    for source in crate::i18n_scan::sources() {
        for (at, _) in source.code.match_indices("tr_format!(") {
            let rest = source.code.get(at + "tr_format!(".len()..).unwrap_or_default().trim_start();
            let Some(body) = rest.strip_prefix('"') else {
                if rest.starts_with('r') {
                    odd.push(format!("{}: a raw string", source.file));
                }
                continue;
            };
            // the literal as written, up to its closing quote
            let (mut raw, mut chars, mut spelled_alike) = (String::new(), body.chars(), true);
            while let Some(c) = chars.next() {
                match c {
                    '"' => break,
                    '\\' => {
                        let next = chars.next().unwrap_or_default();
                        spelled_alike &= matches!(next, 'n' | 't' | '"' | '\\');
                        raw.extend([c, next]);
                    }
                    _ => raw.push(c),
                }
            }
            if !spelled_alike {
                odd.push(format!("{}: \"{raw}\"", source.file));
            }
        }
    }
    assert!(odd.is_empty(), "format messages that match no catalog arm (write the characters themselves):\n{}", odd.join("\n"));
}
