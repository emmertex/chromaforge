---
title: Glossary
description: Chromaforge terms in a line each.
sidebar:
  order: 4
---

- **Activity stack**: the one place long jobs (export, import, previews, AI) show a row with progress and a cancel button. Classic shows it in the module bar; agents read it with `activity.list`.
- **Action**: a recorded, replayable sequence of commands. See [Actions](../../automate/actions/).
- **Command**: a named operation with an id; everything the app does. See [Commands](../../automate/commands/).
- **Control**: one Develop slider id such as `light.exposure`.
- **Control channel**: the local JSON-lines port that drives a running app.
- **Control surface**: hardware with dials and keys (MX Creative Console, Loupedeck+).
- **Eboda**: the grading layout with Lightroom Classic-style panels (3-way Color Grading).
- **Witchcraft**: the grading layout with colourist tools (Primaries wheels, HSL curves). Both share one recipe; switch in **Window ▸ Grading Mode**.
- **Recipe**: a photo's edit settings. Non-destructive; the original is never changed. Presets, history, XMP and scopes work on the recipe.
- **Collection / Smart Collection**: a manual or rule-based group of photos.
- **Quick Collection / Target Collection**: the scratch collection and the one `B` adds to.
- **Module**: Library, Develop, Map, Book, Slideshow, Print, Web, Export.
- **Layout**: Classic, Modern or Compact presentation of the same app.
- **Preview-only**: a raw whose sensor data cannot be decoded yet, shown from its embedded JPEG.
- **Smart preview**: a small stand-in for editing when the original is away.
- **Plug-in**: a sandboxed WebAssembly add-on. See [Plug-ins](../../automate/plugins/).
- **Virtual copy / version / snapshot**: another recipe for the same file, or a saved state of it.
- **Library**: the catalog folder holding photos' records, previews and edits. One program has it open at a time.
