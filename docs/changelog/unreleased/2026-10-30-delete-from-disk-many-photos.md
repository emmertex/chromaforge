## Performance

- **Delete from Disk with many photos:** the sidecar lookup listed and stat-ed the folder again for every photo and compared every entry with every file of the run, so deleting hundreds of photos took tens of minutes on Windows. Each folder is now listed once per run. Thanks @geneing. Fork #692. (no PR)
