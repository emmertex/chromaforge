//! ONNX Runtime for the AI features (issue #491): runs an `.onnx` model on the graphics card through one of ONNX
//! Runtime's execution providers, or on ONNX Runtime's own CPU kernels. The graphics card is reached through ONNX
//! Runtime's WebGPU plug-in (Dawn: Direct3D 12 on Windows, Vulkan on Linux, Metal on macOS; MIT/BSD, open source),
//! which works with any card; Core ML (macOS), CUDA (NVIDIA, when the user has it installed) and DirectML (only when
//! the loaded library has it built in; the packages don't ship the closed `DirectML.dll`) are used when present.
//!
//! The app is Rust first: every model also has a pure-Rust runner, which stays the reference and the fallback, and
//! is all the web and Android builds have. This crate is the one place ONNX Runtime (C++, MIT) enters the product:
//!
//! - **Feature `ort`** (desktop builds only) pulls in the [`ort`](https://crates.io/crates/ort) wrapper with
//!   `load-dynamic`: nothing is linked or downloaded at build time, and the build stays pure Rust. Without the
//!   feature every call returns [`Error::Unavailable`].
//! - **The library is found at run time** ([`library_candidates`]): `<PREFIX>_ONNXRUNTIME` (a file, or `off`), next to
//!   the executable, in the package's `lib/` or macOS `Frameworks/`, then (not on Windows) the system's library path.
//!   No library, one older than ONNX Runtime 1.24, or one that fails to start: [`runtime`] says why, once, and callers
//!   keep their pure-Rust runners. The WebGPU plug-in ([`webgpu_names`]) is looked for beside the library, or named by
//!   `<PREFIX>_ONNXRUNTIME_WEBGPU`, and registered when found.
//! - **Never crash:** every call into `ort` runs under `catch_unwind`, inputs and outputs are bounded and checked, and
//!   the model file is read with a size cap. A fault inside the C++ library itself can't be caught; that is the price
//!   of the feature, which is why callers check a session's answer against their own runner before trusting it.
//!
//! The `unsafe` lives in `ort` (FFI and `libloading`); this crate is safe code only.

#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::path::{Path, PathBuf};

#[cfg(all(feature = "ort", not(any(target_arch = "wasm32", target_os = "android"))))]
mod imp;
#[cfg(not(all(feature = "ort", not(any(target_arch = "wasm32", target_os = "android")))))]
mod stub;
#[cfg(not(all(feature = "ort", not(any(target_arch = "wasm32", target_os = "android")))))]
use stub as imp;

/// Largest model file read (bytes).
pub const MAX_MODEL_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Most elements one input or output tensor may have.
pub const MAX_ELEMENTS: usize = 256 * 1024 * 1024;
/// Most dimensions a tensor may have.
pub const MAX_RANK: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// ONNX Runtime can't be used here: not in this build, no library found, or it failed to start.
    #[error("ONNX Runtime is not available: {0}")]
    Unavailable(String),
    /// The model could not be loaded (a bad file, an operator the runtime lacks, no usable execution provider).
    #[error("ONNX Runtime could not load the model: {0}")]
    Load(String),
    /// Running the model failed.
    #[error("ONNX Runtime failed while running the model: {0}")]
    Run(String),
    /// The caller's tensors don't fit the model, or the model's answer is not what was promised.
    #[error("{0}")]
    Input(String),
}

/// Where a session's work runs: ONNX Runtime's name for its execution provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Provider {
    /// ONNX Runtime's WebGPU plug-in (Dawn on Direct3D 12, Vulkan or Metal: any graphics card).
    WebGpu,
    /// DirectX 12 (any Windows graphics card).
    DirectMl,
    /// Core ML (Apple GPU and Neural Engine).
    CoreMl,
    /// NVIDIA CUDA (needs the CUDA and cuDNN libraries installed).
    Cuda,
    /// ONNX Runtime's own CPU kernels.
    Cpu,
}

impl Provider {
    /// The name shown to people ("DirectML").
    pub fn label(self) -> &'static str {
        match self {
            Provider::WebGpu => "WebGPU",
            Provider::DirectMl => "DirectML",
            Provider::CoreMl => "Core ML",
            Provider::Cuda => "CUDA",
            Provider::Cpu => "CPU",
        }
    }

    /// A stable id for status JSON (`directml`, `coreml`, `cuda`, `cpu`).
    pub fn id(self) -> &'static str {
        match self {
            Provider::WebGpu => "webgpu",
            Provider::DirectMl => "directml",
            Provider::CoreMl => "coreml",
            Provider::Cuda => "cuda",
            Provider::Cpu => "cpu",
        }
    }

    pub fn is_gpu(self) -> bool {
        self != Provider::Cpu
    }

    /// The graphics-card providers worth trying on this operating system, best first.
    pub fn gpu_candidates() -> &'static [Provider] {
        if cfg!(windows) {
            &[Provider::WebGpu, Provider::DirectMl, Provider::Cuda]
        } else if cfg!(target_vendor = "apple") {
            &[Provider::CoreMl, Provider::WebGpu]
        } else {
            &[Provider::WebGpu, Provider::Cuda]
        }
    }
}

