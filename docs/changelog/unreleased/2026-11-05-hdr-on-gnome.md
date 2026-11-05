## Improvements

- **HDR display on GNOME:** Wayland compositors that offer HDR only as BT.2100 PQ (GNOME's Mutter) now get an HDR window instead of SDR; `--hdr-probe` honours `<PREFIX>_HDR_DISPLAY=0` and says why a window stays SDR. Thanks @cromefire.
