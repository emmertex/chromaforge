### Smoother slider drags on HDR edits and heavy photos
- Dragging a slider on an HDR edit no longer runs two extra full-quality renders per update for the live HDR histogram: the histogram now renders at draft grade, like the picture beside it.
- With "High quality adjustment previews" on, a heavy photo falls back to drafts after at most two slow full-quality updates, and a photo already known to be slow under one slider starts its other sliders' drags in the fallback instead of freezing again to find out.
