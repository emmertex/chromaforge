### Invalid GPS minutes and seconds in XMP are ignored
- An XMP sidecar whose GPS coordinate had 60 or more minutes or seconds placed the photo at a wrong spot on the
  map. Such a coordinate is now treated as missing; the original text is kept in the metadata.
