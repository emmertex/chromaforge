# LightCraft plugin for Logi Options+

Use a **Logitech MX Creative Console** (Keypad + Dialpad) with [LightCraft](https://github.com/storytold/lightcraft),
the open-source photo library and raw developer.

This plugin is deliberately thin: it forwards dial ticks and key presses to LightCraft and draws what LightCraft sends
back (key icons, labels, live values, the dial's value). **LightCraft decides what every key and dial does** — its
profiles (Default, Wedding, Portrait, Fashion, Landscape, Real Estate), pages that follow the tool you use, dial speed
and acceleration, one undo step per turn, applying a turn to every selected photo, the on-screen readout. All of that
is set up in LightCraft: **Settings ▸ Controls**. Nothing needs assigning in Logi Options+.

The engine in LightCraft is a port of TactileBridge, the author's own Capture One bridge for the MX Creative Console.

Copyright © 2026 RAD Films - Roman Alurkoff. MIT License (see `LICENSE`).

## How it works

```
MX Creative Console ──► Logi Options+ / Logi Plugin Service ──► LightCraftPlugin.dll
                                                                   │  JSON lines over 127.0.0.1 (surface protocol v1)
                                                                   ▼
                                                     LightCraft (Settings ▸ Controls)
```

- The plugin connects to LightCraft's control-surface port (default `127.0.0.1:7981`; the plugin data folder's
  `config.json` sets another, or `LIGHTCRAFT_SURFACE_PORT`).
- The first time, it asks to **pair**: the keys show *Pair* and a six-digit code, and LightCraft lists the request in
  Settings ▸ Controls with the same code. Approve it there. The plugin keeps its token
  (`…\Logi\LogiPluginService\PluginData\LightCraft\token.txt`); LightCraft stores only a hash of it.
- Then it subscribes to the MX Creative Console's layout and LightCraft pushes every change (a value turning, a page
  switch, a context change).
