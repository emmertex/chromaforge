## Performance

- **Large libraries open much faster from network shares and hard disks:** the catalog store is read front to back in one pass and opened from memory, instead of one 4 KiB page at a time in key order (a round trip or a seek each: nearly 8 minutes for 328,000 photos in #979). Stores over 4 GiB, or that don't fit in memory, are read as before. Thanks @GregDunn1953 and @1234destiny1234 for the logs.
