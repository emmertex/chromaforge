### Fewer idle freezes on large libraries

- The window no longer stops responding while the library's catalog is compacted: **Optimize Library** now writes its
  snapshot on a worker thread, like the automatic compaction always did (issue #448). On a library of tens of
  thousands of photos this froze the window for tens of seconds at a time.
- Files that can't be decoded (an unsupported raw compression, a broken JPEG) are remembered as unreadable instead of
  being decoded again on every visit to their cell: fewer repeated work and repeated warnings while you scroll a large
  library (issue #448). Relinking the file or editing the photo tries again.
- The log now records when a catalog compaction starts and finishes, and how long each stage took, so a freeze can be
  matched against the catalog's own writes.
