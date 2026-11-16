### Lightroom history from newer catalogs
- Importing a catalog from Lightroom Classic 13 or later now brings each photo's Lightroom history into its
  History panel. These versions store the history compressed, and the import used to skip every step with a
  warning. A step that can't be read is left out with a warning naming it, and the import goes on.
- The recovery archive the import saves in the library now keeps those steps' settings too. When the archive
  would be too big with them (over 32 MiB), it is saved without them, with a warning, instead of not at all.
