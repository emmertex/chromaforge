### Safer files, downloads and remote workers
- Imported profiles are read and deleted only from the library's own `Profiles/` folder, and capped in size.
- Library files, the thumbnail cache and sidecar backups are never written, cut or read through a link planted at their name.
- Develop settings from a sidecar, preset or agent keep every list (masks, strokes, colour samples, spots, red eye) within a limit.
- The remote SAM 3 worker and the denoise server check a shared token before any pixels move, and slow or wrong clients can't hold their connections.
- A download's token goes only to the host and port it was set for; files without a pinned checksum come over https only.
- Export file names are valid on Windows too (forbidden characters, trailing dots, reserved device names).
- An edit with a value that can't be saved is refused instead of leaving a library that won't open.
