### Grid selection keys, folder renames and smaller fixes
- In Photo Grid and Square Grid, Shift+Up / Shift+Down add every photo up to the cell above or below the active one,
  and Ctrl+Shift (⌘⇧ on macOS) with an arrow extends the selection like Shift with that arrow.
- A date folder inside the library (an import's `Originals/2026-10-16`) can be renamed; the library's own folders
  still can't, and a refused rename now says why.
- Opening another library while the face scan runs starts the scan's progress over instead of showing it nearly done.
- Compact cameras with a built-in lens find their lens profile even when the file names no camera make.
- Lens corrections with Remove Chromatic Aberration keep the crop inside every colour plane, so no edge shows.
- On Windows, a hard link to an original counts as that original.
- A SAM 3 mirror given as a `file://` URL may use `%20`-style escapes in its path.
