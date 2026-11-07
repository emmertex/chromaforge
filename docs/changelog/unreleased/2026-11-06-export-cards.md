## Performance

- **Graphics cards for exports:** with a second graphics card, Settings ▸ Performance ▸ Exports chooses which cards exports use: both (the default, each photo on the card that will finish it sooner), the card for editing only (the second card stays free for previews and thumbnails), or the second card only, to measure its speed (the card for editing takes over if it stops). `app.exportTuning {cards: both | editing | second}` sets it too. Ported from OneSlips' dual-GPU work.
