### Undo takes back the whole of a large Synchronize Folder
- Edit > Undo after a Synchronize Folder that brought in thousands of photos took back only the last part of it,
  because the history keeps 1,000 steps and had already dropped the run's first ones. The history now keeps a
  synchronize's steps until it ends, folds them into one step, and then applies its usual limit; an edit or Undo
  made while it runs still ends the hold at once.
