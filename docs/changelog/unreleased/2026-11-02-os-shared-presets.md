## Presets

- **Shared presets:** develop, local adjustment, export and other presets, LUT profiles, import defaults and slideshow settings live in the settings folder's `Library Store/`, shared by every library; each library merges in once, read only, with profile ids remapped (`store.info`, `store.mergeLibraries`, `store.gatherPresets`, `store.useLibrary`, `cli run --shared-settings`); At Import / hidden marks move from `presets.json` into the store. Thanks @OneSlips. (no PR)
