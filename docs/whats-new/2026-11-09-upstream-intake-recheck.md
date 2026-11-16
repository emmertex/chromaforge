### Reload, Edit In and header fixes
- Reload refreshes a virtual copy together with its original and its siblings only while they use the same file; a copy relinked to another file keeps its own, and is rechecked on its own after an app update.
- Edit In on a photo whose last copy was never changed opens that copy again (back on top of its stack, even if it was in Recently Deleted or hidden under the original) and never deletes the file it hands to the editor; an unrelated photo that happens to hold the same bytes is not used.
- The library header shows "1 selected" for a single selected photo (as for several), and a long count is cut with the full text on hover instead of losing its start.
- An export subfolder name Windows would refuse (`CON`, `NUL`, `COM1`, `*`, `?`, `"`, `<`, `>`, `|`, a trailing dot) is refused up front on every system.
- `topFaces` on suggestions and recognition checks is refused unless it is 1 to 8 (or null for the Settings choice), and a person with thousands of faces is ranked without sorting them all.
