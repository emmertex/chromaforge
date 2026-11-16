### Retouch: Smooth Skin and Find Blemishes
- A new Retouch section in the Edit panel, below Detail. Skin smooths the skin of the people in a photo (Amount,
  Texture, Even Tone, Redness, Reduce Shine; face, body or both; everyone or one person) and keeps the pores.
  Blemishes ▸ Find Blemishes puts a heal spot on each small dark or red spot of the faces' skin, and on the body's if
  you ask. Each part has its own eye for a before/after.
- Both use Google's open MediaPipe people and face models (Apache-2.0), which the AI Models dialog downloads when you
  ask. Detect Faces uses the same face model now: YuNet, whose training data doesn't allow commercial use, is retired.
  Ported from OneSlips' fork.
