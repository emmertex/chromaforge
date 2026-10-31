### Delete from Disk lists each folder once
- Deleting hundreds of photos from disk no longer takes minutes to hours (fork issue #692). The app read the folder
  again for every photo to find its sidecars, and asked the file system about every entry each time. It now lists each
  folder once for the whole deletion and answers from that listing.
