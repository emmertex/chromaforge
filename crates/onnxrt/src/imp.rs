//! The `ort`-backed implementation (feature `ort`, desktop targets).

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, PoisonError};

use ort::ep::{self, ExecutionProvider};
use ort::session::Session as OrtSession;
use ort::value::{Tensor as OrtTensor, TensorElementType, ValueType};

use crate::{Error, MAX_ELEMENTS, MAX_RANK, Provider, Runtime, Tensor, Want};

struct Loaded {
    runtime: Runtime,
    env: ort::environment::Environment,
}

static LOADED: OnceLock<Result<Loaded, Error>> = OnceLock::new();

/// Run `f`, turning a panic inside `ort` into an error.
fn guarded<T>(what: &str, f: impl FnOnce() -> Result<T, Error>) -> Result<T, Error> {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|_| Err(Error::Run(format!("ONNX Runtime gave up while {what}"))))
}

pub(crate) fn runtime() -> Result<&'static Runtime, Error> {
    match LOADED.get_or_init(load_runtime) {
        Ok(l) => Ok(&l.runtime),
        Err(e) => Err(e.clone()),
    }
}

pub(crate) fn peek() -> Option<Result<&'static Runtime, Error>> {
    LOADED.get().map(|l| match l {
        Ok(l) => Ok(&l.runtime),
        Err(e) => Err(e.clone()),
    })
}

fn env() -> Result<&'static ort::environment::Environment, Error> {
    match LOADED.get_or_init(load_runtime) {
        Ok(l) => Ok(&l.env),
        Err(e) => Err(e.clone()),
    }
}

fn load_runtime() -> Result<Loaded, Error> {
    if crate::switched_off() {
        return Err(Error::Unavailable(format!("switched off with {}", lcc_brand::env_var("ONNXRUNTIME"))));
    }
    let candidates = crate::library_candidates();
    if candidates.is_empty() {
        return Err(Error::Unavailable("no ONNX Runtime library was found".into()));
    }
    let mut why = Vec::new();
    for path in candidates {
        // `init_from` loads the library and checks its version; a failure leaves nothing loaded, so the next
        // candidate may be tried
        let builder = match catch_unwind(AssertUnwindSafe(|| ort::init_from(&path))) {
            Ok(Ok(b)) => b,
            Ok(Err(e)) => {
                why.push(format!("{}: {e}", path.display()));
                continue;
            }
            Err(_) => {
                why.push(format!("{}: loading it failed", path.display()));
                continue;
            }
        };
        // the library is loaded now (for the whole process): build the environment or give up
        return guarded("starting", || {
            let env = builder.with_name(lcc_brand::BINARY).with_telemetry(false).build().map_err(|e| Error::Unavailable(e.to_string()))?;
            let library = absolute(&path);
            let (webgpu, webgpu_missing) = match register_webgpu(&env, &library) {
                Ok(p) => (Some(p), None),
                Err(why) => (None, Some(why)),
            };
            let runtime = Runtime { version: version(), library, providers: providers(&env), webgpu, webgpu_missing };
            log::info!(
                "ONNX Runtime {} from {} ({})",
                runtime.version.as_deref().unwrap_or("(version not stated)"),
                runtime.library.display(),
                provider_list(&runtime.providers)
            );
            Ok(Loaded { runtime, env })
        });
    }
    let why = why.join("; ");
    log::info!("ONNX Runtime not used: {why}");
    Err(Error::Unavailable(why))
}

fn provider_list(p: &[Provider]) -> String {
    p.iter().map(|p| p.label()).collect::<Vec<_>>().join(", ")
}

fn absolute(path: &Path) -> PathBuf {
    if path.is_absolute() { path.to_path_buf() } else { std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()) }
}

/// "1.32.0", from the build-info string when it has one ("ORT Build Info: git-branch=rel-1.32.0, …"; Microsoft's
/// PyPI builds say `git-branch=HEAD`).
fn version() -> Option<String> {
    let info = catch_unwind(ort::info).unwrap_or("");
    parse_version(info)
}

pub(crate) fn parse_version(info: &str) -> Option<String> {
    let at = info.find("rel-")?;
    let rest = info.get(at + 4..)?;
    let v: String = rest.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    (!v.is_empty()).then_some(v)
}

/// ONNX Runtime's name for the WebGPU plug-in's provider.
const WEBGPU_EP: &str = "WebGpuExecutionProvider";

