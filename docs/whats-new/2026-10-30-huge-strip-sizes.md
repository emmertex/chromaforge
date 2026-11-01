### Raw files with huge strip sizes
- A damaged or crafted Olympus ORF, Samsung SRW, Sony ARW or Pentax PEF whose strip sizes add up past the largest
  64-bit number no longer crashes the app (debug builds) when it is read: the file is refused, or read as far as
  its data go.