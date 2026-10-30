## Performance

- **Delete from Disk lists each folder once:** deleting hundreds of photos from one folder took half an hour or more, the folder being listed and every entry stat-ed again for each photo's sidecar lookup. One deletion now lists each folder once and answers the sidecar and shared-stem questions from that listing. Fork #692. (no PR)
