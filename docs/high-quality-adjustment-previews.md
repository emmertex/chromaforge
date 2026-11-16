# High quality adjustment previews

Settings ▸ Performance ▸ **High quality adjustment previews** keeps the Detail view at its settled
rendering quality while a slider is dragged, instead of the faster adaptive drafts. It applies at
once (also in the middle of a drag), is saved as an application preference (`highQualityAdjustmentPreviews`
in `ui.set` / the settings file) and is off by default. Contributed by Kaan88 (fork PR #426).

## What it renders

With the setting on, a drag renders what the view shows at rest:

- **Fit:** the same render size, full-quality processing and Lanczos display fit as the settled
  view, so the picture does not soften while the slider moves and does not change when it is let go.
  Neither does the histogram: the Histogram panel shows each update's own, HDR bins included for an
  HDR edit, which is what the released view keeps (`panels::edit::shown_histogram`).
- **Zoomed in (100 % and above):** the existing zoomed-window renderer, one window of the visible
  part per pane at native resolution with the filter context around it, not the whole original per
  update. The window reuses its stage cache through the drag.
- **Before/After, the HDR view, Compare / Reference zoomed panes and the second window** follow the
  same choice. HDR edits stay on the float HDR display path throughout (fork PR #423). A drag
  shown in HDR renders no 8-bit frames, so the Histogram panel shows a small live render beside it
  and keeps that until the released view's render lands (it showed the histogram from before the
  drag for a frame or two, with the setting on or off).

The **Preview size** preference, the GPU's texture limit, source availability and colour management
still apply. Exports, undo history and the HDR output mode are unchanged. Updates are still
coalesced: a newer value replaces queued ones, an older result never replaces a newer one on screen,
and the last finished image stays visible until the next is ready.

With the setting off, the adaptive drafts remain: up to 60 % of the width and height on the CPU (or
after slow drags), cheaper processing and no display fit (see `region::DragScale`).

## When full quality is too slow

A heavy edit (many masks, AI denoise, the CPU path, a 6K display) can take hundreds of milliseconds
per full-quality update, which leaves the slider dragging a frozen picture. So a drag falls back
(`region::HqDrag`):

1. Every drag starts at full quality. Its updates are timed as their render plus the frame that
   shows them, leaving out the first (which may rebuild stage caches). What is timed is what shows
   the drag: the zoomed window when there is one, otherwise the whole frame.
2. Once the median is over **100 ms** (fewer than ten updates a second), or one update is over
   **300 ms**, the drag falls back for the rest of the drag.
3. In the fallback, while the slider **moves**, the view renders the adaptive drafts the setting
   otherwise replaces. Once the slider **holds still for 0.2 s**, mouse button still down, it
   renders full quality again, so the picture is sharp whenever you stop to look. Moving again
   drafts again. Letting go renders full quality as always.
4. A slider that fell back on a photo starts its next drags on that photo in the fallback (no
   frozen updates to find out again), for 30 s; then a drag tries full quality again. A retry that
   is still slow doubles the wait (up to 4 min); one that keeps up forgets it. Other sliders and
   other photos are timed afresh.

`ui.inspect` → `perf.adjustQuality` reports the decision: `idle` (no drag, or the setting is off),
`full`, `draft` (falling back, slider moving) or `settled` (falling back, slider still).

## Measurements

Measured on 2026-10-24 and 2026-10-25, before main's 26.44 changes; see the pull request for the A/B
against main of that time.

Kaan88's warmed measurements at Fit in a 1600 × 1000 window on an RTX 4070 Ti SUPER (DX12), HDR
output off. Latency includes control-channel dispatch; update rates are Main-render completions
during a burst, not compositor FPS. Local measurements, not guarantees.

| File | Adaptive latency | High quality latency | Adaptive updates/s | High quality updates/s |
|---|---:|---:|---:|---:|
| CHDK DNG, 3264 × 2448 | 35 ms | 70 ms | 40.5 | 21.4 |
| JPEG, 3264 × 2448 | 35 ms | 70 ms | 40.6 | 21.1 |
| Sony ARW, 6000 × 4000 | 35 ms | 85 ms | 28.6 | 18.4 |

These stay at full quality: they are inside the 100 ms budget. With high quality on, the pixels of
the last update and of the release matched exactly for all three files at Fit and 100 %.

2026-10-24, same machine with an HDR display, eight raws one per camera (24–102 MP), an exposure
drag at 60 Hz, medians: the display fit now reads and writes bytes (`lcc_raster::resample::fit_rgba8`,
same pixels, ~30 → ~7 ms at 2560 px), so a high quality update renders in 21 ms instead of 35 and a
drag gets 46.5 updates/s instead of 27.7 (adaptive: 60.4). One update on its own still takes about
75 ms to show: the loupe draws an update in the frame it was asked for only when it renders within
40 % of a frame, which a preview-level render and its fit don't.

With the display fit on the GPU (`lcc_gpu::try_render_fitted`, same session, back-to-back A/B on
three of the raws): a high quality update renders in 4.9 ms instead of 21.9 and a drag gets 58.1
updates/s (adaptive: 60.7); one update on its own still takes ~63 ms (adaptive: 35). HDR edits
shown in HDR are unchanged (their HDR view has no display fit).

A full-quality update now gets 60 % of a frame to finish in and be drawn in the frame that asked
for it (`panels::detail::wait_budget`; drafts keep 40 %): one update on its own 48.7 → 41.6 ms
(median; half of them now take ~35 ms, as drafts do), the slowest UI frame of a drag 1.8 → 6.9 ms.

An HDR edit's full-quality updates render its HDR histogram a second time, at 384 px
(`pipeline::hdr_histogram`); that render now reuses the view's stage cache
(`hdr_histogram_cached`, the same histogram): ~15 → ~8.5 ms. With HDR Preview off, a drag gets
49.5 instead of 30.9 updates/s, one update 76.7 → 66.3 ms.

## Tests

`crates/ui-egui/src/tests_adjustment_quality.rs` (headless, procedural photo): preference round
trip and default, the settings control, switching during a drag, 4:3 and 3:2 photos at 100–200 %
display scaling, preview and texture limits, proofed CPU rendering, every Before/After layout,
native-resolution windows instead of tiles, HDR float output, the histogram unchanged on release
(SDR and HDR edits) and an HDR view's release never showing the histogram from before its drag,
and the fallback with renders that
report as slow (whole frame and zoomed window: drafts while moving, settled render while held
still, the next drag starting in the fallback) or fast (never falls back). `region.rs` unit tests
cover `HqDrag` itself, including hostile clocks and timings.

Sources: own design, on the existing render scheduling (`DragScale`'s timing and verdicts).

## The view preview and the first zoom

The photo's view preview (the size-independent view cache, in memory and on disk with a library) keeps
the render at the level it was made, not the size it is drawn. A render at rest that the GPU fits to the
drawn size reads its full image back too, for that cache (about 3 ms at 2560 px). A high quality drag's
updates don't: each would pay those 3 ms, and in HDR they held updates back by a frame, so a drag's last
update caches its drawn size until the next render at rest replaces it (`RenderJob::drawn_view_cache`).
The GPU fit matches the CPU's within one output level.

Preparing the original after a look is opportunistic: it waits for 1.5 s with nothing else to render,
and a 100 MP raw can still be decoding when the zoom comes, which then waits for it as before (on the
GFX100S one run measured no gain, a later one 6.8 → 4.4 s). What a zoom shows first is the tiles' work; the ring of tiles
around the view finishes later.
