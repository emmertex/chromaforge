//! P6.3 localisation coverage: every catalog carries the same messages (a message missing from one
//! language fails, it doesn't quietly show English), and every text the UI passes to `tr`
//! (directly, or through a helper that translates; read by `i18n_scan`) is in every catalog.

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

/// Print upstream's own translation gaps (a report, not a failure).
fn report(what: &str, gaps: &[String]) {
    if !gaps.is_empty() {
        eprintln!("{} {what} (reported only); first ones:\n{}", gaps.len(), gaps.iter().take(20).cloned().collect::<Vec<_>>().join("\n"));
    }
}

#[test]
fn every_catalog_carries_every_message() {
    for formats in [false, true] {
        let all: Vec<(Locale, BTreeMap<String, String>)> = translated().map(|l| (l, catalog(l.code(), formats))).collect();
        // Help ▸ What's New's lines are upstream's release notes, translated as each language gets
        // to them (a language with them all, another with none): the translation gap report counts
        // them (`i18n`), they don't fail here
        let notes = whats_new_lines();
        // a contextual entry (`book text|Tracking`) is only where a language's plain translation reads
        // wrongly in that place: no other language needs it
        let union: BTreeSet<&String> =
            all.iter().flat_map(|(_, c)| c.keys()).filter(|k| !notes.contains(k.as_str()) && crate::i18n::contextual(k).is_none()).collect();
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
    }
}

