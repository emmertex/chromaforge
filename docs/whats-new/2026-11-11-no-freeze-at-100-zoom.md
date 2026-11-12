### Develop no longer freezes at 100 % zoom

- Returning to 100 % zoom draws at once: the photo on screen keeps its rendered tiles while you zoom out and back in, instead of re-rendering every tile from the original (about 5 s for a 45 MP raw before, about 150 ms now).
- The window's own renders never queue behind exports and 1:1 previews any more, 1:1 tiles render on a second graphics card when one is present, and the big look-ahead pictures upload at most one per frame.
- Hovering the histogram's clipping triangles shows the clipping preview once the pointer rests there for about 0.3 s, instead of at every crossing, which used to re-render the photo repeatedly.
