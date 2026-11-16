# lcc-camprof

Camera profiling from a ColorChecker photograph (layer L1): find the chart in a raw frame, measure its 24
patches, fit a forward matrix, a colour matrix and a hue/saturation correction table, and express them as a
DNG Camera Profile (`lcc_raw::dcp::Dcp`, written by `lcc_raw::dcp::write`). Two chart photos under lights far
enough apart (Daylight / Flash and Tungsten) give a dual-illuminant profile (`build_dual`).

- `find_chart` locates the chart by its colours; `refine` snaps hand-placed corners onto it; `orient` picks
  the rotation.
- `build` / `build_dual` return the profile plus a per-patch CIEDE2000 report.
- Hostile frames, corners and options give `CamprofError` or `None`, never a panic (`tests/it/hostile.rs`).

The app uses it through the `profile.findChart` and `profile.createFromChart` commands
(`crates/engine/src/chart_profiling.rs`) and the Create Camera Profile dialog.

## Provenance

The engine of RADcamProfiles by RAD Films - Roman Alurkoff (@OneSlips), MIT, relicensed MIT OR Apache-2.0 by
its author. **Clean-room, own work, confirmed by the author 2026-11-03**: no DCamProf or other GPL code was
read or used. Its data is public or the author's own:

- CIE 1931 2° colour-matching functions and CIE standard illuminants A, D50, D55, D65, D75, E
  (CIE 015:2018 / ISO/CIE 11664-1 and -2 published tables), `src/data/`.
- ColorChecker Classic reference values (CIELAB D50, pre-November 2014), published by X-Rite,
  `src/reference.rs`.

No Adobe file, matrix or pixel is used. Licence: MIT OR Apache-2.0. See `NOTICE` and `assets/ATTRIBUTION.md`.
