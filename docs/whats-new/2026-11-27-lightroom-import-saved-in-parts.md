### Large Lightroom imports are saved as they go

- A Lightroom catalog import is now committed and saved in parts of 20,000 changes, still undone in one step, so a very large catalog needs less memory: on a synthetic 200,000-photo catalog most of the import runs about 2 GB lower and the peak drops from 7.8 to 7.3 GB (#1361).
