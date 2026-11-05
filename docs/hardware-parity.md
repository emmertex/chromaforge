# Hardware parity

> **Last reviewed:** 2026-10-10 · **Last updated:** 2026-11-05 · **Change:** minor (HDR display on GNOME / Mutter: BT.2100 PQ float surface fallback); previously minor (control surfaces from OneSlips: the MX Creative Console, Loupedeck+) · **Target:** Adobe Lightroom Classic 15.5 / Lightroom 9.6

What Lightroom does with the machine's hardware, per platform, and what the app does. Sources:
[gpu-pipeline.md](gpu-pipeline.md), [display-profiles.md](display-profiles.md), [denoise.md](denoise.md),
[ai-masks.md](ai-masks.md), `crates/gpu`, `crates/engine/src/devices.rs`, `crates/ui-egui/src/panels/second.rs`.

| Feature | Target | The app: macOS | Windows | Linux / BSD | Web | Status |
|---|---|---|---|---|---|---|
| GPU for display and editing | Metal / DX12, "Use GPU for display / image processing / export" | wgpu Metal: geometry, resize, blur, WB, NR, guided filter, dehaze, masks, finish; CPU fallback per render | DX12 only (Vulkan opt-in, #136) | Vulkan (CPU ↔ GPU equivalence verified on an AMD Radeon 8060S, RADV) | CPU only (no WebGPU yet) | 🟡 defringe, spot removal, heuristic masks, long brushes and high-bit exports stay on CPU |
| GPU export | ✅ | ✅ (6000 × 4000 in ~0.3 s vs ~1.2 s CPU) | ✅ | ✅ | — | ✅ (black-export fix on an Intel iGPU, #78) |
| AI on GPU / neural engine | Metal / DirectML / Core ML, model per feature | AI denoise on GPU or CPU (setting); SAM 3 on Metal via candle; faces on CPU | CPU / GPU denoise | CPU / GPU denoise | — | 🟡 no Core ML / Neural Engine / DirectML |
| HDR (EDR) display | ✅ (macOS EDR, Windows HDR) | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ (LR-VIEW-HDR-DISPLAY) |
| Monitor profile | automatic per display, follows the window | 🟡 picked by hand | 🟡 | 🟡 | — | 🟡 macOS and Linux read the screen's profile and follow the window (X11 `_ICC_PROFILE_n` per RandR monitor, colord); Wayland gets the first colord display only; Windows reads the WCS default profile of the first display device (registry), not per window |
| Second window / second monitor | ✅ (Classic) | ✅ (⌘F11) | ✅ | ✅ | — | ✅ (Classic secondary-window keys missing) |
| Pen pressure / tilt (brush masks) | ✅ pressure for size / flow | ⬜ winit reports no tablet pressure on macOS | 🟡 pen force from winit touch events (untested on a tablet) | ⬜ winit has no X11 / Wayland tablet input | 🟡 pointer-event force where winit passes it | 🟡 brush size and flow follow pen pressure wherever egui gets a touch force (Brush ▸ Pen Pressure); no tilt; macOS / Linux tablets wait on winit |
| Trackpad gestures | pinch zoom, pan | ✅ pinch, two-finger pan | 🟡 (winit) | 🟡 | 🟡 | ✅ on macOS |
| Card readers / cameras as import sources | ✅ (devices panel, eject after import) | ✅ mounted volumes with DCIM | ✅ | ✅ | — | 🟡 no eject-after-import check, no MTP / PTP phones |
| Tethered capture | ✅ Classic (Canon, Nikon, Sony, Leica) | ⬜ | ⬜ | ⬜ | — | ⬜ |
| Printers | ✅ Print module, printer colour management | ⬜ (PDF contact sheets only) | ⬜ | ⬜ | — | ⬜ |
| Control surfaces (MIDI / Loupedeck via plug-ins) | via SDK plug-ins | built-in surface engine ([control-surfaces.md](control-surfaces.md)): profiles and pages, pages that follow the tool, dial feel and acceleration, one undo step per turn, apply to every selected photo, on-screen readout, Settings ▸ Controls built around a drawing of each device as it is laid out (action picker by category, drag and drop, Touch to find), the Actions Ring's slots, pairing; Logitech MX Creative Console through the Chromaforge Logi plugin, installed from Settings ▸ Controls, with Chromaforge's actions listed in Logi Options+; the Loupedeck+ through the same plugin (works as labelled, imports a Loupedeck profile's layout); TactileBridge setups import | same (plugin verified on Windows) | engine and protocol, no Logi Options+ there | — | 🟡 MX Creative Console ✅; Loupedeck+ (untested on the hardware); MIDI and Tangent panels not yet |
| Multi-core CPU | ✅ | ✅ worker pools, parallel JPEG decode, parallel import copy | ✅ | ✅ | single thread | ✅ |
| Memory pressure | managed | ✅ memory budget, `crates/sysmem` returns allocator pages on macOS | ✅ budget | ✅ budget | — | ✅ |

## Score

**~40% ready** (estimated): the GPU editing path is real and fast (measured: 24 MP exposure drag
3.9 ms on GPU vs 22–42 ms CPU; NR drag 18 ms vs 301–471 ms), but HDR display, pen pressure,
tethering, printing, Windows monitor profiles and neural-engine inference are missing. GPU
correctness is proven on Apple GPUs; other vendors rely on user reports.

## Remaining (Opus 5.5 agent hours)

| Work | Hours | Needs |
|---|---:|---|
| HDR display (EDR on macOS, Windows HDR, Linux where possible) | done (2026-10-14, #47) | the vendored egui-wgpu (`vendor/egui-wgpu`, see `vendor/README.md`) configures an `Rgba16Float` surface in extended linear sRGB (scRGB / EDR) where offered, reads the display's headroom and SDR white (`wgpu::DisplayHdrInfo`), and draws the UI at paper white; the loupe shows HDR edits from a float render up to the headroom (whole-frame render; zoomed tiles stay SDR; the float HDR render runs on the GPU when there is one, 2026-10-21). X11 / XWayland stays SDR: X11 has no HDR surface or headroom protocol, a platform limit. On Wayland the headroom comes from the compositor's colour-management protocol (output image description: target max ÷ reference; `apps/app/src/wayland_hdr.rs`), the app then runs native Wayland instead of XWayland, and `crates/hdrmeta` sets Vulkan HDR metadata (peak = headroom × 80 cd/m² × paper white) so KWin shows the range; where the compositor lacks `extended_target_volume` (GNOME's Mutter: Mesa then offers no extended-linear colour space) the window uses an `Rgba16Float` BT.2100 PQ surface instead (the shader converts to BT.2020 and PQ-encodes; SDR white is `<PREFIX>_HDR_PAPER_WHITE`, else the compositor's reference white, else 203 cd/m²), checked by eye on GNOME 50.1 / Mesa 26.2.6 (RX 7900 XTX, 700 cd/m² OLED); overrides `<PREFIX>_HDR_HEADROOM` / `<PREFIX>_HDR_PAPER_WHITE`. `<binary> --hdr-probe` reports what a machine offers. Verified on Windows 11 / RTX 4090 (DX12, `Rgba16Float` scRGB, headroom ≈ 12.9×); Linux: the loupe in HDR confirmed on a laptop panel with KWin 6.7.5 EDR (AMD RADV, headroom 1.4× from the compositor); macOS EDR confirmed on an Apple M1 Max built-in XDR display (2026-10-15); results per machine: PR body |
| Per-monitor profile on Windows and Wayland | 2–4 | multi-monitor setup |
| Pen pressure for brush masks: done where winit reports a touch force; macOS and Linux tablets need winit tablet events (or a pure-Rust tablet layer) | 3–8 | a tablet |
| Remaining stages on GPU (defringe, spot removal, masks) | 10–20 | |
| Tethered capture: native PTP exists (pure Rust, USB via nusb, PTP/IP, simulated camera); verify on Canon / Nikon / Sony bodies | 5–15 | cameras (human) |
| Printing with colour management | 15–25 | printers (human) |
| Neural-engine / DirectML inference for AI features | 10–20 | depends on the AI model decision |
| Control surfaces: MIDI (pure-Rust input backend, relative encoders, feedback) and Tangent panels (from Tangent's public docs) on the existing engine | 10–20 | a MIDI controller and a Tangent panel (human) |
| **Total** | **90–170 h** (the core set for beta, HDR display + monitor profile + pen pressure: 20–40 h) | |

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-14 | minor | Control surfaces: the built-in engine (ported from TactileBridge), Settings ▸ Controls, the surface protocol, the MX Creative Console via the Chromaforge Logi plugin; MIDI and Tangent still to come |
| 2026-10-15 | minor | Control surfaces: Settings ▸ Controls rebuilt around the device; the Logi plugin installs from Chromaforge; Chromaforge's actions in Logi Options+ |
| 2026-10-16 | minor | Control surfaces: the Loupedeck+ |
| 2026-10-17 | minor | Control surfaces: the Actions Ring, Touch to find, drag and drop, devices drawn as laid out; the Loupedeck+ and the ring need no driver setup (plugin 1.0.7) |
| 2026-10-12 | minor | Monitor profile: Linux reads the display profile from colord; macOS row reflects #787 |
| 2026-10-12 | minor | CPU ↔ GPU equivalence suite run on an AMD Radeon 8060S (Vulkan, RADV, Linux): 16 tests / 135 cases pass, max 2 LSB, mean ≤ 0.0003; first GPU verified outside Apple |
| 2026-10-12 | minor | Pen pressure: strokes record per-point pressure from egui touch force, scaling dab size and flow; per-platform status |
| 2026-10-10 | major | Created |
| 2026-10-13 | minor | Windows: the display's WCS default profile from the registry (first display device); 3–6 h → 2–4 h |
| 2026-10-13 | minor | Linux: the screen's profile from X11 `_ICC_PROFILE` / colord, followed across monitors; 5–10 h → 3–6 h |
| 2026-10-14 | minor | HDR display row done: vendored egui-wgpu patch, loupe in HDR on macOS EDR / Windows HDR, `--hdr-probe` (#47) |
| 2026-11-05 | minor | HDR display row: BT.2100 PQ float surface where the compositor offers no extended-linear colour space (GNOME / Mutter), ported from the upstream HDR display PR (contributed by @cromefire) |
