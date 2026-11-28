# The shared AI backend

The AI features planned in issue #453 — semantic search (#487), aesthetic scores (#488), culling suggestions (#489)
and orientation detection (#490) — run their models through one backend (issue #491), so a big import is worked
through once, in the background, on the best device Chromaforge can use, without the window waiting.

The plan this follows (runtime choice, models, storage, milestones) is the investigation posted on issue #491. This
page describes what exists.

## What exists

**`crates/ml` (`lcc-ml`, L3)** provides the shared queue, device selection, model manifests, CLIP encoder, and binary
embedding cache store.

- **Device** (`device.rs`): `Preference::Auto` takes the GPU when the probe finds one, else the CPU, with the reason
  kept (`Device::why_cpu`); `Preference::Cpu` never asks for the GPU. The probe runs on the worker thread before the
  first job (it may create a device), and a probe that panics means "no GPU".
- **Queue** (`queue.rs`): one worker thread (`ml-queue`) takes jobs in priority order (`Interactive` before
  `Background`), one at a time. Each job has a key (`"embed:1234"`): a job submitted again while it still waits
  replaces the waiting one, and the job's owner routes results by key prefix when it calls `Queue::drain`.
  - **Cancel:** `cancel(key)` drops a waiting job or tells the running one to stop; `cancel_all()` does it for
    everything. A running job checks `Ctx::cancelled()` / `Ctx::check()?` between steps (between batches or layers).
  - **CPU fallback:** a job that returns `JobError::Device` on the GPU runs again on the CPU, and the queue stays on
    the CPU (with the reason) until the preference is set again.
  - **Never crash:** a job that panics ends as `Outcome::Failed` and the worker goes on; full queues refuse more
    (`SubmitError::Full`); results nobody drains are capped (the oldest go, with a warning).
  - **No threads** (the browser build): the worker can't start, and `drain` runs one job in place each call.
  - Dropping the queue tells the running job to stop and doesn't wait for the thread.
- **Activity row** (`Rows` / `Row`): while the queue has work it holds one batch row; the row's ✕ sets the batch's
  cancel flag (the running job stops, everything waiting ends as cancelled), and the row goes when the queue is empty.
- **Model manifests & known models** (`manifest.rs`, `known.rs`): each model lives in its own folder under
  `<config>/ml-models/<id>` (or `<PREFIX>_ML_MODELS`) with a `manifest.json` naming its role (`embedder`,
  `aestheticHead`, `orientationHead`), architecture, embedding dimension, input preprocessing (`InputSpec`), licence
  (`Licence`, `Commercial`), and every file's size and SHA-256 digest (`ModelManifest::verify_dir`). The known-models
  catalogue includes `clip-vit-b32-laion2b` (`laion/CLIP-ViT-B-32-laion2B-s34B-b79K`, MIT, `Commercial::Yes`), pinned
  to a revision and verified by SHA-256 before installation.
- **CLIP ViT + Text encoder** (`encoder.rs`, `tokenizer.rs`, `weights.rs`, `clip.rs`): a clean-room MIT CLIP vision +
  text transformer implementation on `candle-core` (the CPU reference). On the GPU the full vision and text towers
  (patch/token + position embeddings, layer norms, transformer blocks, CLS/EOT pooling, and final projections) run in
  a single submission on `lcc-gpu`'s wgpu kernels (`crates/gpu/src/clip.rs`, `wgsl/clip.wgsl`: Vulkan, DX12 or Metal,
  the same on every OS), reading back only `[batch, embed_dim]` floats; plugged in by the engine through
  `accel::Accelerator`; each tower is checked against the CPU on a test input (`accel::agree`) before use, and any
  error falls back to the CPU via `JobError::Device`, paired with a positional `.safetensors` reader that validates
  tensor shapes, bounds, and finiteness (rejecting `NaN`/`Inf` weights) and a byte-level BPE tokenizer (`vocab.json` +
  `merges.txt`).
