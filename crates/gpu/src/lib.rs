//! The develop pipeline on the GPU (wgpu compute, WGSL kernels).
//!
//! The CPU pipeline (`lcc-pipeline`) is the reference: every kernel here is a port of a CPU
//! stage, both read the same resolved parameters ([`lcc_pipeline::Plan`],
//! [`lcc_pipeline::finish::FinishParams`]), and the equivalence tests (`tests/`) render the
//! same settings on both and bound the difference in 8-bit sRGB. Stages without a kernel run on the
//! CPU inside the same render (per-stage hybrid); see `docs/gpu-pipeline.md`.
//!
//! Which backends wgpu may load (DX12 only on Windows; `{ENV_PREFIX}_GPU_BACKEND`, `WGPU_BACKEND`) and
//! the crash sentinel around device creation: [`backend`] (issue #136). Device creation has a time
//! limit (`{ENV_PREFIX}_GPU_INIT_TIMEOUT`, issue #3068, see `init.rs`).
//!
//! Use [`render`]: it returns `None` when the GPU is unavailable, disabled (`{ENV_PREFIX}_GPU=0`,
//! `{ENV_PREFIX}_GPU_BACKEND=off` or [`set_enabled`]), the render does not fit the device, or the device reported an error, ran out
//! of memory or returned an incomplete image — callers then render on the CPU. [`try_render`] is
//! the same call reporting why *this* render fell back ([`Fallback`]); [`unavailable_reason`] and
//! [`last_fallback`] (the latest fallback in the process) feed the status displays (`ui.inspect` →
//! `perf.gpuReason` / `perf.gpuFallback`).
//! The browser build has no GPU path yet (WebGPU device creation is asynchronous): everything here
//! compiles to the CPU fallback on wasm32.
//!
//! Before the process ends: [`begin_shutdown`], then [`wait_idle`] (issue #620, see `exit.rs`).
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use lcc_develop::DevelopSettings;
use lcc_pipeline::{RenderRequest, Rendered, SourceInfo, StageCache};
use lcc_raster::Rgb32f;

#[cfg(not(target_arch = "wasm32"))]
pub mod backend;
#[cfg(all(feature = "denoise", not(target_arch = "wasm32")))]
pub mod bm3d;
#[cfg(not(target_arch = "wasm32"))]
mod budget;
#[cfg(not(target_arch = "wasm32"))]
mod ctx;
#[cfg(not(target_arch = "wasm32"))]
mod exit;
#[cfg(not(target_arch = "wasm32"))]
pub mod faces;
#[cfg(not(target_arch = "wasm32"))]
mod gate;
#[cfg(not(target_arch = "wasm32"))]
mod init;
#[cfg(all(feature = "denoise", not(target_arch = "wasm32")))]
pub mod nn;
#[cfg(not(target_arch = "wasm32"))]
mod params;
#[cfg(not(target_arch = "wasm32"))]
mod render;
#[cfg(not(target_arch = "wasm32"))]
mod slot;
#[cfg(not(target_arch = "wasm32"))]
pub mod video_lut;

#[cfg(not(target_arch = "wasm32"))]
pub use render::GpuStages;

#[cfg(not(target_arch = "wasm32"))]
pub use slot::{EventKind, GpuEvent, MAX_RECOVERIES};

/// Run `f` with this thread's GPU renders counted as batch work (an export's photos): they take
/// turns at the GPU, behind the window's own renders (see `gate.rs`).
pub fn as_batch<R>(f: impl FnOnce() -> R) -> R {
    #[cfg(not(target_arch = "wasm32"))]
    {
        gate::as_batch(f)
    }
    #[cfg(target_arch = "wasm32")]
    {
        f()
    }
}

/// Run `f`, several batch renders of one photo (an export's SDR and HDR renditions, `src_px`
/// source and `out_px` output pixels), in one turn at the GPU: the stages the first leaves on the
/// device for the second stay its own. Outside batch work, or without a device, it is just `f`.
pub fn in_one_turn<R>(src_px: usize, out_px: usize, f: impl FnOnce() -> R) -> R {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let Some(g) = existing_device().filter(|_| enabled()) else { return f() };
        let vram = g.vram.unwrap_or(g.max_buffer);
        // (the HDR rendition's float output on top of the renders' own memory)
        let bytes = gate::job_bytes(src_px, out_px).saturating_add((out_px as u64).saturating_mul(16));
        gate::in_one_turn(bytes, vram, &shutting_down, f)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (src_px, out_px);
        f()
    }
}

/// A photo's turn at the GPU taken by [`try_batch_turn`], held until dropped.
pub struct BatchTurn {
    #[cfg(not(target_arch = "wasm32"))]
    _turn: Option<gate::Turn>,
}

