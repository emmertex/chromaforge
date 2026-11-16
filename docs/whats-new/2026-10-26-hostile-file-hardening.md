### Sturdier with damaged and hostile files
- Damaged ARW, SRW and ORF raws, lossy DNG tiles and CR3 previews that declare more data than they hold are refused with an error instead of using up memory.
- A JPEG's colour profile split over several chunks is checked chunk by chunk, IPTC text loses trailing padding, and very long rational tag lists are shortened like other lists.
- HEIF files with too many transforms, `.cube` LUTs with infinite values, and impossible or truncated capture dates are refused.
- Library files are never read or written through a planted link, and MCP renders go through a private temporary folder.
- Planar TIFFs read every plane, GIF and BMP sizes come from their headers, and Canon lenses are read from the maker note when the lens model is missing.
