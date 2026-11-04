### Masks that follow the people on each photo
- A People mask can name who it is about: a person ("Alex") or a role in the shoot ("Bride", set per folder, album,
  dates or photos with `people.setRole`). Presets, Paste and Sync find that person on each photo from its named
  faces; with nobody in the role there, the mask covers everyone and says so. Gradients can be anchored to the
  people or one person, and move with them.
- `masks.recompute` makes Object and Describe selections again for the photo they are on, so pasted or preset
  masks never carry another photo's pixels. Thanks @OneSlips.
