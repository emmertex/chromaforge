### Stricter GPS entry, virtual-copy numbering and location-free Original export
- Typed GPS coordinates must be a complete decimal or degrees/minutes/seconds value: unknown text, exponents, wrong-axis or conflicting hemispheres and minutes or seconds of 60 or more are rejected instead of being reinterpreted.
- Creating numbered virtual copies reports when the `Copy N` sequence is exhausted, before any copy in the batch is made; an explicit name still works.
- With Remove Location Info on, an Original export is also refused when the file embeds a sublocation, city, state or country.
