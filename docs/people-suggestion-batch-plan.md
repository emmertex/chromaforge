# Plan: batch assign / reject in a person's suggestions (issue #478)

Status: **draft plan, nothing built.** The open questions in the last section are for the maintainer to decide before
work starts.

## The request

On a person's page in **People**, the **More** strip (unnamed faces that look like this person) should let the user:

- give a suggested face a *different* name inline, picking an existing person from an autocomplete list or making a new
  one ("+ Create 'Alex'");
- select several suggestions (checkbox overlay and/or Shift / Cmd / Ctrl click);
- act on the selection from a batch bar: **Assign to…** (existing or new person) and **Reject**.

## What exists already (see `docs/faces.md`)

- **Unnamed faces** (People overview) already has the selection model: click to select, Shift-click range,
  **Select all**, and a name bar with completion from named people; Enter names every selected face as one undo step.
- `faces.nameFaces {faces: [{photo, index}], name}` already names many faces at once, one undo step, and creates a
  person implicitly by using a new name (people are names, not separate records).
- On a person's page, each suggestion can be confirmed for *this* person (click, `↑`, `A`), or hidden with × **for the
  session only**. Nothing persistent records "this face is not this person".

So batch *assign* is mostly UI reuse; batch *reject* needs a decision about what rejecting means (question 4).

## Commands (engine; everything is a command)

1. **Assign:** reuse `faces.nameFaces`. Only add to it if needed: e.g. a `{created: bool}` in the reply so the UI can say
   "New person Alex" and the undo label reads correctly. No new command unless review finds a gap.
2. **Reject:** depends on question 4.
   - Session-only: no engine command; the UI's existing hidden set takes many ids. (Not reachable from CLI/MCP.)
   - Persistent "not this person": new `faces.rejectSuggestions {person, faces: [{photo, index}]}` plus
     `faces.unrejectSuggestions` (or undo only). Needs a catalog store of (face, person) negative pairs, a schema
     migration, and `faces.person` / `faces.suggest` / `faces.unnamed` filtering them out. One undo step per batch.
   - Persistent "ignore this face entirely": `faces.ignore {faces}` with an ignored flag on the face, excluded from every
     suggestion list and from Unnamed faces, with a way to list and restore ignored faces.
3. Hostile-input handling per CLAUDE.md: unknown photo ids, out-of-range indexes, empty or whitespace names, duplicate
   entries and very long lists produce errors or are skipped and counted, never a panic; cap the list length.
4. Each command gets its `ctl:`/`cmd:` row in `docs/parity-checklist.md`, a line in `docs/faces.md` and `docs/mcp.md`
   if it is new.

## UI (`crates/ui-egui`, People ▸ person page)

1. **Selection on the More strip**, shared with the Unnamed faces code rather than copied: click / modifier-click,
   Shift range, Select all / Select none, Esc clears. How a plain click behaves is question 1.
2. **Batch bar** when one or more suggestions are selected: count, **Confirm as <person>** (existing behaviour, now for
   the selection), **Assign to…** (name field), **Reject** (wording per question 4), **Clear**.
3. **Name field** using `TextField` (not `egui::TextEdit`) with a unique widget id, completion popup of named people
   (matched the way cards group names: trimmed, case-insensitive), and a final "+ Create '<typed>'" row when nothing
   matches exactly. Keyboard: ↑/↓ moves in the list, Enter takes the highlighted row, Esc closes. Where it lives (always
   on each card vs. on demand) is question 2; what Enter does on an unknown name is question 3.
4. After an assign or reject the faces leave the strip, the selection clears, and one Undo restores all of them.
5. Strings go through the i18n catalogs (`{app}` never spelled out).

## Tests

- Engine: `faces.nameFaces` with a mix of valid/invalid entries, new vs existing name, undo restores all; hostile-input
  table (bad ids, huge lists, empty names) asserts errors, not panics.
- If a reject store is added: rejected faces drop out of `faces.person` for that person (and only that person, for
  "not this person"), survive reopening the catalog, and undo brings them back; migration from an old catalog.
- MCP / CLI end-to-end: one round trip through `crates/mcp/tests/e2e.rs` for each new command.
- UI via the SIL harness and headless snapshots (`lcc-cli snapshot --demo --script …`): select three suggestions,
  assign to an existing name, assign to a new name, reject; check the strip and the person cards afterwards.
- No change-detector tests (positions, labels, counts of widgets).

## Release notes and docs

`docs/whats-new/<date>-people-batch-suggestions.md`, a `docs/changelog/unreleased/` entry, the person-page section of
`docs/faces.md`, and the relevant `docs/parity-checklist.md` / `docs/gaps.md` rows.

## Open questions (undecided)

### 1. How is a suggestion selected?

- **A. Checkbox overlay on hover, plus modifier-click.** Pros: discoverable; works with touch (Android/web); a plain
  click can keep meaning "confirm". Cons: one more control on small thumbnails, next to the existing `↑` and ×.
- **B. Modifier-click only (as in the grid / Unnamed faces).** Pros: consistent with what already exists; no new
  chrome. Cons: hidden on touch; conflicts with plain click = confirm unless click semantics change.
- **C. Plain click selects (as in Unnamed faces), confirming moves to the batch bar.** Pros: one selection model across
  People. Cons: changes today's one-click confirm, which users may rely on.

### 2. Where does the name field live?

- **A. On every card, always.** Pros: one step to retag a single face (the issue's walkthrough). Cons: clutter; many
  text fields in a scrolling strip; harder keyboard focus handling.
- **B. On demand: an "Assign…" action on a card, and in the batch bar.** Pros: clean strip; a single card is just a
  selection of one, so one code path. Cons: one extra click for a single face.

### 3. Enter on a name nobody has yet

- **A. Create the person and assign immediately (undoable).** Pros: fastest; matches the Unnamed faces bar, where a new
  name already just works. Cons: typos make stray people (fixable with Rename / merge).
- **B. Highlight "+ Create '<name>'"; Enter on an exact match assigns, otherwise asks once.** Pros: guards against typos.
  Cons: an extra step, and behaves differently from the Unnamed faces bar.

### 4. What does Reject mean?

- **A. Session-only hide (today's ×, for many).** Pros: no schema change, trivially safe. Cons: the same wrong faces come
  back next session, which is what the issue complains about; not available to CLI/MCP.
- **B. Persistent "not this person".** Pros: suggestions for this person improve and stay clean; the face can still be
  suggested for others. Cons: new catalog table and migration, filtering in every suggestion query, needs a way to see /
  undo rejections beyond Undo.
- **C. Persistent "ignore this face" (not a person worth naming: strangers, posters, false detections).** Pros: also
  cleans Unnamed faces. Cons: a different meaning from "wrong person"; needs an ignored-faces view to restore them.
- **B + C as two actions.** Pros: covers both cases in the issue ("misidentified" vs "invalid detections"). Cons:
  largest scope; two concepts to explain.
