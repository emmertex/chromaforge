### Camera previews first while culling
- A raw you have changed (even only its camera profile) now shows the camera's own JPEG in the grid, the filmstrip and the loupe at once, until the app's render of your edit replaces it; cells no longer stay empty while raws decode.
- These quick previews have workers of their own, and background work (the real thumbnails behind them, eye finding, the photos prepared ahead) waits until every cell and the loupe on screen has its first picture. Thanks to @OneSlips.
