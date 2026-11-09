# Localization

> Names in angle brackets (`<app>`, `<binary>`, `<cli>`, `<PREFIX>`, `<settings_dir>`, …) are the values set in
> [`brand.toml`](../brand.toml); see the README.

The interface ships in the language it is written in (English) plus every language in the table in
`crates/ui-egui/src/i18n.rs`. Today that is English, Simplified Chinese (`zh-hans`), Traditional
Chinese (`zh-hant`, Taiwan), Japanese (`ja`), Brazilian Portuguese (`pt-br`), Spanish (`es`), German (`de`), Russian (`ru`), French (`fr`), Ukrainian (`uk`), Czech (`cs`), Italian (`it`), Georgian (`ka`), Swedish (`sv`), Finnish (`fi`), Indonesian (`id`), Vietnamese (`vi`) and Polish (`pl`). This file is the reference for **adding or maintaining a language**; the per-language notes
are in [`localization-zh-hans.md`](localization-zh-hans.md), [`localization-zh-hant.md`](localization-zh-hant.md),
[`localization-ja.md`](localization-ja.md), [`localization-pt-br.md`](localization-pt-br.md), [`localization-es.md`](localization-es.md),
[`localization-de.md`](localization-de.md), [`localization-ru.md`](localization-ru.md),
[`localization-cs.md`](localization-cs.md), [`localization-fi.md`](localization-fi.md), [`localization-ka.md`](localization-ka.md), [`localization-id.md`](localization-id.md), [`localization-vi.md`](localization-vi.md) and [`localization-pl.md`](localization-pl.md).

## Adding a language

