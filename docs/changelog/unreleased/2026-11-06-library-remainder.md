## Library

- **Grid thumbnails follow a drag on every selected photo:** with several photos selected, each one's thumbnail refreshes while a slider is dragged, not only the active photo's.
- **Memory for caches fits the computer:** the Settings choices go past 4 GB, up to the most the computer allows (`memory::max_budget`), and Automatic shows the size it picks.
- **No backup prompt for an unchanged catalog:** the quit backup prompt (and the backup on exit) is skipped when the catalog and its settings are the same as at the last backup.
- **Finishing an import survives a restart:** the photos and previews `library.finishImport` was asked for are kept in the library (`import-finishing.json`) until done; a library opened with an unfinished plan takes it up again. A finish the user stops is not resumed.
