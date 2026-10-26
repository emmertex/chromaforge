### Sidecar keywords, album commands and lens profiles
- XMP sidecars write keyword names in `dc:subject` and keyword paths in `lr:hierarchicalSubject`, as other apps expect; older sidecars with paths in `dc:subject` still read as before.
- Album commands skip photos the library no longer has, and a command that adds nothing records no undo step.
- Replacing the selection with an id given twice keeps that photo once.
- Lens profiles that don't state a crop factor are kept as unknown instead of taken as full frame.
