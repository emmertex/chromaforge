### XMP sidecars are written in the background
- With automatic XMP writing on, the desktop app writes sidecars on a thread of its own, so edits on a network share
  no longer wait for the write. Several changes before a write starts become one write, and queued writes finish
  before a file moves, metadata is read or saved, or the library closes.