/// What a session is asked to run on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Want {
    /// The first graphics-card provider that loads the model (see [`Provider::gpu_candidates`]); an error when none
    /// does, so the caller can keep its own GPU or CPU runner.
    Gpu,
    /// ONNX Runtime's CPU kernels, with at most this many threads (0: ONNX Runtime's choice).
    Cpu { threads: usize },
}

/// The loaded runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Runtime {
    /// ONNX Runtime's version ("1.32.0"), when its build says it: Microsoft's release builds name no branch
    /// (`git-branch=HEAD`), and the version call needs `unsafe`.
    pub version: Option<String>,
    /// The library file it was loaded from.
    pub library: PathBuf,
    /// Providers this build of the library was compiled with, or registered as plug-ins, of those the app uses.
    pub providers: Vec<Provider>,
    /// The WebGPU plug-in's file, when one was registered.
    pub webgpu: Option<PathBuf>,
    /// Why the WebGPU plug-in is not used, when it isn't.
    pub webgpu_missing: Option<String>,
}

impl Runtime {
    /// Whether some graphics-card provider was compiled into the library.
    pub fn has_gpu(&self) -> bool {
        self.providers.iter().any(|p| p.is_gpu())
    }
}

/// ONNX Runtime, loaded on first use (every later call gets the same answer). `Err` says why it can't be used.
pub fn runtime() -> Result<&'static Runtime, Error> {
    imp::runtime()
}

/// The runtime when something already loaded it (or tried to), without loading it: `None` until the first
/// [`runtime`] call. For status displays, which must not stall on loading a library.
pub fn peek() -> Option<Result<&'static Runtime, Error>> {
    imp::peek()
}

/// For status JSON: `{state: "notLoaded" | "ready" | "unavailable", compiledIn, version?, library?, providers?,
/// webgpu?, webgpuMissing?, reason?}`. Never loads anything.
pub fn status() -> std::collections::BTreeMap<&'static str, String> {
    let mut m = std::collections::BTreeMap::new();
    m.insert("compiledIn", compiled_in().to_string());
    match peek() {
        None => {
            m.insert("state", "notLoaded".into());
        }
        Some(Ok(rt)) => {
            m.insert("state", "ready".into());
            if let Some(v) = &rt.version {
                m.insert("version", v.clone());
            }
            m.insert("library", rt.library.display().to_string());
            m.insert("providers", rt.providers.iter().map(|p| p.id()).collect::<Vec<_>>().join(","));
            if let Some(p) = &rt.webgpu {
                m.insert("webgpu", p.display().to_string());
            }
            if let Some(w) = &rt.webgpu_missing {
                m.insert("webgpuMissing", w.clone());
            }
        }
        Some(Err(e)) => {
            m.insert("state", "unavailable".into());
            m.insert("reason", e.to_string());
        }
    }
    m
}

/// Whether this build can use ONNX Runtime at all (the `ort` feature on a desktop target), library or not.
pub fn compiled_in() -> bool {
    cfg!(all(feature = "ort", not(any(target_arch = "wasm32", target_os = "android"))))
}

/// A tensor of `f32`s, row-major.
#[derive(Debug, Clone, PartialEq)]
pub struct Tensor {
    pub shape: Vec<usize>,
    pub data: Vec<f32>,
}

impl Tensor {
    /// A checked tensor: the shape's product is `data.len()`, at most [`MAX_ELEMENTS`] and [`MAX_RANK`], no zero
    /// dimension.
    pub fn new(shape: Vec<usize>, data: Vec<f32>) -> Result<Tensor, Error> {
        check_shape(&shape, data.len())?;
        Ok(Tensor { shape, data })
    }
}