/// [`in_one_turn`]'s turn for a photo of `src_px` source and `out_px` output pixels, if it is free
/// now: its renders then run in it until the result is dropped. `None` when it would have to
/// wait for other exports: the caller renders the photo on the CPU instead ([`on_cpu`]), so the
/// processor and the GPU export photos at once. Without a device (or outside batch work) there
/// is nothing to wait for: always a turn.
pub fn try_batch_turn(src_px: usize, out_px: usize) -> Option<BatchTurn> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let Some(g) = existing_device().filter(|_| enabled()) else { return Some(BatchTurn { _turn: None }) };
        let vram = g.vram.unwrap_or(g.max_buffer);
        let bytes = gate::job_bytes(src_px, out_px).saturating_add((out_px as u64).saturating_mul(16));
        gate::try_turn(bytes, vram).map(|t| BatchTurn { _turn: Some(t) })
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (src_px, out_px);
        Some(BatchTurn {})
    }
}

/// Run `f` with this thread's renders on the CPU ([`try_render`] doesn't try the GPU): a photo
/// whose turn at a busy GPU isn't free.
pub fn on_cpu<R>(f: impl FnOnce() -> R) -> R {
    #[cfg(not(target_arch = "wasm32"))]
    {
        gate::on_cpu(f)
    }
    #[cfg(target_arch = "wasm32")]
    {
        f()
    }
}

/// Batch renders don't start for `d` from now: the window needs the GPU. Renders already running
/// finish.
pub fn hold_batch(d: std::time::Duration) {
    #[cfg(not(target_arch = "wasm32"))]
    gate::hold(d);
    #[cfg(target_arch = "wasm32")]
    let _ = d;
}

/// How much batch work may be on the GPU at once: device `bytes` (`None`: a share of the card's
/// memory) and at most `jobs` renders (`None`: up to 4).
pub fn set_batch_limits(bytes: Option<u64>, jobs: Option<usize>) {
    #[cfg(not(target_arch = "wasm32"))]
    gate::set_limits(bytes, jobs);
    #[cfg(target_arch = "wasm32")]
    let _ = (bytes, jobs);
}

/// Batch work on the GPU now and so far.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BatchUsage {
    /// Batch renders on the GPU now, and the device bytes they hold.
    pub jobs: usize,
    pub bytes: u64,
    /// Batch renders waiting for their turn now.
    pub waiting: usize,
    /// The device bytes and renders batch work may use ([`set_batch_limits`]).
    pub limit_bytes: u64,
    pub limit_jobs: usize,
    /// Batch renders that had to wait so far, and their total wait (ms).
    pub waits: u64,
    pub waited_ms: u64,
}

/// See [`BatchUsage`] (zero without a device).
pub fn batch_usage() -> BatchUsage {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let (jobs, bytes, waiting, waits, waited_ms) = gate::usage();
        let (limit_bytes, limit_jobs) = gate::limits(adapter_memory().unwrap_or(0));
        BatchUsage { jobs, bytes, waiting, limit_bytes, limit_jobs, waits, waited_ms }
    }
    #[cfg(target_arch = "wasm32")]
    {
        BatchUsage::default()
    }
}

/// The graphics card's memory (bytes) as its backend reports it (`lcc_gpumem`), else the largest
/// buffer the device allows; `None` before the device exists or without one.
pub fn adapter_memory() -> Option<u64> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        existing_device().map(|g| g.vram.unwrap_or(g.max_buffer))
    }
    #[cfg(target_arch = "wasm32")]
    {
        None
    }
}

/// What happened to the GPU in this session after event number `after` (0: everything kept), in
/// plain words: it stopped responding, it was restarted, the app gave up on it.
#[cfg(not(target_arch = "wasm32"))]
pub fn events_after(after: u64) -> Vec<GpuEvent> {
    slot::events_after(after)
}

/// Why the GPU last failed in this session (a lost device, a device error), even when a new device
/// has replaced it since; `None` when it never did.
pub fn last_loss() -> Option<String> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        slot::last_loss()
    }
    #[cfg(target_arch = "wasm32")]
    {
        None
    }
}

/// Lost devices replaced so far in this session.
pub fn recoveries() -> u32 {
    #[cfg(not(target_arch = "wasm32"))]
    {
        slot::recoveries()
    }
    #[cfg(target_arch = "wasm32")]
    {
        0
    }
}

/// Treat the device in use as lost, as a driver reset would (tests of the recovery): renders go to
/// the CPU, and a new device replaces it after the pause.
#[doc(hidden)]
pub fn simulate_device_loss(reason: &str) {
    mark_broken(0, reason);
}

