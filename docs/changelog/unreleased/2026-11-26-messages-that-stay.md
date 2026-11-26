## Interface
- **Messages that stay:** a message about a condition that is still true (unsaved changes, XMP sidecars waiting, a pairing request) stays until dismissed or the condition ends. `ui.toast` raises and clears one by key; `ui.inspect` lists it as `kind: "sticky"` with its `key`.
