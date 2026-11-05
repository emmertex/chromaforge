## Performance

- **Both graphics cards share batch work:** exports, Build Previews and the background preparation take a turn per photo on whichever card would finish it soonest (each card's speed is measured), instead of sending every batch render to the second card; the card for editing takes batch photos only while the interface is idle, one at a time, and a running one pauses between submissions when you touch a slider. 1:1 previews build two photos per card. Ported from OneSlips' LightCraft dual-GPU work.
