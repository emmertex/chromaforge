### Exports share the graphics card, and it comes back after a failure
- Photos exporting side by side take turns at the graphics card (at most four at once, within a share of its memory),
  always behind the window's own work, so the loupe stays responsive during a big export.
- A graphics card that stops responding no longer turns the GPU off for the rest of the session: the photos it was
  working on are made on the processor, the card is restarted a few seconds later and used again. The GPU status
  (`app.gpu`) tells what happened in plain words. Thanks @OneSlips.
