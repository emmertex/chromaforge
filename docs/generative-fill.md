# Content-aware and generative fill

> **Last updated:** 2026-10-14 · **Change:** new page (structure-aware Fill, source exclusion,
> masks as spots; generative fill through the user's own cloud API key, placed in the photo's
> scene-linear pixels)

The Remove panel has two ways to replace an area with something new instead of copying one source:

- **Content-Aware** (Fill): LightCraft's own patch synthesis, on this computer, no model.
- **Generative**: a cloud image model you choose and pay for (your own API key). Off until you set
  it up; nothing is sent until you click **Generate**.

Both start from a painted stroke, or from a mask: select something in Masking (an **Object** mask
from a click with SAM 2.1, a brushed mask, a People mask…) and click **Fill Selected Mask** or
**Generate into Selected Mask** in Remove (`spot.fromMask`). The mask becomes a spot made of dabs
that cover it; the mask itself is removed (undo brings it back).

## Content-Aware (Fill)

`crates/inpaint` (PatchMatch completion, ported from PhotoCraft) rebuilds the hole from patches of
its surroundings, coarse to fine:

- **Structure first.** Straight edges and thin lines that cross the hole (a horizon, a door frame, a
  power line, a fence rail) are found on a ring around it (Hough votes on edge pixels, kept only when
  both sides match), refit to sub-pixel accuracy and continued through the hole: each line adds a
  guide channel (signed distance to the line) to the patch distance, and PatchMatch also tries
  copying the band along the line from beyond the hole. Texture without such lines fills as before.
- **Multi-scale.** The structure is decided at a scale set by the spot's size, so a preview and a
  full-size export agree.
- **Never copies from the wrong place.** Other spots' areas and faces (face regions of the photo)
  are excluded as sources at every pyramid level.
- **A little more than painted.** Fill and Generate grow the painted radius by 5 % + 1 px to take in
  an object's soft edge and the start of its shadow; the completion also covers one extra pixel.
- **Perceptual matching.** Patches are compared on the square root of the linear values, so
  highlights don't dominate every match.

Measured on synthetic cases (hole error, 200 × 200 box, radius 32; before → after): diagonal edge
0.030 → 0.0007, power line 0.100 → 0.015, horizon 0.064 → 0.037, stripes 0 → 0, noisy texture
unchanged, an excluded red block copied 1,749 → 0 pixels. Time (release): 400 px box with a 60 px
hole 60–150 ms, 1200 px box with a 200 px hole 0.26–0.45 s. The spot size limit (radius up to 25 %
of the long edge) is unchanged.

## Generative fill

### Providers

| Provider | Model (fill / remove) | Mask | Native crop | About | Uploads used for training |
|---|---|---|---|---|---|
| **OpenAI** (default) | gpt-image-1.5 edits | yes | 1024², 1536 × 1024, 1024 × 1536 | $0.13–0.20 / image (high quality) + input tokens | no (API); 30-day abuse retention |
| Stability AI | Inpaint / Erase | yes | up to 3072 px a side | $0.05 | **yes, unless you switch it off** in your account |
| Black Forest Labs | FLUX.1 Fill [pro] / Erase | yes | up to 2048 px | $0.05 | **terms allow it**; no images of anyone under 18 |
| fal.ai | FLUX Fill / object removal | yes | up to 2048 px | $0.05 / MP | not stated; LightCraft asks fal not to store the request |

Prices and policies were read from each provider's own pages on 2026-10-14; the consent dialog
shows the summary and links the page. Adobe Firefly is not offered: LightCraft is a clean-room
alternative to Adobe's apps and uses no Adobe services or assets (and the API is enterprise-only).
Google's Gemini image models have no pixel mask (prompt-only edits) and Replicate takes inline images
only up to 256 KB; both are possible later.

### Setting it up

1. Create an API key in your account with the provider (Settings ▸ Generative Fill ▸ *Get an API key*).
2. Paste it in Settings ▸ Generative Fill and click **Save Key**. Or set
   `LCC_GENFILL_<PROVIDER>_KEY` (e.g. `LCC_GENFILL_OPENAI_KEY`) before starting LightCraft.
3. The first Generate with a provider opens a dialog that shows the crop that will be uploaded (the
   area to fill tinted red), what the provider does with uploads, a link to its policy, and the
   cost. **Agree and Generate** records your consent for that provider; Settings ▸ Generative Fill
   withdraws it.

Keys are kept in `genfill-keys.json` in LightCraft's settings folder (`%APPDATA%\LightCraft` on
Windows, `~/Library/Application Support/LightCraft` on macOS, `~/.config/lightcraft` on Linux),
written with owner-only permissions on macOS and Linux. They are never in the catalog, XMP, exports
or logs, never returned by a command (`genfill.providers` shows where a key comes from and its last
four characters), and removed from any error a provider sends back. `genfill.setKey` and
`spot.generate` are never journaled. Windows Credential Manager / macOS Keychain storage would need a
native call, which LightCraft keeps in `crates/sysmem` only: a proposed follow-up.

### What is sent

Only a **context crop** around the spot, with the mask of the area to fill — never the whole photo
(unless it is smaller than one crop), never its metadata:

