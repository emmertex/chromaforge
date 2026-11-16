### Save or read a whole folder's metadata from a script

- `folder.saveMetadata {path}` and `folder.readMetadata {path}` (CLI, control channel and MCP)
  do what Save Metadata to File and Read Metadata from File do, for every library photo of a
  folder and the folders inside it, with a row in the activity stack. A photo whose file can't
  be written or read is listed in `failed` and the rest go on.
