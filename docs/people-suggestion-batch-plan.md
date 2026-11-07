# Plan: batch assign / reject in People (issues #478 and #259)

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

---

# Part 2: group unknown faces and ignore unwanted ones (issue #259)

## The request

After a scan, **Unnamed faces** should show similar faces grouped into stacks (as digiKam's Unknown tab does, used here
only as a behaviour reference). Right-click a stack to **Name…** all of it, or **Ignore** it so the faces leave the list
and stay gone. Any single unwanted or badly detected face should also be removable.

## What exists already

Unnamed faces puts look-alikes *next to each other*: it orders faces by comparing every pair, for up to the first 1,500
embedded faces (`faces.unnamed {limit?}`, "look-alikes together"). That is ordering, not stacks: no group boundaries,
nothing stored, and nothing past the cap is grouped. There is no ignore state; a face only leaves the list by being named.

## Ignore state (stays across rescans)

A rescan (`faces.detect`) finds the same faces again, so whatever is stored must match a re-detected face back to the
ignored one (same photo, boxes overlapping above an IoU threshold, or the same embedding) or the face comes back.

- **A. Catalog only (a flag or table keyed by photo + face box / embedding).** Pros: no change to the user's files; fast
  to query and filter; easy to undo and to list in an "Ignored faces" view; nothing to clash with other tools. Cons: lost
  if the catalog is rebuilt from the files or the photo is imported into another catalog; peer sync must carry it.
- **B. Written to XMP as well.** Pros: survives re-import and travels with the file. Cons: MWG regions have no standard
  "ignored" field, so it needs a private namespace other tools ignore (or might drop or misread); writes to every
  affected sidecar; today naming does not write back to files at all ("Nothing is written back to the files"), so this
  would be a policy change.
- **C. Catalog now, XMP later behind the existing metadata write-back settings.** Pros: ships the behaviour without the
  file-format question. Cons: two steps; the XMP shape still has to be decided later.

Separate sub-question: does Ignore keep the face's detection and embedding (hidden but restorable) or **delete** them
("purge the face signatures")? Keeping them allows an undo and an Ignored view; deleting saves space but a rescan
detects the face again unless a "do not detect here" marker is kept, which is a kept record anyway.

Commands: `faces.ignore {faces: [{photo, index}]}` and `faces.unignore {faces}` (one undo step each), and
`faces.ignored {limit?}` to list. `faces.unnamed`, `faces.person` and `faces.suggest` leave ignored faces out. Same
hostile-input rules and list cap as Part 1.

## Grouping unknown faces

### Algorithm and threshold

- **A. Threshold graph + connected components (single linkage) on cosine similarity, at the model's suggestion bar.**
  Pros: simple, deterministic, reuses the per-model thresholds already tuned (SFace 0.55, AuraFace 0.40). Cons: chaining
  (A~B~C) can merge two different people into one large stack.
- **B. Agglomerative clustering with average or complete linkage, cut at the threshold.** Pros: far less chaining;
  still deterministic. Cons: O(n²) memory/time like today's pairwise pass, so it still needs a cap or blocking.
- **C. Density clustering (DBSCAN / HDBSCAN-style, own implementation).** Pros: leaves odd faces as "singletons" rather
  than forcing them into a stack; copes with uneven group sizes. Cons: more parameters to explain; harder to make
  incremental.
- **Threshold:** fixed at the suggestion bar, a stricter "grouping" bar per model, or a user slider ("tighter / looser
  stacks"). A slider is flexible but one more control; a fixed bar is predictable but some libraries will over- or
  under-group. The `faces.evaluate` data could pick the default per model.

Each option needs its own "Sources:" line (paper or own design) and must not borrow from GPL code.

### On demand or stored

- **On demand (computed when Unnamed faces opens, cached in memory).** Pros: never stale; no schema. Cons: cost grows
  with the library; needs the cap (today 1,500) or an approximate nearest-neighbour index; stacks can reshuffle between
  sessions.
- **Stored (cluster id per face in the catalog, updated incrementally as faces are embedded).** Pros: scales to large
  libraries; stable stacks; queries are cheap. Cons: schema and migration; must be invalidated when the model changes,
  faces are named/ignored, or the threshold changes; incremental assignment drifts from a full recluster.

Either way the work runs in the activity stack (`TaskGuard`, cancellable) when it can take more than a moment.

### Command shape

- **A. New `faces.clusters {threshold?, limit?, minSize?}`** returning stacks (id, faces, a representative face, best
  suggested name). Pros: clean, separately testable, MCP agents get groups directly. Cons: one more command, overlaps
  with `faces.unnamed`.
- **B. Extend `faces.unnamed` with a `group: true` option / a `cluster` field per face.** Pros: one listing; existing
  callers unchanged. Cons: the reply shape gets two modes; harder to page stacks.

## UI: stacks in Unnamed faces

- Each stack is one tile: the representative face with a count badge (and a fan or edge to show it is a stack); the best
  suggested name along the bottom as today. Singletons stay as plain faces after the stacks (or in their own "Others").
- Click expands the stack in place (or opens it like a person page) to review faces; faces can be pulled out of it
  ("Not in this group").
- Right-click a stack (and the selection): **Name…** (the same `TextField` + completion + "+ Create" from Part 1, names
  every face via `faces.nameFaces`), **Ignore** (`faces.ignore`), **Split / Expand**. The menu works on a single face too.
- Selection, Select all and the name bar keep working, with stacks counted as all their faces.
- Order: largest stacks first vs most alike first vs newest photos first (open; largest first suits bulk naming).
- An **Ignored faces** view (People ▸ Ignored, or a filter) to restore faces, if ignore is restorable.

Tests: grouping is deterministic for a fixed embedding set; two clearly separate synthetic identities never share a stack
and near-duplicates do (behaviour, not exact counts); ignored faces stay out after a rescan of the same photo; undo of
Name / Ignore restores every face; hostile inputs to the new commands; headless snapshots of stacks and the menu.

## How Part 1's Reject and Part 2's Ignore fit together

Part 1 question 4 asks what **Reject** means on a person's page. Part 2 gives a persistent **Ignore**. Options:

- **A. One concept: Reject = Ignore.** A rejected suggestion is ignored everywhere. Pros: one stored state, one command,
  one Ignored view. Cons: a real person wrongly suggested for Jane disappears from Unnamed faces too, so they can never be
  named.
- **B. Two concepts: Reject = "not this person" (per person, Part 1 option B) and Ignore = "not worth naming" (global).**
  Both on the person page's batch bar and context menu. Pros: matches the two cases both issues describe
  (misidentified vs junk detections). Cons: two stores, two commands, and the UI must explain the difference.
- **C. Reject stays session-only; Ignore is the only persistent action.** Pros: smallest; Ignore covers junk. Cons:
  wrong-person suggestions keep coming back (#478's complaint).

Whichever is picked, "not this person" pairs can also be used as cannot-link constraints when grouping (Part 2), so a
stack never merges faces the user has said are different people; that is optional and adds work.

Suggested order if both are built: ignore state + commands, then Part 1 batch bar, then grouping, then the stack UI.
