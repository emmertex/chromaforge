## Develop

- **Generative Fill (bring your own key):** Remove ▸ Generative fills a painted area through OpenAI, Stability AI, Black Forest Labs or fal.ai with the user's own API key (opt-in, consent dialog showing the uploaded crop and cost); results are stored as patches of the photo's own pixels and never lost. New crate `lcc-genfill`. Thanks @OneSlips. (no PR)
- **Remove tools:** content-aware Fill with grain, seamless edges and source exclusion (`lcc-inpaint` structure pass); Remove rebuilds brushed strokes; per-tool tooltips; the panel remembers its tool. Thanks @OneSlips. (no PR)
- **Sensor defects:** hot, dead and stuck pixels fixed on the raw mosaic with per-body maps; Remove Dust with dust maps, Even Out spots, batch and on-import runs. Thanks @OneSlips. (no PR)
