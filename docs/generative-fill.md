# Content-aware and generative fill

> **Last updated:** 2026-10-23 · **Change:** a paid fill is never lost (test note 5): a generated spot keeps its
> stroke, Generate Again keeps earlier results, every generation can be restored, Retry fetches what the service
> already made; the HTTPS client no longer corrupts slow uploads ("BadRecordMac"). Before (2026-10-17): a fill is
> never sent in pieces: one request or none
> ("The model can't take an area this large. Please select a smaller area."). Before (2026-10-16):
> first real-account test (OpenAI): the HTTPS upload fixed, a Generative spot only marks its area until its result
> arrives, the short consent dialog, errors with Retry, a card per service. Before: new page
> (2026-10-14).

## The Remove tools

| Tool | What it does | Source |
|---|---|---|
| **Remove** | Chromaforge decides: a click copies the best-matching area nearby and blends it in (as Heal), and chooses again when the spot is moved; a painted stroke is rebuilt from its surroundings (as Content-Aware) | its own |
| **Content-Aware** | rebuilds the area from small pieces of its surroundings, continuing lines and edges (below) | none |
| **Generative** | an online image model with your own account (below) | none |
| **Heal** | the texture of a source, the colour and tone of the destination's surroundings: a gradient-domain (Poisson) blend, the membrane of the edge's difference, at any size of area | yours (a pin, starting at the best match) |
| **Clone** | the source exactly as it is, with a soft edge | yours |

Each button's tooltip says the same in a sentence. The panel opens on the tool used last.

The Remove panel has two ways to replace an area with something new instead of copying one source:

- **Content-Aware** (Fill): Chromaforge's own patch synthesis, on this computer, no model.
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
- **No seams.** The completion also fills a 2-px ring of known pixels around the hole; the mismatch
  there (smoothed along the ring) is spread over the fill as a smooth correction (a membrane, as in
  Poisson image editing), so a band copied from one side of a long hole meets the other side without
  a hard, straight edge. A fill that already matches is unchanged.
- **Multi-scale.** The structure is decided at a scale set by the spot's size, so a preview and a
  full-size export agree.
- **Never copies from the wrong place.** Other spots' areas and faces (face regions of the photo)
  are excluded as sources at every pyramid level.
- **A little more than painted.** Fill and Generate grow the painted radius by 12 % + 1 px to take in
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
| fal.ai | FLUX Fill / object removal | yes | up to 2048 px | $0.05 / MP | not stated; Chromaforge asks fal not to store the request |

**Provenance marks.** OpenAI says images from its API carry C2PA provenance metadata *and* an invisible
SynthID watermark (help center: "C2PA and SynthID in OpenAI-generated images"); Google's image models
add SynthID; Black Forest Labs may embed C2PA Content Credentials and its terms forbid removing them;
Stability's API docs say nothing (third parties report C2PA on its hosted images); fal depends on the
model. **Chromaforge keeps only the pixels** of the filled area: C2PA metadata in the returned PNG is
not read, so it is not in the patch, the photo or any export. A watermark hidden in the pixels
(SynthID) may survive in the patch, weakened by the colour match and the added grain. The consent
dialog says so. Writing Chromaforge's own note that generative fill was used (XMP) is a follow-up.

Prices and policies were read from each provider's own pages on 2026-10-14; the consent dialog
shows the summary and links the page. Adobe Firefly is not offered: Chromaforge is a clean-room
alternative to Adobe's apps and uses no Adobe services or assets (and the API is enterprise-only).
Google's Gemini image models have no pixel mask (prompt-only edits) and Replicate takes inline images
only up to 256 KB; both are possible later.

### Which service, for what

Settings ▸ Generative Fill has a card per service: what it is best at, its weak spots, the largest
area it takes at full resolution, the price per fill, whether it trains on uploads and what marks
it adds. In short (2026-10-16):

- **OpenAI (GPT Image)**: best at following a written prompt and understanding the whole scene;
  redraws the whole crop (details can drift), at most 1536 px per request, 30–60 s per request,
  about $0.20. Does not train on API uploads.
- **Stability AI (Erase / Inpaint)**: best at removing without a prompt and at large areas (3072
  px); follows prompts less well; trains on uploads unless switched off in the account. About $0.05.
- **Black Forest Labs (FLUX Fill)**: photographic realism, textures, extending a scene (2048 px);
  its terms allow training on uploads with no opt-out; not for photos of children. About $0.05.
- **fal.ai**: FLUX Fill and object removal in one pay-as-you-go account (2048 px); does not say
  whether it trains on uploads. About $0.05–0.06.

Recommended for removing distractions from client photos: OpenAI, or Stability with training
switched off. Only OpenAI has been tried with a real account so far (below); a side-by-side
comparison of the services needs the user's keys and test crops.

### Setting it up

