## Performance

- **Face recognition and two graphics cards:** the face recognition model keeps a device on each graphics card. It runs on the second graphics card when there is one (the card for editing stays free for the window), else on the card for editing; when its card stops working mid-scan, the rest of the scan moves to the other card, then to the processor. At a normal or full pace the second card is not held back, since it draws no window; while you work both are held back as before. `faces.gpu` names the card (`recognition.card`). Ported from OneSlips' dual-GPU work.