/// `Ok` when `shape` is non-empty, has no zero dimension, at most [`MAX_RANK`] dimensions, and holds exactly `len`
/// (≤ [`MAX_ELEMENTS`]) values.
pub fn check_shape(shape: &[usize], len: usize) -> Result<(), Error> {
    if shape.is_empty() || shape.len() > MAX_RANK || shape.contains(&0) {
        return Err(Error::Input(format!("a tensor shape {shape:?} the runtime does not take")));
    }
    let n = shape.iter().try_fold(1usize, |a, d| a.checked_mul(*d));
    match n {
        Some(n) if n == len && n <= MAX_ELEMENTS => Ok(()),
        Some(n) if n > MAX_ELEMENTS => Err(Error::Input(format!("a tensor of {n} values is larger than the runtime takes"))),
        Some(n) => Err(Error::Input(format!("{len} values do not fill the shape {shape:?} ({n})"))),
        None => Err(Error::Input(format!("the shape {shape:?} overflows"))),
    }
}

/// A model loaded into ONNX Runtime. Runs are serialised (one at a time per session), as DirectML needs.
pub struct Session {
    inner: imp::Session,
    provider: Provider,
    inputs: Vec<String>,
    outputs: Vec<String>,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session").field("provider", &self.provider).field("inputs", &self.inputs).field("outputs", &self.outputs).finish()
    }
}

impl Session {
    /// Load the model file at `path` for `want`.
    pub fn load(path: &Path, want: Want) -> Result<Session, Error> {
        Session::load_for_shape(path, want, &[])
    }

    /// Load the model file at `path` for `want`, for a first input that is always `shape` (empty: any shape).
    ///
    /// The model's named free dimensions on that input are fixed to `shape`'s sizes. Some exports name an output's
    /// dimensions after the input's even when the output is larger (Real-ESRGAN's `realesr-general-x4v3` says its
    /// 4× output is `height × width`); ONNX Runtime's WebGPU provider then plans the output into the input's buffer
    /// and fails. Fixed sizes let it work the real sizes out.
    pub fn load_for_shape(path: &Path, want: Want, shape: &[usize]) -> Result<Session, Error> {
        let bytes = read_model(path)?;
        Session::from_bytes_for_shape(&bytes, want, shape)
    }

    /// Load a model from its bytes.
    pub fn from_bytes(bytes: &[u8], want: Want) -> Result<Session, Error> {
        Session::from_bytes_for_shape(bytes, want, &[])
    }

    /// Load a model from its bytes, for a first input that is always `shape` ([`Session::load_for_shape`]).
    pub fn from_bytes_for_shape(bytes: &[u8], want: Want, shape: &[usize]) -> Result<Session, Error> {
        if bytes.is_empty() {
            return Err(Error::Load("the model file is empty".into()));
        }
        if shape.len() > MAX_RANK {
            return Err(Error::Load(format!("an input shape with {} dimensions", shape.len())));
        }
        let rt = runtime()?;
        let (inner, provider, inputs, outputs) = imp::load(rt, bytes, want, shape)?;
        Ok(Session { inner, provider, inputs, outputs })
    }

    /// Where this session runs.
    pub fn provider(&self) -> Provider {
        self.provider
    }

    /// The model's input names, in order.
    pub fn inputs(&self) -> &[String] {
        &self.inputs
    }

    /// The model's output names, in order.
    pub fn outputs(&self) -> &[String] {
        &self.outputs
    }

    /// "ONNX Runtime (WebGPU)", with the version when the library says it ("ONNX Runtime 1.32.0 (WebGPU)").
    pub fn describe(&self) -> String {
        match runtime().ok().and_then(|rt| rt.version.as_deref()) {
            Some(v) => format!("ONNX Runtime {v} ({})", self.provider.label()),
            None => format!("ONNX Runtime ({})", self.provider.label()),
        }
    }

    /// Run the model on `inputs`, given in the model's input order, and return every output in the model's order.
    pub fn run(&self, inputs: &[Tensor]) -> Result<Vec<Tensor>, Error> {
        if inputs.len() != self.inputs.len() {
            return Err(Error::Input(format!("the model takes {} inputs, {} were given", self.inputs.len(), inputs.len())));
        }
        for t in inputs {
            check_shape(&t.shape, t.data.len())?;
        }
        let out = imp::run(&self.inner, &self.inputs, inputs)?;
        if out.len() != self.outputs.len() {
            return Err(Error::Run(format!("the model gave {} outputs, it names {}", out.len(), self.outputs.len())));
        }
        for t in &out {
            check_shape(&t.shape, t.data.len())?;
        }
        Ok(out)
    }

    /// Run a model with one input and one output.
    pub fn run_one(&self, input: Tensor) -> Result<Tensor, Error> {
        self.run(std::slice::from_ref(&input))?.into_iter().next().ok_or_else(|| Error::Run("the model gave no output".into()))
    }
}