- Actions it registers (one Options+ action per parameter; the parameterless SDK constructor is what makes each
  parameter its own action — with the display-name constructor Options+ knows only one action and every profile key
  shows a warning triangle):
  - **LightCraft Key** (`p1k1`–`p3k9`: Keypad page 1–3, key 1–9; `k1`–`k9`: LightCraft's current key page;
    `pad1`–`pad4`: the Dialpad's buttons) and **LightCraft Dial** (`dial`, `roller`), in the category *0. Keys and
    dials set in LightCraft*. The default profiles it ships (`profiles/DefaultProfile70.lp5` for the Keypad,
    `DefaultProfile71.lp5` for the Dialpad) put them on every control. Their images are drawn from LightCraft's
    layout: icon, value, lit border; dimmed from the last layout while LightCraft isn't running.
  - **LightCraft: Loupedeck+ button** (`b33`–`b116`) and **LightCraft: Loupedeck+ knob** (`r1`–`r48`): the
    Loupedeck+'s controls by Loupedeck's own numbers, forwarded as LightCraft's `ld.b<n>` / `ld.r<n>`. The default
    profile `profiles/DefaultProfile15.lp5` puts each on its control, and the package's `applicationPatterns` make it
    the console's profile whenever LightCraft is in front. LightCraft decides what they do (by default what the
    console's labels say; Settings, Controls, Loupedeck+ imports the user's own Loupedeck profile's layout).
  - **LightCraft Action** and **LightCraft Value**: every action and value in LightCraft's catalogue
    (`surface.catalog`), in numbered categories (*1. Light*, *2. Color* …), with icons and live values
    (`surface.watch`), to put on keys, dials or the Actions Ring in Options+ directly.
- `config.json` can also set `"textContrast"`: `none` (the default: the device draws a key's text), `outline`,
  `shadow` or `strip` (the plugin draws the text into the key's image with a dark outline, a soft shadow or a dark
  strip behind it, for icon packs that are bright where the text sits).
- The client keeps the last layout, the icons and the catalogue in `PluginData\LightCraft\cache`, so Options+ shows
  icons and the action list before LightCraft starts.
- Haptics: the plugin raises Logi plugin events *Key pressed*, *Value reset* and *Value at its limit* when LightCraft's
  haptics setting is on; Options+ can map them to haptic feedback on devices that have it.

The protocol is documented in LightCraft's `docs/control-surfaces.md`.

## Layout

| Path | What |
|---|---|
| `src/LightCraft.Surface.Client/` | the protocol client (no Logi dependency): connect, pair, subscribe, input, events, reconnect |
| `src/LightCraftPlugin/` | the Logi Actions SDK plugin (`LightCraftPlugin.dll`), its `metadata/` and default `profiles/` |
| `tools/GenerateProfiles/` | writes the default Options+ profiles (ported from TactileBridge's generator) |
| `tools/Harness/` | test harness: drives a running LightCraft through the same client and checks the results |
| `scripts/package.ps1` | builds `dist/LightCraft.lplug4` (the plugin package) |
| `scripts/install.ps1`, `scripts/uninstall.ps1` | install into / remove from Logi Options+ on this PC |

## Build

Needs the .NET 8 SDK and Logi Options+ (its Logi Plugin Service provides `PluginApi.dll`).

```powershell
dotnet build src\LightCraftPlugin\LightCraftPlugin.csproj -c Release
dotnet run --project tools\GenerateProfiles -c Release -- src\LightCraftPlugin\profiles
```

The current Logi Plugin Service runs on .NET 10 and its `PluginApi.dll` references .NET 10's `System.Runtime`, which the
.NET 8 compiler refuses as a reference. The plugin project therefore compiles against the runtime assemblies the
service ships (`C:\Program Files\Logi\LogiPluginService`, property `LogiDir`); the output is an ordinary `net8.0`
library the service loads.

## Test

Start LightCraft with its full control port and control surfaces on (Settings ▸ Controls), then:

```powershell
dotnet run --project tools\Harness -c Release -- 18532
```

It pairs (clicking Approve through the control channel, as you would), checks the layout, turns the dial and checks
Exposure moved and the new value was pushed, undoes it, presses Keypad page 1 key 2 (Next photo in the Default
profile), and forgets its pairing again. It uses its own client id, so it never touches the plugin's own pairing.

## Install

LightCraft installs the plugin itself: **Settings ▸ Controls** offers *Install the plugin* when Logi Options+ is on the
computer, using the `LightCraft.lplug4` that comes with LightCraft. By hand:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\package.ps1             # builds dist\LightCraft.lplug4
powershell -ExecutionPolicy Bypass -File scripts\install.ps1             # package + install, LightCraft's surface port 7981
powershell -ExecutionPolicy Bypass -File scripts\install.ps1 -Port 18532 # another port
```

`install.ps1` writes the plugin's `config.json` and hands the package to Logi's own installer
(`LogiPluginServiceTool.exe install-plugin`, what double-clicking a `.lplug4` does). **Logi Options+ asks you to
confirm** ("Ready to install plugins", Continue), installs the plugin and its default profiles, and reloads its plugin
service by itself. Other plugins (such as TactileBridge's Capture One plugin), their profiles and your Options+
settings are left alone.

## Uninstall

In LightCraft: Settings ▸ Controls ▸ Profiles and Setup ▸ *Remove the plugin* (Logi Options+ asks to confirm), or

```powershell
powershell -ExecutionPolicy Bypass -File scripts\uninstall.ps1
```

which removes `Plugins\LightCraft`, `PluginData\LightCraft` (the token and cache) and the `@_lightcraft` profiles under
`Applications\Loupedeck70` / `Loupedeck71` / `Loupedeck72`, restarting the Logi Plugin Service once. Then, in
LightCraft, Settings ▸ Controls ▸ *Forget* the paired plugin.

## Credits

- The plugin icon (`src/LightCraftPlugin/metadata/Icon256x256.png`) is LightCraft's app icon, from the LightCraft
  repository (`assets/app-icon`, MIT OR Apache-2.0).
- Key icons are sent by LightCraft: TactileBridge's hand-drawn icon pack (© RAD Films - Roman Alurkoff, MIT), in
  LightCraft's `assets/surface-icons`.
