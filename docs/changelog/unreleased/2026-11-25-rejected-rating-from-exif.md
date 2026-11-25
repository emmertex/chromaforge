## Fixes

- An Exif rating of 65535 (rejected) reads as rejected, not as five stars (from craft-libs #967, upstream #1995).
- Lightroom XMP: a `rdf:parseType="Resource"` struct written with attribute fields reads like the other forms (from craft-libs #967, upstream #1226).
