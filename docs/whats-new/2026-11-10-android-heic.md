### HEIC/HEIF photos on Android

- The Android app now opens HEIC/HEIF photos (what iPhones shoot) with the same pure-Rust decoder as the desktop builds, so the import review, the camera roll and the file pickers accept them. Distributors who must avoid HEVC (patents) can build the Android app with `--no-default-features`, as on the desktop.