/// Which device renders now (each one created gets the next number); `None` without one.
#[doc(hidden)]
pub fn device_generation() -> Option<u64> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        slot::current().map(|g| g.generation)
    }
    #[cfg(target_arch = "wasm32")]
    {
        None
    }
}

static ENABLED: AtomicBool = AtomicBool::new(true);
/// The latest render that fell back to the CPU, and why.
static LAST_FALLBACK: Mutex<Option<String>> = Mutex::new(None);

/// The device in use failed (a device error, a lost device, a render that panicked): renders go
/// to the CPU until it is replaced (`slot.rs`: after a short pause, a new device). `generation`
/// 0: whichever device is in use.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
fn mark_broken(generation: u64, reason: &str) {
    #[cfg(not(target_arch = "wasm32"))]
    slot::lose(generation, reason);
    #[cfg(target_arch = "wasm32")]
    let _ = (generation, reason);
}

#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
fn record_fallback(reason: String) {
    *LAST_FALLBACK.lock().unwrap_or_else(|e| e.into_inner()) = Some(reason);
}

/// Why renders do not use the GPU, or `None` when they do (or will, once the device is created).
/// E.g. "disabled by {ENV_PREFIX}_GPU=0", "software adapter (llvmpipe …) skipped", "device lost …".
/// Never blocks.
pub fn unavailable_reason() -> Option<String> {
    if let Some(why) = env_off_reason() {
        return Some(why);
    }
    if !ENABLED.load(Ordering::Relaxed) {
        return Some("disabled by the GPU rendering preference (app.gpu)".into());
    }
    if shutting_down() {
        return Some("the app is closing".into());
    }
    if let Some(r) = broken_here() {
        return Some(format!("stopped after a GPU failure: {r}"));
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        if let Some(r) = slot::lost_reason() {
            return Some(if slot::recoveries() >= slot::MAX_RECOVERIES {
                format!("stopped after a GPU failure: {r}")
            } else {
                format!("paused after a GPU failure, restarting the graphics card: {r}")
            });
        }
        slot::creation_error()
    }
    #[cfg(target_arch = "wasm32")]
    {
        Some("the browser build has no GPU path yet".into())
    }
}

/// The latest render in the process that fell back from the GPU to the CPU and why (device limit,
/// out of memory, device error, incomplete or blank result), if any. A diagnostic for status
/// displays (`ui.inspect` → `perf.gpuFallback`): renders on other threads overwrite it, so a caller
/// that wants the reason for *its own* render uses [`try_render`], which returns it.
pub fn last_fallback() -> Option<String> {
    LAST_FALLBACK.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Why [`try_render`] made no GPU image: the caller renders on the CPU.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fallback {
    /// The GPU was not tried: it is unavailable, disabled or closing ([`unavailable_reason`]), or
    /// the request is one only the CPU renders (high bit depth, soft proof, HDR range overlay).
    NotTried,
    /// The render was tried on the GPU and failed, for this reason (the same text
    /// [`last_fallback`] reports). The CPU renders it instead.
    Failed(String),
}

impl Fallback {
    /// The failure's reason; `None` when the GPU was not tried.
    pub fn reason(&self) -> Option<&str> {
        match self {
            Fallback::NotTried => None,
            Fallback::Failed(r) => Some(r),
        }
    }
}

/// A simulated GPU failure for the next [`render`] on this thread (tests of the CPU fallback).
#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    /// A command the device rejects (validation error).
    Validation,
    /// The per-pixel stage's work never runs (what a driver that silently drops or resets a
    /// submission looks like: the readback holds no image).
    DropWork,
    /// A storage-buffer limit of this many bytes for this render.
    Limit(u64),
    /// Every buffer the render asks for finds the device out of memory, on the first attempt
    /// only: the retry with the device to itself succeeds.
    OutOfMemory,
}

