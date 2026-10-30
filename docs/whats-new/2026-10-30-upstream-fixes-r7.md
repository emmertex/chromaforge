### Samsung panoramas, RAF decoding and download fixes
- Some panoramas from Samsung phones were refused as truncated, although other apps open them whole: their image data runs straight into the Samsung (SEF) block at the end of the file. When the file ends with a valid SEF block, the image now ends where that block begins, so these photos import and show. A JPEG that is really cut short is still refused.
- Compressed Fujifilm RAF files decode 13 to 17 % faster.
- Malformed base64 in segmentation masks and in agent (MCP) image arguments is refused instead of being half-decoded.
- Downloads of AI models no longer fail when the server sends an interim "103 Early Hints" response first.
- A render request for a size far beyond the photo is held to 512 megapixels instead of aborting the app on allocation.
- Negative pixel sizes in an XMP sidecar are ignored, a GPX file with more than one root element is refused, and the white balance menu in the Edit panel is translated.
- The People cards show the full number of photos on the picture (6110, not 999+); the line under the name that repeated it is gone.
- Show Pins now draws a pin for Object masks (at the first click) and for Describe masks (on what was found). Clicking one selects its mask, and with the Object tool on, a click on another mask's pin selects that mask instead of adding an Object click to the selected one.
- The side buttons of a mouse now do what the arrow keys do: Forward shows the next photo and Back the previous one. Like the arrow keys, they do nothing while a dialog or a text field has the keyboard, and pressed over a slider they only change the photo.
