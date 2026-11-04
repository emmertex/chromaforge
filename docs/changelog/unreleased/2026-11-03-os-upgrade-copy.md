## Library

- **Copy-first catalog upgrades:** before a catalog format upgrade (the v3 → v4 migration, or a store marked with a newer format), the library's catalog files are copied to `backups/before-v<N> <time>/` and verified byte for byte, through a partial folder renamed only when complete; a full disk, crash or damaged catalog leaves the library as the older version left it, with an actionable error. A notice and `library.info` → `load.upgradeBackup` say where the copy is and how to go back. Thanks @OneSlips. (no PR)
