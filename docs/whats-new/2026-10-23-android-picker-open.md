### Android: photos picked from Images import again

- Picking a photo through the system picker no longer fails with "the file dialog ended without an answer" on
  devices whose media provider refuses the plain open (seen on Waydroid): the app opens the photo under its
  media-library address instead.
