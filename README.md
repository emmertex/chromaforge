<p align="center">
  <img alt="" src="brand/logo.svg" width="128">
</p>

<h1 align="center">Chromaforge</h1>

<p align="center">
  <b>Your photos. Your pixels. Your machine.</b><br>
  A Lightroom Classic-style photo library and non-destructive raw developer, written in pure Rust.
</p>

<p align="center">
  <a href="LICENSE"><img alt="Licence: MIT" src="https://img.shields.io/badge/licence-MIT-blue"></a>
  <img alt="Pure Rust" src="https://img.shields.io/badge/pure-Rust-orange">
  <img alt="Platforms: macOS, Windows, Linux, web, Android" src="https://img.shields.io/badge/platforms-macOS%20%7C%20Windows%20%7C%20Linux%20%7C%20web%20%7C%20Android-lightgrey">
</p>

<p align="center">
  <a href="#features">Features</a> ·
  <a href="#install">Install</a> ·
  <a href="#build-from-source">Build</a> ·
  <a href="#built-for-agents">Agents &amp; MCP</a> ·
  <a href="#feature-status">Status</a> ·
  <a href="ROADMAP.md">Roadmap</a> ·
  <a href="#licence-and-credits">Licence</a>
</p>

<p align="center">
  <img src="docs/images/classic/library.jpg" alt="Library module: Grid view of the 24-photo demo library, with Navigator, Catalog, Folders and Collections on the left and the histogram, Quick Develop and Keywording on the right" width="100%">
</p>

> [!NOTE]
> **New name.** The app was renamed Chromaforge in v26.43.4: settings, libraries and updates carry over.

## What it is

Chromaforge is one catalog and seven modules (Library, Develop, Map, Book, Slideshow, Print, Web) in the Lightroom
Classic layout, with its own raw decoders, a scene-referred GPU/CPU develop pipeline and a command registry that the
UI, the CLI, a control channel and an MCP server all share.

- **Native** on macOS, Windows and Linux, **in the browser** via WebAssembly, and early on Android.
- **Local-first:** no account, no cloud, no telemetry, no subscription. The network is used only for things you turn on.
- **Non-destructive:** originals are never touched; edits live in the catalog and, if you like, in XMP sidecars.
- **Drivable end to end by AI agents** over MCP, with undo for everything they do.
- **Open source** (MIT). Chromaforge is a fork of an open-source project; its copyright notices are kept in [NOTICE](NOTICE)
  and [LICENSE](LICENSE).

## Contents

