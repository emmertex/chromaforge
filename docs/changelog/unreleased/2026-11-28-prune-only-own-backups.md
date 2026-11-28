## Library

- **Backup pruning deletes only this library's backups:** pruning removed, whole and not to the trash, every folder in the backup folder that held a catalog store, oldest name first: another library's backups in a shared backup folder, or a library kept there. A backup now records its library's id, pruning and the "restore from" hint consider only that library's backups (older ones without an id only in the library's own `backups` folder), and two backups started in the same second get folders of their own.