/// Every text the UI hands to `tr` (directly, or through a helper that translates), in every
/// non-test source file (`i18n_scan`), is in every catalog.
#[test]
fn ui_text_is_in_every_catalog() {
    let texts = crate::i18n_scan::translated_texts();
    assert!(texts.len() > 2000, "the scan finds the UI's text ({} messages)", texts.len());
    assert!(texts.contains_key("Map Style") && texts.contains_key("Zoom In"), "helpers and access names are scanned");
    let mut missing = Vec::new();
    for l in translated() {
        let (plain, formats) = (catalog(l.code(), false), catalog(l.code(), true));
        for (text, files) in &texts {
            // a place that asks with a context (`focus|Tracking`) shows the plain message where the
            // language has no contextual entry
            let text = crate::i18n::contextual(text).map_or(text.as_str(), |(_, plain)| plain);
            if !plain.contains_key(text) && !formats.contains_key(text) {
                let files: Vec<&str> = files.iter().map(String::as_str).collect();
                missing.push(format!("{}: {text:?} ({})", l.code(), files.join(", ")));
            }
        }
    }
    assert!(missing.is_empty(), "{} untranslated message(s):\n{}", missing.len(), missing.join("\n"));
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

/// A catalog key that starts with a context (lowercase words, then `|`: `book text|Tracking`) is
/// one of the contextual lookups the code makes (`crate::i18n::CONTEXTUAL`): a context or English
/// message spelled otherwise (`focus|Trackng`, `fokus|Tracking`) would never be shown, and the
/// place would show the plain translation without a word. (Messages with a `|` further on, such as
/// `Keyword (a|b nests it under a)`, are plain.)
#[test]
fn contextual_entries_name_a_lookup_in_the_code() {
    let is_context = |text: &str| !text.is_empty() && text.chars().all(|c| c.is_ascii_lowercase() || c == ' ' || c == '-');
    assert!(crate::i18n::CONTEXTUAL.iter().all(|(context, _)| is_context(context)), "contexts are lowercase words");
    let mut stray = Vec::new();
    for l in translated() {
        for formats in [false, true] {
            for key in catalog(l.code(), formats).into_keys() {
                let context_shaped = key.split_once('|').is_some_and(|(context, _)| is_context(context));
                if context_shaped && (formats || crate::i18n::contextual(&key).is_none()) {
                    stray.push(format!("{}{}: {key:?}", l.code(), if formats { " (formats)" } else { "" }));
                }
            }
        }
    }
    assert!(stray.is_empty(), "contextual entries no place asks for (each must be one of `CONTEXTUAL`'s):\n{}", stray.join("\n"));
}

/// Each contextual lookup (`crate::i18n::CONTEXTUAL`) is asked for somewhere in the UI's source as the
/// literal `"<context>|<English>"`, and every literal shaped like one is listed: a place spelled
/// otherwise (`"plug-in tabs|Log"`) would paint the key itself in every language, and a place gone
/// back to the plain word would quietly drop its language's contextual entries.
#[test]
fn every_contextual_lookup_is_asked_for_in_the_code() {
    let sources = crate::i18n_scan::sources();
    let mut asked: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for source in &sources {
        let code = source.code.as_str();
        for (at, _) in code.match_indices('"') {
            // `"` + a lowercase context + `|` + a capitalised English word
            let rest = code.get(at + 1..).unwrap_or_default();
            let Some((context, english)) = rest.split_once('|') else { continue };
            let context_shaped =
                context.starts_with(|c: char| c.is_ascii_lowercase()) && context.chars().all(|c| c.is_ascii_lowercase() || c == ' ' || c == '-');
            if !context_shaped || !english.starts_with(|c: char| c.is_ascii_uppercase()) {
                continue;
            }
            let Some((english, _)) = english.split_once('"') else { continue };
            asked.entry(format!("{context}|{english}")).or_default().insert(source.file.clone());
        }
    }
    // the smart-album values are built from `BOOL_CONTEXT` and the rule's label, not written out
    let listed: BTreeSet<String> =
        crate::i18n::CONTEXTUAL.iter().filter(|(context, _)| *context != "smart album value").map(|(c, e)| format!("{c}|{e}")).collect();
    let unasked: Vec<&String> = listed.iter().filter(|key| !asked.contains_key(*key)).collect();
    assert!(unasked.is_empty(), "contextual lookups no source file asks for: {unasked:?}");
    let unlisted: Vec<String> = asked
        .iter()
        .filter(|(key, _)| !listed.contains(*key))
        .map(|(key, files)| format!("{key:?} ({})", files.iter().cloned().collect::<Vec<_>>().join(", ")))
        .collect();
    assert!(unlisted.is_empty(), "contextual keys in the source that `CONTEXTUAL` doesn't list (shown as written):\n{}", unlisted.join("\n"));
}

/// The string literals in Rust source, as written (escapes left as they are), outside comments.
/// Char literals and lifetimes are stepped over; raw strings (`r"…"`, `r#"…"#`) are read whole.
fn string_literals(code: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let bytes = code.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let rest = &code[i..];
        if rest.starts_with("//") {
            i += rest.find('\n').unwrap_or(rest.len());
        } else if rest.starts_with("/*") {
            let (mut depth, mut j) = (0usize, 0usize);
            while j < rest.len() {
                if rest[j..].starts_with("/*") {
                    depth += 1;
                    j += 2;
                } else if rest[j..].starts_with("*/") {
                    depth -= 1;
                    j += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    j += rest[j..].chars().next().map_or(1, char::len_utf8);
                }
            }
            i += j;
        } else if (rest.starts_with('r') || rest.starts_with("br"))
            && (i == 0 || !(bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_'))
            && rest.trim_start_matches('b').trim_start_matches('r').trim_start_matches('#').starts_with('"')
        {
            let open = rest.find('"').unwrap();
            let hashes = &rest[rest.find('r').unwrap() + 1..open];
            let close = format!("\"{hashes}");
            let end = rest[open + 1..].find(&close).map_or(rest.len(), |e| open + 1 + e);
            out.push(&rest[open + 1..end]);
            i += (end + close.len()).min(rest.len());
        } else if rest.starts_with('"') {
            let mut j = 1;
            while j < rest.len() && bytes[i + j] != b'"' {
                j += if bytes[i + j] == b'\\' { 2 } else { 1 };
            }
            out.push(&rest[1..j.min(rest.len())]);
            i += j + 1;
        } else if let Some(after) = rest.strip_prefix('\'') {
            // a char literal ('x', '\n', '\u{2014}', '"') or a lifetime ('a)
            i += match after.chars().next() {
                Some('\\') => after.get(2..).and_then(|r| r.find('\'')).map_or(1, |e| e + 4),
                Some(c) if after[c.len_utf8()..].starts_with('\'') => 2 + c.len_utf8(),
                _ => 1,
            };
        } else {
            i += rest.chars().next().map_or(1, char::len_utf8);
        }
    }
    out
}

/// The UI's copy has no em dashes (U+2014): a comma, colon, full stop or parentheses read better,
/// in every language. Checked in every catalog (keys and translations, upstream's and the fork's),
/// the commands' labels, menus and descriptions (shown in the command palette, the control
/// surface, the CLI and to MCP agents), What's New, and every string literal outside comments and
/// tests in the workspace's crates and apps.
#[test]
fn ui_text_has_no_em_dashes() {
    const DASH: char = '\u{2014}';
    let has = |s: &str| s.contains(DASH) || s.contains("\\u{2014}");
    let mut found = Vec::new();
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("locales");
    for sub in [dir.clone(), dir.join("fork")] {
        for entry in std::fs::read_dir(&sub).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|e| e == "json") {
                let text = std::fs::read_to_string(&path).unwrap();
                let messages: BTreeMap<String, String> = serde_json::from_str(&text).unwrap();
                for (k, v) in &messages {
                    if has(k) || has(v) {
                        found.push(format!("{}: {k:?} = {v:?}", path.display()));
                    }
                }
            }
        }
    }
    for c in lcc_engine::command_specs() {
        if has(c.label) || has(c.params) || c.menu.iter().any(|m| has(m)) {
            found.push(format!("command {}: {:?} {:?}", c.id, c.label, c.params));
        }
    }
    for line in crate::panels::dialogs::WHATS_NEW.lines().filter(|l| has(l)) {
        found.push(format!("What's New: {line}"));
    }
    // every crate's and app's `src/`, test code left out (as `i18n_scan` reads the UI's)
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut dirs: Vec<std::path::PathBuf> = ["crates", "apps"]
        .iter()
        .flat_map(|top| std::fs::read_dir(root.join(top)).unwrap().map(|e| e.unwrap().path().join("src")))
        .filter(|p| p.is_dir())
        .collect();
    while let Some(dir) = dirs.pop() {
        for path in std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().path()) {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
            if path.is_dir() {
                if name != "tests" {
                    dirs.push(path);
                }
                continue;
            }
            if !name.ends_with(".rs") || name.starts_with("tests_") || name == "tests.rs" {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap().replace("\r\n", "\n");
            let code = text.split("#[cfg(test)]\nmod tests {").next().unwrap_or_default();
            for literal in string_literals(code).into_iter().filter(|l| has(l)) {
                let short: String = literal.chars().take(120).collect();
                found.push(format!("{}: {short:?}", path.display()));
            }
        }
    }
    assert!(found.is_empty(), "{} em dash(es) in UI text; rewrite with a comma, colon, full stop or parentheses:\n{}", found.len(), found.join("\n"));
}

#[test]
fn the_literal_reader_skips_comments_chars_and_lifetimes() {
    let code = "// a \"comment\"\nlet x: &'a str = \"one\"; /* \"two\" /* \"nested\" */ */ let c = '\"'; let r = r#\"raw \"q\"\"#; b\"bytes\"";
    assert_eq!(string_literals(code), vec!["one", "raw \"q\"", "bytes"]);
}
