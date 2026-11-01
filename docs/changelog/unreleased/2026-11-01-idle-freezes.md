## Performance

- **No more idle freezes while the catalog is compacted (fork #448):** *Optimize Library* writes its snapshot on a
  worker thread now, like the automatic compaction; on a 28 000-photo library it froze the window for tens of seconds
  at a time. Files that can't be decoded (an unsupported raw compression, a broken JPEG) are negatively cached, so
  visiting their cell no longer decodes them again and again. The log records when a compaction starts and lands, and
  each stage's timings. (no PR)
