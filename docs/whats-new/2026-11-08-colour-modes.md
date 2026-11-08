### Two colour-grading modes: Eboda and Witchcraft
- Settings ▸ General has a **Grading Mode** row beside Layout (also Window ▸ Grading Mode, Ctrl+Shift+G): **Eboda**,
  the Lightroom Classic panels as they always were, and **Witchcraft**, colour grading the DaVinci Resolve way. Both
  modes edit the same photo — the switch changes only how the tools are laid out.
- **Witchcraft** leads with a strip of **nodes** — the grade's stages (Balance, Primaries, HSL Curves, Curve, Colour,
  Grade, each of the photo's masks, Effects, Output). Click a node to open its panel; click its number to bypass that
  stage, as a colourist disables a node.
- The **Primaries** palette: four colourist wheels — **Lift, Gamma, Gain, Offset**, each with its master wheel beneath
  — with the same drag holds as the Color Grading wheels (Shift keeps the hue, Cmd/Ctrl the saturation, Alt
  fine-adjusts, double-click resets). The palette's own switch picks the variant: **Color** wheels (wide tonal
  overlap) or **Log** wheels (Shadows / Midtones / Highlights with **Low Range** and **High Range** boundaries).
- **HSL Curves**: Hue vs Hue, Hue vs Sat and Hue vs Lum, drawn as curves over the colour mixer's bands — drag the
  curve, the band under the pointer follows. A view of the Color Mixer's own controls, so the two always agree.
- Everything works in Eboda too: the wheels, curves and scopes are ordinary develop controls (`primaries.*`,
  `develop.primaries`), so presets, history, XMP sidecars, the CLI and agents see one edit like any other.