/// Register the WebGPU plug-in found for `library`: its file, or why there is none.
fn register_webgpu(env: &ort::environment::Environment, library: &Path) -> Result<PathBuf, String> {
    let candidates = crate::webgpu_candidates(library);
    if candidates.is_empty() {
        return Err("the WebGPU plug-in was not found".into());
    }
    let mut why = Vec::new();
    for p in candidates {
        match catch_unwind(AssertUnwindSafe(|| env.register_ep_library("webgpu", &p))) {
            Ok(Ok(())) => {
                if webgpu_devices(env).next().is_none() {
                    return Err("the WebGPU plug-in found no graphics card (on Linux it needs the Vulkan loader)".into());
                }
                return Ok(p);
            }
            Ok(Err(e)) => why.push(format!("{}: {e}", p.display())),
            Err(_) => why.push(format!("{}: registering it failed", p.display())),
        }
    }
    Err(why.join("; "))
}

/// The devices the WebGPU plug-in offers.
fn webgpu_devices(env: &ort::environment::Environment) -> impl Iterator<Item = ort::device::Device<'_>> {
    env.devices().filter(|d| matches!(d.ep(), Ok(WEBGPU_EP)))
}

fn providers(env: &ort::environment::Environment) -> Vec<Provider> {
    let has = |r: ort::Result<bool>| matches!(r, Ok(true));
    let mut out = Vec::new();
    if webgpu_devices(env).next().is_some() {
        out.push(Provider::WebGpu);
    }
    if has(ep::DirectML::is_available()) {
        out.push(Provider::DirectMl);
    }
    if has(ep::CoreML::is_available()) {
        out.push(Provider::CoreMl);
    }
    if has(ep::CUDA::is_available()) {
        out.push(Provider::Cuda);
    }
    out.push(Provider::Cpu);
    out
}

pub(crate) struct Session(Mutex<OrtSession>);

type LoadedSession = (Session, Provider, Vec<String>, Vec<String>);

pub(crate) fn load(rt: &Runtime, bytes: &[u8], want: Want, shape: &[usize]) -> Result<LoadedSession, Error> {
    let env = env()?;
    let fixed = if shape.is_empty() { Vec::new() } else { guarded("reading the model", || fixed_dimensions(env, bytes, shape))? };
    let session = |p, threads| session(env, bytes, p, threads, &fixed);
    match want {
        Want::Gpu => {
            let mut why = Vec::new();
            for &p in Provider::gpu_candidates() {
                if !rt.providers.contains(&p) {
                    why.push(format!("{} is not in this ONNX Runtime", p.label()));
                    continue;
                }
                match guarded("loading the model", || session(p, 0)) {
                    Ok(s) => return Ok(s),
                    Err(e) => why.push(format!("{}: {e}", p.label())),
                }
            }
            if why.is_empty() {
                why.push("no graphics-card provider on this system".into());
            }
            Err(Error::Load(why.join("; ")))
        }
        Want::Cpu { threads } => guarded("loading the model", || session(Provider::Cpu, threads)),
    }
}

/// The first input's named free dimensions and the sizes `shape` gives them. A name that two different sizes would
/// claim is left free; a size the model fixes itself must match.
fn fixed_dimensions(env: &ort::environment::Environment, bytes: &[u8], shape: &[usize]) -> Result<Vec<(String, i64)>, Error> {
    let load = |e: ort::Error<_>| Error::Load(e.to_string());
    let probe = OrtSession::builder(env)
        .map_err(|e| Error::Load(e.to_string()))?
        .with_optimization_level(ort::session::builder::GraphOptimizationLevel::Disable)
        .map_err(load)?
        .commit_from_memory(bytes)
        .map_err(|e| Error::Load(e.to_string()))?;
    let Some(first) = probe.inputs().first() else { return Err(Error::Load("the model has no inputs".into())) };
    let ValueType::Tensor { shape: dims, dimension_symbols, .. } = first.dtype() else {
        return Err(Error::Load(format!("`{}` is not a tensor", first.name())));
    };
    if dims.len() != shape.len() {
        return Err(Error::Load(format!("`{}` has {} dimensions, the caller gives {}", first.name(), dims.len(), shape.len())));
    }
    let mut fixed: Vec<(String, i64)> = Vec::new();
    let mut clash: Vec<&str> = Vec::new();
    for ((&d, name), &want) in dims.iter().zip(dimension_symbols.iter()).zip(shape) {
        let want = i64::try_from(want).map_err(|_| Error::Load(format!("an input dimension of {want}")))?;
        if d >= 0 {
            if d != want {
                return Err(Error::Load(format!("`{}` is {:?}, the caller gives {shape:?}", first.name(), &dims[..])));
            }
            continue;
        }
        if name.is_empty() || clash.contains(&name.as_str()) {
            continue;
        }
        match fixed.iter().position(|(n, _)| n == name) {
            Some(i) if fixed.get(i).is_some_and(|(_, v)| *v != want) => {
                fixed.remove(i);
                clash.push(name);
            }
            Some(_) => {}
            None => fixed.push((name.clone(), want)),
        }
    }
    Ok(fixed)
}

