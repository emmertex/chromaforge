### HDR display on GNOME
- On GNOME (Mutter) and other Wayland compositors that offer HDR only as PQ, HDR edits now show in HDR instead of falling back to SDR.
- SDR white there follows the compositor's reference white (203 cd/m² if it reports none); `<PREFIX>_HDR_PAPER_WHITE` overrides it.
- `--hdr-probe` honours `<PREFIX>_HDR_DISPLAY=0` and explains why a window stays SDR.