- [Features](#features): [modules](#seven-modules-one-catalog) · [editing](#editing) · [masking](#masking) ·
  [library & culling](#library-and-culling) · [video](#video) · [beyond the darkroom](#beyond-the-darkroom)
- [Install](#install) · [Build from source](#build-from-source) · [Keyboard](#keyboard)
- [Built for agents](#built-for-agents) · [Fast, native, private](#fast-native-private)
- [Feature status](#feature-status) · [How it's built](#how-its-built) · [Contributing](#contributing) ·
  [Licence and credits](#licence-and-credits)

# Features

## Seven modules, one catalog

Library and Develop are where you spend your day; Map, Book, Slideshow, Print and Web turn the same photos into
places, books, shows, prints and websites without exporting first. Pick a module at the top right, from
`Window → Modules`, or with ⌘⌥1–7.

<table>
<tr>
<td width="50%"><img src="docs/images/classic/develop.jpg" alt="Develop module: a lake photo in the loupe with Highlights -55 and Shadows +35 in the Basic panel, and the filmstrip below"><br><sub><b>Develop.</b> Non-destructive raw development, from white balance to masks.</sub></td>
<td width="50%"><img src="docs/images/classic/map.jpg" alt="Map module: an OpenStreetMap world map with the location filter bar and Map Style, Saved Locations and Tracklog panels"><br><sub><b>Map.</b> Geotag by drag and drop, saved locations, GPX tracklogs, reverse geocoding.</sub></td>
</tr>
<tr>
<td width="50%"><img src="docs/images/classic/book.jpg" alt="Book module: a multi-page view of a landscape book laid out automatically, with Book Settings on the right"><br><sub><b>Book.</b> Auto layout, templates, text cells and cover; exports a print-ready PDF.</sub></td>
<td width="50%"><img src="docs/images/classic/slideshow.jpg" alt="Slideshow module: a sunset slide framed in the stage with Options, Layout, Overlays and Backdrop panels"><br><sub><b>Slideshow.</b> Play full screen (silently for now), or export the show as PDF or JPEGs.</sub></td>
</tr>
<tr>
<td width="50%"><img src="docs/images/classic/print.jpg" alt="Print module: a contact sheet of four photos with rulers, and Layout Style, Image Settings, Layout and Margins panels"><br><sub><b>Print.</b> Single image, contact sheets and picture packages; colour-managed with printer profiles; print or save as PDF/JPEG.</sub></td>
<td width="50%"><img src="docs/images/classic/web.jpg" alt="Web module: a grid gallery preview of the demo photos with Layout Style, Site Info, Color Palette and Appearance panels"><br><sub><b>Web.</b> Static HTML galleries, previewed live and uploaded over SFTP.</sub></td>
</tr>
</table>

## Editing

Every slider renders through a **scene-referred, wide-gamut, 32-bit float pipeline**: highlights roll off like film,
shadows open up without halos, and colour stays clean from capture to export.

| | |
|---|---|
| **Light** | Exposure, Contrast, Highlights, Shadows, Whites, Blacks, with edge-aware local tone mapping (a guided filter on log-luminance): −100 Highlights recovers a blown sky without grey halos. |
| **Color** | White balance by temperature and tint (Kelvin for raw, relative for JPEG) with presets, Auto and a click-to-neutralise eyedropper; Vibrance that protects skin tones, Saturation, an 8-band Color Mixer and 3-way Color Grading, all computed in OkLCh. |
| **Effects** | Texture, Clarity, Dehaze (dark-channel prior with guided refinement; negative adds atmosphere), post-crop Vignette with highlight priority, and resolution-independent film Grain. |
| **Tone Curve** | Parametric region curve with movable splits plus point curves for RGB, Red, Green and Blue; monotone by construction, so no accidental tone inversions. |
| **Crop & geometry** | Free or locked aspect ratios, drag-to-rotate straightening that keeps the largest crop inside the image, thirds / grid / golden-ratio overlays, flips, 90° rotations, manual transforms and Upright. |
| **Black & White** | One key (<kbd>V</kbd>) to monochrome, an 8-band B&W Mix, then tone it with Color Grading for selenium, sepia or split-tone prints. |
| **Presets & profiles** | Eighteen hand-built presets (*Golden Hour, Teal & Orange, Faded Matte, Selenium Tone, Crisp Landscape* and more), each with an Amount slider from 0 to 200 %; save your own, mark favourites, and copy, paste or sync exactly the groups you choose. |

<table>
<tr>
<td width="50%"><img src="docs/images/classic/grading.jpg" alt="Develop module with the Color Grading panel open: Midtones, Shadows and Highlights wheels with warm highlights and cool shadows on a lake scene"><br><sub><b>Color Grading.</b> Warm highlights (42°) and cool shadows (215°), with blending and balance.</sub></td>
<td width="50%"><img src="docs/images/classic/before-after.jpg" alt="Develop module in Before/After side by side: the original lake photo on the left, the edited one on the right"><br><sub><b>Before &amp; after.</b> Hold <kbd>\\</kbd> for the original, <kbd>Y</kbd> side by side, <kbd>Shift</kbd>+<kbd>Y</kbd> split.</sub></td>
</tr>
</table>

### Histogram & scopes

The histogram stays pinned at the top of the Edit panel while the controls scroll under it; drag across it to move
Blacks, Shadows, Exposure, Highlights or Whites. Beside the RGB, Luminance, R/G/B and Chroma histograms it has a
grading suite's scopes: **Waveform**, **RGB Parade** and **Vectorscope** with skin-tone line, on linear or
logarithmic scales.

<p align="center">
  <img src="docs/images/classic/scopes.jpg" alt="The Edit panel's histogram in four modes: RGB histogram, RGB Parade, Waveform with red, green and blue overlaid, and Vectorscope with graticule and colour targets" width="70%">
</p>

## Masking

Paint with a **Brush** (size, feather, flow, density, erase), drop **Linear** and **Radial Gradients** with draggable
pins, or select by **Luminance Range**, **Color Range**, **Sky**, **Subject**, **Background** and **Object**.
Combine components with **Add / Subtract / Intersect**, invert any of them, and dial in 15 local adjustments per mask
(Temp, Tint, Exposure, Contrast, Highlights, Shadows, Whites, Blacks, Texture, Clarity, Dehaze, Hue, Saturation,
Sharpness, Noise) plus an overall Amount.

<p align="center">
  <img src="docs/images/classic/masking.jpg" alt="Develop module with the Masking panel: a linear Sky mask and a selected radial Sun glow mask, its ellipse and pins drawn over the sun on a lake scene" width="100%">
  <br>
  <sub>A radial "Sun glow" mask (Temp +40, Exposure +0.50) layered over a linear "Sky" mask.</sub>
</p>

## Library and culling

The **Catalog** (All Photos, Quick Collection, Previous Import, Recently Added, Picks, Recently Deleted), **By
Date**, **Folders** and **Collections** in sets, with smart collections. The filter bar takes several keywords,
cameras or lenses at once. Rate with <kbd>0</kbd>–<kbd>5</kbd>, flag with <kbd>P</kbd> / <kbd>X</kbd> /
<kbd>U</kbd>, colour-label with <kbd>6</kbd>–<kbd>9</kbd>. Search understands fields:
`rating:>3 flag:pick iso:>800 camera:x2 date:2026-04 keyword:mountains`. Every view sorts by capture, import or edit
date, name, rating, size or at random; albums also have a **Custom Order** (drag photos in the grid or filmstrip) that
slideshows, books, prints, exports and Rename `{seq}` follow. The grids are virtualized, so they stay smooth with forty
photos or forty thousand.

<table>
<tr>
<td width="50%"><img src="docs/images/classic/survey.jpg" alt="Library module in Survey view: four demo photos side by side with their ratings, Quick Develop and Keywording on the right"><br><sub><b>Survey</b> (<kbd>N</kbd>): narrow a selection down side by side.</sub></td>
<td width="50%"><img src="docs/images/classic/compare.jpg" alt="Library module in Compare view: a Select photo of dunes beside a Candidate sunset photo"><br><sub><b>Compare</b> (<kbd>C</kbd>): Select against Candidate, with synced zoom.</sub></td>
</tr>
</table>

## Video

Clips live in the same library as your photos. MP4, MOV, MKV, WebM, MPEG-TS and MXF with H.264, HEVC, VP9, AV1,
ProRes, DNxHR, APV or MPEG-2 decode in pure Rust. Play a clip in the loupe (<kbd>Space</kbd>, <kbd>←</kbd> /
<kbd>→</kbd> for single frames), drag the trim handles, pick the poster frame the grid shows, and capture any frame as
a photo. The Light and Color controls (and presets that use only them) grade every frame. Export writes H.264 MP4 or
ProRes MOV with the trim, the grade and the sound (graded on the GPU for H.264); `<cli> render clip.mov -o out.mp4 --trim 2,8` does the same from
the command line. Interlaced H.264 (AVCHD 1080i) is deinterlaced. Masks, spot removal, lens corrections and crop don't apply to video.
See [docs/video.md](docs/video.md).


## Beyond the darkroom

| | |
|---|---|
| **Catalog** | Catalog v4: a redb store plus an operation log, several catalogs (New / Open Catalog), scheduled backups, integrity test, optimize; opens and migrates older library folders. |
| **Immich** | Connect one or more Immich (v3+) servers (API key in the system keychain); link existing photos, import from Immich, two-way metadata and album sync, People and smart search, Immich as a publish service ([docs/immich.md](docs/immich.md)). |
| **Lightroom migration** | Imports a Lightroom Classic `.lrcat`: collections, smart collections, folder stacks, History and snapshots (as Versions). |
| **Network drives** | Catalogs and photos on mapped drives, `\\server\share` UNC paths and SMB/NFS mounts load and import on every OS. |
| **Tethering** | Studio capture (watched folder) and native PTP tethering over USB or Wi-Fi (PTP/IP). |
| **Plug-ins** | WebAssembly plug-ins in a `wasmi` sandbox: menu commands, export post-processing and destinations, custom metadata fields, metadata suggestions, publish services (Lightroom Lua plug-ins do not run). |
| **Output** | Publish services (Hard Drive, Immich, plug-ins), Edit In (copy with adjustments, TIFF / layered PSD), saved Actions, export presets, watermarks, HDR export, contact sheets. |
| **Print** | Printer ICC profile with rendering intent and black point compensation, Print Adjustment, 16-bit output; print to IPP/CUPS printers (`ipps://` with certificate trust; paper type, tray, quality and two-sided from the driver) or save PDF/JPEG. |
| **Updates** | A built-in updater checks this project's release feed, downloads and installs (off at start-up unless you turn it on). |

# Install


Release builds are listed on the project's releases page. `<ver>` in the file names is the version number,
`<binary>`/`<cli>` are the brand binary names, and `SHA256SUMS.txt` lists a checksum for every file. Build them yourself
with `cargo xtask package`.

### Windows

| Build | Installer | Portable |
|---|---|---|
| x64 (64-bit Intel/AMD) | `<binary>-<ver>-windows-x64.msi` | `<binary>-<ver>-windows-x64-portable.zip` |
| arm64 (Snapdragon and other ARM PCs) | `<binary>-<ver>-windows-arm64.msi` | `<binary>-<ver>-windows-arm64-portable.zip` |

Installers and executables are code-signed. The installer asks where to install (`C:\Program Files\<app>` by
default; upgrades stay in the folder you chose) and ends on a page confirming the app was installed, with an
option to start it. For unattended installs: `msiexec /i <binary>-<ver>-windows-x64.msi /qn INSTALLFOLDER="D:\Apps\<app>\"`.

### macOS

| Build | File | Notes |
|---|---|---|
| App, universal (Apple silicon + Intel) | `<binary>-<ver>-macos-universal.dmg` | Signed and notarized |
| Command-line tool, universal | `<cli>-<ver>-macos-universal.zip` | Signed and notarized |

### Linux

| Format | x86_64 | Notes |
|---|---|---|
| AppImage | `<binary>-<ver>-linux-x86_64.AppImage` | Runs anywhere; updates itself with [AppImageUpdate](https://github.com/AppImageCommunity/AppImageUpdate) (`.zsync` files) |
| Flatpak | `<binary>-<ver>-linux-x86_64.flatpak` | Sandboxed; `flatpak install --user <file>` |
| Debian/Ubuntu | `<binary>-<ver>-linux-x86_64.deb` | |
| Fedora/RHEL/openSUSE | `<binary>-<ver>-linux-x86_64.rpm` | |
| Tarball | `<binary>-<ver>-linux-x86_64.tar.gz` | Unpack anywhere |

Linux ARM64 (aarch64) builds are paused; `packaging/linux/package.sh aarch64` still builds one locally.

### Android (early: see [docs/android.md](docs/android.md))

The Android app is the same app as the desktop one, with the same catalog, raw developer and edits, in a
**touch layout** made first for tablets and foldables (it works on phones too). Develop shows the photo alone; tap
it for a slim top bar and a tool strip, pick a tool (Basic, Colour, Tone, Detail, Optics, Effects) and drag on the
photo to set one slider at a time, then Done or Cancel; hold the photo to see the original. Swipe to scroll,
long-press to select several photos, and Back steps out one level at a time. Import photos, a folder or the camera
roll; export from the Export sheet to Pictures or share to another app.

The touch layout is on by default on Android and available on every platform: **Settings ▸ General ▸ Layout**
switches between Touch and Desktop (the choice is remembered).

The app collects nothing and works offline; see the [privacy policy](PRIVACY.md) for the few features that go
online and what the Android permissions are for.

<table>
<tr>
<td width="62%"><img src="docs/images/touch/tablet-develop.jpg" alt="Touch layout on a tablet in landscape: a sunset photo filling the screen, a slim top bar with undo, compare, filmstrip, history, panel and share buttons, and a tool strip at the bottom"><br><sub><b>Tablet.</b> Photo first, tools at the bottom.</sub></td>
<td width="38%" rowspan="2"><img src="docs/images/touch/phone-library.jpg" alt="Touch layout on a phone in portrait: the Library grid with two columns of demo photos"><br><sub><b>Phone.</b> The Library grid.</sub></td>
</tr>
<tr>
<td><img src="docs/images/touch/tablet-adjust.jpg" alt="Adjust mode: the Basic tool's sliders as chips under the photo, Shadows selected and shown as +35 at the top"><br><sub><b>Adjust.</b> Drag on the photo to set the chosen slider.</sub></td>
</tr>
<tr>
<td><img src="docs/images/touch/tablet-settings.jpg" alt="Touch Settings with the Layout row (Touch / Desktop) at the top of General"><br><sub><b>Settings.</b> Layout switches Touch / Desktop.</sub></td>
<td><img src="docs/images/touch/tablet-export.jpg" alt="The touch Export sheet: JPEG full size, JPEG 2048 px for sharing or TIFF, long edge and quality buttons, and an Export button"><br><sub><b>Export.</b> Save to Pictures or share.</sub></td>
</tr>
</table>

Install from the release's APKs:

| Build | File | Notes |
|---|---|---|
| Phones and tablets (arm64) | `<binary>-<ver>-android-arm64-v8a.apk` | Sideload: allow installs from your browser or file manager, then open the file |
| Emulator and Chromebooks (x86_64) | `<binary>-<ver>-android-x86_64.apk` | |
| Play Store bundle | `<binary>-<ver>-android-arm64-v8a-x86_64.aab` | For Play Console uploads, not for installing |

Android 9 or newer. Until a release signing key is set up, the APKs carry a test signature: they install, but a
build signed with the release key later will need the test build uninstalled first.

### Web (WebAssembly)

| Build | File | Notes |
|---|---|---|
| Static site | `<binary>-web-<ver>.zip` | Runs in a modern browser; host it on any static server |


# Build from source

Workspace builds, including the desktop, web and CLI applications, require Rust 1.95 or newer.

```sh
git clone https://example.invalid/repo && cd repo
cargo xtask brand check                                 # the product name lives only in brand.toml
cargo xtask run                                         # build and open your library (~/Pictures/<app> Library; a new one starts with demo photos)
cargo xtask run -- ~/Pictures/trip                      # import your photos (folders are scanned, duplicates skipped)
cargo xtask run -- --memory                             # a throwaway in-memory demo session (writes nothing)
cargo xtask run -- --control 7980                       # with the automation channel
cargo xtask web --serve                                 # the same app in the browser: http://127.0.0.1:8080/
cargo run --release -p lcc-cli -- render photo.jpg -o out.jpg --set light.exposure=0.5
cargo xtask package                                     # installers and archives under the brand binary names
cargo xtask install                                     # install the app and CLI for the current user
cargo xtask ci                                          # fmt, clippy, tests, layering, wasm checks
```

Inside `target/` the binaries are always called `app` and `app-cli`; `cargo xtask run`, `package` and `install`
give them the brand names from `brand.toml`. `cargo run -p lcc-app` / `-p lcc-cli` also work for development.

CI defaults to line-table debug information and one build job per 1.5 GB of RAM
available when it starts (at most one per CPU; 4 if memory can't be read), so an
8 GB machine with 6 GB free builds with 4, and a busy machine with little free
builds with fewer. Test threads follow, capped at 4. Full-debuginfo linkers, one
per CPU, otherwise exhaust RAM before any test runs. GPU coverage is unchanged. Explicit
`CARGO_PROFILE_DEV_DEBUG`, `CARGO_BUILD_JOBS` and `RUST_TEST_THREADS` settings
override these defaults. Ordinary development commands keep their own settings.

**Chinese and Japanese text** need the shared font repo, an optional build input (official releases always include it):

```sh
git clone https://github.com/storytold/craft-fonts ../craft-fonts
CRAFT_FONTS_DIR=../craft-fonts cargo run --release -p lcc-app
```

Without it the app builds and runs the same, but Chinese and Japanese text have no glyphs. Fonts are never committed to this
repo — the standard for what goes there lives with it: see the [craft-fonts README](https://github.com/storytold/craft-fonts#readme).

**Interface language:** **Edit → Language** (English, 简体中文, 繁體中文（台灣）, 日本語, Português (Brasil), Español, Deutsch, Русский, Français) or **Settings → General →
Language**; the choice applies immediately and persists. See [docs/localization.md](docs/localization.md).

**Logs:** the desktop app writes its log to standard error and to `logs/<binary>.log` in its settings folder
(Linux `$XDG_CONFIG_HOME/<settings_dir>/logs/`, by default `~/.config/<settings_dir>/logs/`; macOS
`~/Library/Application Support/<settings_dir>/logs/`; Windows `%APPDATA%\<settings_dir>\logs\`), never in the library. A
launch from a desktop menu or the Dock has no terminal, so attach this file to a bug report (**Help ▸ Open Log
Folder** shows it in the file manager). Each start moves the
previous log to `<binary>.1.log` and that one to `<binary>.2.log`, so the log of a run that crashed survives the
next start; the file stops growing at 16 MiB, `--version` and `--help` write none, and runs with
`<PREFIX>_NO_PREFS` log to standard error only. By default the app's own crates log at `info` and everything else
at `warn`. `<PREFIX>_LOG=info` or `debug` works as before (that level for the app's own crates, warnings and
errors from the rest; any other value: warnings and errors only) and wins over `RUST_LOG`, which otherwise replaces
the default with env_logger-style directives such as `RUST_LOG=debug` or `RUST_LOG=warn,lcc_pipeline=trace` (a
directive ending in `*` covers every target starting with it, as in `lcc*=debug`). Panics are recorded there
too, and in `<binary>-panics.log` beside the log files. The logger is `apps/app/src/logging.rs`.

The web build needs the `wasm32-unknown-unknown` target and the matching `wasm-bindgen` CLI
(`cargo xtask web` prints the exact install command); see [docs/web.md](docs/web.md).

**Nix** builds the desktop app and `<cli>` (the Nix build always includes the craft-fonts input, so Japanese
text has glyphs):

```sh
nix run <flake-url>                    # the desktop app
nix build <flake-url>                  # → ./result/bin/{<binary>,<cli>}
nix develop <flake-url>                # rust toolchain + native deps + fonts
```

In a flake configuration (NixOS, home-manager, nix-darwin):

```nix
# flake.nix
inputs.app.url = "<flake-url>";
# optional: build against your own nixpkgs instead of the one the app pins
# inputs.app.inputs.nixpkgs.follows = "nixpkgs";

# then, in a NixOS or home-manager module (where `inputs` is in scope):
nixpkgs.overlays = [ inputs.app.overlays.default ];   # makes `pkgs.<binary>` available
environment.systemPackages = [ pkgs.<binary> ];           # home-manager: home.packages = [ pkgs.<binary> ];
```

`nix build` installs the same desktop file, hicolor icons and AppStream metadata as the .deb/.rpm, and runs
`cargo test --workspace` as its check phase (skip it with `pkgs.<binary>.overrideAttrs { doCheck = false; }`).

## Keyboard

The Classic keymap is the default:

| Key | Action | Key | Action |
|---|---|---|---|
| <kbd>G</kbd> | Grid | <kbd>E</kbd> | Loupe |
| <kbd>D</kbd> | Develop | <kbd>C</kbd> / <kbd>N</kbd> | Compare / Survey |
| <kbd>R</kbd> | Crop | <kbd>Shift</kbd>+<kbd>W</kbd> | Masking |
| <kbd>\\</kbd> | Original | <kbd>Y</kbd> | Before/after |
| <kbd>Tab</kbd> | Side panels | <kbd>L</kbd> | Lights out |
| <kbd>F</kbd> | Full-screen preview | <kbd>F11</kbd> | Full screen |
| <kbd>⌘L</kbd> | Enable filters | <kbd>⌘⌥1</kbd>–<kbd>7</kbd> | Modules |
| <kbd>⌘Z</kbd> | Undo | <kbd>⌘/</kbd> | All shortcuts |

Settings → Shortcuts → Keymap switches to the *Alternative* set.


# Reference

## Built for agents

Every menu item, slider, brush stroke, crop handle and keystroke in the app is a **command** with a stable id and
JSON parameters. The UI, the keyboard, the CLI, a JSON-lines control channel and an **MCP server** all dispatch
through the same entry point. An agent can cull a shoot, develop it, mask a sky and export it, and *see* the result.

```sh
<binary> --control 7980 ~/Pictures/trip
```
```jsonc
{"method": "engine.execute", "params": {"command": "photo.flag",  "params": {"flag": "pick"}}}
{"method": "engine.execute", "params": {"command": "develop.set", "params": {"values": {"light.highlights": -45, "light.shadows": 38}}}}
{"method": "engine.execute", "params": {"command": "mask.add",    "params": {"kind": "radial", "center": [0.62, 0.4], "rx": 0.2, "ry": 0.14}}}
{"method": "ui.clickWidget",   "params": {"id": "slider:effects.clarity"}}       // drive any widget by name
{"method": "ui.pointer",       "params": {"events": [{"kind":"down","x":0.2,"y":0.3}, {"kind":"up","x":0.4,"y":0.3}]}}
{"method": "ui.screenshot",    "params": {"path": "after.png"}}
```

- **Command registry.** `engine.commands` lists the available commands; `develop.controls` lists every slider's
  range, default and current value.
- **MCP server.** `<cli> mcp` exposes the command registry to Claude (or any MCP client), alongside
  helpers for import, query, develop, mask, render (returned as an image) and export. It runs headless, or attached
  to the running app with screenshots, clicks and gestures. See [docs/mcp.md](docs/mcp.md).

  ```sh
  cargo build --release -p lcc-cli
  claude mcp add <mcp_server> -- "$PWD/target/release/app-cli" mcp ~/Pictures/shoot          # headless
  claude mcp add <mcp_server>-app -- "$PWD/target/release/app-cli" mcp --connect 127.0.0.1:7980  # live app
  ```
- **Scriptable CLI:** `<cli> run --import in.dng develop.set control=light.exposure value=0.7 app.export
  path=out.jpg longEdge=2048` runs any chain of commands (headless, on a saved library, or against the running app)
  and prints one JSON result per command; `<cli> render in.dng -o out.jpg --set light.exposure=0.7 --preset …`.
- **Undo for everything**, including agent actions: a slider drag (or a scripted burst of updates) is one undo step.
- **Contact sheets (native app / CLI):** select photos, then **File → Contact Sheet PDF…**. Choose A4 or Letter, landscape,
  rows/columns and filename captions; save a paginated PDF and print it from your PDF viewer. Exports use the
  current edits, fit each photo without further cropping, and render at 150 dpi in sRGB. The background export
  supports cancellation and protects originals and XMP sidecars. From the CLI:

  ```sh
  <cli> run --import ~/Pictures/shoot library.selectAll export.contactSheet path=Contact.pdf paper=a4 columns=3 rows=4 captions=true
  ```

  `export.contactSheet` also accepts explicit `ids`, `landscape=true`, and `paper=letter`. It replaces an existing
  output only after the entire PDF succeeds. Limits: 1,000 photos, 100 pages, 256 MiB; captions are rasterized
  with the same font coverage as watermarks (CJK requires craft-fonts), with long names shortened in the middle.
  For printer profiles, driver options and picture packages, use the Print module.
- **Every widget is addressable** (`ui.widgets`) and clickable by name, so agents operate the real UI, not a
  side door.
- The screenshots in this README were produced end to end by the [`docs/showcase/`](docs/showcase/) scripts.
  Protocol reference: [docs/control-protocol.md](docs/control-protocol.md).


## Fast, native, private

- **Pure Rust, no C.** Our own RAW decoders (DNG, Canon CR2/CR3, Sony ARW, Nikon NEF, Fujifilm RAF incl. X-Trans,
  Panasonic RW2 / Leica RWL, Pentax PEF, Olympus ORF), our own colour science, our own pipeline. JPEG, PNG, TIFF, WebP,
  PSD composites, JPEG XL and HEIC/HEIF (iPhone photos) open today (HEVC stays a build-time choice: the `heif`
  feature is on by default and can be turned off).
- **Scene-referred & wide-gamut.** Linear Rec.2020 float internally, Bradford-adapted white balance, gamut mapping
  instead of clipping, a filmic shoulder for raw and pixel-exact pass-through for JPEGs you haven't touched.
- **Resolution-independent edits.** Radii and brush sizes are relative to the image, so a 400 px preview, your
  5K display and a 60 MP export look the same.
- **GPU-accelerated, CPU-exact.** The whole develop pipeline runs as wgpu compute kernels (Metal / Vulkan / DX12),
  checked against the CPU pipeline to within 1/255. On a 24 MP raw (Apple M4 Pro): a slider update re-renders in
  ~4 ms, a cold 2.5 MP loupe in ~30 ms, and a full-size export in ~0.3 s including a parallel JPEG encode.
  Without a GPU the same pipeline runs on all CPU cores, redoing only the stages a slider affects. Batch exports
  render several photos side by side, so one's decode and encode overlap another's render.
- **Instant culling.** Opening a raw shows its embedded camera preview or cached render within ~0.1 s while the
  full render follows (~0.2–0.5 s for 24 MP). The next and previous photos are prepared in the background, so stepping
  through a shoot takes ~50 ms per photo.
- **Background rendering.** A worker pool renders the loupe, before/after and every visible thumbnail off the UI
  thread: drafts during drags, full quality on release.
- **Local-first.** No account, no cloud, no telemetry, no subscription. Your catalog is an append-only log of
  human-readable operations you can diff, back up or replay.
- **Online only when you ask.** The app reaches the network only for something you turn on or start: update
  checks (Help ▸ Check for Updates…, or at start-up once you tick "Check for updates when the app starts", which is
  off by default), model and lens-profile downloads, map tiles and place-name searches (geocoding), Immich,
  publishing (SFTP), network printing (IPP), Wi-Fi tethering (PTP/IP to the camera), remote denoise and AI-mask
  (SAM 3) workers on a server you name, and plug-ins you grant network access to. The control port listens on this
  computer only, and Edit in PhotoCraft talks to PhotoCraft on this computer. An update check asks GitHub for this
  project's release list; GitHub sees your IP address, the time and a User-Agent of the app's name and version, and
  nothing else is sent.

## Feature status

The app is young and moving fast. **Where we honestly stand** (details in the [roadmap](ROADMAP.md#where-we-stand)):

- **By feature count we're at ~89%** of Lightroom (core features 96%, important ones 91%; 497 of 598 rows done and 66
  partial), tracked row by row in
  [docs/parity-checklist.md](docs/parity-checklist.md).
- **As a day-to-day Lightroom replacement we're nearer 60–70%.** It's great for JPEG/DNG and most Nikon / Sony /
  older-Canon raws on one machine.
- **The biggest gaps:**
  - **camera colour calibration:** Sony, Nikon, Panasonic, Fujifilm and Canon CR3 raws have guarded estimates from their camera JPEGs, with built-in profiles for twelve bodies (ILCA-99M2, ILCE-7CR, ILCE-7M4, ILCE-7RM5, X-H2S, X-T4, Nikon Z 6II, Z 8, D5300, D750, D780, D90), matrices fitted to measured spectral sensitivities for 52 more (Profile ▸ Calibrated camera colour), dcraw's matrices for about 550 more models, and your own `.dcp` camera profiles import; colour-chart and Lightroom-matched calibration tools exist, but a measured calibration database of our own is still missing;
  - **per-model raw verification:** 596 of 1,446 models (41%) are verified on real files, so about 7 in 8 photographers shoot a verified body; other bodies are unverified, see [docs/raw-parity.md](docs/raw-parity.md);
  - **AI quality:** Subject / Background (BiRefNet) and Object (SAM 3) masks are optional downloads; sky is still heuristic, denoise quality is unverified;
  - **network libraries:** a preview reads the raw's header and its embedded JPEG only (56.5 MB → 5.9 MB, 10 s → 1 s on the test share), but the *developed* picture still needs the sensor data over the wire; see [docs/raw-preview-reads.md](docs/raw-preview-reads.md);
  - **HDR display covers the loupe only** (zoomed tiles, Before/After and the second window are still SDR); **video is new** (global edits only).
- **What's next:** see [where we're going](ROADMAP.md#where-were-going).

| Area | Status |
|---|---|
| Library: albums, folders, smart albums, stacks (incl. auto-stack), virtual copies, ratings, flags, labels, filter bar, search, sort, grids, filmstrip | ✅ |
| Culling: Compare (synced zoom) and Survey views, auto-advance, instant previews | ✅ |
| Light, Color, Effects (vignette styles), Tone Curve (+ refine saturation, targeted adjustment), Color Mixer (+ targeted), Point Color, Color Grading, Calibration, B&W | ✅ |
| Histogram (RGB, luminance, R/G/B, chroma; linear or log; drag a tonal zone) and scopes (Waveform, RGB Parade, Vectorscope), pinned above the Edit panel | ✅ |
| Masking: brush (A/B presets, density), linear, radial, luminance/colour range, per-mask feather and Curve, add/subtract/intersect; AI Subject / Background (BiRefNet) and Object (SAM 3), optional downloads | ✅ |
| Crop, straighten tool + auto straighten, flip, rotate, aspect ratios, overlays | ✅ |
| Profiles (Color, Neutral, Vivid, Landscape, Portrait, Monochrome: our own looks), presets, versions, history, copy/paste/sync settings and metadata, Auto Sync, per-photo Auto | ✅ |
| Camera colour: DNG files use their own matrices; import your own `.dcp` profiles | ✅ DNG, `.dcp` import · 🟡 built-in profiles for eleven bodies, spectral matrices for 52 more, dcraw matrices for ~550; chart / Lightroom calibration tools; measured calibration database missing |
| Native macOS menu bar (generated from the command registry), control channel + every widget addressable, headless UI snapshots | ✅ |
| RAW: DNG, CR2, CR3 (lossless CRX Bayer and version 0x100/0x200 C-RAW), ARW, NEF (uncompressed + lossless/lossy compressed), Fujifilm RAF (uncompressed + lossless/lossy compressed, Bayer + X-Trans), Panasonic RW2 / Leica RWL / Panasonic RAW (every raw format, DMC-LX1 to DC-S1RM2), Pentax PEF, Olympus ORF (uncompressed + compressed), Samsung SRW (uncompressed + compressed), CR3 tiled C-RAW, Minolta MRW, Sony ARW v1, packed NEF, SuperCCD RAF, Canon CRW, Epson ERF, Mamiya MEF; AVIF import; embedded previews for every format incl. CR3; 596 models verified | 🟡 · per-model verification continues |
| Detail: sharpening, luminance + colour noise reduction | ✅ · AI Denoise, Super Resolution ⬜ |
| Remove (content-aware fill) / Heal / Clone spots (auto source), Visualize Spots, Red Eye and Pet Eye (auto pupil detection, catchlight) | ✅ · spot pin editing 🚧 |
| Export: JPEG / PNG / TIFF / WebP / AVIF / DNG / original, sizing, file-size limit, output sharpening, naming templates, batch, metadata policy, text or image watermark | ✅ |
| HDR: HDR editing with a headroom limit, SDR rendition, Visualize HDR; export as ISO 21496-1 gain map JPEG, PQ or HLG AVIF, or 32-bit float TIFF; HDR display on macOS (EDR), Windows (HDR) and Linux (Wayland) | ✅ · HDR display of zoomed tiles and Before/After 🚧 |
| Colour-managed display: previews follow the monitor profile on macOS, Linux (X11 / colord) and Windows (WCS) | ✅ |
| Library persistence (crash-safe op log + snapshots, background compaction, failed saves reported), disk thumbnail cache | ✅ |
| Import: Add in place / Copy / Move, rename and folder templates, devices, duplicate detection, watched folders; Local folder browsing | ✅ |
| Output: Print (printer ICC, intent, BPC, Print Adjustment, 16-bit, `ipps://`, driver options), Book (PDF, pair kerning), Slideshow (PDF/JPEG/video with music), Web (HTML galleries, SFTP), publish services with grid sections | ✅ |
| MCP server (headless or live app, persistent libraries), CLI, control channel | ✅ |
| XMP sidecars (read/write, auto-write), reading `crs:` develop settings, preset files (`.lcpreset`, XMP presets) | ✅ |
| Optics (distortion, vignetting, auto + manual CA, defringe, lens corrections embedded in DNG files and Panasonic / Leica RW2 / RWL distortion data), Geometry (transforms, Constrain Crop), Upright (Auto/Level/Vertical/Full/Guided) | ✅ · lensfun-format lens database set up or downloaded from Settings ▸ Lens profiles, with a lens picker for manual or misnamed lenses 🟡 · lens profiles of our own ⬜ |
| Photo Merge: HDR (auto-align, deghost), Panorama (spherical/cylindrical/perspective, boundary warp, auto crop), HDR Panorama → DNG | ✅ |
| GPU pipeline (wgpu compute, CPU-exact within 1/255), CPU fallback on device limits / errors | ✅ · WebGPU in the browser 🚧 |
| Video: MP4/MOV/MKV/WebM/TS/MXF decode (H.264, HEVC, VP9, AV1, ProRes, DNxHR, APV, MPEG-2), playback, trim, poster frame, Capture Frame, global grading, H.264 / ProRes export with audio | 🟡 · local adjustments and hardware decode on video ⬜ |
| AI: segmentation masks (BiRefNet, SAM 3), Bayer AI denoise, faces (whole networks on the GPU when faster, batched) | 🟡 · super resolution ⬜ (see [roadmap](ROADMAP.md#where-were-going)) |
| Web build (same UI in the browser via WASM): persistent library in OPFS/IndexedDB, Web Worker rendering, export downloads | ✅ · WebGPU, Safari/Firefox testing 🚧 |

<sub>✅ works today · 🚧 in progress · ⬜ not started</sub>

## How it's built

An engine-first Cargo workspace of small, tested crates with enforced layering (`cargo xtask layers`): `geom`,
`color`, `raster`, `tiff` → `raw`, `codecs`, `meta`, `develop` → `pipeline` → `catalog` → `engine` → `ui-egui`. The
egui frontend is one swappable crate; nothing below it knows a UI exists. `mcp` and the apps (`app`,
`<cli>`) sit on top of `engine`.

## Contributing

Humans and agents follow the same rules, so read [AGENTS.md](AGENTS.md) first. The short version:

- **Clean-room.** Never read Adobe binaries or GPL / LGPL raw and photo code (darktable, RawTherapee, LibRaw,
  rawspeed…); work from public specs and black-box observation. One exception: dcraw's functions outside its
  RESTRICTED (Foveon) block, which its author releases free for all uses, may serve as a reference under the dcraw
  reference policy in [AGENTS.md](AGENTS.md#clean-room-sources-in-addition-to-the-non-negotiables).
- **No Adobe assets, ever:** no icons, screenshots, presets, profiles, LUTs or fonts from Adobe products. Every
  image, icon and font in the repo is original, public domain, Creative Commons, OFL or permissively licensed, and has
  an entry in [assets/ATTRIBUTION.md](assets/ATTRIBUTION.md) added in the same commit. New fonts go to
  [storytold/craft-fonts](https://github.com/storytold/craft-fonts), not here.
- **Pure Rust**, enforced crate layering, everything is a command, and `cargo xtask ci` green before every commit
  (one task id per commit).
- **Never crash.** Non-test code returns errors instead of panicking: no `unwrap()`, `expect()`, `panic!` or
  `unsafe`, checked indexing on anything derived from input, and a regression test with every crash fix. Details in
  [AGENTS.md](AGENTS.md#never-crash-outranks-feature-work).

To take something on, claim its issue with a comment and open a pull request with `Fixes #123` in the description;
[CONTRIBUTING.md](CONTRIBUTING.md) has the whole workflow. Questions, ideas or a bug you'd like to talk through
first? [Join the Discord](https://discord.gg/c3dfYATzsQ) and bring them to [the project's channel](https://discord.com/channels/1359579021108842617/1559361045468942346).


## Licence and credits

The app is licensed under the [MIT License](LICENSE), except `crates/segment` and `crates/fetch`, which are
Apache-2.0 only. It is a fork of an open-source project; that project's copyright notices and required
third-party notices are in [LICENSE](LICENSE) and [NOTICE](NOTICE).

Privacy: the app collects no data; see [PRIVACY.md](PRIVACY.md).

Bundled fonts, icons, images and other assets keep their own open licenses; each one is listed
with its author, source and license in [assets/ATTRIBUTION.md](assets/ATTRIBUTION.md).

The screenshots show the demo library, which the app generates procedurally. UI font:
Inter (SIL OFL 1.1). Builds made with [craft-fonts](https://github.com/storytold/craft-fonts) (all official releases)
also embed its Chinese and Japanese fonts (Noto Sans CJK SC, BIZ UDPGothic, BIZ UDMincho, Shippori Mincho; SIL OFL 1.1), listed in its
[ATTRIBUTION.md](https://github.com/storytold/craft-fonts/blob/main/ATTRIBUTION.md). All icons are original.


<sub>Adobe, Photoshop, Illustrator, Premiere Pro, Lightroom, Acrobat, After Effects and InDesign are trademarks or registered trademarks of Adobe Inc. in the United States and/or other countries. The app is an independent, open-source project and is not affiliated with, sponsored by or endorsed by Adobe Inc.; these names are used only to describe the workflows it is compatible with.</sub>

