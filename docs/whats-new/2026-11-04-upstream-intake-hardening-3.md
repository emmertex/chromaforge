### More hardening and small fixes
- On Windows, the title bar takes the app's colours as soon as the window has focus, so its title is no longer unreadable when the app's theme differs from Windows'.
- The CLI's `calibrate` walks folders with the bounded walk, so folders of links pointing back at their parents no longer take forever.
- Preset archives share one unpack budget across the packs inside them, brush masks have one stamp budget in all, and Defringe is faster on large radii.
- Raw files with absurd preview tables, shared tile payloads or opcode lists are refused or capped instead of using gigabytes of memory.
- The web app caps its render workers and plans a library restore before writing; `web --serve` only serves plain files inside the site.
- Exporting to one explicit file path now follows the Conflict setting (unique by default, so an existing file is no longer replaced unless you ask): sidecars keep the chosen name, and a folder or a catalogued original as the path is refused.
- A JPEG that carries its develop settings in embedded XMP (and has no sidecar) now imports with them; Find Similar decodes the library in parallel; very long, narrow images resize without using gigabytes; and Lightroom catalogs with huge XMP packets or keyword lists are read within limits.
