### Lightroom catalogs with collection-name rules import again
- A smart collection whose Collection rule matched the name of a smart collection, its own included, made the whole Lightroom Classic catalog import fail with "would include itself". Collection rules now match ordinary collections only, as in Lightroom.
- A smart collection whose rules would still make it include itself is imported as a plain collection with its current photos and a warning; the rest of the catalog imports.