fn session(
    env: &ort::environment::Environment,
    bytes: &[u8],
    provider: Provider,
    threads: usize,
    fixed: &[(String, i64)],
) -> Result<LoadedSession, Error> {
    let load = |e: ort::Error<_>| Error::Load(e.to_string());
    let mut b = OrtSession::builder(env).map_err(|e| Error::Load(e.to_string()))?;
    for (name, size) in fixed {
        b = b.with_dimension_override(name, *size).map_err(load)?;
    }
    b = match provider {
        // the plug-in goes through the device API; it takes one device (Dawn picks the adapter: the high-performance one)
        Provider::WebGpu => b.with_devices(webgpu_devices(env).take(1), None).map_err(load)?,
        // DirectML runs one node at a time and can't reuse memory patterns
        Provider::DirectMl => b
            .with_memory_pattern(false)
            .map_err(load)?
            .with_parallel_execution(false)
            .map_err(load)?
            .with_execution_providers([ep::DirectML::default()
                .with_performance_preference(ep::directml::PerformancePreference::HighPerformance)
                .build()
                .error_on_failure()])
            .map_err(load)?,
        Provider::CoreMl => b
            .with_execution_providers([ep::CoreML::default().with_compute_units(ep::coreml::ComputeUnits::All).build().error_on_failure()])
            .map_err(load)?,
        Provider::Cuda => b.with_execution_providers([ep::CUDA::default().build().error_on_failure()]).map_err(load)?,
        Provider::Cpu if threads > 0 => b.with_intra_threads(threads).map_err(load)?,
        Provider::Cpu => b,
    };
    let s = b.commit_from_memory(bytes).map_err(|e| Error::Load(e.to_string()))?;
    let names = |outlets: &[ort::value::Outlet]| -> Result<Vec<String>, Error> {
        outlets
            .iter()
            .map(|o| match o.dtype() {
                ValueType::Tensor { ty: TensorElementType::Float32, .. } => Ok(o.name().to_string()),
                other => Err(Error::Load(format!("`{}` is a {other}, only float32 tensors are supported", o.name()))),
            })
            .collect()
    };
    let inputs = names(s.inputs())?;
    let outputs = names(s.outputs())?;
    if inputs.is_empty() || outputs.is_empty() {
        return Err(Error::Load("the model has no inputs or no outputs".into()));
    }
    Ok((Session(Mutex::new(s)), provider, inputs, outputs))
}

pub(crate) fn run(s: &Session, names: &[String], inputs: &[Tensor]) -> Result<Vec<Tensor>, Error> {
    guarded("running the model", || {
        let mut values = Vec::with_capacity(inputs.len());
        for (name, t) in names.iter().zip(inputs) {
            let v = OrtTensor::from_array((t.shape.clone(), t.data.clone())).map_err(|e| Error::Input(e.to_string()))?;
            values.push((name.as_str(), v));
        }
        let mut session = s.0.lock().unwrap_or_else(PoisonError::into_inner);
        let outputs = session.run(values).map_err(|e| Error::Run(e.to_string()))?;
        let mut out = Vec::with_capacity(outputs.len());
        for (_, value) in &outputs {
            let (shape, data) = value.try_extract_tensor::<f32>().map_err(|e| Error::Run(e.to_string()))?;
            if shape.len() > MAX_RANK {
                return Err(Error::Run(format!("an output with {} dimensions", shape.len())));
            }
            let dims = shape
                .iter()
                .map(|&d| usize::try_from(d).map_err(|_| Error::Run(format!("an output dimension of {d}"))))
                .collect::<Result<Vec<_>, _>>()?;
            if data.len() > MAX_ELEMENTS {
                return Err(Error::Run(format!("an output of {} values is larger than the runtime takes", data.len())));
            }
            let mut copy = Vec::new();
            copy.try_reserve_exact(data.len()).map_err(|_| Error::Run("not enough memory for the model's answer".into()))?;
            copy.extend_from_slice(data);
            out.push(Tensor { shape: dims, data: copy });
        }
        Ok(out)
    })
}
