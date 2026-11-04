### Mask tools that wait for the photo
- Brush, Linear and Radial now wait for the photo: the first stroke or drag makes the mask (a drag draws the gradient, a click places one), as one undo step. Picking another mask or photo, or leaving Masking, puts the tool away.
- Clicking a mask's component row edits that component; a Brush component arms the brush on it.
- Holding ⌥ / Alt part-way through a stroke erases from that point on. On a mask with settings the stroke's effect shows while you paint.
- The eraser keeps its own Size, Feather, Flow and Density. Size is now a size on the screen (1–100), the same at every zoom; sizes saved before are converted.
- The brush centre shows a cross while painting and a minus while erasing; the Object tool shows a minus while ⌥ leaves things out.
- Masks are named after what they select ("Radial Gradient − Linear Gradient") until you rename them.
- Assisted Culling can reject photos with closed, blurry or soft eyes, for the main subjects only or everyone.
- Space can zoom to the main subject's eyes when a photo has no focus point (`view.spaceZoomsTo`). Ported from OneSlips' fork.
