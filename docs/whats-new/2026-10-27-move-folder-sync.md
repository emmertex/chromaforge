### Move keeps the original until the copy is on disk
- Import ▸ Move, and a move to another volume, now write the copy's folder entry to disk before removing the
  original, and keep the original when that fails (upstream issue 1501). The copy's contents were already synced,
  but not its name in the folder, so a crash right after the move could keep the deletion and lose the copy.
