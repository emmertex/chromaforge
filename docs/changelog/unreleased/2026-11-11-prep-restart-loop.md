## Fixes

- **Background preparation stuck on "Stopping…":** a photo on screen that the build leaves out of its view (unmarked with "Picks, rated and labelled", or hidden by the filter) stopped the build as if the user had moved on, and the same build was started and stopped again every frame, so the loupe and the filmstrip never rendered (regression in 26.46.1).
