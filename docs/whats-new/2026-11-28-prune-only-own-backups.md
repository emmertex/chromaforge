### Pruning old backups touches only this library's backups
- Each backup now records which library it belongs to, and pruning old backups deletes only that library's own. Another library's backups in a shared backup folder, or a library or any other folder kept there, are left alone (before, one could be deleted whole).