thread_local! {
    static FAULT: std::cell::Cell<Option<Fault>> = const { std::cell::Cell::new(None) };
    /// Why a simulated failure ([`inject_fault`]) stopped GPU use on this thread. A simulated
    /// fatal failure behaves like a real one for the thread that asked for it — [`render`] returns
    /// `None`, [`available`] is false, [`unavailable_reason`] says why — but leaves the rest of
    /// the process on the GPU: tests of the fallback run beside other tests rendering on it.
    static BROKEN_HERE: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// Make the next [`render`] on this thread fail with `f` (tests). A fatal `f` then stops GPU use
/// on this thread only, until [`reset_failures`].
#[doc(hidden)]
pub fn inject_fault(f: Fault) {
    FAULT.with(|c| c.set(Some(f)));
}

#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
fn take_fault() -> Option<Fault> {
    FAULT.with(|c| c.take())
}

/// Stop using the GPU on this thread (a simulated fatal failure).
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
fn mark_broken_here(reason: &str) {
    let _ = BROKEN_HERE.try_with(|b| {
        let mut b = b.borrow_mut();
        if b.is_none() {
            *b = Some(reason.to_string());
        }
    });
}

/// Why a simulated failure stopped GPU use on this thread, if one did.
fn broken_here() -> Option<String> {
    BROKEN_HERE.try_with(|b| b.borrow().clone()).ok().flatten()
}

/// Use the GPU again on this thread after a simulated failure ([`inject_fault`]; tests). Real
/// failures are not undone here (a new device replaces a lost one: see `slot.rs`), nor anything
/// another thread did.
#[doc(hidden)]
pub fn reset_failures() {
    let _ = BROKEN_HERE.try_with(|b| *b.borrow_mut() = None);
}

/// Allow or forbid GPU rendering at runtime (a preference). `{ENV_PREFIX}_GPU=0` forbids it for the
/// whole process regardless.
pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

/// Whether GPU rendering is allowed and the device is usable now (environment, preference, no
/// failure waiting for the device to be replaced, no simulated failure on this thread).
pub fn enabled() -> bool {
    allowed() && lost().is_none()
}

/// GPU rendering allowed (environment, preference, no simulated failure on this thread), whatever
/// the device's state (a lost device is replaced by the next [`device`] call after the pause).
fn allowed() -> bool {
    ENABLED.load(Ordering::Relaxed) && !env_disabled() && broken_here().is_none()
}

/// Why the device in use failed, while it waits to be replaced (`None`: it works, or there is none).
pub fn lost() -> Option<String> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        slot::lost_reason()
    }
    #[cfg(target_arch = "wasm32")]
    {
        None
    }
}

/// The process is about to end: no GPU work starts from now on, on any thread. [`render`] returns
/// `None` (callers render on the CPU), the devices are not created and denoise tiles fail over to
/// the CPU. There is no way back. Follow it with [`wait_idle`].
pub fn begin_shutdown() {
    #[cfg(not(target_arch = "wasm32"))]
    exit::begin_shutdown();
}

/// Whether [`begin_shutdown`] was called.
pub fn shutting_down() -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    {
        exit::shutting_down()
    }
    #[cfg(target_arch = "wasm32")]
    {
        false
    }
}

/// Wait up to `timeout` for the GPU work in flight on every thread to end. `true`: none is left,
/// and after [`begin_shutdown`] none can start, so the process may end; `false`: the deadline
/// passed with a render (or a device being created) still inside the driver.
pub fn wait_idle(timeout: std::time::Duration) -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    {
        exit::wait_idle(timeout)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = timeout;
        true
    }
}

/// Why GPU compute that has a device of its own (denoise, faces) is switched off by the user's settings, or `None`: the
/// environment switches and the rendering preference, but not a render-device failure (the devices are separate).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn switched_off() -> Option<String> {
    if let Some(why) = env_off_reason() {
        return Some(why);
    }
    (!ENABLED.load(Ordering::Relaxed)).then(|| "disabled by the GPU rendering preference (app.gpu)".into())
}

/// Why the environment switches the GPU off for the whole process (`{ENV_PREFIX}_GPU=0` or
/// `{ENV_PREFIX}_GPU_BACKEND=off`), or `None`. Code with a GPU path of its own (the SAM 3 model on
/// Metal) follows it too.
pub fn env_off_reason() -> Option<String> {
    if !env_disabled() {
        return None;
    }
    #[cfg(not(target_arch = "wasm32"))]
    if backend::env_off() {
        return Some(format!("disabled by {}=off", lcc_brand::env_var("GPU_BACKEND")));
    }
    Some(format!("disabled by {}=0", lcc_brand::env_var("GPU")))
}

/// `{ENV_PREFIX}_GPU=0` (or `{ENV_PREFIX}_GPU_BACKEND=off`): no GPU for the whole process.
fn env_disabled() -> bool {
    static OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *OFF.get_or_init(|| {
        let off = lcc_brand::env("GPU").is_some_and(|v| matches!(v.trim(), "0" | "off" | "false" | "no"));
        #[cfg(not(target_arch = "wasm32"))]
        let off = off || backend::env_off();
        off
    })
}

/// The render device: created on first use; after a failure, replaced once a short pause is over
/// (`None` meanwhile, and while GPU rendering is turned off: render on the CPU). See `slot.rs`.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn device() -> Option<Arc<ctx::Gpu>> {
    // turned off (preference, environment): don't even load the driver (issue #136)
    if env_disabled() || !ENABLED.load(Ordering::Relaxed) {
        return slot::current();
    }
    if let Some(g) = slot::current() {
        return Some(g);
    }
    // creating the device is a long call into the driver: not once the process is ending
    let _work = exit::enter()?;
    slot::get(new_device)
}