1. Create an API key in your account with the provider (Settings ▸ Generative Fill ▸ *Get an API key*).
2. Paste it in Settings ▸ Generative Fill and click **Save Key**. Or set
   `<PREFIX>_GENFILL_<PROVIDER>_KEY` (e.g. `<PREFIX>_GENFILL_OPENAI_KEY`) before starting Chromaforge.
3. The first Generate with a provider opens a short dialog: the crop that will be uploaded (the
   area to fill lightly tinted), its size and the cost ("About $0.20, billed to your OpenAI
   account"), with **Generate** and **Cancel**. What the provider does with uploads, its
   provenance marks and a link to its policy are behind **Details**; a warning shows only when it
   matters (the area will be softer; the service trains on uploads by default). **Generate**
   records your consent for that provider and starts; you are not asked again. Settings ▸
   Generative Fill withdraws it.

The Remove panel shows only the services that have a key (none: **Set Up a Service…**).
**Variations** is 1 unless you choose more (each is a paid result) and is remembered, with the
price per variation; once an area is painted the estimate shows the requests and the total. A
failed generation stays in the panel with what went wrong, **Retry** and **Dismiss** (and a
toast); while one runs, a spinner and "Generating…" sit over the area on the photo.

### A paid result is never lost

Every generation is paid for, so Chromaforge keeps it (test note 5: a fill "vanished" after the brush size,
changed with the generated spot still selected, resized the spot, and a nudge moved it):

- **Kept on disk first.** The patches (`genfill/<photo id>/<id>-<n>.lcpatch`) and a record of the generation
  (`<id>.json`: the stroke it was made for, the service, the prompt, when) are written before the result shows.
  Chromaforge never deletes them.
- **The spot keeps its stroke.** A spot that shows a generated fill is not moved or resized: the Size, Feather and
  Opacity sliders (and `[` / `]`, a console dial) set the next stroke's brush; dragging its pin says "A generated
  fill stays where it was made". A spot moved or resized by an earlier build shows **Put Back**
  (`spot.putBack`), which returns it to the stroke its fill was made for (from the fill, or the photo's history).
- **Generate Again keeps the earlier results.** They are offered with the variations ("Variation 2 of 3"), any
  can be shown again (`spot.variation` `result` / `delta`); a spot keeps up to 32 generations.
- **A finished generation always lands**, also when its spot was moved, resized or deleted while it ran (it goes
  back on its stroke, or on a spot of its own).
- **Restore.** "Fills made earlier for this photo, not on it now" lists every generation in the library that no
  spot shows (a deleted spot, an older build), each with **Restore** (`genfill.results`, `spot.restoreGenerated`).
- **Retry does not pay twice.** Black Forest Labs and fal accept a request, then the result is fetched from an
  address they give. When fetching fails, Chromaforge keeps that address: Retry (or Generate with the same
  choices) within a few minutes fetches the result instead of sending a new paid request, and the error says so.

### Connections

Every request opens a new connection (none is reused). A broken connection while asking for a result or
downloading it is followed by a new one (three tries); a paid request is sent again on its own only when it
certainly never reached the service (the connection broke while it was being sent, or the service's TLS layer
refused it with an alert). Otherwise the error says what happened, that it tried, and that a VPN, proxy or
antivirus that inspects secure connections may be in the way.

The "received fatal alert: BadRecordMac" of test note 5 was Chromaforge's own: the client waited on the socket
with 250 ms send timeouts and sent again after one, but on Windows a send that times out may already have sent
part of its data, so a slow upload could repeat bytes inside an encrypted record and the service refused it.
The sockets are non-blocking now (a send that can't go out sends nothing), tested against a local TLS server
that reads slowly.

Keys are kept in `genfill-keys.json` in Chromaforge's settings folder (`<settings_dir>` (per OS) on
Windows, macOS and Linux),
written with owner-only permissions on macOS and Linux. They are never in the catalog, XMP, exports
or logs, never returned by a command (`genfill.providers` shows where a key comes from and its last
four characters), and removed from any error a provider sends back. `genfill.setKey` and
`spot.generate` are never journaled. Windows Credential Manager / macOS Keychain storage would need a
native call, which Chromaforge keeps in `crates/sysmem` only: a proposed follow-up.

### What is sent

Only a **context crop** around the spot, with the mask of the area to fill — never the whole photo
(unless it is smaller than one crop), never its metadata:

- The crop is as large as the provider takes **at the photo's native resolution** (OpenAI: one of
  its three sizes; others: up to 2048–3072 px), centred on the hole, so the model sees the scene.
- **One request per fill, never several pieces** (the user's decision, 2026-10-16: each piece is a
  paid request, and pieces that don't match show seams). A hole too large for one native crop is
  **downscaled**: the hole with a little context, sent smaller and upscaled back. That patch is
  softer than the photo, and the panel and the consent dialog say so ("generated at 72 %"). A
  result sent smaller than native has the model's own grain smoothed away before the photo's grain
  is added.
- **Too large for one request.** When the area would have to go at under half the photo's
  resolution (`MIN_SCALE` in `crates/engine/src/genfill.rs`), nothing is sent and no cost is
  incurred: the estimate (`genfill.plan`), Generate (`spot.generate`) and the Remove panel all say
  *"The model can't take an area this large. Please select a smaller area."* (A long, thin area
  such as a wire across the frame counts by its whole extent.)
- The cost is known before sending: the panel and the consent dialog show the requests and the
  price; the patch's edge is feathered into the photo over the spot's feather (its blend weight
  falls from 1 over the hole to 0 on a ring of original pixels).
- **Variations** (1–4): pick with ‹ › under the spot; **Generate Again** makes new ones.

### Where the patch goes: the photo's own pixels

Image models read and write display-referred 8-bit RGB. Chromaforge edits scene-linear camera data.
A generated patch is brought back to the raw-level picture and placed **before every edit**:

1. **Sending.** The crop is cut from the full-size source: the demosaicked raw, linear, at the
   camera's as-shot white balance (Chromaforge's raw development before any user edit). It goes
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

What changes a patch: only what is upstream of it — the raw decode itself (a new Chromaforge version
that demosaics differently) and the AI Denoise amount (the patch keeps its pixels; only the noise
added changes). The patch belongs to the stroke it was made for: moving or resizing the spot leaves the photo
untouched there until it is generated again.

**Until its result arrives a Generative spot only marks its area**: painting with the Generative
tool runs no heal or content-aware fill (an early build did, and it looked like a poor AI result).
The same holds while a generation runs, after a move, and when the patch file is missing.

Limits:

- **Highlights.** 8 bits over two stops of headroom: areas above about 4× the crop's white clip in
  what is sent, and come back as flat highlight; near-clipped skies lose some gradation.
- **Out-of-gamut colours** (saturated neon, some flowers) clip to sRGB in what is sent; matching pulls
  the average back, not every hue.
- **Resolution.** A downscaled generation is softer; the warning says by how much.
- **Model changes.** A model redraws its whole crop; only the painted area (and its feather) is kept.
- **Missing files.** A patch file that is gone (library moved without its `genfill` folder, or the
  XMP opened elsewhere) is never a crash: the photo shows through and the panel says to generate
  it again.

### Tested with a real account (2026-10-16)

The first real upload failed with "secure connection failed: failed to write whole buffer": the
pure-Rust HTTPS client handed rustls the whole multi-megabyte body at once, and rustls takes 64 KiB
at a time. It now feeds it in pieces (`crates/fetch/src/http.rs`, test: a 6 MB POST to a local
rustls server). Then, with OpenAI (gpt-image-1.5) on wedding photos (a test copy, the user's own key
and consent): a guest in the background removed (one request, 1270 px crop sent at 81 %, 46 s), and
a striped flag across the full height of the frame (then sent as 4 tiles at 50 %, about 4 minutes;
two of the four didn't match, which is why fills are no longer split); both blend with the photo's colour and grain, and white balance and exposure
changed afterwards apply to the patch. `cargo run --release -p lcc-engine --example
genfill_check` with `GENFILL_REAL=1` repeats such a test (each run is a paid request).

### Trying it without an account

`<cli> genfill-mock --port 18510` serves the four providers' request and response shapes on
127.0.0.1 and fills holes with Chromaforge's own content-aware fill. Start Chromaforge with
`<PREFIX>_GENFILL_MOCK=http://127.0.0.1:18510`, save any key, and Generate goes to the mock. Only
loopback addresses are accepted there, so a key can't be sent elsewhere by mistake.

## Commands

| Command | Params | Result |
|---|---|---|
| `spot.add` | `{mode: generate, points, size, …}` | a Generative spot (renders as Fill until generated) |
| `spot.fromMask` | `{mask, mode?: fill\|generate, deleteMask?}` | a spot covering the mask |
| `genfill.providers` | `{}` | providers, key status (never the key), consent, policy, price |
| `genfill.setKey` | `{provider, key \| null}` | stores or forgets your key |
| `genfill.consent` | `{provider, accept}` | records or withdraws consent |
| `genfill.plan` | `{index?, provider?, variations?, prompt?, previewPath?}` | requests, cost, the crop, scale, warnings; sends nothing; an error for an area too large for one request |
| `spot.generate` | `{index?, provider?, variations?, prompt?}` | generates (background in the app, activity row with ✕) |
| `spot.variation` | `{index?, variant \| result \| delta}` | which result a spot shows (`result`, `delta`: across every generation kept) |
| `spot.putBack` | `{index?}` | a moved or resized generated spot back where its fill was made |
| `genfill.results` | `{}` | every generation in the library for the photo: id, service, prompt, when, the spot that keeps it |
| `spot.restoreGenerated` | `{id}` | shows that generation again (its spot, or a new spot where it was made) |
| `genfill.status`, `genfill.cancel` | `{}` | the running generation |
