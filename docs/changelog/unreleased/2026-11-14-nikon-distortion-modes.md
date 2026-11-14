## Fixes
- **Nikon distortion correction set to Off**: Z-body NEFs whose in-camera distortion correction is Off (mode 2), or whose mode is unknown, are no longer warped; only "On (Optional)" and "On (Required)" are. Previews already built for such files keep their old look until they are rebuilt.
