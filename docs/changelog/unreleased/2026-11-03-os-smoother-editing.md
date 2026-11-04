## Performance

- **Smoother editing:** a still photo refines to the full render while a slider or control-surface gesture stays open; drafts no longer wait for a photo's first full render (one shared raw decode); masks keep their planes between slider ticks; zoomed brushed masks render in ~150 ms instead of seconds; presets, catalog flags and GPU uploads never block the window; renders leave cores free at below-normal priority. Thanks @OneSlips. (no PR)