1. **Translate the two catalogs.** Copy `locales/ja.json` and `locales/ja-formats.json` to
   `locales/<code>.json` and `locales/<code>-formats.json`, then translate the values. Keys are the
   English source strings and must stay byte-for-byte identical. Every message and format must be
   translated: one a catalog lacks, or translates as an empty string, shows in English and fails the
   coverage tests (see [Testing and looking at it](#testing-and-looking-at-it)), Help ▸ What's New's
   lines aside. Contextual entries are added only where needed (below). The formats catalog also holds the date
   patterns (`{year}`, `{month} {year}`, `{weekday}, {day} {month} {year}`…) that date headings and
   capture times use in every language but English; weekday names go in the plain catalog.
2. **Add the table entry** in `crates/ui-egui/src/i18n.rs`:

   ```rust
   language_table! {
       En, "en";
       ZhHans, "zh-hans", "简体中文", "Hans", include_str!("../locales/zh-hans.json");
       ZhHant, "zh-hant", "繁體中文（台灣）", "Hant", include_str!("../locales/zh-hant.json");
       Ja, "ja", "日本語", "Jpan", include_str!("../locales/ja.json");
       PtBr, "pt-br", "Português (Brasil)", "Latn", include_str!("../locales/pt-br.json");
       De, "de", "Deutsch", "Latn", include_str!("../locales/de.json");
       Ru, "ru", "Русский", "Cyrl", include_str!("../locales/ru.json");
       Uk, "uk", "Українська", "Cyrl", include_str!("../locales/uk.json");
       Cs, "cs", "Čeština", "Latn", include_str!("../locales/cs.json");
       It, "it", "Italiano", "Latn", include_str!("../locales/it.json");
       Ka, "ka", "ქართული", "Geor", include_str!("../locales/ka.json");
       Fi, "fi", "Suomi", "Latn", include_str!("../locales/fi.json");
       Vi, "vi", "Tiếng Việt", "Latn", include_str!("../locales/vi.json");
   }
   ```

   The fields are the BCP-47 code (also the settings-file value), the **endonym** shown in the
   Language menu, the ISO 15924 script (`Latn`, `Jpan`, `Hans`, `Hant`, `Kore`…), and the embedded
   catalog. `build.rs` picks up `<code>-formats.json` by name, so the macro and the format lookup
   follow automatically. Add the corresponding `app.language.<name>` entry to
   `LANGUAGE_COMMANDS` and `language_from_command` in `src/menus.rs`, and to the
   `language_commands_cover_every_language` test. Settings read the locale table; menus and
   the control channel share the language-command mapping.
3. **Make sure the glyphs exist.** The script drives which craft-fonts faces the UI installs: a
   language whose script has no face in the build shows boxes (the test says so instead of failing).
   Fonts live in [storytold/craft-fonts](https://github.com/storytold/craft-fonts), never here; a
   missing face is added there, not worked around in this repository. Sentence-cased messages go
   through `i18n::upper_first`, which leaves Georgian lowercase (upper-casing Mkhedruli would make
   Mtavruli).
4. Run `cargo xtask ci` (it includes `fmt`, `clippy`, the tests, `parity` and the wasm check).

## How a message is found

`tr("Exposure")` looks the English label up in the active language's catalog and returns the source
text unchanged when there is no entry; `tr_format!` handles text with runtime values. Both are
presentation-only: command ids, file names and user-entered metadata are never translated.

A few messages take a **context**: one English word with two meanings in the UI, where a language's
shared translation reads wrongly in one of the places. The catalog may then carry
`"<context>|<English>"`, which wins there; without it the plain message is used, so every language
carries the plain message and only the languages that need it add a contextual one. Today's contexts:

| Context | English | Where |
|---|---|---|
| `smart album value` | Yes, No | the value of a yes/no smart-album rule (Has Edits, Cropped…) |
| `book text` | Tracking | Book ▸ Type: the letter-spacing slider |
| `focus` | Tracking | the Info panel's Focus row: the camera's AF area |
| `book graphic` | Frame | Book ▸ Background ▸ Graphic: a border around the page |
| `time-lapse` | Frame | the time-lapse export dialog: Fit (letterbox) or Fill (crop) |
| `color wheels` | Log | Develop ▸ Primaries (Witchcraft grading mode): the wheels' Color / Log switch |
| `plug-in tab` | Log | the Plug-in Manager's Log tab |

Japanese, for example, shows a rule's Yes / No as あり / なし (`"smart album value|Yes": "あり"`), since
はい / いいえ answer a question rather than describe a photo; Swedish shows the letter spacing as its
plain *Teckenavstånd* and the AF area as `"focus|Tracking": "Spårning"`.

To give a word a context, the place asks for `"<context>|<English>"` (`tr("book text|Tracking")`, or
that text handed to a helper that translates, as the Log switch does), and the pair goes in
`CONTEXTUAL` in `crates/ui-egui/src/i18n.rs`; `tr` reads only the pairs listed there, so other text
with a `|` in it is a plain message. A context is lowercase words, and the place writes the key out
as one literal. A catalog key that starts with a context but isn't one of `CONTEXTUAL`'s pairs (a
context or English word spelled otherwise) fails `contextual_entries_name_a_lookup_in_the_code`; a
listed pair no source file writes out (the smart-album values aside, whose keys the code builds), or a key written in the source but not listed, fails
`every_contextual_lookup_is_asked_for_in_the_code`. The coverage tests and gaps report never count
a missing contextual entry.

`tr_format!` is generated by `crates/ui-egui/build.rs`: one arm per English message, which formats
the active language's translation with `format!` (English is the fallback arm). Every catalog's
format string is therefore checked by Rust against the call site's own arguments:

```rust
tr_format!("Added {n} photo{} to “{}”", if n == 1 { "" } else { "s" }, album, n = n)
```

- A translation is an ordinary Rust format string over the same arguments. Refer to named values by
  name (`{n}`) and to positional ones by index when the word order changes (`{0}`, `{1}`):
  `"已将 {n} 张照片添加到“{1}”{0:.0}"`. Keep the source's spec (`{:.1}`, `{d:+}`, `{:.0}%`).
- `format!` rejects an unused argument, so a value a language does not need — the English plural
  suffix (`"s"` / `""`) — is consumed as `{:.0}` (a string at precision 0 prints nothing).
- A language whose formats catalog lacks a message shows the English source there, and a
  `tr_format!` call whose message is in no catalog formats the English source (the macro's last
  arm). A new message builds with no catalog entry, and the coverage tests fail until every
  language has it.
- **Write a message with its characters** (`“{}”`, `→`, `…`), never with an escape (`\u{201c}`,
  `\x…`, `\0`, `\'`) or as a raw string (`r"…"`). The macro has one arm per message, written as the
  catalog's JSON spells it, and `macro_rules!` matches a literal as it is written in the source, not
  by its value: a call spelled another way matches no arm and shows English in every language.
  JSON and Rust spell only `\n`, `\t`, `\"` and `\\` the same way. `format_messages_are_spelled_as_their_macro_arms` fails on any other.

**Brand placeholders.** The product name is never written into a catalog. Source text and every
translation use `{app}` for the display name, `{cli}` for the command-line binary and `{env}` for the
environment-variable prefix, all filled from `brand.toml` at run time (`brand_fill` in
`crates/ui-egui/src/i18n.rs`). Keys keep the placeholder as written (`"About {app}"`), and a translation
must keep the same placeholders: `"About {app}"` → `"{app}について"`. Never translate, transliterate or
inflect the placeholder, and phrase sentences so the name needs no grammatical case or gender.

| Placeholder | Filled with (from `brand.toml`) | Example key |
|---|---|---|
| `{app}` | `display_name` | `About {app}`, `{app} could not start` |
| `{cli}` | `cli_binary` | command-line hints |
| `{env}` | `env_prefix` | environment-variable hints |

## Chromaforge's own messages (`locales/fork/`)

Upstream's catalogs (`locales/<code>.json`, `locales/<code>-formats.json`) stay as upstream wrote them.
Chromaforge's added and changed messages go in `locales/fork/<code>.json` and `locales/fork/<code>-formats.json`,
which are merged over them when the app is built.

**The fork catalogs are kept sorted by key**, in Unicode code-point order (what Python's `sorted` and Rust's
`BTreeMap` give), with a two-space indent, UTF-8 text (no `\u` escapes beyond the control characters JSON
requires) and a newline at the end. A new key then sits between its neighbours instead of after the last entry, so
two PRs that add strings conflict only when their keys sort next to each other. To add or change messages:

1. Add the keys anywhere in the fork catalogs (at the end is fine).
2. Run `cargo xtask i18n --sort`. It rewrites the fork catalogs in order and never adds, drops or changes a key or
   a value; it changes nothing when a catalog isn't valid JSON or repeats a key, and names the file (and the repeated key, or the line and column of the error).
3. Commit the result.

`cargo xtask i18n --check`, part of `cargo xtask ci --fast`, fails when a fork catalog is out of order (naming the
file and the first key out of place) or not in that format. A script that writes a catalog itself can use
`json.dumps(catalog, indent=2, ensure_ascii=False, sort_keys=True) + "\n"` in Python, which gives the same file.
Upstream's catalogs are neither checked nor sorted, so upstream changes ported by hand still apply where upstream
put them.

## Testing and looking at it

```sh
cargo test -p lcc-ui-egui --lib -- i18n                    # every check below
CRAFT_FONTS_DIR=../craft-fonts cargo test -p lcc-ui-egui --lib -- i18n   # + glyph coverage
cargo test -p lcc-ui-egui --lib -- --exact i18n::tests::translation_gaps_report --nocapture
```

The tests fail when a language would show English, or a catalog is broken:

| Fails on | Test |
|---|---|
| a message or format missing from any language, or translated as an empty string: every catalog's keys, the labels the UI translates from tables (commands, controls, rule fields…), every `tr_format!` message and every literal the source scan finds (Help ▸ What's New's lines aside) | `i18n::tests::no_language_has_translation_gaps` |
| a literal the UI hands to `tr`, `tr_format!`, `crate::button::Button`, `.tip(`, a function that passes its text to `tr` (`section_header`, `hint`, `row`…) or a file's own macro that does (`trf!`), missing from any language | `tests_i18n_coverage::ui_text_is_in_every_catalog` |
| a Chromaforge message (`locales/fork/`) missing from any language | `tests_i18n_coverage::every_catalog_carries_every_message` |
| built-in text painted in English on a toured screen (the Library views, each Develop panel, Map, Book, Slideshow, Print, Web, the main dialogs) where the language translates it: text shown without `tr` | `tests_i18n_sweep` |
| a `tr_format!` message written with escapes or as a raw string (it matches no catalog arm) | `tests_i18n_coverage::format_messages_are_spelled_as_their_macro_arms` |
| a catalog key with a context that isn't one of `CONTEXTUAL`'s pairs | `tests_i18n_coverage::contextual_entries_name_a_lookup_in_the_code` |
| a `CONTEXTUAL` pair no source file writes out (the smart-album values aside: the code builds those keys), or a `"<context>\|<English>"` literal in the source that `CONTEXTUAL` doesn't list | `tests_i18n_coverage::every_contextual_lookup_is_asked_for_in_the_code` |
| a broken catalog: malformed JSON, a repeated key, placeholders that differ from the source | `i18n::tests::catalogs_load`, `…::fork_catalogs_are_valid_json`, `…::catalogs_agree_on_placeholders` |
| a fork catalog out of order or format | `cargo xtask i18n --check` (part of `cargo xtask ci --fast`) |

The source scan (`crates/ui-egui/src/i18n_scan.rs`) reads only string literals handed straight to a
translating call. Text built at run time, held in a variable or a table (`[("Face", "face"), …]`), or
passed from one helper to another is the sweep's to find on the screens it tours.

The last command is the translation gaps report: per language, the plain and format messages it
lacks and the number of What's New lines it hasn't translated. It always passes, and `cargo xtask ci`
prints it after its test step. While the tests above pass it lists only What's New lines, the one
gap nothing fails on: they are upstream's release notes, translated line by line as a language gets
to them. The glyph tests are skipped without `CRAFT_FONTS_DIR` (there are no CJK faces to check then).
To look at a language, render it headless:

```sh
<PREFIX>_LANGUAGE=zh-hans <cli> snapshot --demo --script tour.jsonl -o out.png --size 1600x1000
```

or, with the app running (`--control 7980`), run the language command
(`engine.execute {"command": "app.language.simplifiedChinese"}`) and take a screenshot.

## Language codes and settings

The settings file stores the BCP-47 code, never the Rust variant name, so a language can be renamed
in code without invalidating anyone's saved preference. `<PREFIX>_LANGUAGE` accepts what a system
locale looks like (`zh_Hans`, `zh-CN`, `en_US`, `ja_JP.UTF-8`) and falls back to the base language
when a region has no dedicated entry.

## Release notes

Help ▸ What's New shows upstream's `docs/whats-new.md` joined with our entries in `docs/whats-new/` (one file
per change; the app's build joins them), and each of its lines (a heading, or one wrapped line of a
bullet, without the `- ` or `### `) is looked up like any message, so a catalog can translate the notes
line by line (German does). A line that is re-wrapped or edited no longer matches its translation and
shows in English until the catalog is updated; the translation gaps report counts the untranslated
lines.