/// Read a model file, refusing one that is empty or larger than [`MAX_MODEL_BYTES`].
fn read_model(path: &Path) -> Result<Vec<u8>, Error> {
    use std::io::Read;
    let file = std::fs::File::open(path).map_err(|e| Error::Load(format!("{}: {e}", path.display())))?;
    let len = file.metadata().map_err(|e| Error::Load(e.to_string()))?.len();
    if len == 0 || len > MAX_MODEL_BYTES {
        return Err(Error::Load(format!("the model file is empty or larger than {} MiB", MAX_MODEL_BYTES / (1024 * 1024))));
    }
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(usize::try_from(len).map_err(|_| Error::Load("the model file is too large".into()))?)
        .map_err(|_| Error::Load("not enough memory to read the model".into()))?;
    file.take(MAX_MODEL_BYTES).read_to_end(&mut bytes).map_err(|e| Error::Load(e.to_string()))?;
    Ok(bytes)
}

/// The library's file names on this operating system, most specific first.
pub fn library_names() -> &'static [&'static str] {
    if cfg!(windows) {
        &["onnxruntime.dll"]
    } else if cfg!(target_vendor = "apple") {
        &["libonnxruntime.dylib"]
    } else {
        &["libonnxruntime.so", "libonnxruntime.so.1"]
    }
}

/// The WebGPU plug-in's file names on this operating system.
pub fn webgpu_names() -> &'static [&'static str] {
    if cfg!(windows) {
        &["onnxruntime_providers_webgpu.dll"]
    } else if cfg!(target_vendor = "apple") {
        &["libonnxruntime_providers_webgpu.dylib"]
    } else {
        &["libonnxruntime_providers_webgpu.so"]
    }
}

/// Where the WebGPU plug-in is looked for: `<PREFIX>_ONNXRUNTIME_WEBGPU` (a file, or `off`), else beside `library`.
pub fn webgpu_candidates(library: &Path) -> Vec<PathBuf> {
    let set = lcc_brand::env_os("ONNXRUNTIME_WEBGPU").filter(|v| !v.is_empty());
    let dir = library.parent().filter(|d| !d.as_os_str().is_empty()).map(Path::to_path_buf);
    candidates(set.as_deref(), dir.as_deref(), webgpu_names(), false)
}

/// Where the library is looked for, in order: `<PREFIX>_ONNXRUNTIME` when it names a file or folder (`off` or `0`:
/// nowhere), then beside the executable (Windows), its `lib/` folder, the Linux packages' `../lib/<binary>/`, `../lib/`,
/// and the macOS bundle's `../Frameworks/`, then
/// (not on Windows, where a bare name would search the current folder) the bare names for the system's loader.
pub fn library_candidates() -> Vec<PathBuf> {
    let set = lcc_brand::env_os("ONNXRUNTIME").filter(|v| !v.is_empty());
    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf));
    candidates(set.as_deref(), exe_dir.as_deref(), library_names(), !cfg!(windows))
}

/// Whether ONNX Runtime was switched off with `<PREFIX>_ONNXRUNTIME=off`.
pub fn switched_off() -> bool {
    lcc_brand::env_os("ONNXRUNTIME").is_some_and(|v| is_off(&v))
}

fn is_off(v: &std::ffi::OsStr) -> bool {
    matches!(v.to_str().map(|s| s.trim().to_ascii_lowercase()).as_deref(), Some("off" | "0" | "no" | "false"))
}

fn candidates(set: Option<&std::ffi::OsStr>, exe_dir: Option<&Path>, names: &[&str], bare: bool) -> Vec<PathBuf> {
    if let Some(v) = set {
        if is_off(v) {
            return Vec::new();
        }
        let p = PathBuf::from(v);
        // a folder: the library's names in it
        if p.is_dir() {
            return names.iter().map(|n| p.join(n)).collect();
        }
        return vec![p];
    }
    let mut out = Vec::new();
    if let Some(dir) = exe_dir {
        let packaged = format!("../lib/{}", lcc_brand::BINARY);
        for sub in [None, Some("lib"), Some(packaged.as_str()), Some("../lib"), Some("../Frameworks")] {
            let base = match sub {
                Some(s) => dir.join(s),
                None => dir.to_path_buf(),
            };
            out.extend(names.iter().map(|n| base.join(n)).filter(|p| p.is_file()));
        }
    }
    if bare {
        out.extend(names.iter().map(PathBuf::from));
    }
    out
}

#[cfg(test)]
mod tests;
