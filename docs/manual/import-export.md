# Import and export

> Names in angle brackets are the values set in [`brand.toml`](../../brand.toml); see the [manual index](README.md).

## Import

File → Import Photos… (`Cmd+Shift+I`) or Import from Folder… (`library.import`):

- **Add** references the files where they are; **Copy** copies them into the library's `Originals/` (or a destination
  you pick); **Move** copies, verifies the copy byte for byte, then removes the source and its sidecars. Failed,
  duplicate and unverified files keep their sources.
- **Organise** copies by date (`YYYY/YYYY-MM-DD`), month, flat, or a folder template such as `{date:%Y}/{date:%Y%m%d}`.
- **Rename** with tokens (`{name}`, `{seq:4}`, `{date:%Y%m%d}`, `{camera}`, `{lens}` …; `photo.renameTokens` explains
  each), optionally convert raws to DNG while copying. A raw Copy as DNG couldn't convert (Cancel while it waited,
  memory held by a drive that stopped responding, a failed conversion) is imported as its raw copy and listed by
  reason in File → More Imports → Import Warnings…; Convert Photo to DNG… converts it later.
- Apply a develop preset, a metadata preset, keywords, and add the photos to a collection.
- Files on several drives, cards or shares are read side by side (per drive, never more readers than before on any one
  drive). A drive that stops answering for about 30 s (longer after it was idle or the computer woke) has its files
  reported as "volume not responding" while the others are read; Cancel returns even while a drive hangs; a file that
  crashes the reader fails alone.
- During Copy or Move, a card or destination drive that stops answering stops only its own copies: those not yet
  started fail "volume not responding", one under way is reported "outcome unknown" (it may still finish, leaving a
  file in the destination folder that the library doesn't list; the original is always kept). The message at the end
  counts these apart from unreadable files ("not imported: a drive stopped responding"; "copies left unfinished",
  with the folder), and a batch the library couldn't add (a failed save) as "not added", with the reason;
  `library.import` lists such files in `interrupted` (`why`: `volumeNotResponding` or `outcomeUnknown`). Cancel
  returns even while a drive hangs; a raw waiting for Copy as DNG when you cancel is kept as its raw copy.
- Duplicates are detected and reported; a file in Recently Deleted can be skipped, restored with its edits, or
  imported fresh. Library → Find Duplicates… later finds copies already in the library ([duplicates.md](../duplicates.md)).
- Plug-ins can join the import ([plugins.md](../plugins.md)): a before-import hook can replace a file as it is
  prepared (a transcoded twin, say) or skip it, and an after-import hook is told about each imported batch.
- Auto Import (Settings → Import) can watch subfolders, rename copies and apply a develop preset, metadata preset and
  keywords to arriving photos (`library.autoImport`). Photos in subfolders of the watched folder count only with
  Include subfolders on; a file that couldn't be read when tried (a copy still arriving) is tried again once it changes. On Android the camera roll is added in place and is a watched source (the review's Keep adding new
  photos from the camera roll, or Settings → Folders, Watch for new photos).
- **Browse** (the Import tab, Settings → Import → Import as a tab): the bar's **Import** button imports the picks, the
  selection or all; a folder browsed for the first time includes its sub-folders (**Include subfolders** in the bar
  re-browses without them; re-browsing the same folder keeps the choice); **Develop** opens the selected photo without
  importing it. Develop, Print, Export and Slideshow
  chosen while browsing (module picker, shortcuts, File → Export…) work on the browsed selection the same way — export
  writes it with its current edits, print takes it as the only photo — and stay there until you go back to Library;
  a selected file the library already has opens as its library photo. Map, Book and Web switch to the library.
- File → Email Photo… exports the selection as small JPEGs and attaches them to a new message in the system mail program.

Other sources: File → Import Lightroom Catalog… ([migration guide](migrating-from-lightroom.md)), File → Import
from Immich… ([immich.md](../immich.md)), File → More Imports → Import from Another Catalog… ([catalogs.md](catalogs.md)), and
tethered capture ([tethering.md](tethering.md)). File → Import Profiles & Presets… reads presets (including
Lightroom `.xmp` presets); Import Keywords… reads keyword lists.

Supported files: JPEG, PNG, TIFF, WebP, AVIF/HEIF (also on Android), PSD, DNG and the common raw formats (Sony ARW, Nikon NEF and NEFX, Canon
CR2/CR3, Fujifilm RAF, Panasonic RW2, Olympus ORF, Pentax PEF, Samsung SRW and more). Per-model status is in the
[parity tracker](../parity-checklist.md) (`LR-IMP-CAMERA-COVERAGE`). Images over 512 megapixels are refused; the error
gives the image's size in megapixels and the limit.

## Export

File → Export… opens the export dialog; File → Export with Previous (`Cmd+Alt+Shift+E`) repeats the last one.

- Formats: JPEG, PNG, TIFF (8/16/32-bit, LZW/ZIP), WebP (lossy at a quality, or lossless; lossless only on web and Android), AVIF, OpenEXR (scene-linear 16-bit half or 32-bit float, linear Rec. 2020 or ACEScg), DNG, or the original file with an XMP sidecar.
  HDR edits can also export as gain-map JPEGs, PQ AVIF, PNG and JPEG XL, as float TIFFs, and (AVIF and JPEG XL with HDR mode Gain map) as an SDR image with a gain map.
- Size by long/short edge, width/height, megapixels or percent; resolution in ppi; JPEG quality or a file size limit.
- Colour space sRGB, Display P3, Adobe RGB, ProPhoto RGB or Rec. 2020.
- Output sharpening (screen, matte, glossy), metadata (all, all except camera info, copyright only, none), location
  removal, file naming and subfolders, what to do with existing files.
- Watermarks: text (also vertical, for CJK) or a graphic, with size, opacity, anchor and inset.
- Save the settings as an export preset (`export.savePreset`); plug-ins can post-process exported files.
- File → More Exports → Export with Several Presets… exports the selection with several presets in one run, reading
  each photo once (`export.runPresets {presetIds}`).
- File → Contact Sheet PDF… writes a quick contact sheet; the Print module does full layouts.

Export never overwrites an original or its sidecar (`export.checkTarget`). A subfolder or destination that escapes with `..`, or uses a Windows-reserved name, is refused. The Export screen previews the first photo as its file will be (SDR, or in HDR on an HDR display with its SDR | HDR choice); entering Export or Print from Develop selects the photo on screen. Print takes the single selected photo; only an empty selection prints the whole filmstrip.

## From the command line

The same encoder as the dialog:

```sh
<cli> render IMG_0001.CR3 -o out.jpg --set light.exposure=0.3 --preset <id> --size 2048 --quality 90
<cli> render in.ARW -o out.tif --opt colorSpace=proPhoto --opt bitDepth=16
<cli> run --library ~/Pictures/Lib library.import paths='["/media/card/DCIM"]' mode=copy \
    app.export dir=/tmp/out preset="<export preset name>"
```

`library.import`'s result includes `skipped: {unreadable, unsupported}`: folders its walk could not read and files in
the folders that are not a supported photo type (XMP sidecars not counted), as `library.importPreview` reports them.
A file named on its own is always tried, so it shows under `failed`, never under `skipped`.
