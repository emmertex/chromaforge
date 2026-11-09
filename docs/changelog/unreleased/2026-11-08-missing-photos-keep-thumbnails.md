## Fixes

- **Grid:** a photo whose original is missing keeps its last thumbnail after an update that changes how thumbnails are cached, instead of showing an empty cell; thumbnails cached by recent earlier versions are found too. Once the original is back, even while the app is open, its thumbnail is rendered again.
