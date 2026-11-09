# Localization parity

> **Last reviewed:** 2026-10-13 · **Last updated:** 2026-11-09 · **Change:** minor (re-measured with the translation gaps report: every language has all 5,297 interface messages and 551 formats; What's New counted apart; Czech and Swedish rows added); minor (upstream: German interface strings and formats messages reviewed, terms fixed in localization-de.md; re-measured with the fork overlays; AI Denoise screens translated; upstream: de, es, fr, ja, uk catalogs filled, Italian and Georgian interface catalogs added); upstream: Italian: every interface string translated, native-speaker corrections · **Target:** Adobe Lightroom Classic 15.5 / Lightroom 9.6

How far each interface language is from `full`. How to add or maintain a language is in
[localization.md](localization.md); per-language notes are in `localization-<code>.md`
([zh-hans](localization-zh-hans.md), [zh-hant](localization-zh-hant.md), [ja](localization-ja.md),
[pt-br](localization-pt-br.md), [es](localization-es.md), [de](localization-de.md),
[ru](localization-ru.md), [fi](localization-fi.md), [ka](localization-ka.md), [id](localization-id.md), [vi](localization-vi.md), [pl](localization-pl.md)).

## The target

Lightroom 9.6 (installed on the owner's Mac) ships 16 interface languages, listed from its bundle's
`.lproj` folder names: English, German, Spanish, French, Italian, Japanese, Korean, Norwegian Bokmål,
Dutch, Polish, Portuguese (Brazil), Russian, Swedish, Thai, Simplified Chinese and Traditional
Chinese. It does **not** ship Hindi, Arabic, Indonesian or Vietnamese, so for those the app would
go beyond the target.

## How the numbers are measured

**Measured 2026-11-09** with the translation gaps report, which counts from the same lists the coverage
tests check ([localization.md](localization.md#testing-and-looking-at-it)):

```sh
cargo test -p lcc-ui-egui --lib -- --exact i18n::tests::translation_gaps_report --nocapture
```

- **Interface messages:** 5,297 plain messages and 551 formats: every key of every catalog (upstream's and
  Chromaforge's `locales/fork/` overlays; contextual entries such as `focus|Tracking` aside), the labels the
  UI translates from tables (commands, controls, rule fields…), every `tr_format!` message, and every literal
  the source scan (`crates/ui-egui/src/i18n_scan.rs`) finds handed to `tr`, a helper that translates or a
  translating macro. Since 2026-11-09 the tests fail on any of them missing from any language, so every
  translated language has all of them.
- **Help ▸ What's New:** 3,622 lines (upstream's release notes, one message per line), counted apart: a
  language translates them as it gets to them, and no test asks it to.
- A message counts as translated when the catalog has a non-empty value; one spelled as in English counts
  too (`ISO`, `OK`, `Log`). Text that never reaches a catalog (errors raised in lower crates, text built at
  run time outside the screens `tests_i18n_sweep` tours) is not in the count, which is why no language is
  `full` at 100%.

Script support is checked against the code: `language_table!` in `crates/ui-egui/src/i18n.rs` lists
scripts `Latn`, `Cyrl`, `Hans`, `Hant`, `Jpan` only. The UI is egui, which has no bidirectional layout
and no complex-script shaping, so Arabic (RTL, joining) and Devanagari (conjuncts, reordering) cannot
render correctly today.

Native-speaker review: no catalog has had a full formal review; German's interface strings and formats messages have (2026-10-24, What's New not). Several catalogs have edits from community
contributors whose commits only touch that language (ja, zh-hans, zh-hant, fr, de, ru, uk, pt-br);
that is promising but is not recorded as review, so the column says `no (community edits)`.

Form of address: German speaks to the user with *du* ("Wähle ein Foto", "Dein Name"), never *Sie*.
The app is built by its community, and *du* is how people helping each other talk. A new German
string follows that.

## The twelve key languages

| # | Language | Code | UI strings translated | Dialogs / tooltips / help | Script support | Native review | Status | To `full` |
|---|---|---|---:|---|---|---|---|---:|
| 1 | English | `en` | all (source) | all; Help ▸ What's New and the shortcut sheet; no full user manual | Latin (Inter) | n/a | **full** | 0 h |
| 2 | Simplified Chinese | `zh-hans` | all 5,297 + 551 formats; What's New 208 / 3,622 | menus, panels, dialogs, presets, profiles, dates; lower-layer errors English | Hans via craft-fonts (Noto Sans CJK SC); IME through winit; no vertical text in UI (not in target either) | no (community edits) | partial | 2–4 h |
| 3 | Spanish | `es` | all 5,297 + 551 formats; What's New 208 / 3,622 | as zh-hans | Latin | no (community edits) | partial | 2–4 h |
| 4 | Hindi | `hi` | 0 (0%) | none | **Devanagari shaping missing** (egui has no complex shaping) | no | none (target: not shipped) | 50–90 h (shaping 25–50 h, shared with Arabic) |
| 5 | Arabic | `ar` | 0 (0%) | none | **RTL layout and Arabic shaping missing** | no | none (target: not shipped) | 60–110 h (RTL mirroring 30–50 h + shaping, shared with Hindi) |
| 6 | French | `fr` | all 5,297 + 551 formats; What's New 1,103 / 3,622 | as zh-hans | Latin | no (community edits) | partial | 2–4 h |
| 7 | Portuguese (Brazil) | `pt-br` | all 5,297 + 551 formats; What's New 208 / 3,622 | as zh-hans | Latin | no (community edits) | partial | 2–4 h |
| 8 | Indonesian | `id` | all 5,297 + 551 formats; What's New 30 / 3,622 | as zh-hans | Latin (Inter covers it) | no (written with an agent; native-speaker pass pending) | partial (target: not shipped) | 2–4 h (native review, lower-layer errors) |
| 9 | Japanese | `ja` | all 5,297 + 551 formats; What's New 208 / 3,622 | as zh-hans; vertical export watermarks | Jpan via craft-fonts (BIZ UDPGothic / UDMincho); IME through winit | no (community edits) | partial | 2–4 h |
| 10 | German | `de` | all 5,297 + 551 formats; What's New 1,069 / 3,622 | as zh-hans; What's New translated line by line | Latin | partly (2026-10-24: every interface string and formats message, reviewed with an agent and spot-checked by a native speaker; What's New lines not reviewed) | partial | 1–2 h (What's New review, lower-layer errors) |
| 11 | Korean | `ko` | 0 (0%) | none | **no Korean (Hangul) face**: needs a non-Adobe OFL face in craft-fonts (Noto CJK is allowed only for Chinese) | no | none | 14–22 h |
| 12 | Vietnamese | `vi` | all 5,297 + 551 formats; What's New 207 / 3,622 | menus, panels, dialogs, presets, profiles, dates, formats; lower-layer errors English | Latin with stacked diacritics (Inter) | no (community contribution) | partial (catalog complete; lower-layer errors English) | 1–3 h |

## Other languages the app ships (9)

| Language | Code | UI strings translated | Script | Native review | Status | To `full` |
|---|---|---:|---|---|---|---:|
| Traditional Chinese (Taiwan) | `zh-hant` | all 5,297 + 551 formats; What's New 208 / 3,622 | Hant; uses the Simplified Chinese face until craft-fonts has a TC face | no (community edits) | partial | 4–8 h (incl. the font) |
| Russian | `ru` | all 5,297 + 551 formats; What's New 208 / 3,622 | Cyrillic (Inter) | no (community edits) | partial | 2–4 h |
| Ukrainian | `uk` | all 5,297 + 551 formats; What's New 208 / 3,622 | Cyrillic (Inter) | no (community edits) | partial (lower-layer errors English) | 2–4 h |
| Italian | `it` | all 5,297 + 551 formats; What's New 52 / 3,622 | Latin (Inter) | partly (written with an agent; corrections and the 2026-10-15 additions by a native speaker) | partial (lower-layer errors English) | 3–5 h |
| Georgian | `ka` | all 5,297 + 551 formats; What's New 695 / 3,622 | Geor: no craft-fonts face yet (boxes until one is available) | yes (native speaker) | partial (lower-layer errors English) | 2–4 h (+ a Georgian face) |
| Finnish | `fi` | all 5,297 + 551 formats; What's New 29 / 3,622 | Latin (Inter) | yes (written with an agent; native-speaker review by @roopeg) | partial (lower-layer errors English) | 3–5 h |
| Polish | `pl` | all 5,297 + 551 formats; What's New 207 / 3,622 | Latin (Inter) | no (AI-drafted; follow-up by a native speaker) | partial (lower-layer errors English) | 2–4 h (full native review) |
| Czech | `cs` | all 5,297 + 551 formats; What's New 30 / 3,622 | Latin (Inter) | no (written with an agent) | partial (lower-layer errors English) | 2–4 h (native review) |
| Swedish | `sv` | all 5,297 + 551 formats; What's New 207 / 3,622 | Latin (Inter) | no (written with an agent) | partial (lower-layer errors English) | 2–4 h (native review) |

Target languages the app lacks: Korean, Dutch, Norwegian Bokmål, Thai
(Thai needs shaping and line breaking without spaces). Each Latin-script one is ~10–16 h; Thai
~20–35 h on top of the shaping work.

## Shared work before any language is `full` (counted once, ~20–35 h)

- Route lower-layer error messages (catalog, raw, codecs, engine) through translatable message ids
  instead of English `format!` strings (~12–20 h). Translating them must not change what the app does:
  since #1046 the interface decides on an error's kind (`lcc_engine::ErrorKind`, a download's or
  Apple Photos import's `cancelled` flag), not its words; new decisions on an error go through a kind too.
- ~~A check that every user-visible literal reaches a catalog~~: done 2026-11-09. The coverage tests fail on
  any message a language lacks, the source scan reads the literals handed to `tr`, helpers that translate and
  translating macros, and `tests_i18n_sweep` fails on English painted on its toured screens
  ([localization.md](localization.md#testing-and-looking-at-it)).
- A user manual / help pages beyond What's New and the shortcut sheet (Lightroom links to online
  help; ~4–8 h for a translatable in-app help index).
- Native-speaker review passes (human; cannot be done by agents).

## Totals

- Bring the shipped non-English catalogs to `full` (17 on 2026-10-23; the estimate was made for 10): ~55–90 h (incl. the shared work).
- Add the 5 missing key languages: ~145–255 h, most of it RTL and complex shaping in egui.
- Reach the target's 16 languages: + ~30–55 h (Dutch, Norwegian, Thai; Polish and Swedish ship).

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-11-09 | minor | Re-measured with the translation gaps report (the command is under How the numbers are measured): every translated language has all 5,297 interface messages and 551 formats, and the tests now fail on a missing one; What's New lines (3,622) counted apart, 29–1,103 translated per language; Czech and Swedish rows added; the shared literal check is done |
| 2026-10-19 | minor | Indonesian interface catalog (`id`): every plain and formats message any catalog has, plus 69 command labels; What's New left in English |
| 2026-10-17 | minor | Finnish interface catalog (`fi`): 2,610 of 3,308 messages (2,313 plain + 297 formats), What's New left in English |
| 2026-10-15 | minor | Polish row added: the format messages still in English translated, counts made right for every number, dates day.month.year ([localization-pl.md](localization-pl.md)) |
| 2026-10-15 | trivial | Italian (`it`): 339 interface messages and 60 format strings that were missing or still in English translated (2,628 of 3,278; only What's New left in English); native-speaker corrections: Exposure → *Esposizione*, Sharpening → *Nitidezza*, Preset → *Preset*, blurry → *sfocate* |
| 2026-10-13 | minor | Remeasured denominator (2,222 plain + 249 formats = 2,471); Vietnamese catalog complete at 2,471/2,471 (status partial: lower-layer errors still English) |
| 2026-10-12 | minor | Italian interface catalog (`it`): 2,201 of 2,398 messages, What's New left in English |
| 2026-10-13 | minor | Re-measured with the fork overlays (3,378 messages): 96–99% per language; AI Denoise screens translated in de, es, ja, pt-br, zh-hant; last denoise and panel-layout strings in zh-hans, ru, uk |
| 2026-10-11 | minor | Shared work: the interface decides on error kinds, not English error text (#1046) |
| 2026-10-12 | minor | Georgian (`ka`) row: every catalog message |
| 2026-10-10 | major | Created: per-language counts measured from the catalogs, the twelve key languages, script support, estimates |
| 2026-10-12 | minor | de, es, fr, ja and uk catalogs filled to every message another catalog knows (agent translations, native review welcome); those rows re-measured against the current 2,398 messages. Other rows keep their 2026-10-10 counts (open PRs for pt-BR, ru, zh) |
| 2026-10-16 | minor | German (`de`): 217 interface strings and 862 What's New lines added (agent translations, native review welcome); re-measured 2,572 → 3,644 of 3,754 messages (80% → 97%). The denominator grew because German now translates What's New lines no other catalog has; other rows keep their earlier counts |
| 2026-10-24 | minor | German (`de`): interface strings and formats messages reviewed (agent-prepared, spot-checked by a native speaker); 108 messages corrected (Tönung, Beschneidung, Farbmarkierung vs. Markierung, Bedienfeld, Begleitdatei, Dia, en dash and ellipsis spacing, ambiguous *Ihre*); terms listed in localization-de.md; native review column `partly`, estimate 1–2 h |
| 2026-10-20 | minor | German (`de`) speaks to the user with *du*; the 11 messages that used *Sie* changed, no count changes |
