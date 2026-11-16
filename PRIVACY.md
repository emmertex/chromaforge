# Privacy policy

Effective date: 2026-10-23

This policy covers the app (desktop, command line, web and Android builds) published from this repository.

## What the app collects

Nothing. The app has no analytics, telemetry, crash reporting, advertising, accounts or tracking. Your photos,
catalog, edits, keywords, faces and settings stay on your device. The developers never receive any of it.

## When the app goes online

The app works offline. It only connects to the network for the features below. None of these requests carries
personal data beyond what any web request reveals to the server it goes to: your IP address, the time, and a
User-Agent of the form `<binary>/<version>`. No identifier for you or your device is sent.

| Feature | Server | What is sent | When | How to avoid it |
|---|---|---|---|---|
| Update check and download | github.com (the release's `latest.json`, its files and checksum list); api.github.com as a fallback | A plain download request | Help ▸ Check for Updates…, or at start-up if "Check for updates when the app starts" is ticked (off by default; desktop only) | Leave that box unticked and don't check by hand |
| Map tiles | tile.openstreetmap.org by default; opentopomap.org (terrain), server.arcgisonline.com (satellite), or a tile server you add | Requests for the map tiles on screen, which show roughly which area you are viewing | While the Map module is open and tiles are not already cached | Don't open the Map module. There is no switch to turn tiles off yet. |
| Online place lookup and search | nominatim.openstreetmap.org by default, or a server you choose | The latitude and longitude being looked up, or the text you search for | Only with Map ▸ Reverse Geocoding set to Online and "Allow sending photo positions and searches to this server" ticked | Set Reverse Geocoding to Off or Offline |
| Offline place list | download.geonames.org | A download request | Only when you click its download button | Don't download it |
| Lens profiles | github.com (the lensfun database) | A download request | Settings ▸ Lens profiles ▸ Download | Don't download it |
| AI models (denoise, faces, masking) | github.com, huggingface.co, or a model source you configure | A download request | Only when you ask for a model and accept its terms | Don't download them |
| Immich | Your own Immich server | Your API key, and the photos, albums and metadata you import or publish | When you connect, import or publish, and on the sync interval you set | Settings ▸ Connections: remove the account or set the interval to 0 |
| Sync between your devices | Your other paired devices on the local network (mDNS discovery and an encrypted connection) | The device name, then your catalog changes, encrypted | Only after you pair a device in Settings ▸ Sync; from then on at each start | Unpair every device in Settings ▸ Sync |
| Plug-ins | Only hosts you grant to that plug-in | What the plug-in sends | When the plug-in runs | Grant no network access (none is granted by default) |
| Printing | Your CUPS / IPP print server (localhost by default) | The pages you print | Find Printers and Print | Don't print over the network |
| Web gallery publishing (SFTP) | Your own SSH server | The gallery files | When you publish | Don't configure it |
| Wi-Fi tethering | Your camera | Camera commands | When you tether over Wi-Fi | Don't use it |
| "Open in OpenStreetMap" link | openstreetmap.org, in your browser | The photo's coordinates | When you click the link | Don't click it |
| Remote denoise or masking server | A server you name in an environment variable | Image tiles for processing | Only when that variable is set | Leave it unset |

The web version loads only its own files from the site that hosts it; the update check, map tiles and online place
lookup are not part of it.

**Local servers.** The control server for scripts and agents (`--control`) is off unless you start it, listens on
127.0.0.1 only and needs a token. The MCP server talks over standard input/output and connects only to that local
control server. Edit In with PhotoCraft connects to 127.0.0.1 only.

**Exports and shares** go only where you send them: a folder you pick, Pictures on Android, or the app you choose in
the share sheet.

## Android permissions

| Permission | What it is used for |
|---|---|
| Photos and media (READ_MEDIA_IMAGES, READ_MEDIA_VISUAL_USER_SELECTED; READ_EXTERNAL_STORAGE on Android 12 and older) | Reading the camera roll and photos you import (on Android 14 and later you can allow selected photos only) |
| All files access (MANAGE_EXTERNAL_STORAGE), optional, GitHub APK only | Reading a folder of photos in place instead of copying it. The Google Play build does not request it: picked folders are copied into the app instead |
| Storage write (WRITE_EXTERNAL_STORAGE, Android 9 and older only) | Save to Pictures from the Export sheet |
| Internet | The features in the table above |
| Wi-Fi state and multicast | Finding your paired devices for Sync on the local network |

Refusing a permission only turns off the feature that needs it.

## Changes and contact

Changes to this policy are made in this file, with a new effective date. Questions: open an issue on this
repository's GitHub Issues page.