/// A device of our own on the best adapter (see [`ctx::Gpu::new`]): on a thread of its own (under
/// the crash sentinel, counted as GPU work), waited for at most `init::limit()`: a driver or
/// display server that never answers leaves the GPU unavailable instead of every render waiting
/// forever (#3068).
#[cfg(not(target_arch = "wasm32"))]
fn new_device() -> Result<ctx::Gpu, String> {
    let Some(backends) = backend::compute_backends() else { return Err(format!("disabled by {}=off", lcc_brand::env_var("GPU_BACKEND"))) };
    init::create(backends, move || ctx::Gpu::new(backends))
}

/// The device if it has been created, even one that failed (never creates one).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn existing_device() -> Option<Arc<ctx::Gpu>> {
    slot::existing()
}

/// Create the device and compile the kernels on a background thread now (once the window is up),
/// so the first render doesn't wait ~0.3–0.4 s for it and the UI thread never does. Does nothing
/// while GPU rendering is turned off.
pub fn warm_up() {
    #[cfg(not(target_arch = "wasm32"))]
    if !env_disabled() && ENABLED.load(Ordering::Relaxed) && !slot::tried() {
        let _ = std::thread::Builder::new().name("lc-gpu-init".into()).spawn(|| {
            let _ = device();
        });
    }
}

/// Has device creation finished (successfully or not)? Never blocks — for status displays.
pub fn ready() -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    {
        env_disabled() || slot::tried()
    }
    #[cfg(target_arch = "wasm32")]
    {
        true
    }
}

/// Whether a usable GPU adapter exists and GPU rendering is enabled (creates the device on first
/// use).
pub fn available() -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    {
        allowed() && device().is_some()
    }
    #[cfg(target_arch = "wasm32")]
    {
        false
    }
}

/// The adapter's name and backend, e.g. "Apple M4 Pro (Metal)".
pub fn adapter_name() -> Option<String> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        device().map(|g| format!("{} ({:?})", g.info.name, g.info.backend))
    }
    #[cfg(target_arch = "wasm32")]
    {
        None
    }
}

/// Device buffers held by the renderer.
#[cfg(not(target_arch = "wasm32"))]
pub use ctx::GpuMemory;

/// Device buffers held by the renderer.
#[cfg(target_arch = "wasm32")]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GpuMemory {
    pub allocated: u64,
    pub pooled: u64,
    pub retired: u64,
    pub peak: u64,
}

/// Measure [`GpuMemory::peak`] from now on (a benchmark).
pub fn reset_memory_peak() {
    #[cfg(not(target_arch = "wasm32"))]
    ctx::reset_peak();
}

/// Device memory held by the renderer's buffers (zero without a device).
pub fn memory() -> GpuMemory {
    #[cfg(not(target_arch = "wasm32"))]
    {
        ctx::memory()
    }
    #[cfg(target_arch = "wasm32")]
    {
        GpuMemory::default()
    }
}

/// Keep at most `bytes` of recycled buffers in the free pool from now on (trimming it now).
pub fn set_pool_limit(bytes: u64) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        ctx::POOL_LIMIT.store(bytes, Ordering::Relaxed);
        trim_pool(bytes);
    }
    #[cfg(target_arch = "wasm32")]
    let _ = bytes;
}

/// Free recycled buffers until at most `keep` bytes stay pooled (e.g. when the app goes idle).
pub fn trim_pool(keep: u64) {
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(g) = existing_device()
        && let Some(_work) = exit::enter()
    {
        if keep == 0 {
            render::release_shared_source();
        }
        g.trim(keep);
    }
    #[cfg(target_arch = "wasm32")]
    let _ = keep;
}

/// Device bytes of the one big source (a zoom window's original) shared by every view of the photo.
pub fn shared_source_bytes() -> usize {
    #[cfg(not(target_arch = "wasm32"))]
    {
        render::shared_source_bytes()
    }
    #[cfg(target_arch = "wasm32")]
    {
        0
    }
}

/// Big sources uploaded to the shared copy so far (a diagnostic: one big photo is uploaded once, not once per window).
pub fn source_uploads() -> u64 {
    #[cfg(not(target_arch = "wasm32"))]
    {
        render::source_uploads()
    }
    #[cfg(target_arch = "wasm32")]
    {
        0
    }
}

/// Device bytes held by a view's GPU stages (kept with its [`StageCache`]); 0 when it has none.
pub fn stage_bytes(stages: &StageCache) -> usize {
    #[cfg(not(target_arch = "wasm32"))]
    {
        stages.peek_extension::<GpuStages>().map(|g| g.bytes()).unwrap_or(0)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = stages;
        0
    }
}

