### Thumbnails no longer render again and again in Slideshow and Print
- In the Slideshow and Print modules, the filmstrip and the module's own pictures asked for the same photos'
  thumbnails at different sizes and priorities and took them from each other on every frame, keeping the app busy
  while nothing changed. A thumbnail now serves any smaller request, and a queued one only moves up for a more
  urgent one, so they settle.
