### Fewer false warnings when importing Lightroom sidecars
- Importing photos with Lightroom XMP sidecars no longer warns about settings that change nothing: the full-frame
  crop values Lightroom writes when there is no crop, Point Color's variance when no Point Color slot is used,
  profile and lens digests, version stamps, the crop's aspect lock, and a saved Upright analysis with Upright Off.
  On about 19,000 photos edited in Lightroom the warnings went from 3,025 to a handful of photos with settings that
  really don't carry over, plus the photos using a camera profile the app doesn't have.
