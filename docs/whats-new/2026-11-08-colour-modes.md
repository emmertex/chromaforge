### Two colour-grading modes: Eboda and Witchcraft
- Settings ▸ General has a **Grading Mode** row beside Layout (also Window ▸ Grading Mode, Ctrl+Shift+G):
  **Witchcraft**, colour grading the DaVinci Resolve way — now the default — and **Eboda**, the Lightroom
  Classic panels as they always were. Both modes edit the same photo; the switch changes only how the
  tools are laid out. In the Modern layout, Colour ▸ Color Grading follows the mode too (the colourist's
  tools in Witchcraft, Lightroom's 3-way wheels in Eboda).
- Witchcraft's Edit panel runs **Basic, Primaries, HSL Curves** — the grade continues where every photo
  starts — and the two colourist sections carry a small **Witchcraft** pill beside their titles.
- The **Primaries** palette has two sets of colourist wheels, each wheel with its master wheel beneath
  and the same drag holds as the Color Grading wheels (Shift keeps the hue, Cmd/Ctrl the saturation, Alt
  fine-adjusts, double-click resets): the **Color** wheels (Lift, Gamma, Gain, Offset — wide tonal
  overlap) and the **Log** wheels (Shadows, Midtones, Highlights, Offset — narrow ranges you place with
  **Low Range** and **High Range**). Both sets render at once, complimentary as in Resolve — the Color
  wheels establish the range, the Log wheels refine it; the palette's Color | Log tabs just pick which
  set you are editing, and nothing resets when you switch. The Color wheels follow the published ASC CDL
  shapes — lift pivots at white, gain at black, gamma is a power curve — so each has real pull, and the
  Highlight wheel's reach is graded, never a blow-out.
- **HSL Curves**: Hue vs Hue, Hue vs Sat and Hue vs Lum, each a curve you draw yourself — click anywhere to
  add a point, drag it where the edit belongs, double-click a point to remove it, or use the eyedropper
  to pick a colour from the photo and drop its hue onto the curve. The smooth curve through
  your points (neutral at the hue axis's ends) is exactly what renders, on CPU and GPU alike.
- Everything works in Eboda too: the wheels and curves are ordinary develop controls (`primaries.*`,
  `develop.hslCurves`), so presets, history, XMP sidecars, the CLI and agents see one edit like any other.
