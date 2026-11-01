### The graphics card setting now applies to photo rendering
- On a machine with two graphics cards, the GPU adapter environment variable (see the GPU pipeline docs) now also chooses the card that renders photos, not just the AI models. A name that matches no card logs a warning and uses the default card.
