### The control channel refuses pointer positions that aren't on any screen
- `ui.click`, `ui.move`, `ui.drag`, `ui.dragWidget`, `ui.pointer` and `ui.scroll` with a coordinate or delta far past
  any screen (such as `1e308`) used to queue an infinite or NaN position. A debug build panicked on it and a release
  build handed it to every widget. Numbers outside ±1,000,000 are now an error, and nothing is queued.