- **Binary embedding store** (`store.rs`): per-model cache file `ml-embeddings-<id>.bin` stored beside the library
  (rebuildable, never inside `catalog.json`). Stores L2-normalized `f32` embeddings keyed by `(photo_id, fingerprint)`,
  tolerates torn tails on crash recovery, resets cleanly when the model's `tag` (`<id>@<sha256-prefix>`) or dimension
  changes, and saves atomically via a `.tmp` sibling.

**The engine** (`crates/engine/src/ml.rs`, `crates/engine/src/cmd/ml.rs`) integrates `lcc-ml` into `Session`:

- its GPU probe follows the rendering GPU's switches (`<PREFIX>_GPU`, `<PREFIX>_GPU_BACKEND`, `app.gpu`) and the ML device
  setting (`AppSettings::ml_device`, `ml.status {device}`), using `lcc_gpu::clip` (wgpu on Vulkan, DX12 or Metal) on the
  chosen graphics card (or the automatic AI card);
- `<PREFIX>_ML_DEVICE=cpu` keeps the AI jobs on the CPU (`auto` is the default);
- its row in the activity stack is a batch: kind `mlIndex`, label "Indexing photos" ("Indexing photos · 37 of 200 ·
  2 failed"), detail the photo's file name;
- background indexing (`ml.pump`) paces itself with `faces_index::Pace` (`fast` / `idle` / `pause`) and
  `memory::work_gate`, decodes source thumbnails on the worker thread, encodes batches of up to 4 photos on the
  `ml-queue` worker, and persists `ml-embeddings-<id>.bin` beside the library;
- commands:
  - `ml.status {preference?: auto | cpu}`: reports device status, queue counts, `enabled`, `modelId`, `modelInstalled`,
    `indexedPhotos`, and `unindexedPhotos`;
  - `ml.enable {enabled: bool}`: switches semantic search on or off (Settings ▸ AI Models ▸ Semantic search; on unless
    switched off). Off: nothing is indexed, `search.semantic` and `library.filter {semantic}` are refused, an active
    description filter is cleared and the search box loses its **AI** button; the index already made is kept. The choice
    is saved in the models folder's `settings.json` and kept across launches;
  - `ml.indexStatus`, `ml.pause`, `ml.resume`, `ml.clearIndex`, `ml.restart`: where indexing stands (`state`: `off`,
    `idle`, `indexing`, `paused`, `complete`, with `done` of `total` photos), pausing it (saved as `paused` in
    `settings.json`, so it stays paused across launches; queued photos wait on the pace, nothing new is queued), resuming
    it, deleting this library's `ml-embeddings-*.bin` (leaving indexing paused) and deleting them and indexing again. The
    "Indexing photos" row carries `controls` (`activity.list`): a pause button and a gear that opens Settings ▸ AI Models
    at its Semantic search section (`app.settings {section: "semanticSearch"}`); while paused with nothing queued, a row
    of its own stays so resume is there;
  - `ml.models.list`, `ml.models.install`, `ml.models.select`, `ml.models.remove`, `ml.models.download`,
    `ml.models.downloads`, `ml.models.downloadCancel`: manage installed and downloadable ML models behind explicit
    licence acknowledgement;
  - `ml.pump {pace?: fast | idle | pause}` and `ml.index {ids?, wait?, batchSize?}`: drive background or synchronous
    embedding indexing;
  - `search.semantic {text, limit?, minScore?, scope?: all | filtered | selected, filter?: bool, clear?: bool}`: encodes a natural-language query
    with the CLIP text tower, ranks indexed photos by cosine similarity, and (when `filter: true` or via
    `library.filter {semantic: text}`) filters the library grid in similarity order while composing with ratings, flags,
    colour labels, keywords and folders (issue #487); as background `ml.pump` batches finish, an active semantic filter
    refreshes automatically.

**The interface** (`crates/ui-egui/src/panels/topbar.rs`, `crates/ui-egui/src/modern_library.rs`,
`crates/ui-egui/src/panels/ml_models.rs`, `crates/ui-egui/src/panels/settings.rs`) exposes natural-language search
(`search:ai` in the top bar, `modern:search:ai` in the Modern search pill, a removable `AI search: “…”` chip in the
filter-chips bar), an explicit hardware selector dropdown in Settings ▸ AI Models and Settings ▸ Performance, and a
**Semantic search** section in Settings ▸ AI Models (`dialog.aiModels`) to download, select or remove the CLIP model,
choose the hardware device (Automatic, specific detected GPU, or CPU), view active hardware status and fallback reasons,
and toggle background indexing.

**ONNX Runtime** (`crates/onnxrt`, [`docs/onnxruntime.md`](onnxruntime.md)): desktop builds can run `.onnx` models
through ONNX Runtime on the graphics card (its WebGPU plug-in, Core ML or CUDA), loaded at run time from beside the
app. AI Denoise and AI Super Resolution use it first, checked against the CPU; `ml.status` reports it as
`onnxRuntime`. CLIP's vision tower does too (`crates/ml/src/onnx.rs`): it is written as an ONNX graph (opset 17)
from the weights the CPU encoder loaded, so there is no second download, and tried before the wgpu kernels, checked
against the CPU encoder the same way. The text tower stays on the wgpu kernels.

**Orientation detection** (#490; `crates/ml/src/orient.rs`, `crates/engine/src/ml_orient.rs`, manual page
`ai/orientation`): `photo.autoOrient {ids?, minConfidence?, apply?, wait?}` (Photo ▸ Rotate & Flip ▸ Auto Orient (AI))
renders each photo as it is shown (its orientation only, long edge 448 px), turns it 0°, 90°, 180° and 270°
clockwise and embeds the four views with the active CLIP model in one batch (`orient:<id>` jobs on the shared queue).
An `orientationHead` classifier (`4 × embed_dim` weights and a bias, JSON, checked on load: version, shape, finite
numbers) says for each view how far the upright scene is turned in it; the four answers are pooled (view `k` of a
photo turned `r` shows `r + k`, so each votes for one `r`, averaged in log-probability) into a turn and a confidence.
Turns at or above the classifier's threshold are applied when every photo of the run has an answer, as one
"Auto Orient" history step built like Rotate Left/Right (crop and masks turn too); a photo turned by hand meanwhile
is left alone. The run's row is `mlOrient` "Detecting orientation · n of m"; its ✕ stops it and nothing is turned.
`ml.orientation.fit {ids?, out?, steps?}` fits the classifier on the library's photos as shown, taken to be upright:
softmax regression by full-batch gradient descent in plain Rust, one photo in five (by a hash of its id) held out to
measure view and pooled accuracy and to choose the lowest threshold whose held-out answers are 98 % right, then
fitted again on all of them; saved as `<models>/orientation-<model id>.json`, which is used when its `embedder` is
the installed model's exact tag. A head built in for a model (`lcc_ml::orient::builtin`) is used otherwise; none
ships yet. `ml.orientation.status` names the classifier in use, its fit report and the last run's answer.

## What's next

In the order of the plan on issue #491:

1. **Downstream AI features (#488, #489)** on the cached embeddings: aesthetic scoring head (#488) and culling
   suggestions (#489). Suggestions are proposals the user accepts, never silent writes.
2. **Orientation:** a built-in classifier for CLIP ViT-B/32, fitted on photos whose owner agrees (only the numbers
   ship), and an option to run it during import.

## Testing

`crates/ml/src/tests.rs` and unit tests across `manifest.rs`, `known.rs`, `store.rs`, `encoder.rs`, `tokenizer.rs`,
`weights.rs`, and `clip.rs` cover the queue, manifests, SHA-256 verification, hostile `.safetensors` / tokenizer /
store inputs, and synthetic CLIP image/text encoding. `crates/engine/src/tests_ml.rs` and
`crates/ui-egui/src/tests_filter_search.rs` cover the session's activity row, GPU-to-CPU fallback, model installation
and tamper rejection, background and synchronous indexing, persistent embedding stores across library reopen,
content-replacement invalidation, `search.semantic`, `library.filter {semantic}`, filter composition, and the UI toggle
and Settings ▸ AI Models controls.
