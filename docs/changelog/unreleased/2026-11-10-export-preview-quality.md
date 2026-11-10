## Changed

- **Export, full-quality preview:** the Export module's preview of a single photo draws the loupe's own render (with the thumbnail standing in until it arrives), instead of a thumbnail ladder render capped at 512 px scaled up to fill the preview area. Entering the Export or Print module from Develop now selects the photo open on screen (Develop's filmstrip can hold a multi-selection while another photo is stepped to); a selection made outside Develop is kept.
- **Print, the filmstrip selection is what prints:** with one photo selected the Print module's page and its job take that photo (choosing another photo in the filmstrip moves the print to it), instead of falling back to every photo in the filmstrip. Only an empty selection prints the whole filmstrip.
