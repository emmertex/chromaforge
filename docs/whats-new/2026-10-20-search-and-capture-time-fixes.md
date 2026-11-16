### Search and capture time fixes

- Searching for an aperture or focal length, such as `f:2.8`, `aperture:5.6` or `focal:5.6`, now finds the photos shot at exactly that value, and `f:<2.8` / `f:>=2.8` count f/2.8 photos the right way. Smart-album Aperture and Focal Length rules such as “Aperture is ≥ 2.8” count them the right way too.
- A search word with a colon that isn't a field name, such as `10:30` or `Re:`, now finds photos with that text in their title, caption or other text.
- Edit Capture Time and time-zone shifts keep the time zone of a capture time written without seconds (`10:00+02:00`), instead of turning it into an unreadable time.
