# ONNX Runtime

Chromaforge is Rust first: every AI model has a pure-Rust runner, which is the reference and what the web and Android
builds use. On the desktop, the AI features can also run their `.onnx` models through
[ONNX Runtime](https://onnxruntime.ai) (C++, MIT), which reaches the graphics card on every operating system and is
usually much faster than our own kernels (issue #491). This page describes how it is wired in.

## Pieces

- **`crates/onnxrt` (`lcc-onnxrt`, L0).** The one place ONNX Runtime enters the product. With the `ort` feature
  (desktop only; `lcc-app`'s `onnxruntime` feature turns it on, through `lcc-engine/onnxruntime`) it uses the
  [`ort`](https://crates.io/crates/ort) crate with `load-dynamic`: nothing is linked or downloaded at build time, the
  build stays pure Rust, and the library is loaded the first time a model wants it. Without the feature, and on wasm32
  and Android, every call returns `Error::Unavailable` and the callers keep their own runners.
  - `runtime()` loads the library once and registers the WebGPU plug-in when it finds one. `peek()`/`status()` report
    what happened without loading anything (`ml.status` → `onnxRuntime`).
  - `Session::load(path, Want::Gpu | Want::Cpu {threads})` loads a model. `Want::Gpu` tries the graphics-card
    providers in order (`Provider::gpu_candidates`): Windows WebGPU, DirectML, CUDA; macOS Core ML, WebGPU; Linux
    WebGPU, CUDA. It fails, with every reason, when none loads the model.
  - `Session::run` takes and gives float32 tensors, checked on the way in and out (rank ≤ 8, ≤ 256 M values, shape
    matches data). Runs on one session are serialised.
  - **Never crash:** every call into `ort` is under `catch_unwind`; the model file is read with a 2 GiB cap; inputs
    are validated before they reach the runtime. A fault inside the C++ library itself can't be caught, which is why
    callers check a session against their own runner before trusting it.
- **AI Denoise and AI Super Resolution** (`lcc_denoise::ort::OrtRunner`, `crates/engine/src/denoise.rs`): when the
  graphics card is set up for a model, ONNX Runtime is tried first on the card AI work goes to (the second card when
  there is one, else the card for editing), then our wgpu kernels (`lcc_gpu::nn`). Either way the check tile is run
  on the card and on the CPU and must agree (`GPU_AGREE`), and the card is used only while it is faster
  (`card_is_faster`) and keeps getting tiles right; a wrong tile runs on the CPU. Settings ▸ AI Denoise shows the
  result as the adapter, e.g. "ONNX Runtime (WebGPU)" (with the version when the library's build info names it; Microsoft's PyPI builds don't).

## Where the library comes from

`cargo xtask onnxruntime fetch --dest DIR [--platform P]` takes Microsoft's own PyPI builds of `onnxruntime` 1.32.0
and `onnxruntime-ep-webgpu` 0.5.0, pinned by URL and SHA-256 (`xtask/src/onnxruntime.rs`), and unpacks only the
libraries and their licence files. The packaging scripts call it:

| Package | Where | Platforms |
|---|---|---|
| Windows MSI and portable zip | beside the executable | x64, ARM64 (x86: none) |
| Linux AppImage, deb, rpm, tar.gz, Flatpak | `usr/lib/<binary>/` (`/app/lib/<binary>/` in the Flatpak) | x86-64, aarch64 (riscv64: none) |
| macOS DMG | `Contents/Frameworks/`, signed with the app's identity | Apple silicon (Intel Macs run without) |

Not shipped: Microsoft's closed-source `DirectML.dll` (decided 2026-11-27: the open WebGPU plug-in instead). A
library that has DirectML built in is still used when someone supplies one.

The plug-in stays on 0.5.0 (tested 2026-11-27 on Windows 11, RTX 4090): the 0.6.0 Windows build needs the preview
DirectX 12 Agility SDK. Without it, ONNX Runtime ends the process while it builds a WebGPU session (a fast-fail that
`catch_unwind` can't catch); with it, DXC fails to compile Dawn's shaders even with Developer Mode on. Before moving
the pin, run the GPU test on a Windows machine with a graphics card: `cargo xtask onnxruntime fetch --dest target/ort`,
then `<PREFIX>_ONNXRUNTIME=<absolute path>/target/ort <PREFIX>_ONNXRUNTIME_WEBGPU_UNTESTED=1 cargo test -p lcc-onnxrt --features lcc-onnxrt/ort` (it must
report `the_gpu_is_tried_and_says_why_not ... ok`, not skip it or end the test process). Because that crash can't be caught,
the app refuses a Windows plug-in built from an ONNX Runtime newer than the tested one (its DLL's file version, read
without loading it: 1.31 for plug-in 0.5.0; `WEBGPU_TESTED_WINDOWS` in `crates/onnxrt`) and says why in `ml.status`,
unless `<PREFIX>_ONNXRUNTIME_WEBGPU_UNTESTED=1` is set. Moving the pin moves that constant too.

At run time the library is looked for in this order: `<PREFIX>_ONNXRUNTIME` (a file or a folder; `off` switches ONNX
Runtime off), beside the executable, its `lib/`, `../lib/<binary>/`, `../lib/`, `../Frameworks/`, then (not on
Windows) the system's loader by bare name, so a distribution's own `libonnxruntime.so` works. The library must be ONNX
Runtime 1.24 or newer. The WebGPU plug-in is looked for beside the library, or named by `<PREFIX>_ONNXRUNTIME_WEBGPU`;
on Linux it needs the Vulkan loader (`libvulkan.so.1`).

## Licences

ONNX Runtime and the WebGPU plug-in: MIT. Dawn (inside the plug-in): BSD 3-Clause. DirectX Shader Compiler (Windows
plug-in, `dxcompiler.dll`): NCSA. The cargo dependencies (`ort`, `ort-sys`: MIT OR Apache-2.0; `libloading`: ISC) pass `deny.toml` as it is. The shipped
libraries are not cargo dependencies, so `cargo deny` can't see them: `NOTICE` lists them and their licence texts
ship beside them.

## Testing

`crates/onnxrt/src/tests.rs` builds a tiny ONNX model in bytes (`Mul` + `Relu`) and covers shapes, the library
search, hostile model files (garbage, truncated, flipped bytes), wrong inputs, threads and the GPU provider. The tests
that need the library call `lcc_testkit::skip!` without it; point `<PREFIX>_ONNXRUNTIME` at one (for example from
`cargo xtask onnxruntime fetch --dest target/ort`) to run them. `crates/denoise/src/ort.rs` checks that ONNX Runtime
and our CPU runner agree on a synthetic U-Net.

## What's next

- Face detection and recognition (`lcc-faces`), the CLIP search encoder (an ONNX export of the same checkpoint), SAM
  and the MediaPipe models onto `lcc-onnxrt`, each checked against its pure-Rust runner.
- ONNX Runtime's CPU provider as the CPU runner where it is faster than ours.
- Orientation (#490) and aesthetic scores (#488) on top, with commercially licensable models.