/// Render `src` with `s` on the GPU, reusing the device-resident stages kept with `stages` (the
/// view's CPU stage cache). `None`: render on the CPU instead. [`try_render`] also says why.
pub fn render(src: &Arc<Rgb32f>, info: &SourceInfo, s: &DevelopSettings, req: &RenderRequest, stages: Option<&StageCache>) -> Option<Rendered> {
    try_render(src, info, s, req, stages).ok()
}

/// Grade one video frame ([`lcc_pipeline::grade`]): on the GPU when it is available, else on the
/// CPU. Returns the sRGB 8-bit frame and whether the GPU rendered it.
pub fn grade_frame(img: &Arc<Rgb32f>, settings: &DevelopSettings, quality: lcc_pipeline::Quality) -> (lcc_raster::Rgba8, bool) {
    use lcc_pipeline::grade;
    let s = grade::video_settings(settings);
    let req = grade::grade_request(img.width, img.height, quality);
    let info = grade::video_source_info();
    match try_render(img, &info, &s, &req, None) {
        Ok(r) => (r.image, true),
        Err(_) => (lcc_pipeline::render(img, &info, &s, &req).image, false),
    }
}

/// Whether the GPU renders requests of this kind at all (a given render can still fall back to
/// the CPU): the kernel writes 8-bit output or, for HDR renders ([`OutputDepth::F32Hdr`]: HDR
/// views, HDR exports, gain maps), linear float; 16-bit and SDR float exports render on the CPU, and
/// so do soft proofs against an output-device profile (`device_proof`: its 3D table is CPU-side)
/// and Visualize HDR, which needs both renditions at once.
///
/// [`OutputDepth::F32Hdr`]: lcc_pipeline::OutputDepth::F32Hdr [`try_render`] follows it,
/// and the loupe sizes its slider-drag drafts by it (issue #330), so the two can't drift apart.
pub fn handles(depth: lcc_pipeline::OutputDepth, device_proof: bool, overlay: lcc_pipeline::Overlay) -> bool {
    matches!(depth, lcc_pipeline::OutputDepth::U8 | lcc_pipeline::OutputDepth::F32Hdr) && !device_proof && overlay != lcc_pipeline::Overlay::HdrRange
}

/// [`render`], reporting why this render made no GPU image: [`Fallback::NotTried`] when the GPU
/// was not used (callers can ask [`unavailable_reason`]), [`Fallback::Failed`] with the reason
/// when the GPU render failed and the CPU must render it. The reason belongs to this call, unlike
/// [`last_fallback`], which the latest render on any thread overwrites.
pub fn try_render(
    src: &Arc<Rgb32f>,
    info: &SourceInfo,
    s: &DevelopSettings,
    req: &RenderRequest,
    stages: Option<&StageCache>,
) -> Result<Rendered, Fallback> {
    try_render_fitted(src, info, s, req, stages, None)
}

/// [`try_render`], with the result shrunk on the device to fit `fit` (physical pixels) with
/// `lcc_raster::resample::fit_rgba8`'s Lanczos filter, when it is larger (the loupe's display fit):
/// only the fitted image is read back. The histogram stays the full render's. HDR renders,
/// overlays and windows whose spots grew them come back unfitted, for the caller to fit.
pub fn try_render_fitted(
    src: &Arc<Rgb32f>,
    info: &SourceInfo,
    s: &DevelopSettings,
    req: &RenderRequest,
    stages: Option<&StageCache>,
    fit: Option<(usize, usize)>,
) -> Result<Rendered, Fallback> {
    try_render_with(src, info, s, req, stages, fit.map(|size| Fit { size, full: None }), None)
}

/// The display fit asked of a render: the size the image is drawn at, and where to put the
/// full-size image too when the caller keeps that (left `None` when no fit was made).
pub(crate) struct Fit<'a> {
    pub size: (usize, usize),
    pub full: Option<&'a mut Option<lcc_raster::Rgba8>>,
}

/// [`try_render_fitted`] that also reads back the full-size render when it fits one: for a view
/// whose render is kept at its own level (the photo's view preview) while only the fitted image
/// is drawn. The second value is `None` when nothing was fitted (the render is the full image).
pub fn try_render_fitted_keeping_full(
    src: &Arc<Rgb32f>,
    info: &SourceInfo,
    s: &DevelopSettings,
    req: &RenderRequest,
    stages: Option<&StageCache>,
    fit: (usize, usize),
) -> Result<(Rendered, Option<lcc_raster::Rgba8>), Fallback> {
    let mut full = None;
    let r = try_render_with(src, info, s, req, stages, Some(Fit { size: fit, full: Some(&mut full) }), None)?;
    Ok((r, full))
}

