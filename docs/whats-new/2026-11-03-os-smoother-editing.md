### Smoother editing
- A slider held still, or a control surface whose gesture is still open, shows the sharp render a moment after the
  photo stops changing instead of staying a draft until you let go. Thanks @OneSlips.
- On a photo you just stepped to, a slider moves the picture at once: its drafts no longer wait for the photo's
  first full render, and the raw is decoded once for both. Thanks @OneSlips.
- Masks keep their computed shapes while you move other sliders, so photos with several masks redraw faster; a sky
  drawn from the photo's matte no longer re-runs when the exposure moves. Thanks @OneSlips.
- Zoomed in on a photo with a large brushed mask, the view renders in a fraction of a second instead of many
  seconds. Thanks @OneSlips.
- The interface no longer waits for the drive or the graphics card: presets, flags and steps are saved in the
  background, and photos are uploaded to the GPU without holding up the window. Renders leave processor cores free
  and run below normal priority, so the desktop stays responsive. Thanks @OneSlips.