- The crop is as large as the provider takes **at the photo's native resolution** (OpenAI: one of
  its three sizes; others: up to 2048–3072 px), centred on the hole, so the model sees the scene.
- A hole too large for one native crop is either sent as **tiles** along it, at native size, when
  it is long and thin (a wire, a fence; at most 8 crops, each seeing the ones before it filled), or
  **downscaled**: the hole with a little context, sent smaller and upscaled back. That patch is
  softer than the photo, and the panel and the consent dialog say so ("generated at 47 %").
- **Variations** (1–4): pick with ‹ › under the spot; **Generate Again** makes new ones.

### Where the patch goes: the photo's own pixels

Image models read and write display-referred 8-bit RGB. LightCraft edits scene-linear camera data.
A generated patch is brought back to the raw-level picture and placed **before every edit**:

1. **Sending.** The crop is cut from the full-size source: the demosaicked raw, linear, at the
   camera's as-shot white balance (LightCraft's raw development before any user edit). It goes
   through a fixed, invertible transform (`lcc_genfill::encode`): one gain per generation
   (the crop's bright end near white), linear sRGB primaries, a flat log curve with two stops of
   highlight headroom, 8 bits. The look is flat, like a camera's log profile; models continue it in
   the same flat look.
2. **Receiving.** The same steps backwards give linear as-shot light again. The result is then
   **matched** to the original pixels around the hole: the per-channel ratio of original to
   generated on a ring outside the hole, spread smoothly over it, corrects the model's colour and
   level shifts (±2 stops at most), like Heal's tone correction.
3. **Storing.** The matched pixels, at the **native resolution** of their area, and their blend
   weight (1 over the hole, feathered to 0, 0 on an outer ring) are a `.lcpatch` file (32-bit float
   RGB + weight, zlib) in the library's folder `genfill/<photo id>/<generation id>-<n>.lcpatch`.
   Its place is stored in normalized source coordinates. The spot in the develop settings (and XMP)
   only refers to it: `generated: {id, variants, variant, provider, prompt, stroke, scale}`.
4. **Rendering.** First thing in the scene-linear stage, before the raw's own local tone map, white
   balance, defringe, other spots, red eye, noise reduction, exposure, tone, colour, grading, masks
   and effects (`crates/pipeline/src/genpatch.rs`). It is sampled through the render's own mapping
   (orientation, lens corrections, perspective, crop, a zoomed window), which is the same as pasting
   it into the source before that resample. So **white balance, exposure, tone, colour, masks, crop,
   rotation, lens corrections and noise reduction changed after generating apply to the patch as to
   the rest of the photo**, at any export size.
5. **Noise.** The patch is clean. Each render measures the noise of the original pixels around it
   in the image it is rendering (a Poisson-Gaussian fit, variance `a·L + b` per channel, from the
   high-pass residual) and adds the noise the patch lacks, before noise reduction. So the patch has
   the photo's grain at every size, the same noise reduction removes it, and a photo developed from
   its AI-denoised picture (which has almost none) gets almost none.

What changes a patch: only what is upstream of it — the raw decode itself (a new LightCraft version
that demosaics differently) and the AI Denoise amount (the patch keeps its pixels; only the noise
added changes). The patch belongs to the stroke it was made for: moving or resizing the spot shows a
content-aware fill until it is generated again.

Limits:

- **Highlights.** 8 bits over two stops of headroom: areas above about 4× the crop's white clip in
  what is sent, and come back as flat highlight; near-clipped skies lose some gradation.
- **Out-of-gamut colours** (saturated neon, some flowers) clip to sRGB in what is sent; matching pulls
  the average back, not every hue.
- **Resolution.** A downscaled generation is softer; the warning says by how much.
- **Model changes.** A model redraws its whole crop; only the painted area (and its feather) is kept.
- **Missing files.** A patch file that is gone (library moved without its `genfill` folder, or the
  XMP opened elsewhere) is never a crash: the spot renders as a content-aware fill and the panel
  says to generate it again.

### Trying it without an account

`lcc-cli genfill-mock --port 18510` serves the four providers' request and response shapes on
127.0.0.1 and fills holes with LightCraft's own content-aware fill. Start LightCraft with
`LCC_GENFILL_MOCK=http://127.0.0.1:18510`, save any key, and Generate goes to the mock. Only
loopback addresses are accepted there, so a key can't be sent elsewhere by mistake.

## Commands

| Command | Params | Result |
|---|---|---|
| `spot.add` | `{mode: generate, points, size, …}` | a Generative spot (renders as Fill until generated) |
| `spot.fromMask` | `{mask, mode?: fill\|generate, deleteMask?}` | a spot covering the mask |
| `genfill.providers` | `{}` | providers, key status (never the key), consent, policy, price |
| `genfill.setKey` | `{provider, key \| null}` | stores or forgets your key |
| `genfill.consent` | `{provider, accept}` | records or withdraws consent |
| `genfill.plan` | `{index?, provider?, variations?, prompt?, previewPath?}` | requests, cost, crops, scale, warnings; sends nothing |
| `spot.generate` | `{index?, provider?, variations?, prompt?}` | generates (background in the app, activity row with ✕) |
| `spot.variation` | `{index?, variant \| delta}` | which variation a spot shows |
| `genfill.status`, `genfill.cancel` | `{}` | the running generation |
