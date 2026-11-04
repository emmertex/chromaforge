## Develop

- **Lightroom Classic's brush model:** Size is the core, Feather a halo outside it (up to 2.5 × Size), dabs `exp(−(r/a)^p)` black-box-fitted to Lightroom exports, overlapping strokes add up; Lightroom `Mask/Paint` import / XMP preset export in Lightroom's terms; Auto Mask with Lightroom's measured tolerances and an Auto Mask range slider kept with each stroke (`autoRange`). CPU, GPU and the live stroke agree; older strokes keep the old model. Thanks @OneSlips. (no PR)
