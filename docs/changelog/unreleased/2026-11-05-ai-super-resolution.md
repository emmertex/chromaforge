## Develop

- **AI Super Resolution and AI Sharpen:** Photo ▸ Enhance… runs an installed upscaler (Real-ESRGAN General v3, BSD-3-Clause, opt-in `realesrgan-model` download, or any `rgbToRgb4x` ONNX) on the pure-Rust CPU/GPU runners: 2× or 4× (`enhance.superResolution {method: "ai"}`) or sharpened at the same size (`enhance.aiSharpen`), into a new 16-bit TIFF stacked with the photo. Thanks @cbram.