/// [`try_render`] of an HDR render ([`lcc_pipeline::OutputDepth::F32Hdr`]) for an HDR display:
/// its floats become the HDR texture's `Rgba16Float` texels for `headroom` on the device, as
/// `lcc_engine::hdr_texture::encode_rgba16f` makes them, and only those come back with the 8-bit
/// image and histogram (the result has no `deep` samples). Four halves per pixel.
pub fn try_render_hdr_texels(
    src: &Arc<Rgb32f>,
    info: &SourceInfo,
    s: &DevelopSettings,
    req: &RenderRequest,
    stages: Option<&StageCache>,
    headroom: f32,
) -> Result<(Rendered, Vec<u16>), Fallback> {
    if req.depth != lcc_pipeline::OutputDepth::F32Hdr {
        return Err(Fallback::NotTried);
    }
    let mut texels = Vec::new();
    let r = try_render_with(src, info, s, req, stages, None, Some((headroom, &mut texels)))?;
    Ok((r, texels))
}

fn try_render_with(
    src: &Arc<Rgb32f>,
    info: &SourceInfo,
    s: &DevelopSettings,
    req: &RenderRequest,
    stages: Option<&StageCache>,
    mut fit: Option<Fit<'_>>,
    mut texels: Option<(f32, &mut Vec<u16>)>,
) -> Result<Rendered, Fallback> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        // sections switched off with their eye render as if at their defaults (issue #316)
        let effective = s.effective();
        let s: &DevelopSettings = &effective;
        let device_proof = req.proof.is_some_and(|p| lcc_pipeline::proof_profile(p.profile).is_some());
        // pixel-filter plug-ins run in the CPU pipeline
        if !allowed() || gate::cpu_only() || !handles(req.depth, device_proof, req.overlay) || lcc_pipeline::plugin_filters::any_active(s) {
            return Err(Fallback::NotTried);
        }
        let s = &*lcc_pipeline::settings_for(s, req);
        // in flight until this returns; not tried once the process is ending (issue #620)
        let gpu = device().ok_or(Fallback::NotTried)?;
        // The plan (Upright, the masks' tone ranges, red eye, the settings resolved) is the
        // CPU's work: made before the turn, so an export waiting for the GPU never holds it with
        // the processor's part of its render.
        let plan = lcc_pipeline::plan(src, info, s, req);
        // An export's turn at the GPU (see `gate.rs`): it waits while other exports use the card's
        // memory, while a view renders and while the window asked for room. Views never wait.
        let out_px = match req.window {
            Some(w) => w.w.saturating_mul(w.h),
            None => {
                let (w, h) = lcc_pipeline::output_size(src.width, src.height, s, req);
                w.saturating_mul(h)
            }
        };
        let vram = gpu.vram.unwrap_or(gpu.max_buffer);
        let _turn = gate::enter(gate::job_bytes(src.width.saturating_mul(src.height), out_px), vram, &shutting_down).ok_or(Fallback::NotTried)?;
        let _work = exit::enter().ok_or(Fallback::NotTried)?;
        let ext = stages.map(|c| c.extension::<GpuStages>());
        // stages cached on a device that was replaced are no use on this one
        if let Some(e) = &ext {
            e.adopt_generation(gpu.generation);
        }
        let mut fault = take_fault();
        // A simulated failure stops GPU use on this thread only (see `BROKEN_HERE`).
        let simulated = fault.is_some();
        let stop = |reason: &str| if simulated { mark_broken_here(reason) } else { mark_broken(gpu.generation, reason) };
        // Renders share the device's memory: wait for room instead of running out of it, and after
        // running out anyway (the size is an estimate) try once more with the device to this
        // render alone before the CPU takes over (upstream #1764).
        static BUDGET: std::sync::OnceLock<budget::Budget> = std::sync::OnceLock::new();
        let budget = BUDGET.get_or_init(|| budget::Budget::new(budget::initial(gpu.max_buffer)));
        let need = budget::footprint(src.width, src.height);
        let mut alone = false;
        let reason = loop {
            let _permit = budget.admit(need, alone, shutting_down).ok_or(Fallback::NotTried)?;
            let _ = ctx::take_failure(); // nothing left over from an earlier render on this thread
            let (r, scoped) = {
                let _scope = ctx::RenderScope::new(&gpu);
                let errors = ctx::ErrorScopes::push(&gpu);
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let fit = fit.as_mut().map(|f| Fit { size: f.size, full: f.full.as_deref_mut() });
                    render::render(&gpu, &plan, src, info, req, ext.as_deref(), fault, fit, texels.as_mut().map(|(h, t)| (*h, &mut **t)))
                }));
                (r, errors.pop())
            };
            // An error the device raised explains the readback failures that follow it; a buffer
            // over the limit or out of memory explains the errors its placeholder raises.
            let failure = match (ctx::take_failure(), scoped) {
                (Some(f), _) if matches!(f.kind, ctx::FailKind::Limit | ctx::FailKind::OutOfMemory) => Some(f),
                (f, scoped) => scoped.or(f),
            };
            break match (r, failure) {
                (Err(_), _) => {
                    log::error!("gpu: render failed; using the CPU pipeline until the device is replaced");
                    stop("a GPU render panicked");
                    "a GPU render panicked; the device is replaced".to_string()
                }
                (Ok(r), Some(f)) => {
                    // the view's cached device stages may hold results of the failed passes
                    if let Some(e) = &ext {
                        e.clear();
                    }
                    let what = match &r {
                        Some(r) => format!("{}×{}", r.image.width, r.image.height),
                        None => "GPU render".into(),
                    };
                    let reason = match f.kind {
                        ctx::FailKind::Fatal => {
                            stop(&f.reason);
                            format!("{what}: {}; the device is replaced", f.reason)
                        }
                        ctx::FailKind::OutOfMemory => {
                            gpu.trim(0);
                            budget.out_of_memory();
                            if !alone {
                                fault = fault.filter(|f| *f != Fault::OutOfMemory);
                                log::info!("gpu: {what}: {}; retrying with the device to this render alone", f.reason);
                                alone = true;
                                continue;
                            }
                            format!("{what}: {}", f.reason)
                        }
                        ctx::FailKind::Limit | ctx::FailKind::Blank | ctx::FailKind::Busy => format!("{what}: {}", f.reason),
                    };
                    log::warn!("gpu: rendering on the CPU instead: {reason}");
                    if render::profiling() {
                        eprintln!("  gpu fallback to the CPU: {reason}");
                    }
                    reason
                }
                // a device error on another thread or a lost device during the render
                (Ok(Some(_)), None) if lost().is_some() => {
                    format!("device failure during the render: {}", unavailable_reason().unwrap_or_default())
                }
                (Ok(Some(r)), None) => {
                    budget.succeeded();
                    return Ok(r);
                }
                // every path of `render::render` that gives up records a failure first; should one
                // not, the CPU renders it rather than nothing
                (Ok(None), None) => "GPU render: no image came back".to_string(),
            };
        };
        record_fallback(reason.clone());
        Err(Fallback::Failed(reason))
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (src, info, s, req, stages, fit, texels);
        Err(Fallback::NotTried)
    }
}

