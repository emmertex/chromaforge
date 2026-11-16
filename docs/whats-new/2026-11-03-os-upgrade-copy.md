### Library upgrades keep a copy first
- Before the app upgrades a library made by an older version to its newer catalog format, it copies the library's
  catalog to `backups/before-v<N> <time>` inside the library and checks the copy byte for byte. Only then is the
  library upgraded. A notice tells you what was upgraded, where the copy is and how to go back: the older version
  opens that folder.
- If the disk fills up, the app crashes or the catalog turns out to be damaged during the upgrade, the library is
  left exactly as the older version left it, and the message says what to do (for example, free some disk space).
  Thanks @OneSlips.
