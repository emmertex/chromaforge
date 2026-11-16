### Windows zip copies update themselves
- A copy unpacked from the Windows portable zip, with or without `portable.txt`, now installs an update by itself:
  the new zip is unpacked beside the program, its files replace the copy's own, and the new version starts. Before,
  the zip was downloaded and its folder shown for you to unpack by hand.
- Only the zip's files are replaced: `portable.txt`, the portable data folder and anything else you keep there stay
  as they are. If any file can't be replaced, or the new version can't start, the previous files are put back.
- The new version starts once the app has quit, however long that takes. The previous files are kept until the new
  version is up; if it doesn't come up, its next start puts the previous version back.
- A copy without `portable.txt` no longer gets the MSI installer, which installed the app somewhere else and left
  this copy on the old version. Windows Installer is asked whether it installed the running program; only a copy
  it installed updates through the MSI.
