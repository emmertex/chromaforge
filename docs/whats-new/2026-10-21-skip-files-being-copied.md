### Files still being copied wait for the next scan

- Synchronize Folder, folder scans and adding photos where they are now leave out files that are still being copied into the folder, and pick them up on a later scan once they are complete. Temporary names (such as rsync's hidden `.name.XXXXXX` files and `.part` downloads) are skipped, a file another program still has open for writing is skipped on network shares (macOS) and on Windows, and a raw whose header says its data goes past the end of the file is treated as unfinished.
