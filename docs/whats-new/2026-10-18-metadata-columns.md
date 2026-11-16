### Metadata filter columns (issue #218)
- The filter bar's column switch shows Metadata columns under it, as in Lightroom Classic: Date, Camera, Lens and
  Label first, and any column can list Keyword, People, Flag, Rating or File Type instead (`view.filterColumns`).
  Each lists the values the photos in view have, with how many photos have each.
- A click narrows the grid; the columns after it count only what is left. Several cameras, lenses, keywords, people
  or labels can be picked at once (any of them matches); "All" clears a column. Scripts and agents do the same with
  `library.facetPick`.