/// Tests: parse and validate a WGSL module with naga, as wgpu does before it hands a shader to the driver. Needs no
/// GPU, so kernels that don't build (a duplicate definition after a merge, a type error) fail every test run, not only
/// the GPU ones.
#[cfg(test)]
pub(crate) fn validate_wgsl(name: &str, src: &str) -> Result<(), String> {
    use wgpu::naga;
    let module = naga::front::wgsl::parse_str(src).map_err(|e| format!("{name}: {}", e.emit_to_string(src)))?;
    naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all())
        .validate(&module)
        .map(|_| ())
        .map_err(|e| format!("{name}: {}", e.emit_to_string(src)))
}

#[cfg(test)]
mod tests {
    use lcc_pipeline::{OutputDepth, Overlay};

    /// The helper refuses what a driver would: a function defined twice (what an automatic merge of two branches
    /// touching the same kernel can leave).
    #[test]
    fn wgsl_validation_catches_a_duplicate_definition() {
        let ok = "fn f() -> f32 { return 1.0; }\n@compute @workgroup_size(1) fn main() { _ = f(); }\n";
        assert_eq!(super::validate_wgsl("ok", ok), Ok(()));
        let twice = format!("fn f() -> f32 {{ return 2.0; }}\n{ok}");
        assert!(super::validate_wgsl("twice", &twice).is_err());
    }

    // Given a request the kernel can't draw (16-bit or SDR float, a soft proof, Visualize HDR),
    // the GPU doesn't take it; the loupe, HDR renders and diagnostic overlays stay on the GPU
    #[test]
    fn the_gpu_handles_8_bit_and_hdr_unproofed_renders_without_the_hdr_range() {
        assert!(super::handles(OutputDepth::U8, false, Overlay::None));
        assert!(super::handles(OutputDepth::F32Hdr, false, Overlay::None));
        assert!(!super::handles(OutputDepth::F32Linear, false, Overlay::None));
        assert!(super::handles(OutputDepth::U8, false, Overlay::Spots(50)));
        assert!(!super::handles(OutputDepth::U16, false, Overlay::None));
        assert!(!super::handles(OutputDepth::U8, true, Overlay::None));
        assert!(!super::handles(OutputDepth::U8, false, Overlay::HdrRange));
    }
}
