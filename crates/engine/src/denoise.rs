//! AI denoise, the session's side (`docs/denoise.md`): which photos have a denoised picture, making the missing ones
//! in the background, and keeping the cache within its size.
//!
//! The denoised picture of a raw photo is *cached data*, never a file in the library: it is made from the untouched
//! original by the model the user chose, kept in `<library>/denoise/<key>.lcdn`, and thrown away whenever space is
//! needed (it is made again when it is next wanted). Its name is a hash of the model, the algorithm and the photo's
//! content, so a changed file or another model simply does not find it. The Denoise *amount* is an ordinary develop
//! setting (`enhance.denoise`): a render mixes the plain and the denoised picture by it ([`crate::media`]).
//!
//! What runs where: the session thread only decides (which photos, which model) and keeps an index of what is ready
//! ([`crate::media::DenoiseIndex`], memory only). One background thread at a time decodes a photo, runs the model on
//! its mosaic tile by tile and writes the product ([`make_product`]); an export that finds its photo's product missing
//! makes it itself, on its own thread, so it never comes out without the denoise it was asked for.
//!
//! A finished picture (JPEG, PNG, TIFF …) has no mosaic: it is denoised by a model made for finished pictures
//! ([`Domain::RgbToRgb`], `settings.rgbModel`), run over its gamma-encoded sRGB pixels ([`make_rgb_product`]), and its
//! cached picture is swapped in when the file is loaded (`crate::files`). The built-in denoiser is for raw photos only.
//!
//! Failure is a message, never a panic: a file that is not a Bayer raw, a model that does not load, a full disk, a
//! panic inside the runtime and a cancelled job all end as an [`Outcome`] the pump records.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, PoisonError};
use std::time::{Duration, SystemTime};

use lcc_catalog::{Photo, PhotoId, Source};
use lcc_denoise::bayer::Layout;
use lcc_denoise::manifest::{DenoiserManifest, Domain};
use lcc_denoise::run::{Control, Params, TileRunner, denoise_bayer};
use lcc_denoise::{Error as RunError, product};
use lcc_develop::DevelopSettings;
use lcc_preview::Hasher128;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::media::{DenoiseSpec, MakeProduct, PairLoader};
use crate::merge::ByteReader;
use crate::model_download::Downloads;
use crate::{Selection, Session};

/// Extension of a product file.
pub const EXT: &str = "lcdn";
/// Where the products of a library are kept, inside the library's folder (safe to delete, like `thumbs/`).
pub const DIR: &str = "denoise";
/// What the cache may take up unless the user says otherwise (GB). A product is about as large as the raw file's
/// decoded samples, compressed: ~100 MB for 24 megapixels.
pub const DEFAULT_CACHE_GB: u32 = 20;
/// Samples at or above this (white = 1) are clipped: the model leaves them as the camera recorded them.
const CLIP: f32 = 0.99;
/// Most photos waiting in the queue.
const MAX_QUEUE: usize = 5000;
/// Most selected photos the automatic queue looks at in one pump.
const AUTO_LOOK: usize = 64;
/// How long a look at the model settings is trusted.
const SETTINGS_TTL: Duration = Duration::from_secs(2);
/// Stack of the threads that run the model (the runtime recurses through its graph).
pub(crate) const STACK: usize = 16 << 20;

// ---------------------------------------------------------------------------------------------------------------------
// the model

/// Where the model runs (Settings ▸ AI Denoise ▸ Speed; `denoise.settings {runOn}`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RunOn {
    /// The graphics card where it is faster than the processor at the pace the work goes at (both are timed on this
    /// computer when the card is set up), else the processor.
    #[default]
    Auto,
    /// The graphics card whenever it can run the model, even where the processor would be faster.
    Gpu,
    /// Always the processor.
    Cpu,
}

impl RunOn {
    /// `auto`, `gpu` or `cpu`.
    pub fn parse(name: &str) -> Option<RunOn> {
        match name {
            "auto" => Some(RunOn::Auto),
            "gpu" => Some(RunOn::Gpu),
            "cpu" => Some(RunOn::Cpu),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            RunOn::Auto => "auto",
            RunOn::Gpu => "gpu",
            RunOn::Cpu => "cpu",
        }
    }
}

/// A loaded denoise model: what runs tiles, and a self-check.
pub(crate) trait Model: Send + Sync {
    /// What runs a photo's tiles, and how many to run at once, when `threads` may be used and the setting is `run_on`
    /// (a graphics card is kept busy by a few). The first time the cards may be used they are set up and checked here.
    /// The runner holds the photo's graphics card until it is dropped.
    fn runner(&self, run_on: RunOn, threads: usize) -> (Box<dyn TileRunner + '_>, usize);
    /// Photos that may be made at once under `run_on`: one per graphics card that works (one when there is none, or none
    /// was set up yet). Never starts anything.
    fn lanes(&self, _run_on: RunOn) -> usize {
        1
    }
    /// Check it does something sensible: the test's result, or why not. Unless `run_on` is the processor the graphics
    /// card is set up and checked too, and the result says where the work will run.
    fn self_test(&self, run_on: RunOn) -> Result<Value, String>;
    /// Look at the photo's whole mosaic before its tiles run (a model that measures the noise from the photo does it
    /// here, so every tile is cleaned by the same measure).
    fn prepare(&self, _mosaic: &[f32], _width: usize, _height: usize) {}
    /// Conservative scratch bound for an unknown runner.
    fn bytes_per_tile(&self) -> usize {
        512 * 1024 * 1024
    }
    /// Where the tiles run: `{kind: "gpu", adapter}`, `{kind: "cpu", reason?}` or `{kind: "pending"}` (the card is not
    /// set up yet), with the card's and the processor's time for the check tile (`cardMs`, `cpuMs`) once both were
    /// timed. Never starts anything.
    fn device(&self, _run_on: RunOn) -> Value {
        json!({"kind": "cpu"})
    }
}

/// A runner lent for one photo (what [`Model::runner`] returns for a test model whose runner is itself).
#[cfg(test)]
pub(crate) struct Lent<'a>(pub &'a dyn TileRunner);

#[cfg(test)]
impl TileRunner for Lent<'_> {
    fn run(&self, input: &[f32]) -> Result<Vec<f32>, RunError> {
        self.0.run(input)
    }
}

/// The built-in denoiser (`lcc_denoise::builtin`): nothing to load, so it is what runs until a model is chosen. Its
/// filter runs on the graphics card when there is one that agrees with the CPU on a check tile and is faster, else on
/// the CPU (see [`BuiltinCard`]); the rest of it always runs on the CPU.
struct BuiltinModel {
    runner: OnceLock<lcc_denoise::builtin::Builtin>,
    #[cfg(feature = "denoise")]
    card: Arc<BuiltinCard>,
}

impl BuiltinModel {
    /// For a photo, with the card's state for the set-up marker `marker` (where its set-up is recorded).
    fn new(#[cfg_attr(not(feature = "denoise"), allow(unused_variables))] marker: &Path) -> Self {
        BuiltinModel {
            runner: OnceLock::new(),
            #[cfg(feature = "denoise")]
            card: builtin_card(marker),
        }
    }
}

impl Model for BuiltinModel {
    fn runner(&self, run_on: RunOn, threads: usize) -> (Box<dyn TileRunner + '_>, usize) {
        let cpu = *self.runner.get_or_init(Default::default);
        #[cfg(feature = "denoise")]
        {
            let threads = threads.max(1);
            self.card.last_threads.store(threads, Ordering::Relaxed);
            if run_on != RunOn::Cpu {
                self.card.set_up();
            }
            // the photo's card: the one with fewest photos of those it may run on, the second card first
            if let Some(i) = self.card.lease(run_on, threads) {
                let made = self
                    .card
                    .side(i)
                    .and_then(|s| s.gpu.get())
                    .and_then(|g| g.as_ref().ok())
                    .map(|g| lcc_gpu::bm3d::BuiltinRunner::new(g.gpu.clone(), cpu));
                match made {
                    Some(Ok(gpu)) => return (Box::new(CardRunner { gpu, cpu, card: self.card.clone(), side: i }), threads.min(GPU_PARALLEL)),
                    _ => self.card.release(i),
                }
            }
        }
        #[cfg(not(feature = "denoise"))]
        let _ = run_on;
        (Box::new(cpu), threads)
    }

    fn lanes(&self, run_on: RunOn) -> usize {
        #[cfg(feature = "denoise")]
        {
            self.card.lanes(run_on)
        }
        #[cfg(not(feature = "denoise"))]
        {
            let _ = run_on;
            1
        }
    }

    /// Measure the whole photo's noise once, so every tile is cleaned by the photo's noise instead of its own.
    fn prepare(&self, mosaic: &[f32], width: usize, height: usize) {
        let _ = self.runner.set(lcc_denoise::builtin::Builtin::with_noise(lcc_denoise::builtin::estimate_noise(mosaic, width, height)));
    }

    fn self_test(&self, _run_on: RunOn) -> Result<Value, String> {
        let side = 16;
        let out = lcc_denoise::builtin::Builtin::default().run(&vec![0.25; 4 * side * side]).map_err(|e| e.to_string())?;
        if out.len() != 12 * side * side || out.iter().any(|v| !v.is_finite()) {
            return Err("the built-in denoiser gave an unexpected result".into());
        }
        Ok(json!({"ok": true, "builtin": true}))
    }

    /// Planes, the two mosaics and the working copies of a 256-cell tile.
    fn bytes_per_tile(&self) -> usize {
        64 * 1024 * 1024
    }

    fn device(&self, run_on: RunOn) -> Value {
        #[cfg(feature = "denoise")]
        {
            self.card.device(run_on)
        }
        #[cfg(not(feature = "denoise"))]
        {
            let _ = run_on;
            json!({"kind": "cpu", "reason": "built-in"})
        }
    }
}

/// Loads the model at a path as its manifest describes it.
pub(crate) type Loader = Arc<dyn Fn(&Path, &DenoiserManifest) -> Result<Arc<dyn Model>, String> + Send + Sync>;

/// Tiles run at once on a graphics card: enough to keep it fed while others are packed and blended.
#[cfg(feature = "denoise")]
const GPU_PARALLEL: usize = 4;
/// How far the GPU's answer on the check tile may be from the CPU's (of the answer's largest value) before it is not used.
#[cfg(feature = "denoise")]
const GPU_AGREE: f32 = 1e-3;
/// Longest the card may take to set up (build its kernels, run the check tile, time it) before the processor does the
/// work instead.
#[cfg(feature = "denoise")]
const GPU_SETUP_LIMIT: Duration = Duration::from_secs(30);
/// Tiles the card may get wrong (an error, numbers that are not finite) before it stops being used for the model.
#[cfg(feature = "denoise")]
const GPU_MAX_FALLBACKS: usize = 3;
/// The file beside a model while the graphics card is set up for it (see [`set_up_card`]).
pub(crate) const GPU_SETUP_MARKER: &str = "gpu-setup.marker";

/// The set-up marker for `card` beside the card for editing's `marker` (which keeps the name it always had): the second
/// card's has `-helper` before `.marker`, so a crash setting up one card keeps only that card off.
pub(crate) fn card_marker(marker: &Path, second: bool) -> PathBuf {
    if !second {
        return marker.to_path_buf();
    }
    let name = marker.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| GPU_SETUP_MARKER.to_string());
    let name = match name.strip_suffix(".marker") {
        Some(stem) => format!("{stem}-helper.marker"),
        None => format!("{name}-helper"),
    };
    marker.with_file_name(name)
}

/// The set-up marker of `card` for the marker `marker` of the card for editing.
#[cfg(feature = "denoise")]
fn marker_of(marker: &Path, card: lcc_gpu::nn::Card) -> PathBuf {
    card_marker(marker, card == lcc_gpu::nn::Card::Second)
}

/// Whether `card` can be set up at all: the second card only when there is one (asks the drivers the first time).
#[cfg(feature = "denoise")]
fn card_exists(card: lcc_gpu::nn::Card) -> Result<(), String> {
    if card == lcc_gpu::nn::Card::Second && lcc_gpu::nn::card_name(card).is_none() {
        return Err(lcc_gpu::nn::NO_SECOND_CARD.into());
    }
    Ok(())
}

/// One value for where a photo's tiles run, from each card's (the second card first, `None` for a card that isn't
/// there): the first card a photo would use, else why the processor does the work (the card for editing's reason);
/// `cards` lists both when there are two.
#[cfg(feature = "denoise")]
fn merge_cards(cards: Vec<Value>) -> Value {
    let used = cards.iter().find(|c| c["kind"] == "gpu");
    let mut v = match (used, cards.last()) {
        (Some(c), _) | (None, Some(c)) => c.clone(),
        (None, None) => json!({"kind": "cpu", "retry": false, "reason": "the processor is chosen in Settings"}),
    };
    if let Some(o) = v.as_object_mut() {
        o.remove("card");
        o.remove("photos");
        if cards.len() > 1 {
            o.insert("cards".into(), json!(cards));
        }
    }
    v
}
/// Why the card is not tried when a set-up marker is found.
#[cfg(feature = "denoise")]
const GPU_CRASHED: &str = "The app closed while it was setting up the graphics card for AI Denoise last time";
/// Why the card is not used when its set-up ran out of time (the start of it).
#[cfg(feature = "denoise")]
const GPU_SLOW: &str = "setting up the graphics card took longer than";
/// Why the card is not used while a set-up that ran out of time is still going.
#[cfg(feature = "denoise")]
const GPU_STUCK: &str = "the graphics card is still setting up from before (it took too long)";

/// A set-up that may work another time (a crash, the time limit), unlike one the card or the model cannot do.
#[cfg(feature = "denoise")]
fn worth_retrying(why: &str) -> bool {
    why == GPU_CRASHED || why == GPU_STUCK || why.starts_with(GPU_SLOW)
}

/// The model on our pure-Rust CPU runner, and on the graphics card when there is one that can run it: the card is set up for the
/// first photo that may use it, checked against the CPU's answer on a check tile and timed against it, and used while
/// it keeps working; a tile it gets wrong is run on the CPU, so a photo is always finished.
#[cfg(feature = "denoise")]
struct Network {
    cpu: lcc_denoise::runtime::CpuRunner,
    path: PathBuf,
    tile: usize,
    /// Each card's side, in the order photos take them ([`lcc_gpu::nn::CARDS`]: the second card first). A batch makes
    /// a photo on each card that works at once; one photo's tiles stay on its card.
    cards: [CardSide; 2],
    /// Hands photos their card one at a time.
    picking: Mutex<()>,
    /// Tiles at once of the photo started last (0: none yet), to say where the work runs.
    last_threads: AtomicUsize,
    /// Another computer's graphics card (`<PREFIX>_DENOISE_URL` + `<PREFIX>_DENOISE_TOKEN`; see
    /// `docs/denoise.md` → Another computer's graphics card), greeted on first use: it takes tiles while it has a free
    /// slot, on top of this computer's runner.
    remote: Option<RemoteSide>,
}

/// One graphics card's part of a [`Network`].
#[cfg(feature = "denoise")]
struct CardSide {
    card: lcc_gpu::nn::Card,
    gpu: OnceLock<Result<GpuSide, String>>,
    /// Tiles the card got wrong (an error, numbers that are not finite) and the CPU ran.
    fallbacks: AtomicUsize,
    /// Photos running on it now.
    photos: AtomicUsize,
}

#[cfg(feature = "denoise")]
impl CardSide {
    fn new(card: lcc_gpu::nn::Card) -> CardSide {
        CardSide { card, gpu: OnceLock::new(), fallbacks: AtomicUsize::new(0), photos: AtomicUsize::new(0) }
    }
}

#[cfg(feature = "denoise")]
struct RemoteSide {
    runner: lcc_denoise::remote::Remote,
    /// The greeting, and the server's time for the check tile there and back (ms; the faster of two runs).
    greeted: OnceLock<Result<(lcc_denoise::remote::ServerInfo, f64), String>>,
    /// Whether the photo started last shares tiles with the server, and if not because it is slower, the two times.
    share: AtomicBool,
    slower: Mutex<Option<(f64, f64)>>,
}

#[cfg(feature = "denoise")]
impl RemoteSide {
    /// From the environment, for the model whose file has `sha256` and tiles of `tile` cells.
    fn from_env(sha256: &str, tile: usize) -> Option<RemoteSide> {
        let var = |k: &str| lcc_brand::env(k).map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
        let address = var("DENOISE_URL")?;
        let token = var("DENOISE_TOKEN").unwrap_or_default();
        Some(RemoteSide::new(lcc_denoise::remote::Remote::new(&address, &token, sha256, tile)))
    }

    fn new(runner: lcc_denoise::remote::Remote) -> RemoteSide {
        RemoteSide { runner, greeted: OnceLock::new(), share: AtomicBool::new(false), slower: Mutex::new(None) }
    }

    /// Greet the server and time a check tile there and back: a server behind a slow link can take seconds a tile, and
    /// handing it tiles a fast local card would finish sooner only makes the photo wait for it.
    fn greet(&self) -> Result<(lcc_denoise::remote::ServerInfo, f64), String> {
        let info = self.runner.hello().map_err(|e| e.to_string())?;
        let input = lcc_denoise::runtime::check_tile(info.tile);
        let mut best = f64::INFINITY;
        for _ in 0..2 {
            let started = web_time::Instant::now();
            match self.runner.try_run(&input) {
                Some(Ok(_)) => best = best.min(started.elapsed().as_secs_f64() * 1000.0),
                Some(Err(e)) => return Err(format!("the check tile failed: {e}")),
                None => return Err("no free slot for the check tile".into()),
            }
        }
        Ok((info, best))
    }

    /// The server when it can take tiles now (greeting it the first time), else why not.
    fn usable(&self, start: bool) -> Result<&lcc_denoise::remote::Remote, String> {
        let greeted = if start { Some(self.greeted.get_or_init(|| self.greet())) } else { self.greeted.get() };
        match greeted {
            None => Err("not contacted yet".into()),
            Some(Err(why)) => Err(why.clone()),
            Some(Ok(_)) if !self.runner.healthy() => {
                Err(format!("it failed {} tiles ({})", self.runner.failures(), self.runner.last_error().unwrap_or_default()))
            }
            Some(Ok(_)) => Ok(&self.runner),
        }
    }

    /// The server's time for a tile, once greeted.
    fn tile_ms(&self) -> Option<f64> {
        self.greeted.get().and_then(|g| g.as_ref().ok()).map(|g| g.1)
    }

    fn describe(&self) -> Value {
        match self.greeted.get() {
            None => json!({"address": self.runner.address(), "state": "pending"}),
            Some(Err(why)) => json!({"address": self.runner.address(), "state": "unavailable", "reason": why}),
            Some(Ok((info, ms))) => {
                let slower = *self.slower.lock().unwrap_or_else(PoisonError::into_inner);
                let state = match () {
                    _ if !self.runner.healthy() => "given up",
                    _ if slower.is_some() => "slower",
                    _ if self.share.load(Ordering::Relaxed) => "used",
                    _ => "idle",
                };
                let mut v =
                    json!({"address": info.address, "device": info.device, "state": state, "remoteMs": ms, "failures": self.runner.failures()});
                if let (Some((_, local)), Some(o)) = (slower, v.as_object_mut()) {
                    o.insert("localMs".into(), json!(local));
                }
                if let (Some(e), Some(o)) = (self.runner.last_error(), v.as_object_mut()) {
                    o.insert("lastError".into(), json!(e));
                }
                v
            }
        }
    }
}

#[cfg(feature = "denoise")]
struct GpuSide {
    runner: Box<dyn TileRunner + Send>,
    adapter: String,
    /// The check tile on the card, the fastest of a few runs (ms).
    card_ms: f64,
    /// The check tile on the CPU, on one thread (ms).
    cpu_ms: f64,
}

/// Whether the card (`card_ms` a tile) is faster than the CPU running `threads` tiles at once, from the CPU's time for
/// one tile alone (`cpu_ms`). Tiles at once share the memory and the cores' second threads, so each one more counts as
/// half a tile's speed: on a 16-core laptop one tile alone took 1.4 s and sixteen at once finished one every 0.2 s.
#[cfg(feature = "denoise")]
fn card_is_faster(card_ms: f64, cpu_ms: f64, threads: usize) -> bool {
    let at_once = 1.0 + 0.5 * threads.saturating_sub(1) as f64;
    card_ms.is_finite() && card_ms <= cpu_ms / at_once
}

/// Card set-ups of this process that have not finished, by marker (their markers are not a crash's).
#[cfg(feature = "denoise")]
static SETTING_UP: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

/// Removes a set-up's marker when the set-up ends, however it ends — but not when the process dies inside the driver.
#[cfg(feature = "denoise")]
struct SetUpEnded(PathBuf);

#[cfg(feature = "denoise")]
impl Drop for SetUpEnded {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
        SETTING_UP.lock().unwrap_or_else(PoisonError::into_inner).retain(|m| m != &self.0);
    }
}

/// Set the graphics card up with `card` on a thread of its own while `meanwhile` runs here, and give up on the card when
/// it takes longer than `limit` (a driver building kernels for minutes, or stuck). The `marker` file exists while `card`
/// runs: one found before it starts was left by a the app that closed during a set-up (a driver crash takes the app
/// with it), so the card is not tried again until the user chooses where denoise runs (`denoise.settings {runOn}`).
#[cfg(feature = "denoise")]
fn set_up_card<T: Send + 'static, R>(
    marker: &Path,
    limit: Duration,
    card: impl FnOnce() -> Result<T, String> + Send + 'static,
    meanwhile: impl FnOnce() -> R,
) -> Result<(T, R), String> {
    let ended = {
        let mut running = SETTING_UP.lock().unwrap_or_else(PoisonError::into_inner);
        if running.iter().any(|m| m == marker) {
            return Err(GPU_STUCK.into());
        }
        if marker.exists() {
            return Err(GPU_CRASHED.into());
        }
        let secs = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        // the models folder may not exist yet; a marker that cannot be written only loses the crash guard
        if let Some(parent) = marker.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(marker, format!("setting up the graphics card for AI Denoise (unix time {secs})\n"));
        running.push(marker.to_path_buf());
        SetUpEnded(marker.to_path_buf())
    };
    let started = web_time::Instant::now();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("denoise-gpu-setup".into())
        .stack_size(STACK)
        .spawn(move || {
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(card)).unwrap_or_else(|_| Err("the GPU runner gave up".into()));
            drop(ended);
            let _ = tx.send(r);
        })
        .map_err(|e| format!("could not start setting up the graphics card: {e}"))?;
    let here = meanwhile();
    match rx.recv_timeout(limit.saturating_sub(started.elapsed())) {
        Ok(r) => r.map(|t| (t, here)),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Err(format!("{GPU_SLOW} {} s, so it is not used", limit.as_secs_f64())),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Err("the GPU runner gave up".into()),
    }
}

/// The card's time for one tile once it is warm: the fastest of up to three runs, stopping after about two seconds (a
/// slow card is slow enough to tell at once).
#[cfg(feature = "denoise")]
fn card_tile_ms(runner: &dyn TileRunner, input: &[f32]) -> f64 {
    if runner.run(input).is_err() {
        return f64::INFINITY;
    }
    let began = web_time::Instant::now();
    let mut best = f64::INFINITY;
    for _ in 0..3 {
        let one = web_time::Instant::now();
        if runner.run(input).is_err() {
            break;
        }
        best = best.min(one.elapsed().as_secs_f64() * 1000.0);
        if began.elapsed() > Duration::from_secs(2) {
            break;
        }
    }
    best
}

/// Threads for a single tile that is waited on (the card's check tile, the installation self-test): as many as a job at
/// full pace may take, and of their own, as the jobs have, so a busy window's renders keep theirs.
#[cfg(feature = "denoise")]
fn check_pool() -> Result<rayon::ThreadPool, rayon::ThreadPoolBuildError> {
    let threads = Pace::Full.parallel(std::thread::available_parallelism().map_or(1, |n| n.get()), None);
    rayon::ThreadPoolBuilder::new().num_threads(threads).stack_size(STACK).thread_name(|i| format!("denoise-check-{i}")).build()
}

/// Set `card` up for the model at `path`: kernels built, the check tile run on both and compared, both timed.
#[cfg(feature = "denoise")]
fn make_gpu(cpu: &lcc_denoise::runtime::CpuRunner, path: &Path, tile: usize, card: lcc_gpu::nn::Card) -> Result<GpuSide, String> {
    card_exists(card)?;
    let input = Arc::new(lcc_denoise::runtime::check_tile_for(cpu.manifest().domain, tile));
    let (on_card, net) = (input.clone(), cpu.net().clone());
    let card_work = move || -> Result<_, String> {
        let runner = lcc_gpu::nn::runner_on(card, &net, tile)?;
        let got = runner.run(&on_card).map_err(|e| format!("the GPU could not run the check tile: {e}"))?;
        let card_ms = card_tile_ms(&runner, &on_card);
        Ok((runner, got, card_ms))
    };
    // The CPU's answer on the cores a job at full pace may take, so an older CPU doesn't keep the card waiting for
    // minutes; its time summed over the threads stands for one tile on one thread, which is what the card is compared
    // with.
    let on_cpu = || {
        let wide = match check_pool() {
            Ok(pool) => pool.install(|| cpu.run_wide(&input)),
            Err(e) => Err(lcc_denoise::run::Error::Runtime(format!("could not start the worker threads: {e}"))),
        };
        match wide {
            Ok((want, cpu_ms)) => (Ok(want), cpu_ms),
            Err(e) => (Err(e), f64::INFINITY),
        }
    };
    let marker = marker_of(&path.with_file_name(GPU_SETUP_MARKER), card);
    let ((runner, got, card_ms), (want, cpu_ms)) = set_up_card(&marker, GPU_SETUP_LIMIT, card_work, on_cpu)?;
    let want = want.map_err(|e| format!("the CPU could not check the GPU: {e}"))?;
    let scale = want.iter().fold(0f32, |m, v| m.max(v.abs())).max(1e-12);
    let worst = want.iter().zip(&got).fold(0f32, |m, (a, b)| m.max((a - b).abs())) / scale;
    if got.len() != want.len() || !got.iter().all(|v| v.is_finite()) || worst > GPU_AGREE {
        return Err(format!("its answer on the check tile is not the CPU's (off by {worst:.1e} of the largest value): not used"));
    }
    Ok(GpuSide { adapter: runner.adapter(), runner: Box::new(runner), card_ms, cpu_ms })
}

/// Run `make` for each card not tried yet, all at once (each on a thread of its own: a card's set-up runs the CPU's
/// check meanwhile).
#[cfg(feature = "denoise")]
fn set_up_each<'a, S: Sync>(todo: Vec<&'a S>, make: impl Fn(&'a S) + Sync) {
    if todo.is_empty() {
        return;
    }
    std::thread::scope(|scope| {
        for side in todo {
            let make = &make;
            let spawned = std::thread::Builder::new().name("denoise-card".into()).stack_size(STACK).spawn_scoped(scope, move || make(side));
            if spawned.is_err() {
                // no thread: set it up here
                make(side);
            }
        }
    });
}

#[cfg(feature = "denoise")]
impl Network {
    /// Set every card up that was not tried yet, all at once.
    fn set_up(&self) {
        set_up_each(self.cards.iter().filter(|c| c.gpu.get().is_none()).collect(), |side| {
            let _ = self.card(side, true);
        });
    }

    /// `side`'s card when it can be used now, else why not. `start` sets it up when that has not been tried yet;
    /// without it nothing is started.
    fn card<'a>(&self, side: &'a CardSide, start: bool) -> Result<&'a GpuSide, String> {
        let got = if start {
            Some(side.gpu.get_or_init(|| {
                let made = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| make_gpu(&self.cpu, &self.path, self.tile, side.card)))
                    .unwrap_or_else(|_| Err("the GPU runner gave up".into()));
                match &made {
                    Ok(g) => log::info!("denoise: the {} card is {}", side.card.id(), g.adapter),
                    Err(why) => log::info!("denoise: not on the {} card ({why})", side.card.id()),
                }
                made
            }))
        } else {
            side.gpu.get()
        };
        let g = match got {
            None => return Err("the graphics card is not set up yet".into()),
            Some(Err(why)) => return Err(why.clone()),
            Some(Ok(g)) => g,
        };
        if let Some(why) = lcc_gpu::nn::broken_reason_on(side.card) {
            return Err(format!("the graphics card stopped working ({why})"));
        }
        if side.fallbacks.load(Ordering::Relaxed) >= GPU_MAX_FALLBACKS {
            return Err(format!("{} got {GPU_MAX_FALLBACKS} tiles wrong", g.adapter));
        }
        Ok(g)
    }

    /// `side`'s card, when a photo made `threads` tiles at a time under `run_on` runs on it; else why the CPU does.
    fn choose<'a>(&self, side: &'a CardSide, run_on: RunOn, threads: usize, start: bool) -> Result<&'a GpuSide, String> {
        if run_on == RunOn::Cpu {
            return Err("the processor is chosen in Settings".into());
        }
        let g = self.card(side, start)?;
        if run_on == RunOn::Auto && !card_is_faster(g.card_ms, g.cpu_ms, threads) {
            return Err(format!("it is faster than {} on this computer", g.adapter));
        }
        Ok(g)
    }

    /// The card for a photo starting now (none: the CPU): of the cards it may run on, the one with fewest photos, the
    /// second card first. It counts the photo until the lease ends.
    fn lease(&self, run_on: RunOn, threads: usize) -> Option<&CardSide> {
        let _one_at_a_time = self.picking.lock().unwrap_or_else(PoisonError::into_inner);
        let side = self.cards.iter().filter(|s| self.choose(s, run_on, threads, false).is_ok()).min_by_key(|s| s.photos.load(Ordering::Relaxed))?;
        side.photos.fetch_add(1, Ordering::Relaxed);
        Some(side)
    }

    fn fell_back(&self, side: &CardSide, why: &str) {
        let n = side.fallbacks.fetch_add(1, Ordering::Relaxed) + 1;
        if n == 1 {
            log::warn!("denoise: a tile came back wrong from the {} card ({why}); the CPU runs it", side.card.id());
        } else if n == GPU_MAX_FALLBACKS {
            log::warn!("denoise: the {} card got {n} tiles wrong; the CPU does the rest", side.card.id());
        }
    }

    /// Where a photo starting now would run on `side`, and the times measured there (for the status); `None` for a
    /// card that isn't there (no second card).
    fn describe_card(&self, side: &CardSide, run_on: RunOn, threads: usize) -> Option<Value> {
        let made = side.gpu.get()?;
        if matches!(made, Err(why) if why.starts_with(lcc_gpu::nn::NO_SECOND_CARD)) {
            return None;
        }
        let mut v = match self.choose(side, run_on, threads, false) {
            Ok(g) => json!({"card": side.card.id(), "kind": "gpu", "adapter": g.adapter}),
            // a set-up that crashed or ran out of time can be tried again (`denoise.settings {runOn}`)
            Err(why) => json!({"card": side.card.id(), "kind": "cpu", "retry": worth_retrying(&why), "reason": why}),
        };
        if let (Ok(g), Some(o)) = (made, v.as_object_mut()) {
            // measured on this computer, on the check tile: the card's best run, and one tile alone on the CPU
            o.insert("adapter".into(), json!(g.adapter));
            o.insert("cardMs".into(), json!(g.card_ms));
            o.insert("cpuMs".into(), json!(g.cpu_ms));
            o.insert("fellBack".into(), json!(side.fallbacks.load(Ordering::Relaxed)));
            o.insert("photos".into(), json!(side.photos.load(Ordering::Relaxed)));
        }
        Some(v)
    }
}

/// What runs one photo's tiles: a remote server's free slot first (when it shares), then the photo's card (when it
/// has one), with the CPU running any tile either gets wrong. Gives the card back when the photo is done.
#[cfg(feature = "denoise")]
struct Lease<'a> {
    net: &'a Network,
    side: Option<&'a CardSide>,
    remote: bool,
}

#[cfg(feature = "denoise")]
impl TileRunner for Lease<'_> {
    fn run(&self, input: &[f32]) -> Result<Vec<f32>, RunError> {
        let net = self.net;
        if let Some(Ok(remote)) = net.remote.as_ref().filter(|_| self.remote).map(|r| r.usable(false))
            && let Some(Ok(out)) = remote.try_run(input)
        {
            return Ok(out);
        }
        if let Some(side) = self.side
            && let Ok(g) = net.card(side, false)
        {
            match g.runner.run(input) {
                Ok(out) if out.iter().all(|v| v.is_finite()) => return Ok(out),
                Ok(_) => net.fell_back(side, "numbers that are not finite"),
                Err(e) => net.fell_back(side, &e.to_string()),
            }
        }
        net.cpu.run(input)
    }
}

#[cfg(feature = "denoise")]
impl Drop for Lease<'_> {
    fn drop(&mut self) {
        if let Some(side) = self.side {
            // each lease counted itself once (`Network::lease`)
            side.photos.fetch_sub(1, Ordering::Relaxed);
        }
    }
}

#[cfg(feature = "denoise")]
impl Model for Network {
    fn bytes_per_tile(&self) -> usize {
        self.cpu.bytes_per_tile()
    }
    fn runner(&self, run_on: RunOn, threads: usize) -> (Box<dyn TileRunner + '_>, usize) {
        let threads = threads.max(1);
        self.last_threads.store(threads, Ordering::Relaxed);
        if run_on != RunOn::Cpu {
            self.set_up();
        }
        let side = self.lease(run_on, threads);
        let local = if side.is_some() { threads.min(GPU_PARALLEL) } else { threads };
        let mut lease = Lease { net: self, side, remote: false };
        if let Some(r) = &self.remote {
            // this computer's time for a tile: the card's, or the CPU's with `threads` tiles sharing it (as
            // `card_is_faster` counts them); unknown when no card was set up (then the server is used)
            let set_up = self.cards.iter().find_map(|c| c.gpu.get().and_then(|g| g.as_ref().ok()));
            let local_ms = match (side.and_then(|s| s.gpu.get()), set_up) {
                (Some(Ok(g)), _) => g.card_ms,
                (_, Some(g)) => g.cpu_ms / (1.0 + 0.5 * threads.saturating_sub(1) as f64),
                _ => f64::INFINITY,
            };
            let share = match (r.usable(true), r.tile_ms()) {
                (Ok(_), Some(ms)) if ms < local_ms => {
                    *r.slower.lock().unwrap_or_else(PoisonError::into_inner) = None;
                    true
                }
                (Ok(_), Some(ms)) => {
                    *r.slower.lock().unwrap_or_else(PoisonError::into_inner) = Some((ms, local_ms));
                    false
                }
                _ => false,
            };
            r.share.store(share, Ordering::Relaxed);
            if let (true, Ok(remote)) = (share, r.usable(false)) {
                // the server's slots come on top of this computer's own tiles at once
                lease.remote = true;
                return (Box::new(lease), local + remote.slots());
            }
        }
        (Box::new(lease), local)
    }

    fn lanes(&self, run_on: RunOn) -> usize {
        let threads = match self.last_threads.load(Ordering::Relaxed) {
            0 => GPU_PARALLEL,
            n => n,
        };
        self.cards.iter().filter(|s| self.choose(s, run_on, threads, false).is_ok()).count().max(1)
    }

    fn self_test(&self, run_on: RunOn) -> Result<Value, String> {
        // its two tiles on the check tile's threads; without them, on this thread's pool
        let t = match check_pool() {
            Ok(pool) => pool.install(|| self.cpu.self_test()),
            Err(_) => self.cpu.self_test(),
        };
        if !t.ok {
            let failed: Vec<&str> = t.checks.iter().filter(|(_, ok)| !*ok).map(|(c, _)| c.as_str()).collect();
            return Err(format!("the model failed its self-test: it does not {}", failed.join(", and does not ")));
        }
        let mut v = serde_json::to_value(&t).map_err(|e| e.to_string())?;
        // the cards are set up and checked here, so the test says where the work will run
        if run_on != RunOn::Cpu {
            self.set_up();
        }
        if let Some(o) = v.as_object_mut() {
            o.insert("device".into(), self.device(run_on));
        }
        Ok(v)
    }

    fn device(&self, run_on: RunOn) -> Value {
        if run_on != RunOn::Cpu && self.cards.iter().any(|c| c.gpu.get().is_none()) {
            return json!({"kind": "pending"});
        }
        let threads = match self.last_threads.load(Ordering::Relaxed) {
            0 => Pace::Full.parallel(std::thread::available_parallelism().map_or(1, |n| n.get()), None),
            n => n,
        };
        let mut v = merge_cards(self.cards.iter().filter_map(|c| self.describe_card(c, run_on, threads)).collect());
        if let (Some(r), Some(o)) = (&self.remote, v.as_object_mut()) {
            o.insert("remote".into(), r.describe());
        }
        v
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// the built-in denoiser on the graphics card

/// The noise the check tile is made with and the built-in denoiser is told for it (variance = a · signal + b).
#[cfg(feature = "denoise")]
const CHECK_NOISE: (f32, f32) = (0.01, 0.001);
/// How far the card's filter may be from the CPU's on the check tile, in the noise's deviations: at most this at one place
/// (a few blocks whose match or threshold falls either side of a tie in the two devices' roundings change by up to about a
/// tenth; a broken kernel by far more), and this on average.
#[cfg(feature = "denoise")]
const BUILTIN_AGREE_WORST: f32 = 0.3;
#[cfg(feature = "denoise")]
const BUILTIN_AGREE_MEAN: f32 = 1e-3;

/// A mosaic tile (four planes of `side` cells): a ramp, edges and some texture with noise of [`CHECK_NOISE`], the same
/// every time.
#[cfg(feature = "denoise")]
fn builtin_check_tile(side: usize) -> Vec<f32> {
    let mut state = 0x2545_f491_4f6c_dd1du64;
    let mut uniform = move || {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((state >> 40) as f32 + 0.5) / (1u64 << 24) as f32
    };
    let mut out = Vec::with_capacity(4 * side * side);
    for plane in 0..4 {
        for i in 0..side * side {
            let (x, y) = (i % side, i / side);
            let step = if (x / 37 + y / 29) % 2 == 0 { 0.25 } else { 0.0 };
            let signal =
                (0.1 + 0.4 * x as f32 / side as f32 + 0.1 * y as f32 / side as f32 + step + 0.05 * ((x as f32 + 4.0 * plane as f32) * 0.7).sin())
                    .clamp(0.02, 0.95);
            let (u, v) = (uniform(), uniform());
            let noise = (-2.0 * u.ln()).sqrt() * (std::f32::consts::TAU * v).cos();
            out.push((signal + noise * (CHECK_NOISE.0 * signal + CHECK_NOISE.1).sqrt()).max(0.0));
        }
    }
    out
}

/// The card set up for the built-in denoiser: its kernels, and the check tile's time on it and on the CPU.
#[cfg(feature = "denoise")]
struct BuiltinGpu {
    gpu: Arc<lcc_gpu::bm3d::Bm3dGpu>,
    adapter: String,
    /// The check tile on the card, the fastest of a few runs (ms).
    card_ms: f64,
    /// The check tile on the CPU, on one thread (ms).
    cpu_ms: f64,
}

/// Set `on` up for the built-in denoiser: kernels built, the check tile run on both and compared, both timed. `marker`
/// is the card for editing's set-up marker (the second card's is beside it).
#[cfg(feature = "denoise")]
fn make_builtin_gpu(marker: &Path, on: lcc_gpu::nn::Card) -> Result<BuiltinGpu, String> {
    use lcc_denoise::builtin::{Builtin, TILE};
    card_exists(on)?;
    let tile = TILE as usize;
    let builtin = Builtin::with_noise(CHECK_NOISE);
    let params = builtin.bm3d_params().ok_or("the built-in denoiser does not use the block-matching filter")?;
    let input = Arc::new(builtin_check_tile(tile));
    let on_card = input.clone();
    let card = move || -> Result<_, String> {
        let gpu = Arc::new(lcc_gpu::bm3d::Bm3dGpu::new_on(on, &params, tile)?);
        let runner = lcc_gpu::bm3d::BuiltinRunner::new(gpu.clone(), builtin)?;
        let agreement = runner.agreement(&on_card).map_err(|e| format!("the GPU could not run the check tile: {e}"))?;
        let whole = runner.run(&on_card).map_err(|e| format!("the GPU could not run the check tile: {e}"))?;
        let card_ms = card_tile_ms(&runner, &on_card);
        Ok((gpu, agreement, whole, card_ms))
    };
    let on_cpu = || {
        let started = web_time::Instant::now();
        (builtin.run(&input), started.elapsed().as_secs_f64() * 1000.0)
    };
    let ((gpu, agreement, whole, card_ms), (want, cpu_ms)) = set_up_card(&marker_of(marker, on), GPU_SETUP_LIMIT, card, on_cpu)?;
    let want = want.map_err(|e| format!("the CPU could not check the GPU: {e}"))?;
    if whole.len() != want.len() || !whole.iter().all(|v| v.is_finite()) {
        return Err("its picture of the check tile is not the CPU's size or has numbers that are not finite: not used".into());
    }
    if !(agreement.worst <= BUILTIN_AGREE_WORST && agreement.mean <= BUILTIN_AGREE_MEAN) {
        return Err(format!(
            "its filter's answer on the check tile is not the CPU's (off by {:.2} at most and {:.1e} on average, in noise deviations): not used",
            agreement.worst, agreement.mean
        ));
    }
    let adapter = gpu.adapter();
    Ok(BuiltinGpu { gpu, adapter, card_ms, cpu_ms })
}

/// What the built-in denoiser knows about the graphics card in this process: set up for the first photo that may use it,
/// and used while it keeps working. Kept by the marker of its set-up (see [`builtin_card`]) so that every photo shares it.
#[cfg(feature = "denoise")]
struct BuiltinCard {
    marker: PathBuf,
    /// Each card's side, the second card first ([`lcc_gpu::nn::CARDS`]): a batch makes a photo on each at once.
    sides: [BuiltinSide; 2],
    /// Hands photos their card one at a time.
    picking: Mutex<()>,
    /// Tiles at once of the photo started last (0: none yet), to say where the work runs.
    last_threads: AtomicUsize,
}

/// One graphics card's part of a [`BuiltinCard`].
#[cfg(feature = "denoise")]
struct BuiltinSide {
    card: lcc_gpu::nn::Card,
    gpu: OnceLock<Result<BuiltinGpu, String>>,
    /// Tiles the card got wrong (an error, numbers that are not finite) and the CPU ran.
    fallbacks: AtomicUsize,
    /// Photos running on it now.
    photos: AtomicUsize,
}

#[cfg(feature = "denoise")]
impl BuiltinSide {
    fn new(card: lcc_gpu::nn::Card) -> BuiltinSide {
        BuiltinSide { card, gpu: OnceLock::new(), fallbacks: AtomicUsize::new(0), photos: AtomicUsize::new(0) }
    }
}

#[cfg(feature = "denoise")]
static BUILTIN_CARDS: Mutex<Vec<(PathBuf, Arc<BuiltinCard>)>> = Mutex::new(Vec::new());

/// The card's state for the set-up marker `marker`, made the first time it is asked for.
#[cfg(feature = "denoise")]
fn builtin_card(marker: &Path) -> Arc<BuiltinCard> {
    let mut all = BUILTIN_CARDS.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some((_, c)) = all.iter().find(|(m, _)| m == marker) {
        return c.clone();
    }
    let c = Arc::new(BuiltinCard {
        marker: marker.to_path_buf(),
        sides: lcc_gpu::nn::CARDS.map(BuiltinSide::new),
        picking: Mutex::new(()),
        last_threads: AtomicUsize::new(0),
    });
    all.push((marker.to_path_buf(), c.clone()));
    c
}

/// Forget the card's state for the marker (and the marker): the card is tried afresh for the next photo.
#[cfg(feature = "denoise")]
pub(crate) fn forget_builtin_card(marker: &Path) {
    BUILTIN_CARDS.lock().unwrap_or_else(PoisonError::into_inner).retain(|(m, _)| m != marker);
    let _ = std::fs::remove_file(marker);
    let _ = std::fs::remove_file(card_marker(marker, true));
}

/// Where the card's set-up is recorded for the built-in denoiser whose `onnx` is its model's (made-up) path in the models
/// folder: beside it, and in the temporary folder when there is no models folder.
pub(crate) fn builtin_marker(onnx: &Path) -> PathBuf {
    match onnx.parent() {
        Some(p) if !p.as_os_str().is_empty() => onnx.with_file_name(GPU_SETUP_MARKER),
        _ => std::env::temp_dir().join(format!("{}-builtin-{GPU_SETUP_MARKER}", lcc_brand::BINARY)),
    }
}

#[cfg(feature = "denoise")]
impl BuiltinCard {
    /// The side at `i` ([`lcc_gpu::nn::CARDS`]' order).
    fn side(&self, i: usize) -> Option<&BuiltinSide> {
        self.sides.get(i)
    }

    /// Set every card up that was not tried yet, all at once.
    fn set_up(&self) {
        set_up_each(self.sides.iter().filter(|s| s.gpu.get().is_none()).collect(), |side| {
            let _ = self.card(side, true);
        });
    }

    /// `side`'s card when it can be used now, else why not. `start` sets it up when that has not been tried yet;
    /// without it nothing is started.
    fn card<'a>(&self, side: &'a BuiltinSide, start: bool) -> Result<&'a BuiltinGpu, String> {
        let got = if start {
            Some(side.gpu.get_or_init(|| {
                let made = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| make_builtin_gpu(&self.marker, side.card)))
                    .unwrap_or_else(|_| Err("the GPU runner gave up".into()));
                match &made {
                    Ok(g) => log::info!("denoise: the built-in denoiser's {} card is {}", side.card.id(), g.adapter),
                    Err(why) => log::info!("denoise: the built-in denoiser is not on the {} card ({why})", side.card.id()),
                }
                made
            }))
        } else {
            side.gpu.get()
        };
        let g = match got {
            None => return Err("the graphics card is not set up yet".into()),
            Some(Err(why)) => return Err(why.clone()),
            Some(Ok(g)) => g,
        };
        if let Some(why) = lcc_gpu::nn::broken_reason_on(side.card) {
            return Err(format!("the graphics card stopped working ({why})"));
        }
        if side.fallbacks.load(Ordering::Relaxed) >= GPU_MAX_FALLBACKS {
            return Err(format!("{} got {GPU_MAX_FALLBACKS} tiles wrong", g.adapter));
        }
        Ok(g)
    }

    /// `side`'s card, when a photo made `threads` tiles at a time under `run_on` runs on it; else why the CPU does.
    fn choose<'a>(&self, side: &'a BuiltinSide, run_on: RunOn, threads: usize, start: bool) -> Result<&'a BuiltinGpu, String> {
        if run_on == RunOn::Cpu {
            return Err("the processor is chosen in Settings".into());
        }
        let g = self.card(side, start)?;
        if run_on == RunOn::Auto && !card_is_faster(g.card_ms, g.cpu_ms, threads) {
            return Err(format!("it is faster than {} on this computer", g.adapter));
        }
        Ok(g)
    }

    /// The card for a photo starting now (`None`: the CPU), by its index: of the cards it may run on, the one with
    /// fewest photos, the second card first. It counts the photo until [`Self::release`].
    fn lease(&self, run_on: RunOn, threads: usize) -> Option<usize> {
        let _one_at_a_time = self.picking.lock().unwrap_or_else(PoisonError::into_inner);
        let (i, side) = self
            .sides
            .iter()
            .enumerate()
            .filter(|(_, s)| self.choose(s, run_on, threads, false).is_ok())
            .min_by_key(|(_, s)| s.photos.load(Ordering::Relaxed))?;
        side.photos.fetch_add(1, Ordering::Relaxed);
        Some(i)
    }

    /// The photo leased card `i` is done.
    fn release(&self, i: usize) {
        if let Some(s) = self.side(i) {
            s.photos.fetch_sub(1, Ordering::Relaxed);
        }
    }

    /// Photos that may be made at once (see [`Model::lanes`]).
    fn lanes(&self, run_on: RunOn) -> usize {
        let threads = match self.last_threads.load(Ordering::Relaxed) {
            0 => GPU_PARALLEL,
            n => n,
        };
        self.sides.iter().filter(|s| self.choose(s, run_on, threads, false).is_ok()).count().max(1)
    }

    fn fell_back(&self, side: &BuiltinSide, why: &str) {
        let n = side.fallbacks.fetch_add(1, Ordering::Relaxed) + 1;
        if n == 1 {
            log::warn!("denoise: a tile came back wrong from the {} card ({why}); the CPU runs it", side.card.id());
        } else if n == GPU_MAX_FALLBACKS {
            log::warn!("denoise: the {} card got {n} tiles wrong; the CPU does the rest", side.card.id());
        }
    }

    /// Where a photo starting now would run on `side` (`None`: no such card).
    fn describe_card(&self, side: &BuiltinSide, run_on: RunOn, threads: usize) -> Option<Value> {
        let made = side.gpu.get()?;
        if matches!(made, Err(why) if why.starts_with(lcc_gpu::nn::NO_SECOND_CARD)) {
            return None;
        }
        let mut v = match self.choose(side, run_on, threads, false) {
            Ok(g) => json!({"card": side.card.id(), "kind": "gpu", "adapter": g.adapter}),
            // a set-up that crashed or ran out of time can be tried again (`denoise.settings {runOn}`)
            Err(why) => json!({"card": side.card.id(), "kind": "cpu", "retry": worth_retrying(&why), "reason": why}),
        };
        if let (Ok(g), Some(o)) = (made, v.as_object_mut()) {
            o.insert("adapter".into(), json!(g.adapter));
            o.insert("cardMs".into(), json!(g.card_ms));
            o.insert("cpuMs".into(), json!(g.cpu_ms));
            o.insert("fellBack".into(), json!(side.fallbacks.load(Ordering::Relaxed)));
            o.insert("photos".into(), json!(side.photos.load(Ordering::Relaxed)));
        }
        Some(v)
    }

    /// Where the tiles run (see [`Model::device`]).
    fn device(&self, run_on: RunOn) -> Value {
        if run_on != RunOn::Cpu && self.sides.iter().any(|s| s.gpu.get().is_none()) {
            return json!({"kind": "pending"});
        }
        let threads = match self.last_threads.load(Ordering::Relaxed) {
            0 => Pace::Full.parallel(std::thread::available_parallelism().map_or(1, |n| n.get()), None),
            n => n,
        };
        merge_cards(self.sides.iter().filter_map(|s| self.describe_card(s, run_on, threads)).collect())
    }
}

/// The built-in denoiser for a photo that runs on a card: the filter there, and the CPU running any tile the card gets
/// wrong, so a photo is always finished. Gives the card back when the photo is done.
#[cfg(feature = "denoise")]
struct CardRunner {
    gpu: lcc_gpu::bm3d::BuiltinRunner,
    cpu: lcc_denoise::builtin::Builtin,
    card: Arc<BuiltinCard>,
    /// The leased side ([`BuiltinCard::lease`]).
    side: usize,
}

#[cfg(feature = "denoise")]
impl TileRunner for CardRunner {
    fn run(&self, input: &[f32]) -> Result<Vec<f32>, RunError> {
        if let Some(side) = self.card.side(self.side)
            && self.card.card(side, false).is_ok()
        {
            match self.gpu.run(input) {
                Ok(out) if out.iter().all(|v| v.is_finite()) => return Ok(out),
                Ok(_) => self.card.fell_back(side, "numbers that are not finite"),
                Err(e) => self.card.fell_back(side, &e.to_string()),
            }
        }
        self.cpu.run(input)
    }
}

#[cfg(feature = "denoise")]
impl Drop for CardRunner {
    fn drop(&mut self) {
        self.card.release(self.side);
    }
}

/// The loader of this build: our pure-Rust executor with the `denoise` feature, else one that says why nothing can run.
pub(crate) fn default_loader() -> Loader {
    #[cfg(feature = "denoise")]
    {
        Arc::new(|path, manifest| {
            // the server must run this very file: identified by the manifest's hash, else the file's
            let sha256 = match &manifest.sha256 {
                Some(h) => Some(h.clone()),
                None if lcc_brand::env_os("DENOISE_URL").is_some() => lcc_denoise::hash::sha256_file(path).ok(),
                None => None,
            };
            let remote = sha256.and_then(|h| RemoteSide::from_env(&h, manifest.tile as usize));
            Ok(Arc::new(network(path, manifest, remote)?) as Arc<dyn Model>)
        })
    }
    #[cfg(not(feature = "denoise"))]
    {
        Arc::new(|_, _| Err("this build cannot run denoise models".to_string()))
    }
}

/// The model at `path` on this computer, sharing tiles with `remote` when there is one.
#[cfg(feature = "denoise")]
fn network(path: &Path, manifest: &DenoiserManifest, remote: Option<RemoteSide>) -> Result<Network, String> {
    let cpu = lcc_denoise::runtime::CpuRunner::load(path, manifest).map_err(|e| e.to_string())?;
    Ok(Network {
        remote,
        cpu,
        path: path.to_path_buf(),
        tile: manifest.tile as usize,
        cards: lcc_gpu::nn::CARDS.map(CardSide::new),
        picking: Mutex::new(()),
        last_threads: AtomicUsize::new(0),
    })
}

/// A model ready to be served to other computers (`lcc-cli denoise-serve`).
#[cfg(feature = "denoise")]
pub struct Serving {
    pub model: lcc_denoise::remote::ModelId,
    pub tile: usize,
    /// Where its tiles run here: the graphics card's name, or why the CPU does.
    pub device: String,
    pub run: lcc_denoise::remote::RunTile,
}

/// The installed model `id` (default: the one selected in Settings) in the models folder `dir` (default: this user's),
/// set up to run on `run_on` as the app would: the card checked against the CPU first, the CPU when the card can't.
#[cfg(feature = "denoise")]
pub fn serving(dir: Option<&Path>, id: Option<&str>, run_on: RunOn) -> Result<Serving, String> {
    let dir = match dir {
        Some(d) => d.to_path_buf(),
        None => crate::config::default_denoise_models_dir()
            .ok_or_else(|| format!("no denoise models folder (set {})", lcc_brand::env_var("DENOISE_MODELS")))?,
    };
    let want = match id {
        Some(i) => i.to_string(),
        None => read_settings(&dir).model.ok_or_else(|| {
            format!("no denoise model is selected in {} (install one in Settings ▸ AI Denoise, or name it with --model)", dir.display())
        })?,
    };
    let installed = installed_models(&dir)
        .into_iter()
        .find(|m| m.manifest.id == want)
        .ok_or_else(|| format!("no installed denoise model `{want}` in {}", dir.display()))?;
    let m = &installed.manifest;
    // the same identity a client derives (see `default_loader`)
    let sha256 = match &m.sha256 {
        Some(h) => h.clone(),
        None => lcc_denoise::hash::sha256_file(&installed.onnx).map_err(|e| format!("{}: {e}", installed.onnx.display()))?,
    };
    let net = Arc::new(network(&installed.onnx, m, None)?);
    let tile = m.tile as usize;
    // set the card up now (and say where the tiles run) rather than with the first client's tile
    let _ = net.runner(run_on, GPU_PARALLEL);
    let d = net.device(run_on);
    let device = match d["kind"].as_str() {
        Some("gpu") => format!("GPU: {}", d["adapter"].as_str().unwrap_or("?")),
        _ => format!("CPU ({})", d["reason"].as_str().unwrap_or("chosen")),
    };
    let run: lcc_denoise::remote::RunTile = Arc::new(move |input: &[f32]| net.runner(run_on, GPU_PARALLEL).0.run(input));
    Ok(Serving { model: lcc_denoise::remote::ModelId { id: m.id.clone(), version: m.version.clone(), sha256 }, tile, device, run })
}

/// A model that is loaded on first use and kept, shared by the background job and the exports.
pub(crate) type ModelSlot = Arc<Mutex<Option<Arc<dyn Model>>>>;

// ---------------------------------------------------------------------------------------------------------------------
// settings and installed models

/// `settings.json` in the models folder.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct Settings {
    /// The installed model in use for raw photos; `None`: the built-in denoiser.
    pub model: Option<String>,
    /// The installed model in use for finished pictures (JPEG, PNG, TIFF …); `None`: denoise is off for them.
    pub rgb_model: Option<String>,
    /// The installed upscaler AI Super Resolution and AI Sharpen use (`Domain::RgbToRgb4x`); `None`: none is chosen.
    pub upscale_model: Option<String>,
    /// Photos with a Denoise amount above 0 that are being looked at (active, selected) get their picture made without
    /// being asked (`None`: yes).
    pub auto: Option<bool>,
    /// Cache size limit (GB; `None`: [`DEFAULT_CACHE_GB`]).
    pub cache_gb: Option<u32>,
    /// Most tiles run at once (`None`: by how hard the pump says to work).
    pub threads: Option<u32>,
    /// Where the model runs: `auto`, `gpu` or `cpu` (`None` or anything else: `auto`; see [`RunOn`]).
    pub run_on: Option<String>,
    /// What came before `runOn`: `false` was the processor only.
    pub gpu: Option<bool>,
}

impl Settings {
    pub fn auto(&self) -> bool {
        self.auto.unwrap_or(true)
    }

    pub fn run_on(&self) -> RunOn {
        match self.run_on.as_deref().and_then(RunOn::parse) {
            Some(r) => r,
            None if self.gpu == Some(false) => RunOn::Cpu,
            None => RunOn::Auto,
        }
    }

    /// The setting that names the model in use for photos of `domain`.
    pub fn slot(&self, domain: Domain) -> Option<&str> {
        match domain {
            Domain::BayerToRgb => self.model.as_deref(),
            Domain::RgbToRgb => self.rgb_model.as_deref(),
            Domain::RgbToRgb4x => self.upscale_model.as_deref(),
        }
    }

    pub fn slot_mut(&mut self, domain: Domain) -> &mut Option<String> {
        match domain {
            Domain::BayerToRgb => &mut self.model,
            Domain::RgbToRgb => &mut self.rgb_model,
            Domain::RgbToRgb4x => &mut self.upscale_model,
        }
    }

    /// Is `id` the model in use for one kind of photo?
    pub fn chosen(&self, id: &str) -> bool {
        [&self.model, &self.rgb_model, &self.upscale_model].iter().any(|m| m.as_deref() == Some(id))
    }

    pub fn cache_bytes(&self) -> u64 {
        u64::from(self.cache_gb.unwrap_or(DEFAULT_CACHE_GB).clamp(1, 10_000)) << 30
    }
}

pub(crate) fn read_capped(path: &Path, max: usize) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut buf = Vec::new();
    std::fs::File::open(path).ok()?.take(max as u64 + 1).read_to_end(&mut buf).ok()?;
    (buf.len() <= max).then_some(buf)
}

pub(crate) fn read_settings(dir: &Path) -> Settings {
    read_capped(&dir.join("settings.json"), 4096).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// Write a file next to its final name and rename it into place.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let part = path.with_extension("part");
    std::fs::write(&part, bytes).map_err(|e| format!("could not write {}: {e}", path.display()))?;
    std::fs::rename(&part, path).map_err(|e| {
        let _ = std::fs::remove_file(&part);
        format!("could not write {}: {e}", path.display())
    })
}

pub(crate) fn write_settings(dir: &Path, st: &Settings) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("could not create the denoise models folder: {e}"))?;
    write_atomic(&dir.join("settings.json"), &serde_json::to_vec_pretty(st).map_err(|e| e.to_string())?)
}

/// An installed model folder: `<id>/model.onnx`, `denoise-model.json`, `installed.json`.
#[derive(Clone)]
pub(crate) struct Installed {
    pub manifest: DenoiserManifest,
    pub onnx: PathBuf,
    /// What was recorded when it was installed (terms accepted, hash of the file, self-test).
    pub accepted: Value,
}

impl Installed {
    /// Names the model file the cache is made with: a hash of the file as installed, so another file with the same id
    /// never finds this one's products.
    pub fn fingerprint(&self) -> String {
        let sha = self.accepted.get("sha256").and_then(Value::as_str).or(self.manifest.sha256.as_deref()).unwrap_or("");
        let len = std::fs::metadata(&self.onnx).map(|m| m.len()).unwrap_or(0);
        format!("{}@{}:{len}", self.manifest.id, sha)
    }
}

/// The models in the folder. A folder that is not a valid model (wrong name, no model file, a manifest that does not
/// validate or does not match its folder) is ignored, never trusted.
pub(crate) fn installed_models(dir: &Path) -> Vec<Installed> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out = Vec::new();
    for e in entries.flatten() {
        let p = e.path();
        let Some(name) = p.file_name().and_then(|n| n.to_str()).map(str::to_string) else { continue };
        let onnx = p.join("model.onnx");
        if name == lcc_denoise::builtin::ID || !lcc_denoise::licence::valid_id(&name) || !p.is_dir() || !onnx.is_file() {
            continue;
        }
        let Some(m) =
            read_capped(&p.join("denoise-model.json"), lcc_denoise::manifest::MAX_MANIFEST_BYTES).and_then(|b| lcc_denoise::manifest::parse(&b).ok())
        else {
            continue;
        };
        if m.id != name {
            continue;
        }
        let accepted = read_capped(&p.join("installed.json"), 8192).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(Value::Null);
        out.push(Installed { manifest: m, onnx, accepted });
    }
    out.sort_by(|a, b| a.manifest.id.cmp(&b.manifest.id));
    out
}

/// Forget that setting the graphics card up for a model closed the app before (the markers [`set_up_card`] leaves):
/// the user chose where denoise runs, so the card may be tried again.
pub(crate) fn forget_gpu_set_ups(dir: &Path) {
    for i in installed_models(dir) {
        let _ = std::fs::remove_file(i.onnx.with_file_name(GPU_SETUP_MARKER));
        let _ = std::fs::remove_file(card_marker(&i.onnx.with_file_name(GPU_SETUP_MARKER), true));
    }
    // the built-in denoiser's, which is recorded in the models folder itself
    #[cfg(feature = "denoise")]
    forget_builtin_card(&builtin_marker(&dir.join(lcc_denoise::builtin::ID)));
}

/// The model in use.
pub(crate) struct Active {
    pub id: String,
    pub manifest: DenoiserManifest,
    pub onnx: PathBuf,
    pub fingerprint: String,
    pub model: ModelSlot,
}

// ---------------------------------------------------------------------------------------------------------------------
// products

/// Which kind of model photo `p` takes: a raw developed from its sensor data takes a Bayer model, a finished picture
/// (JPEG, PNG, TIFF …) a model for finished pictures; anything else (a video, a raw shown from its embedded preview)
/// none.
pub(crate) fn domain_of(p: &Photo) -> Option<Domain> {
    if p.develops_raw() {
        return Some(Domain::BayerToRgb);
    }
    (p.kind == lcc_catalog::MediaKind::Image).then_some(Domain::RgbToRgb)
}

/// The photos whose files can be denoised at all: raw files developed from their sensor data and finished pictures,
/// read from disk.
pub(crate) fn eligible(p: &Photo) -> bool {
    domain_of(p).is_some() && matches!(p.source, Source::File { .. }) && !p.deleted
}

/// A product's name: what it was made from and with.
pub(crate) fn product_key(fingerprint: &str, p: &Photo) -> String {
    let mut h = Hasher128::new();
    h.str("lcdn").u64(u64::from(product::ALGORITHM)).str(fingerprint).str(&crate::media::content_key(p));
    h.finish().to_string()
}

pub(crate) fn product_path(dir: &Path, key: &str) -> PathBuf {
    dir.join(format!("{key}.{EXT}"))
}

/// The `.lcdn` files in `dir`: path, size, last change.
fn products_in(dir: &Path) -> Vec<(PathBuf, u64, SystemTime)> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == EXT))
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            m.is_file().then(|| (e.path(), m.len(), m.modified().unwrap_or(SystemTime::UNIX_EPOCH)))
        })
        .collect()
}

/// Make sure only one thread writes a product at a time: a second one waits, then finds it made.
fn making() -> &'static (Mutex<HashSet<PathBuf>>, Condvar) {
    static MAKING: OnceLock<(Mutex<HashSet<PathBuf>>, Condvar)> = OnceLock::new();
    MAKING.get_or_init(|| (Mutex::new(HashSet::new()), Condvar::new()))
}

struct Slot(PathBuf);

impl Slot {
    fn enter(path: &Path) -> Slot {
        let (set, cv) = making();
        let mut g = set.lock().unwrap_or_else(PoisonError::into_inner);
        while g.contains(path) {
            g = cv.wait(g).unwrap_or_else(PoisonError::into_inner);
        }
        g.insert(path.to_path_buf());
        Slot(path.to_path_buf())
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        let (set, cv) = making();
        set.lock().unwrap_or_else(PoisonError::into_inner).remove(&self.0);
        cv.notify_all();
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// making a product

/// Everything a thread needs to make one product, detached from the session.
#[derive(Clone)]
pub(crate) struct JobSpec {
    /// The original file.
    pub path: String,
    pub read: Option<ByteReader>,
    pub product: PathBuf,
    pub key: String,
    pub manifest: DenoiserManifest,
    pub onnx: PathBuf,
    pub model: ModelSlot,
    pub loader: Loader,
    /// Tiles run at once.
    pub parallel: usize,
    /// Where the model runs (the setting).
    pub run_on: RunOn,
}

/// Why a product was not made.
#[derive(Debug, PartialEq)]
pub(crate) enum MakeError {
    /// Denoise does not apply to this file (not a Bayer raw): nothing is wrong.
    Unsupported(String),
    Failed(String),
    Cancelled,
}

impl From<RunError> for MakeError {
    fn from(e: RunError) -> Self {
        match e {
            RunError::Cancelled => MakeError::Cancelled,
            other => MakeError::Failed(other.to_string()),
        }
    }
}

/// Progress and cancel of a running job.
#[derive(Default)]
pub(crate) struct Progress {
    done: AtomicUsize,
    total: AtomicUsize,
    cancel: AtomicBool,
}

impl Progress {
    /// Ask the job to stop at the next tile.
    pub(crate) fn cancel_now(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// Has the job been asked to stop?
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

/// The loaded model, loading it first when it is not (kept for the next photo).
fn loaded(spec: &JobSpec) -> Result<Arc<dyn Model>, MakeError> {
    if spec.manifest.id == lcc_denoise::builtin::ID {
        return Ok(Arc::new(BuiltinModel::new(&builtin_marker(&spec.onnx))));
    }
    if let Some(m) = spec.model.lock().unwrap_or_else(PoisonError::into_inner).clone() {
        return Ok(m);
    }
    let m = (spec.loader)(&spec.onnx, &spec.manifest).map_err(|e| MakeError::Failed(format!("the denoise model could not be loaded: {e}")))?;
    *spec.model.lock().unwrap_or_else(PoisonError::into_inner) = Some(m.clone());
    Ok(m)
}

/// The cell `[(0,0), (1,0), (0,1), (1,1)]` colours of a Bayer mosaic and so its layout.
fn layout_of(cfa: &lcc_raw::Cfa) -> Option<Layout> {
    Layout::from_cell([cfa.color_at(0, 0), cfa.color_at(1, 0), cfa.color_at(0, 1), cfa.color_at(1, 1)])
}

/// Make the product of `spec` unless it is there: decode the photo, run the model over its mosaic, write the result.
/// `Ok(true)` when it was made now, `Ok(false)` when it already existed.
pub(crate) fn make_product(spec: &JobSpec, progress: Option<&Progress>) -> Result<bool, MakeError> {
    let _slot = Slot::enter(&spec.product);
    if product::is_current(&spec.product, &spec.key) {
        return Ok(false);
    }
    let began = web_time::Instant::now();
    let cancelled = || progress.is_some_and(|p| p.cancel.load(Ordering::Relaxed));
    if cancelled() {
        return Err(MakeError::Cancelled);
    }
    let bytes = match &spec.read {
        Some(r) => r(&spec.path).map_err(MakeError::Failed)?,
        None => std::fs::read(&spec.path).map_err(|e| MakeError::Failed(format!("{}: {e}", spec.path)))?,
    };
    if spec.manifest.domain == Domain::RgbToRgb {
        return make_rgb_product(spec, bytes, progress, began);
    }
    // the decode and the normalised mosaic wait their turn at the process-wide memory gate, as every decode does
    let (mosaic, layout) = {
        let _held = crate::memory::work_gate().acquire(bytes.len().saturating_mul(4));
        if lcc_raw::probe(&bytes).is_none() {
            return Err(MakeError::Unsupported("not a raw file".into()));
        }
        let mut raw = match lcc_raw::decode(&bytes) {
            Ok(r) => r,
            Err(lcc_raw::RawError::Unsupported(why)) => return Err(MakeError::Unsupported(why)),
            Err(e) => return Err(MakeError::Failed(e.to_string())),
        };
        drop(bytes);
        // hot, dead and stuck pixels go before the denoiser sees them, as in the plain development
        crate::files::repair_pixels(&mut raw, &crate::files::RawOptions::for_path(&spec.path, true));
        if raw.cpp != 1 || !raw.cfa.as_ref().is_some_and(lcc_raw::Cfa::is_bayer) {
            return Err(MakeError::Unsupported(
                "only Bayer raw files can be denoised (this one has another sensor layout or is already demosaiced)".into(),
            ));
        }
        let n = raw.normalized().map_err(|e| MakeError::Failed(e.to_string()))?;
        drop(raw);
        let layout =
            n.cfa.as_ref().and_then(layout_of).ok_or_else(|| MakeError::Unsupported("the colour filter layout is not a Bayer pattern".into()))?;
        (n, layout)
    };
    if cancelled() {
        return Err(MakeError::Cancelled);
    }
    let decoded = began.elapsed();
    let model = loaded(spec)?;
    model.prepare(&mosaic.data, mosaic.width, mosaic.height);
    // Packed input, returned RGB and the preceding group's RGB overlap while tiles run.
    let scratch =
        model.bytes_per_tile().saturating_add(128usize.saturating_mul(spec.manifest.tile as usize).saturating_mul(spec.manifest.tile as usize));
    let (_, budget) = crate::memory::work_gate().usage();
    let parallel = spec.parallel.clamp(1, 4).min((budget / scratch.max(1)).max(1));
    let _held = crate::memory::work_gate().acquire(mosaic.data.len().saturating_mul(20).saturating_add(scratch.saturating_mul(parallel)));
    let (runner, parallel) = model.runner(spec.run_on, parallel);
    let runner = &*runner;
    let params = Params {
        tile: spec.manifest.tile as usize,
        overlap: spec.manifest.overlap as usize,
        gain: spec.manifest.gain,
        clip: Some(CLIP),
        parallel: parallel.max(1),
    };
    let report = |done: usize, total: usize| {
        if let Some(p) = progress {
            p.total.store(total, Ordering::Relaxed);
            p.done.store(done, Ordering::Relaxed);
        }
    };
    let ctl = Control { cancel: progress.map(|p| &p.cancel), progress: Some(&report) };
    // the tiles run on threads of their own, so a busy window's renders keep theirs
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(params.parallel)
        .stack_size(STACK)
        .thread_name(|i| format!("denoise-{i}"))
        .build()
        .map_err(|e| MakeError::Failed(format!("could not start the worker threads: {e}")))?;
    let rgb = pool.install(|| denoise_bayer(&mosaic.data, mosaic.width, mosaic.height, layout, runner, &params, &ctl))?;
    drop(mosaic);
    if cancelled() {
        return Err(MakeError::Cancelled);
    }
    let ran = began.elapsed();
    product::write(&spec.product, &rgb, &spec.key).map_err(|e| MakeError::Failed(format!("could not save the denoised picture: {e}")))?;
    if lcc_brand::env_os("PROFILE").is_some() {
        eprintln!(
            "[denoise] read + decode {:.0} ms, model (set-up, tiles, blend) {:.0} ms, write {:.0} ms",
            decoded.as_secs_f64() * 1e3,
            (ran - decoded).as_secs_f64() * 1e3,
            (began.elapsed() - ran).as_secs_f64() * 1e3
        );
    }
    Ok(true)
}

/// A finished picture as the model for finished pictures takes it: decoded at full size, unrotated (the product is the
/// file's own pixels, as a raw's is its mosaic's), transparency flattened onto white as the plain picture's is
/// ([`lcc_codecs::Decoded::to_working`]), in gamma-encoded sRGB clipped to 0 to 1 (colours outside sRGB and values above
/// white are clipped: the denoised picture of a wide-gamut or HDR file is the sRGB part of it).
pub(crate) fn srgb_encoded(d: &lcc_codecs::Decoded) -> lcc_raster::Rgb32f {
    let m = d.space.to_space(&lcc_color::SRGB).to_f32();
    let enc = lcc_color::transfer::linear_to_srgb;
    let mut image = d.image.clone();
    if let Some(a) = d.alpha.as_ref().filter(|a| a.width == image.width && a.height == image.height) {
        // white is (1, 1, 1) in every space here, so it is flattened before the conversion as after it
        for (p, &av) in image.data.iter_mut().zip(&a.data) {
            let av = av.clamp(0.0, 1.0);
            *p = p.map(|c| c * av + (1.0 - av));
        }
    }
    image.map(|p| {
        let q = [
            m[0][0] * p[0] + m[0][1] * p[1] + m[0][2] * p[2],
            m[1][0] * p[0] + m[1][1] * p[1] + m[1][2] * p[2],
            m[2][0] * p[0] + m[2][1] * p[1] + m[2][2] * p[2],
        ];
        q.map(|v| enc(v.clamp(0.0, 1.0)))
    })
}

/// [`make_product`] for a finished picture: decode it whole, run the model over it, write what it gives.
fn make_rgb_product(spec: &JobSpec, bytes: Vec<u8>, progress: Option<&Progress>, began: web_time::Instant) -> Result<bool, MakeError> {
    let cancelled = || progress.is_some_and(|p| p.cancel.load(Ordering::Relaxed));
    let image = {
        let _held = crate::memory::work_gate().acquire(bytes.len().saturating_mul(16));
        if lcc_raw::probe(&bytes).is_some() {
            return Err(MakeError::Unsupported("a raw file takes a model for raw files".into()));
        }
        let d = lcc_codecs::decode(&bytes, lcc_codecs::DecodeOptions::default())
            .map_err(|e| MakeError::Unsupported(format!("not a picture this model can read: {e}")))?;
        drop(bytes);
        // the decode and its sRGB copy (12 bytes a pixel each), counted once their size is known; it does not wait (a
        // second wait on the gate while this job holds a permit could wait for itself)
        let _pixels = crate::memory::work_gate().acquire_urgent(d.image.data.len().saturating_mul(24));
        srgb_encoded(&d)
    };
    if cancelled() {
        return Err(MakeError::Cancelled);
    }
    let model = loaded(spec)?;
    let t = spec.manifest.tile as usize;
    let scratch = model.bytes_per_tile().saturating_add(32usize.saturating_mul(t).saturating_mul(t));
    let (_, budget) = crate::memory::work_gate().usage();
    let parallel = spec.parallel.clamp(1, 4).min((budget / scratch.max(1)).max(1));
    // the picture, the blended result and its weights wait their turn (32 bytes a pixel); the colour restore's two
    // blurred copies and their working buffers (48 more at the peak) are counted without waiting, so a big picture does
    // not hold the whole gate (and the grid's thumbnails) for its whole run
    let _held = crate::memory::work_gate().acquire(image.data.len().saturating_mul(32).saturating_add(scratch.saturating_mul(parallel)));
    let _restore = crate::memory::work_gate().acquire_urgent(image.data.len().saturating_mul(48));
    let (runner, parallel) = model.runner(spec.run_on, parallel);
    let params = Params { tile: t, overlap: spec.manifest.overlap as usize, gain: spec.manifest.gain, clip: None, parallel: parallel.max(1) };
    let report = |done: usize, total: usize| {
        if let Some(p) = progress {
            p.total.store(total, Ordering::Relaxed);
            p.done.store(done, Ordering::Relaxed);
        }
    };
    let ctl = Control { cancel: progress.map(|p| &p.cancel), progress: Some(&report) };
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(params.parallel)
        .stack_size(STACK)
        .thread_name(|i| format!("denoise-{i}"))
        .build()
        .map_err(|e| MakeError::Failed(format!("could not start the worker threads: {e}")))?;
    let rgb = pool.install(|| lcc_denoise::run::denoise_rgb(&image, &*runner, &params, &ctl))?;
    drop(image);
    if cancelled() {
        return Err(MakeError::Cancelled);
    }
    product::write(&spec.product, &rgb, &spec.key).map_err(|e| MakeError::Failed(format!("could not save the denoised picture: {e}")))?;
    if lcc_brand::env_os("PROFILE").is_some() {
        eprintln!("[denoise] finished picture {}×{}: {:.0} ms", rgb.width, rgb.height, began.elapsed().as_secs_f64() * 1e3);
    }
    Ok(true)
}

// ---------------------------------------------------------------------------------------------------------------------
// the session's state

/// How hard the background work may go (the app picks it from what the user is doing, like the face scan).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pace {
    /// Start nothing new.
    Pause,
    /// Two tiles at once.
    Light,
    /// Half the machine.
    Normal,
    /// Most of the machine: the user is watching the progress.
    Full,
}

impl Pace {
    /// `pause`, `light`, `normal` or `full`; anything else is `normal`.
    pub fn parse(name: Option<&str>) -> Pace {
        match name {
            Some("pause") => Pace::Pause,
            Some("light") => Pace::Light,
            Some("full") => Pace::Full,
            _ => Pace::Normal,
        }
    }

    /// Tiles to run at once on a machine with `cores` threads (`cap`: the user's own limit).
    pub fn parallel(self, cores: usize, cap: Option<u32>) -> usize {
        let n = match self {
            Pace::Pause | Pace::Light => 2,
            Pace::Normal => cores / 2,
            Pace::Full => cores * 4 / 5,
        }
        .clamp(1, 16);
        cap.map_or(n, |c| n.min(c.max(1) as usize))
    }
}

/// How a photo stands, for the interface.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum PhotoState {
    /// Not a photo denoise applies to (not a raw, offline, a demo).
    NotApplicable,
    /// Its picture is made: the Amount slider works.
    Ready,
    Queued {
        /// Photos ahead of it (0 = next).
        ahead: usize,
    },
    Running {
        done: usize,
        total: usize,
    },
    /// Making it failed (`why`), or denoise does not apply to this file (`unsupported`).
    Failed {
        why: String,
        unsupported: bool,
    },
    /// Nothing has asked for it yet.
    Idle,
}

#[derive(Clone)]
struct Failure {
    key: String,
    why: String,
    unsupported: bool,
}

enum Outcome {
    Done,
    Failed(String, bool),
    Cancelled,
}

struct Running {
    photo: PhotoId,
    key: String,
    product: PathBuf,
    progress: Arc<Progress>,
    outcome: Arc<Mutex<Option<Outcome>>>,
    started: web_time::Instant,
    /// Its row under the batch row (its tiles).
    task: Option<crate::activity::TaskGuard>,
}

/// Most photos made at once: one per graphics card (the second card and the card for editing).
const MAX_LANES: usize = 2;

pub(crate) struct State {
    /// Where the models are kept (`None` where there is no file system).
    pub models_dir: Option<PathBuf>,
    /// Models being downloaded at the user's request.
    pub downloads: Downloads,
    pub(crate) install: Option<crate::cmd::denoise::InstallJob>,
    /// How a model is loaded (tests put another in).
    pub loader: Loader,
    pub(crate) active: Option<Active>,
    /// The model in use for finished pictures (`Settings::rgb_model`); `None`: they are not denoised.
    pub(crate) active_rgb: Option<Active>,
    active_seen: Option<web_time::Instant>,
    /// `settings.json` as last read (with the model choice).
    pub(crate) settings: Settings,
    /// The index must be made again (the library, the model or the cache changed).
    dirty: bool,
    /// The catalog revision and time of the last index, to look again when photos changed.
    reindexed: Option<(u64, web_time::Instant)>,
    queue: VecDeque<PhotoId>,
    /// The photos being made, oldest first: one per graphics card that works ([`Model::lanes`]).
    running: Vec<Running>,
    failed: HashMap<PhotoId, Failure>,
    /// Counts every change of what is ready, so the interface knows to draw again.
    pub generation: u64,
    /// Photos made since the session started.
    made: u64,
    /// AI Denoise's batch row in the activity stack, while a photo is being made or waits its
    /// turn: the photos done and failed since the queue was last idle, and the time left.
    task: Option<crate::activity::TaskGuard>,
    /// Photos whose batch was cancelled (the row's cancel): looking at them doesn't queue them again; asking for them
    /// does (the switch, `denoise.queue`, an export).
    held: HashSet<PhotoId>,
    /// Photos an import switched AI Denoise on for that didn't fit in the queue: queued as it
    /// drains (a big import of high-ISO raws doesn't lose any).
    backlog: VecDeque<PhotoId>,
    /// How many photos the queue holds (tests make it small).
    pub(crate) queue_cap: usize,
}

impl Default for State {
    fn default() -> Self {
        State {
            models_dir: None,
            downloads: Downloads::default(),
            install: None,
            loader: default_loader(),
            active: None,
            active_rgb: None,
            active_seen: None,
            settings: Settings::default(),
            dirty: true,
            reindexed: None,
            queue: VecDeque::new(),
            running: Vec::new(),
            failed: HashMap::new(),
            generation: 0,
            made: 0,
            task: None,
            held: HashSet::new(),
            backlog: VecDeque::new(),
            queue_cap: MAX_QUEUE,
        }
    }
}

impl Drop for State {
    /// Quitting stops the jobs instead of leaving them to finish photos nobody will see.
    fn drop(&mut self) {
        for r in &self.running {
            r.progress.cancel_now();
        }
    }
}

impl State {
    /// The library was replaced: nothing carries over (photo ids are per library).
    pub fn library_changed(&mut self) {
        for r in self.running.drain(..) {
            r.progress.cancel_now();
        }
        self.queue.clear();
        self.backlog.clear();
        self.failed.clear();
        self.held.clear();
        self.dirty = true;
        self.generation += 1;
    }

    /// The model choice or the cache changed on disk: look again at the next pump.
    pub fn touch(&mut self) {
        self.active_seen = None;
        self.dirty = true;
    }

    pub fn queued(&self) -> usize {
        self.queue.len()
    }
}

impl Session {
    /// Where the active model's tiles run (see [`Model::device`]); `{kind: "none"}` while no model is loaded.
    pub(crate) fn denoise_device(&self) -> Value {
        let loaded = [&self.denoise.active, &self.denoise.active_rgb]
            .into_iter()
            .flatten()
            .find_map(|a| a.model.lock().unwrap_or_else(PoisonError::into_inner).clone());
        match loaded {
            Some(m) => m.device(self.denoise.settings.run_on()),
            // the built-in denoiser is made afresh for each photo; what it knows about the card is kept apart
            #[cfg(feature = "denoise")]
            None if self.denoise.active.as_ref().is_some_and(|a| a.id == lcc_denoise::builtin::ID) => {
                let onnx = self.denoise.active.as_ref().map(|a| a.onnx.clone()).unwrap_or_default();
                builtin_card(&builtin_marker(&onnx)).device(self.denoise.settings.run_on())
            }
            None => json!({"kind": "none"}),
        }
    }

    /// The folder the products of the open library are kept in; `None` without a library on disk.
    pub(crate) fn denoise_products_dir(&self) -> Option<PathBuf> {
        self.library.as_ref().filter(|l| l.on_disk).map(|l| l.dir.join(DIR))
    }

    /// Read which model is in use (at most every couple of seconds): the manifest and file of the model the settings
    /// name, if it is installed.
    pub(crate) fn denoise_refresh_active(&mut self, force: bool) {
        let fresh = self.denoise.active_seen.is_some_and(|t| t.elapsed() < SETTINGS_TTL);
        if fresh && !force {
            return;
        }
        self.denoise.active_seen = Some(web_time::Instant::now());
        self.denoise.settings = self.denoise.models_dir.as_deref().map(read_settings).unwrap_or_default();
        let installed = self.denoise.models_dir.as_deref().map(installed_models).unwrap_or_default();
        // each slot takes the installed model its setting names, and only one made for its kind of photo
        let pick = |id: Option<&String>, domain: Domain| {
            let id = id?;
            installed.iter().find(|i| &i.manifest.id == id && i.manifest.domain == domain).cloned()
        };
        let chosen = pick(self.denoise.settings.model.as_ref(), Domain::BayerToRgb);
        let rgb = pick(self.denoise.settings.rgb_model.as_ref(), Domain::RgbToRgb);
        let rgb = rgb.map(|i| (i.fingerprint(), i));
        let rgb_same = match (&self.denoise.active_rgb, &rgb) {
            (None, None) => true,
            (Some(a), Some((fp, _))) => a.fingerprint == *fp,
            _ => false,
        };
        // without a model of the user's own the built-in denoiser is in use, so AI Denoise needs no set-up; its
        // (made-up) file is in the models folder, which is where its graphics-card set-up is recorded
        let (fingerprint, manifest, onnx) = match chosen {
            Some(i) => (i.fingerprint(), i.manifest, i.onnx),
            None => {
                let manifest = lcc_denoise::builtin::manifest();
                let onnx = self.denoise.models_dir.as_deref().map(|d| d.join(lcc_denoise::builtin::ID)).unwrap_or_default();
                (format!("{}@{}", manifest.id, manifest.version), manifest, onnx)
            }
        };
        let raw_same = self.denoise.active.as_ref().is_some_and(|a| a.fingerprint == fingerprint);
        if raw_same && rgb_same {
            return;
        }
        // another model: what was made with the old one is not this one's, and what failed may work now; the photos of
        // the other kind keep their job and their place in the queue
        let mut changed: Vec<Domain> = Vec::new();
        if !raw_same {
            self.denoise.active = Some(Active { id: manifest.id.clone(), manifest, onnx, fingerprint, model: Arc::new(Mutex::new(None)) });
            changed.push(Domain::BayerToRgb);
        }
        if !rgb_same {
            self.denoise.active_rgb = rgb.map(|(fingerprint, i)| Active {
                id: i.manifest.id.clone(),
                manifest: i.manifest,
                onnx: i.onnx,
                fingerprint,
                model: Arc::new(Mutex::new(None)),
            });
            changed.push(Domain::RgbToRgb);
        }
        let affected =
            |catalog: &lcc_catalog::Catalog, id: PhotoId| catalog.photo(id).and_then(|p| domain_of(p)).is_none_or(|d| changed.contains(&d));
        let (stop, keep): (Vec<_>, Vec<_>) = std::mem::take(&mut self.denoise.running).into_iter().partition(|r| affected(&self.catalog, r.photo));
        self.denoise.running = keep;
        for r in stop {
            r.progress.cancel_now();
        }
        let catalog = &self.catalog;
        self.denoise.queue.retain(|id| !affected(catalog, *id));
        self.denoise.failed.retain(|id, _| !affected(catalog, *id));
        self.denoise.dirty = true;
    }

    /// The model in use for photo `p`'s kind (raw or finished picture).
    pub(crate) fn denoise_active_for(&self, p: &Photo) -> Option<&Active> {
        match domain_of(p)? {
            Domain::BayerToRgb => self.denoise.active.as_ref(),
            Domain::RgbToRgb => self.denoise.active_rgb.as_ref(),
            // no photo is of an upscaler's kind
            Domain::RgbToRgb4x => None,
        }
    }

    /// Is any model in use (for raw photos or for finished pictures)?
    pub(crate) fn denoise_any_active(&self) -> bool {
        self.denoise.active.is_some() || self.denoise.active_rgb.is_some()
    }

    fn denoise_spec(&self, p: &Photo) -> Option<(DenoiseSpec, String)> {
        let (dir, active) = (self.denoise_products_dir()?, self.denoise_active_for(p)?);
        let key = product_key(&active.fingerprint, p);
        Some((DenoiseSpec { product: product_path(&dir, &key), key: key.clone(), make: None }, key))
    }

    /// Make the index of ready photos from the products on disk (a directory listing and a hash per photo: no file is
    /// opened). Pictures that appeared or vanished drop the decoded sources they affect. The photo being made is its
    /// job's to report (see [`Self::denoise_check_photo`]).
    pub(crate) fn denoise_reindex(&mut self) {
        self.denoise.dirty = false;
        self.denoise.reindexed = Some((self.catalog.revision, web_time::Instant::now()));
        let running: Vec<PhotoId> = self.denoise.running.iter().map(|r| r.photo).collect();
        let mut now: HashMap<PhotoId, DenoiseSpec> = HashMap::new();
        if let Some(dir) = self.denoise_products_dir().filter(|_| self.denoise_any_active()) {
            let names: HashSet<String> =
                products_in(&dir).into_iter().filter_map(|(p, _, _)| p.file_stem().and_then(|s| s.to_str()).map(str::to_string)).collect();
            if !names.is_empty() {
                for p in self.catalog.photos().filter(|p| eligible(p) && !running.contains(&p.id)) {
                    let Some(active) = self.denoise_active_for(p) else { continue };
                    let key = product_key(&active.fingerprint, p);
                    if names.contains(&key) {
                        now.insert(p.id, DenoiseSpec { product: product_path(&dir, &key), key, make: None });
                    }
                }
            }
        }
        let mut changed = false;
        for id in self.media.denoise.ids() {
            if !now.contains_key(&id) {
                self.media.denoise.remove(id);
                self.media.forget(id);
                changed = true;
            }
        }
        for (id, spec) in now {
            if self.media.denoise.spec(id).is_none_or(|old| old.key != spec.key) {
                self.media.denoise.set(id, spec);
                self.media.forget(id);
                changed = true;
            }
        }
        if changed {
            self.denoise.generation += 1;
        }
    }

    /// A photo whose file changed (its content key is not the one its index entry was made for) has no picture.
    ///
    /// The photo whose picture is being made is left to its job: the worker saves the picture before it reports, so the
    /// file is on disk while the job still counts as running. Finding it here would make the photo Ready while the session
    /// is busy with it (#719); [`Self::denoise_poll`] makes it Ready in the pump that takes the job's result instead.
    fn denoise_check_photo(&mut self, id: PhotoId) {
        if self.denoise.running.iter().any(|r| r.photo == id) {
            return;
        }
        let Some(p) = self.catalog.photo(id).cloned() else { return };
        let Some((spec, _)) = self.denoise_spec(&p) else { return };
        match self.media.denoise.spec(id) {
            Some(have) if have.key != spec.key => {
                self.media.denoise.remove(id);
                self.media.forget(id);
                self.denoise.generation += 1;
            }
            None if spec.product.is_file() && product::is_current(&spec.product, &spec.key) => {
                // made by an export, or by another session
                self.media.denoise.set(id, spec);
                self.media.forget(id);
                self.denoise.generation += 1;
            }
            _ => {}
        }
    }

    /// How photo `id` stands.
    pub fn denoise_photo_state(&self, id: PhotoId) -> PhotoState {
        let Some(p) = self.catalog.photo(id) else { return PhotoState::NotApplicable };
        if !eligible(p) {
            return PhotoState::NotApplicable;
        }
        if self.denoise.active.is_none() {
            // the model in use has not been looked at yet (just started)
            return PhotoState::Idle;
        }
        if self.denoise_active_for(p).is_none() {
            return PhotoState::Failed {
                why: "AI Denoise needs a model for finished pictures: install one in Settings ▸ AI Denoise.".into(),
                unsupported: true,
            };
        }
        if self.denoise_products_dir().is_none() {
            return PhotoState::Failed {
                why: "AI Denoise keeps its pictures in the library folder, and this session has none.".into(),
                unsupported: true,
            };
        }
        if self.media.denoise.spec(id).is_some() {
            return PhotoState::Ready;
        }
        if let Some(r) = self.denoise.running.iter().find(|r| r.photo == id) {
            return PhotoState::Running { done: r.progress.done.load(Ordering::Relaxed), total: r.progress.total.load(Ordering::Relaxed) };
        }
        if let Some(ahead) = self.denoise.queue.iter().position(|q| *q == id) {
            return PhotoState::Queued { ahead: ahead + self.denoise.running.len() };
        }
        let key = self.denoise_spec(p).map(|s| s.1);
        match self.denoise.failed.get(&id) {
            Some(f) if Some(&f.key) == key.as_ref() => PhotoState::Failed { why: f.why.clone(), unsupported: f.unsupported },
            _ => PhotoState::Idle,
        }
    }

    /// Ask for photos' pictures to be made (the ones that are not made, queued or known to be impossible). With `retry`
    /// a photo that failed before is tried again. A photo that is not raw is skipped. Returns how many were queued.
    pub(crate) fn denoise_enqueue(&mut self, ids: &[PhotoId], front: bool, retry: bool) -> usize {
        self.denoise_refresh_active(false);
        if !self.denoise_any_active() || self.denoise_products_dir().is_none() {
            return 0;
        }
        let mut added = 0;
        for id in ids {
            let Some(p) = self.catalog.photo(*id).cloned() else { continue };
            if !eligible(&p) || self.media.denoise.spec(*id).is_some() || self.denoise.queue.contains(id) {
                continue;
            }
            if self.denoise.running.iter().any(|r| r.photo == *id) || self.denoise.queue.len() >= self.denoise.queue_cap {
                continue;
            }
            let Some((_, key)) = self.denoise_spec(&p) else { continue };
            if retry {
                self.denoise.failed.remove(id);
            } else if self.denoise.failed.get(id).is_some_and(|f| f.key == key) {
                continue;
            }
            if front {
                self.denoise.queue.push_front(*id);
            } else {
                self.denoise.queue.push_back(*id);
            }
            added += 1;
        }
        added
    }

    /// Queue photos that should all be made (an import's): what doesn't fit in the queue waits in
    /// the backlog and is queued as the queue drains.
    pub(crate) fn denoise_enqueue_all(&mut self, ids: &[PhotoId]) {
        let room = self.denoise.queue_cap.saturating_sub(self.denoise.queue.len());
        let (now, later) = ids.split_at(room.min(ids.len()));
        self.denoise_enqueue(now, false, false);
        for id in later {
            if !self.denoise.backlog.contains(id) {
                self.denoise.backlog.push_back(*id);
            }
        }
    }

    /// Move backlog photos into the queue while it has room.
    fn denoise_refill(&mut self) {
        while self.denoise.queue.len() < self.denoise.queue_cap {
            let Some(id) = self.denoise.backlog.pop_front() else { break };
            self.denoise_enqueue(&[id], false, false);
        }
    }

    /// Was photo `id` waiting in a batch that was cancelled? It is then not made until asked for again.
    pub fn denoise_held(&self, id: PhotoId) -> bool {
        self.denoise.held.contains(&id)
    }

    /// Photo `id` was cancelled by hand (`denoise.cancel {id}`): looking at it doesn't queue it again
    /// until it is asked for, as with a cancelled batch's photos.
    pub(crate) fn denoise_hold(&mut self, id: PhotoId) {
        self.denoise.held.insert(id);
    }

    /// Photos asked for again (the switch, `denoise.queue`): a cancelled batch no longer holds them back.
    pub(crate) fn denoise_release(&mut self, ids: &[PhotoId]) {
        for id in ids {
            self.denoise.held.remove(id);
        }
    }

    /// Stop the jobs that are running (`photo`'s only, when given), and with `all` forget everything that waits.
    pub(crate) fn denoise_cancel(&mut self, photo: Option<PhotoId>, all: bool) -> usize {
        let mut n = 0;
        for r in self.denoise.running.iter().filter(|r| photo.is_none_or(|p| p == r.photo)) {
            r.progress.cancel_now();
            n += 1;
        }
        let before = self.denoise.queue.len() + self.denoise.backlog.len();
        match photo {
            Some(p) => {
                self.denoise.queue.retain(|q| *q != p);
                self.denoise.backlog.retain(|q| *q != p);
            }
            None if all => {
                self.denoise.queue.clear();
                self.denoise.backlog.clear();
            }
            None => {}
        }
        n + before.saturating_sub(self.denoise.queue.len() + self.denoise.backlog.len())
    }

    /// The photos the automatic queue looks at: the one open and the ones selected, those with a Denoise amount.
    fn denoise_wanted(&self) -> Vec<PhotoId> {
        let Selection { active, ids, .. } = &self.selection;
        active
            .iter()
            .chain(ids.iter().take(AUTO_LOOK))
            .copied()
            // (a photo of a folder being browsed isn't imported: its picture is made when asked for)
            .filter(|id| self.catalog.photo(*id).is_some_and(|p| !p.local && p.develop.denoise_amount() > 0.0))
            .collect()
    }

    fn denoise_job_spec(&self, p: &Photo, parallel: usize) -> Option<JobSpec> {
        let Source::File { path } = &p.source else { return None };
        let (spec, _) = self.denoise_spec(p)?;
        let active = self.denoise_active_for(p)?;
        Some(JobSpec {
            path: path.clone(),
            read: self.media.file_bytes.clone(),
            product: spec.product,
            key: spec.key,
            manifest: active.manifest.clone(),
            onnx: active.onnx.clone(),
            model: active.model.clone(),
            loader: self.denoise.loader.clone(),
            parallel,
            run_on: self.denoise.settings.run_on(),
        })
    }

    /// Start the next queued photo on its own thread; `false` when nothing was started.
    fn denoise_start_next(&mut self, parallel: usize) -> bool {
        while let Some(id) = self.denoise.queue.pop_front() {
            let Some(p) = self.catalog.photo(id).cloned() else { continue };
            if !eligible(&p) || self.media.denoise.spec(id).is_some() {
                continue;
            }
            let Some(spec) = self.denoise_job_spec(&p, parallel) else { continue };
            // another session or an export may have made it since it was queued
            if product::is_current(&spec.product, &spec.key) {
                self.denoise_check_photo(id);
                continue;
            }
            let progress = Arc::new(Progress::default());
            let outcome: Arc<Mutex<Option<Outcome>>> = Arc::new(Mutex::new(None));
            let (key, product) = (spec.key.clone(), spec.product.clone());
            let (worker_progress, worker_outcome) = (progress.clone(), outcome.clone());
            let started = std::thread::Builder::new().name("denoise".into()).stack_size(STACK).spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| make_product(&spec, Some(&worker_progress))));
                let o = match result {
                    Ok(Ok(_)) => Outcome::Done,
                    Ok(Err(MakeError::Cancelled)) => Outcome::Cancelled,
                    Ok(Err(MakeError::Unsupported(why))) => Outcome::Failed(why, true),
                    Ok(Err(MakeError::Failed(why))) => Outcome::Failed(why, false),
                    Err(_) => Outcome::Failed("the denoiser stopped unexpectedly".into(), false),
                };
                *worker_outcome.lock().unwrap_or_else(PoisonError::into_inner) = Some(o);
            });
            match started {
                Ok(_) => {
                    self.denoise.running.push(Running { photo: id, key, product, progress, outcome, started: web_time::Instant::now(), task: None });
                }
                Err(e) => {
                    self.denoise.failed.insert(id, Failure { key, why: format!("could not start the denoiser: {e}"), unsupported: false });
                    continue;
                }
            }
            return true;
        }
        false
    }

    /// Photos that may be made at once now: one per graphics card the model in use works on (one before it is loaded
    /// and its cards set up), and only one at a light pace (the user is working: it goes to the second card).
    pub(crate) fn denoise_lanes(&self, pace: Pace) -> usize {
        if matches!(pace, Pace::Pause | Pace::Light) {
            return 1;
        }
        let Some(active) = self.denoise.active.as_ref() else { return 1 };
        let run_on = self.denoise.settings.run_on();
        let loaded = active.model.lock().unwrap_or_else(PoisonError::into_inner).clone();
        let lanes = match loaded {
            Some(m) => m.lanes(run_on),
            // the built-in denoiser is made afresh for each photo; what it knows about the cards is kept apart
            #[cfg(feature = "denoise")]
            None if active.id == lcc_denoise::builtin::ID => builtin_card(&builtin_marker(&active.onnx)).lanes(run_on),
            None => 1,
        };
        lanes.clamp(1, MAX_LANES)
    }

    /// Take in the running jobs' results.
    fn denoise_poll(&mut self) {
        let mut i = 0;
        while let Some(r) = self.denoise.running.get(i) {
            let outcome = r.outcome.lock().unwrap_or_else(PoisonError::into_inner).take();
            match outcome {
                Some(o) => {
                    let r = self.denoise.running.remove(i);
                    self.denoise_finished(r, o);
                }
                None => i += 1,
            }
        }
    }

    /// Record how job `r` ended.
    fn denoise_finished(&mut self, r: Running, outcome: Outcome) {
        match outcome {
            Outcome::Done => {
                self.denoise.made += 1;
                if let Some(t) = &self.denoise.task {
                    t.item_done();
                }
                self.denoise_check_photo(r.photo);
                self.denoise_evict(Some(&r.product));
            }
            Outcome::Failed(why, unsupported) => {
                if let Some(t) = &self.denoise.task {
                    let name = self.catalog.photo(r.photo).map_or("", |p| p.file_name.as_str());
                    t.item_failed(&format!("{name}: {why}"));
                }
                self.denoise.failed.insert(r.photo, Failure { key: r.key, why, unsupported });
            }
            Outcome::Cancelled => {}
        }
        // the picture's state changed: the interface draws it again
        self.denoise.generation += 1;
    }

    /// Keep the cache within its limit: the pictures nobody asks for go first, the oldest first; `keep` (the one just
    /// made) never goes. The index forgets what was deleted.
    pub(crate) fn denoise_evict(&mut self, keep: Option<&Path>) {
        let Some(models) = self.denoise.models_dir.clone() else { return };
        self.denoise_evict_to(read_settings(&models).cache_bytes(), keep);
    }

    /// [`Self::denoise_evict`] for a limit of `limit` bytes.
    pub(crate) fn denoise_evict_to(&mut self, limit: u64, keep: Option<&Path>) {
        let Some(dir) = self.denoise_products_dir() else { return };
        let mut files = products_in(&dir);
        let mut total: u64 = files.iter().map(|f| f.1).sum();
        if total <= limit {
            return;
        }
        // the pictures photos use now (a Denoise amount above 0) are the last to go
        let wanted: HashSet<PathBuf> = self
            .media
            .denoise
            .ids()
            .into_iter()
            .filter(|id| self.catalog.photo(*id).is_some_and(|p| p.develop.denoise_amount() > 0.0))
            .filter_map(|id| self.media.denoise.spec(id).map(|s| s.product.clone()))
            .collect();
        files.sort_by_key(|(path, _, modified)| (wanted.contains(path), *modified));
        let mut gone: HashSet<PathBuf> = HashSet::new();
        for (path, size, _) in files {
            if total <= limit {
                break;
            }
            if keep == Some(path.as_path()) {
                continue;
            }
            if std::fs::remove_file(&path).is_ok() {
                total = total.saturating_sub(size);
                gone.insert(path);
            }
        }
        if gone.is_empty() {
            return;
        }
        for id in self.media.denoise.ids() {
            if self.media.denoise.spec(id).is_some_and(|s| gone.contains(&s.product)) {
                self.media.denoise.remove(id);
                self.media.forget(id);
            }
        }
        self.denoise.generation += 1;
    }

    /// Delete every product of this library (they are made again when wanted).
    pub(crate) fn denoise_clear(&mut self) -> (usize, u64) {
        self.denoise_cancel(None, true);
        let (mut files, mut bytes) = (0, 0);
        if let Some(dir) = self.denoise_products_dir() {
            for (path, size, _) in products_in(&dir) {
                if std::fs::remove_file(&path).is_ok() {
                    files += 1;
                    bytes += size;
                }
            }
        }
        self.denoise.failed.clear();
        self.denoise.touch();
        (files, bytes)
    }

    /// Per frame: take in what finished, look at what the user is looking at, and keep one job going. Cheap when
    /// nothing is happening (no file is opened unless a photo with a Denoise amount lacks its picture).
    pub(crate) fn denoise_pump(&mut self, pace: Pace) {
        crate::cmd::denoise::finish_downloads(self);
        self.denoise_refresh_active(false);
        // photos that changed (a replaced file has another key) must not keep the picture of what they were
        let stale = !self.media.denoise.is_empty()
            && self.denoise.reindexed.is_none_or(|(rev, at)| rev != self.catalog.revision && at.elapsed() > SETTINGS_TTL);
        if self.denoise.dirty || stale {
            self.denoise_reindex();
        }
        self.denoise_poll();
        if !self.denoise_any_active() || self.denoise_products_dir().is_none() {
            return;
        }
        self.denoise_refill();
        let (cap, auto) = (self.denoise.settings.threads, self.denoise.settings.auto());
        let wanted = self.denoise_wanted();
        for id in &wanted {
            self.denoise_check_photo(*id);
        }
        if auto {
            // the photo being edited goes first; photos whose batch was cancelled wait until asked for
            let (open, others): (Vec<PhotoId>, Vec<PhotoId>) =
                wanted.into_iter().filter(|id| !self.denoise.held.contains(id)).partition(|id| self.selection.active == Some(*id));
            self.denoise_enqueue(&others, false, false);
            self.denoise_enqueue(&open, true, false);
        }
        if pace != Pace::Pause {
            let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
            let lanes = self.denoise_lanes(pace);
            // each photo's tiles run on its own card: the threads are not split between them
            while self.denoise.running.len() < lanes && self.denoise_start_next(pace.parallel(cores, cap)) {}
        }
    }

    /// The statistics `denoise.status` reports besides the queue (reads the cache folder).
    pub(crate) fn denoise_cache_usage(&self) -> (usize, u64) {
        match self.denoise_products_dir() {
            Some(dir) => {
                let files = products_in(&dir);
                (files.len(), files.iter().map(|f| f.1).sum())
            }
            None => (0, 0),
        }
    }

    /// Are pictures made without being asked for photos being looked at (Settings ▸ AI Denoise)?
    pub fn denoise_auto(&self) -> bool {
        self.denoise.settings.auto()
    }

    /// Is a picture being made, or waiting for its turn with a model to make it? (Headless runs wait for this.)
    pub fn denoise_busy(&self) -> bool {
        !self.denoise.running.is_empty() || ((!self.denoise.queue.is_empty() || !self.denoise.backlog.is_empty()) && self.denoise_any_active())
    }

    /// Keep AI Denoise's rows in the activity stack up to date (each `denoise.pump`). One batch row while a photo is
    /// being made or waits its turn: "AI Denoise: 37 of 200 · about 6 min left", the photos counted
    /// since the queue was last idle, failures counted and named in its summary. Under it, the running photo's row with
    /// its tiles. Gone when the queue is idle. ✕ forgets the photos waiting and stops the one being made (they are made
    /// again when wanted: shown, exported, or queued by hand).
    pub(crate) fn denoise_track(&mut self) {
        let drop_rows = |running: &mut Vec<Running>| {
            for r in running {
                r.task = None;
            }
        };
        if !self.denoise_busy() {
            drop_rows(&mut self.denoise.running);
            self.denoise.task = None;
            return;
        }
        if self.denoise.task.as_ref().is_some_and(|t| t.is_cancelled()) {
            // what was waiting stays put until asked for again (not re-queued by looking at it)
            let held: Vec<PhotoId> =
                self.denoise.queue.iter().chain(self.denoise.backlog.iter()).copied().chain(self.denoise.running.iter().map(|r| r.photo)).collect();
            self.denoise.held.extend(held);
            self.denoise_cancel(None, true);
            drop_rows(&mut self.denoise.running);
            self.denoise.task = None;
            return;
        }
        // jobs told to stop, with nothing waiting after them, are no new batch while they wind down
        let stopping = self.denoise.queue.is_empty()
            && self.denoise.backlog.is_empty()
            && !self.denoise.running.is_empty()
            && self.denoise.running.iter().all(|r| r.progress.cancelled());
        if stopping && self.denoise.task.is_none() {
            drop_rows(&mut self.denoise.running);
            return;
        }
        let task = self.denoise.task.get_or_insert_with(|| self.activity.start_batch("denoise", "AI Denoise", crate::activity::Cancel::Yes));
        let left = (self.denoise.queue.len() + self.denoise.backlog.len()) as u64 + self.denoise.running.len() as u64;
        let done = task.done();
        task.progress(done, done.saturating_add(left));
        // the batch row names the photo started first; each photo being made (one on each card) has its row under it
        let mut first = None;
        for r in &mut self.denoise.running {
            let name = self.catalog.photo(r.photo).map_or("", |p| p.file_name.as_str());
            first.get_or_insert_with(|| name.to_string());
            let child = r.task.get_or_insert_with(|| {
                let t = task.child("denoise", "AI Denoise");
                t.set_unit(crate::activity::Unit::Percent);
                t
            });
            child.progress(r.progress.done.load(Ordering::Relaxed) as u64, r.progress.total.load(Ordering::Relaxed) as u64);
            child.detail(name);
        }
        task.detail(first.as_deref().unwrap_or(""));
    }

    /// Seconds the job started first has been going, and its progress; with two photos being made (one on each
    /// graphics card) `photos` lists both, the same way.
    pub(crate) fn denoise_running_json(&self) -> Value {
        let one = |r: &Running| {
            json!({
                "photo": r.photo.0,
                "done": r.progress.done.load(Ordering::Relaxed),
                "total": r.progress.total.load(Ordering::Relaxed),
                "seconds": r.started.elapsed().as_secs_f64(),
            })
        };
        let Some(first) = self.denoise.running.first() else { return Value::Null };
        let mut v = one(first);
        if let (true, Some(o)) = (self.denoise.running.len() > 1, v.as_object_mut()) {
            o.insert("photos".into(), self.denoise.running.iter().map(one).collect());
        }
        v
    }

    pub(crate) fn denoise_made(&self) -> u64 {
        self.denoise.made
    }

    /// The running job's product path and key (tests stand in for the worker's save).
    #[cfg(test)]
    pub(crate) fn denoise_running_product(&self) -> Option<(PathBuf, String)> {
        self.denoise.running.first().map(|r| (r.product.clone(), r.key.clone()))
    }

    /// What an export of photo `id` with `settings` needs for its Denoise amount: its denoised picture, made now when the
    /// queue has not got to it. `Ok(None)` when denoise does not apply (amount 0, not a Bayer raw), `Err` when it should
    /// have and cannot (no model chosen, nowhere to keep the picture, the model failing).
    pub(crate) fn denoise_for_export(&mut self, id: PhotoId, settings: &DevelopSettings) -> Result<Option<(DenoiseSpec, PairLoader)>, String> {
        if settings.denoise_amount() <= 0.0 {
            return Ok(None);
        }
        let Some(p) = self.catalog.photo(id).cloned() else { return Ok(None) };
        let Some(loader) = self.media.denoise.loader.clone() else { return Ok(None) };
        if !eligible(&p) {
            return Ok(None);
        }
        self.denoise_refresh_active(false);
        let name = &p.file_name;
        if self.denoise_active_for(&p).is_none() {
            let kind = if domain_of(&p) == Some(Domain::RgbToRgb) { "finished pictures" } else { "raw photos" };
            return Err(format!(
                "{name}: Denoise is on for this photo but no denoise model is chosen for {kind}; choose one in Settings, or set Denoise to 0"
            ));
        }
        if self.denoise_products_dir().is_none() {
            return Err(format!("{name}: Denoise needs a library on disk to keep its pictures in"));
        }
        self.denoise_check_photo(id);
        let Some(job) = self.denoise_job_spec(&p, Pace::Full.parallel(std::thread::available_parallelism().map_or(1, |n| n.get()), None)) else {
            return Ok(None);
        };
        let label = name.clone();
        let spec = job.clone();
        let make: MakeProduct = Arc::new(move || match make_product(&spec, None) {
            Ok(_) => Ok(true),
            Err(MakeError::Unsupported(_)) => Ok(false),
            Err(MakeError::Failed(why)) => Err(format!("{label}: could not denoise: {why}")),
            Err(MakeError::Cancelled) => Err(format!("{label}: denoise was cancelled")),
        });
        Ok(Some((DenoiseSpec { product: job.product, key: job.key, make: Some(make) }, loader)))
    }
}

#[cfg(test)]
#[cfg(feature = "denoise")]
mod gpu_tests {
    use super::*;
    use lcc_denoise::manifest::{Domain, Gain};

    /// A folder of the test's own: the card's set-up leaves its marker beside the model.
    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lc-engine-gpu-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // exclusive: a directory planted at this predictable name fails the test instead of receiving its files
        std::fs::create_dir(&dir).unwrap();
        dir
    }

    /// The model file is read again when the card is set up (for the first photo), so it stays until the test ends.
    fn model(name: &str, tile: u64) -> (Arc<dyn Model>, lcc_denoise::runtime::CpuRunner, PathBuf) {
        let path = folder(name).join("model.onnx");
        std::fs::write(&path, lcc_denoise::synthetic::unet_onnx(tile, 8, 2, 7)).unwrap();
        let manifest = DenoiserManifest {
            id: "synthetic".into(),
            name: "Synthetic".into(),
            version: "1".into(),
            licence: Default::default(),
            source: None,
            sha256: None,
            size_bytes: None,
            provenance: String::new(),
            domain: Domain::BayerToRgb,
            tile: tile as u32,
            overlap: 16,
            gain: Gain::MatchMean { nominal: 1.0, max_deviation: 0.05 },
        };
        let loaded = default_loader()(&path, &manifest).unwrap();
        let cpu = lcc_denoise::runtime::CpuRunner::load(&path, &manifest).unwrap();
        (loaded, cpu, path)
    }

    /// The two cards' sides: the card for editing `editing`, no second card.
    fn one_card(editing: Result<GpuSide, String>) -> [CardSide; 2] {
        two_cards(Err(lcc_gpu::nn::NO_SECOND_CARD.into()), editing)
    }

    /// The two cards' sides, set up as given.
    fn two_cards(second: Result<GpuSide, String>, editing: Result<GpuSide, String>) -> [CardSide; 2] {
        use lcc_gpu::nn::Card;
        let side = |card, g| {
            let s = CardSide::new(card);
            let _ = s.gpu.set(g);
            s
        };
        [side(Card::Second, second), side(Card::Editing, editing)]
    }

    /// A network on `cpu` with these cards.
    fn with_cards(cpu: &lcc_denoise::runtime::CpuRunner, path: &Path, cards: [CardSide; 2]) -> Network {
        Network {
            cpu: cpu.clone(),
            path: path.to_path_buf(),
            tile: 32,
            cards,
            picking: Mutex::new(()),
            last_threads: AtomicUsize::new(0),
            remote: None,
        }
    }

    fn worst(want: &[f32], got: &[f32]) -> f32 {
        assert_eq!(want.len(), got.len());
        let scale = want.iter().fold(1e-9f32, |m, v| m.max(v.abs()));
        want.iter().zip(got).fold(0f32, |m, (a, b)| m.max((a - b).abs())) / scale
    }

    #[test]
    fn a_tile_on_the_card_is_the_cpus_tile() {
        let (model, cpu, path) = model("same", 64);
        let (input, _) = lcc_denoise::runtime::test_tile(64);
        let want = cpu.run(&input).unwrap();
        let (runner, at_once) = model.runner(RunOn::Gpu, 16);
        assert!(worst(&want, &runner.run(&input).unwrap()) < 1e-3);
        let device = model.device(RunOn::Gpu);
        eprintln!("device: {device}");
        match device["kind"].as_str() {
            Some("gpu") => {
                assert!(device["adapter"].as_str().is_some_and(|a| !a.is_empty()));
                assert!(at_once <= GPU_PARALLEL, "a card is fed by a few tiles at once");
                assert!(device["cardMs"].as_f64().is_some_and(|ms| ms > 0.0) && device["cpuMs"].as_f64().is_some_and(|ms| ms > 0.0), "{device}");
            }
            Some("cpu") => {
                assert!(device["reason"].as_str().is_some_and(|r| !r.is_empty()), "the CPU is only used for a reason: {device}");
                assert_eq!(at_once, 16);
            }
            other => panic!("the first photo decides where the work runs, not {other:?}"),
        }
        assert!(!path.with_file_name(GPU_SETUP_MARKER).exists(), "a set-up that ended leaves no marker");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// A server on this computer running `cpu` for the model with `sha256`.
    fn server(cpu: lcc_denoise::runtime::CpuRunner, sha256: &str, tile: usize) -> String {
        use lcc_denoise::remote::{ModelId, ServerOptions, serve};
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap().to_string();
        let id = ModelId { id: "synthetic".into(), version: "1".into(), sha256: sha256.into() };
        let run: lcc_denoise::remote::RunTile = Arc::new(move |input: &[f32]| cpu.run(input));
        let opts = ServerOptions { token: "remote-test-token-0123".into(), ..Default::default() };
        std::thread::spawn(move || serve(l, id, tile, "CPU (test server)".into(), run, opts, Arc::new(|_| {})));
        addr
    }

    fn with_remote(path: &Path, tile: u64, addr: &str, sha256: &str) -> Network {
        let manifest = DenoiserManifest {
            id: "synthetic".into(),
            name: "Synthetic".into(),
            version: "1".into(),
            licence: Default::default(),
            source: None,
            sha256: None,
            size_bytes: None,
            provenance: String::new(),
            domain: Domain::BayerToRgb,
            tile: tile as u32,
            overlap: 16,
            gain: Gain::MatchMean { nominal: 1.0, max_deviation: 0.05 },
        };
        let remote = RemoteSide::new(lcc_denoise::remote::Remote::new(addr, "remote-test-token-0123", sha256, tile as usize));
        network(path, &manifest, Some(remote)).unwrap()
    }

    #[test]
    fn a_remote_server_shares_the_tiles_and_drops_out_when_it_cannot() {
        let (_, cpu, path) = model("remote", 64);
        let sha = lcc_denoise::hash::sha256_file(&path).unwrap();
        let (input, _) = lcc_denoise::runtime::test_tile(64);
        let want = cpu.run(&input).unwrap();
        // the same model on the server: its slots come on top of this computer's threads, answers within f16 rounding
        let addr = server(lcc_denoise::runtime::CpuRunner::load(&path, &cpu.manifest().clone()).unwrap(), &sha, 64);
        let net = with_remote(&path, 64, &addr, &sha);
        let (runner, at_once) = net.runner(RunOn::Cpu, 2);
        assert_eq!(at_once, 2 + 4, "four server slots on top of two local threads");
        assert!(worst(&want, &runner.run(&input).unwrap()) < 5e-3);
        let d = net.device(RunOn::Cpu);
        assert_eq!(d["remote"]["state"], "used", "{d}");
        assert_eq!(d["remote"]["device"], "CPU (test server)");
        // a local card faster than the server there and back (here a stand-in that takes no time): nothing is shared,
        // the photo never waits for the slower server, and the status says why
        let mut net = with_remote(&path, 64, &addr, &sha);
        net.cards = one_card(Ok(GpuSide { runner: Box::new(cpu.clone()), adapter: "Fast card".into(), card_ms: 1e-6, cpu_ms: 1000.0 }));
        let (_, at_once) = net.runner(RunOn::Gpu, 8);
        assert_eq!(at_once, GPU_PARALLEL, "no server slots");
        let d = net.device(RunOn::Gpu);
        assert_eq!(d["remote"]["state"], "slower", "{d}");
        assert!(d["remote"]["remoteMs"].as_f64().is_some_and(|ms| ms > 0.0) && d["remote"]["localMs"].as_f64().is_some(), "{d}");
        // a server running another model is refused: everything runs here
        let other = server(lcc_denoise::runtime::CpuRunner::load(&path, &cpu.manifest().clone()).unwrap(), "0000", 64);
        let net = with_remote(&path, 64, &other, &sha);
        let (runner, at_once) = net.runner(RunOn::Cpu, 2);
        assert_eq!(at_once, 2);
        assert_eq!(runner.run(&input).unwrap(), want, "the CPU path, bit for bit");
        let d = net.device(RunOn::Cpu);
        assert!(d["remote"]["reason"].as_str().is_some_and(|r| r.contains("another model")), "{d}");
        // nobody listening: the same
        let net = with_remote(&path, 64, "127.0.0.1:1", &sha);
        assert_eq!(net.runner(RunOn::Cpu, 3).1, 3);
        assert_eq!(net.device(RunOn::Cpu)["remote"]["state"], "unavailable");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn with_the_processor_chosen_everything_runs_on_the_cpu() {
        let (model, cpu, path) = model("cpu", 64);
        let (input, _) = lcc_denoise::runtime::test_tile(64);
        let (runner, at_once) = model.runner(RunOn::Cpu, 16);
        assert_eq!(runner.run(&input).unwrap(), cpu.run(&input).unwrap(), "the CPU path is the CPU runner, bit for bit");
        assert_eq!(at_once, 16, "the threads asked for are the threads used");
        assert_eq!(model.device(RunOn::Cpu)["kind"], "cpu");
        assert_eq!(model.device(RunOn::Auto)["kind"], "pending", "the card was not even set up");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn the_card_is_used_where_it_is_faster_than_the_processor() {
        // measured: a tile alone on one CPU thread 1.4 s; an RTX 4090 Laptop 13 ms; a Radeon 610M 490 ms
        for threads in [1, 2, 8, 16, 64] {
            assert!(card_is_faster(13.0, 1400.0, threads), "a big card wins at {threads}");
        }
        assert!(card_is_faster(490.0, 1400.0, 2), "a small card beats the two threads background work gets");
        assert!(!card_is_faster(490.0, 1400.0, 16), "and loses to sixteen");
        assert!(!card_is_faster(f64::INFINITY, 1400.0, 1) && !card_is_faster(f64::NAN, 1400.0, 1), "a card that never ran is never faster");
        assert!(!card_is_faster(10.0, 0.0, 1));
    }

    /// A card that gives numbers that are not finite.
    struct Garbage(AtomicUsize);

    impl TileRunner for Garbage {
        fn run(&self, input: &[f32]) -> Result<Vec<f32>, RunError> {
            self.0.fetch_add(1, Ordering::Relaxed);
            let n = input.len() / 4 * 12;
            Ok(vec![f32::NAN; n])
        }
    }

    #[test]
    fn a_tile_the_card_gets_wrong_is_run_on_the_processor_and_three_stop_the_card() {
        let (_, cpu, path) = model("garbage", 64);
        let card = GpuSide { runner: Box::new(Garbage(AtomicUsize::new(0))), adapter: "Test card".into(), card_ms: 1.0, cpu_ms: 1000.0 };
        let model = with_cards(&cpu, &path, one_card(Ok(card)));
        let (input, _) = lcc_denoise::runtime::test_tile(64);
        let want = cpu.run(&input).unwrap();
        let (runner, at_once) = model.runner(RunOn::Auto, 8);
        assert_eq!(at_once, GPU_PARALLEL, "the (fast) card is chosen");
        for _ in 0..GPU_MAX_FALLBACKS {
            assert_eq!(runner.run(&input).unwrap(), want, "a wrong tile is the CPU's tile");
        }
        let device = model.device(RunOn::Auto);
        assert_eq!(device["kind"], "cpu", "{device}");
        assert!(device["reason"].as_str().unwrap().contains("tiles wrong"), "{device}");
        assert_eq!(device["fellBack"], GPU_MAX_FALLBACKS);
        let (_, at_once) = model.runner(RunOn::Gpu, 8);
        assert_eq!(at_once, 8, "the next photo runs on the CPU, even with the card chosen");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// A card that runs the CPU's model and counts its tiles.
    struct Counting(lcc_denoise::runtime::CpuRunner, Arc<AtomicUsize>);

    impl TileRunner for Counting {
        fn run(&self, input: &[f32]) -> Result<Vec<f32>, RunError> {
            self.1.fetch_add(1, Ordering::Relaxed);
            self.0.run(input)
        }
    }

    fn counting(cpu: &lcc_denoise::runtime::CpuRunner, name: &str) -> (GpuSide, Arc<AtomicUsize>) {
        let n = Arc::new(AtomicUsize::new(0));
        (GpuSide { runner: Box::new(Counting(cpu.clone(), n.clone())), adapter: name.into(), card_ms: 1.0, cpu_ms: 1000.0 }, n)
    }

    /// Photos of a batch run on both cards at once, the second graphics card first, each photo's tiles on its own card,
    /// with the CPU's answers; a card given back takes the next photo.
    #[test]
    fn two_photos_run_on_two_cards_the_second_card_first() {
        let (_, cpu, path) = model("twocards", 64);
        let ((second, on_second), (editing, on_editing)) = (counting(&cpu, "Second card"), counting(&cpu, "Editing card"));
        let net = with_cards(&cpu, &path, two_cards(Ok(second), Ok(editing)));
        assert_eq!(net.lanes(RunOn::Gpu), 2, "a photo on each card");
        assert_eq!(net.lanes(RunOn::Cpu), 1, "the processor chosen: one photo at a time");
        let (input, _) = lcc_denoise::runtime::test_tile(64);
        let want = cpu.run(&input).unwrap();
        let (first, at_once) = net.runner(RunOn::Gpu, 16);
        assert_eq!(at_once, GPU_PARALLEL);
        assert_eq!(first.run(&input).unwrap(), want);
        assert_eq!((on_second.load(Ordering::Relaxed), on_editing.load(Ordering::Relaxed)), (1, 0), "the first photo takes the second card");
        let (second, _) = net.runner(RunOn::Gpu, 16);
        assert_eq!(second.run(&input).unwrap(), want);
        assert_eq!(on_editing.load(Ordering::Relaxed), 1, "the second photo takes the free card");
        let d = net.device(RunOn::Gpu);
        assert_eq!((d["kind"].as_str(), d["adapter"].as_str()), (Some("gpu"), Some("Second card")), "{d}");
        let cards = d["cards"].as_array().unwrap();
        assert_eq!(
            cards.iter().map(|c| (c["card"].as_str().unwrap(), c["photos"].as_u64().unwrap())).collect::<Vec<_>>(),
            [("second", 1), ("editing", 1)]
        );
        drop(first);
        let (third, _) = net.runner(RunOn::Gpu, 16);
        third.run(&input).unwrap();
        assert_eq!(on_second.load(Ordering::Relaxed), 2, "a card given back takes the next photo");
        drop((second, third));
        assert!(net.cards.iter().all(|c| c.photos.load(Ordering::Relaxed) == 0), "every card was given back");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// A second graphics card that gets tiles wrong: its tiles are the CPU's, and after three it gets no more photos;
    /// the card for editing takes them, and the batch never loses a tile.
    #[test]
    fn a_broken_second_card_leaves_the_work_to_the_editing_card_and_the_cpu() {
        let (_, cpu, path) = model("brokensecond", 64);
        let garbage = Arc::new(AtomicUsize::new(0));
        struct Shared(Arc<AtomicUsize>);
        impl TileRunner for Shared {
            fn run(&self, input: &[f32]) -> Result<Vec<f32>, RunError> {
                Garbage(AtomicUsize::new(0)).run(input).inspect(|_| {
                    self.0.fetch_add(1, Ordering::Relaxed);
                })
            }
        }
        let second = GpuSide { runner: Box::new(Shared(garbage.clone())), adapter: "Broken card".into(), card_ms: 1.0, cpu_ms: 1000.0 };
        let (editing, on_editing) = counting(&cpu, "Editing card");
        let net = with_cards(&cpu, &path, two_cards(Ok(second), Ok(editing)));
        let (input, _) = lcc_denoise::runtime::test_tile(64);
        let want = cpu.run(&input).unwrap();
        let (photo, _) = net.runner(RunOn::Gpu, 8);
        for _ in 0..GPU_MAX_FALLBACKS + 2 {
            assert_eq!(photo.run(&input).unwrap(), want, "a wrong tile is the CPU's tile");
        }
        assert_eq!(garbage.load(Ordering::Relaxed), GPU_MAX_FALLBACKS, "after three wrong tiles the card is not asked again");
        drop(photo);
        assert_eq!(net.lanes(RunOn::Gpu), 1, "one card left");
        let (next, at_once) = net.runner(RunOn::Gpu, 8);
        assert_eq!(at_once, GPU_PARALLEL, "the next photo still runs on a card");
        assert_eq!(next.run(&input).unwrap(), want);
        assert_eq!((garbage.load(Ordering::Relaxed), on_editing.load(Ordering::Relaxed)), (GPU_MAX_FALLBACKS, 1), "on the editing card");
        let d = net.device(RunOn::Gpu);
        assert_eq!(d["adapter"], "Editing card", "{d}");
        let second = d["cards"].as_array().unwrap().iter().find(|c| c["card"] == "second").cloned().unwrap();
        assert!(second["reason"].as_str().is_some_and(|r| r.contains("tiles wrong")), "{d}");
        // both broken: the CPU does it all
        net.cards[1].fallbacks.store(GPU_MAX_FALLBACKS, Ordering::Relaxed);
        drop(next);
        let (cpu_only, at_once) = net.runner(RunOn::Gpu, 8);
        assert_eq!((at_once, cpu_only.run(&input).unwrap()), (8, want));
        assert_eq!(net.device(RunOn::Gpu)["kind"], "cpu");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// Each graphics card is set up on its own and passes the check tile against the CPU ([`GPU_AGREE`]); the two cards'
    /// answers on a test tile agree as closely. A card that isn't here is skipped (one card: the adapter variable's;
    /// `<PREFIX>_GPU_HELPER=same` makes the second device on that same card).
    #[test]
    fn each_card_passes_the_check_tile() {
        let (_, cpu, path) = model("eachcard", 64);
        let (input, _) = lcc_denoise::runtime::test_tile(64);
        let want = cpu.run(&input).unwrap();
        let mut answers = Vec::new();
        for card in lcc_gpu::nn::CARDS {
            match make_gpu(&cpu, &path, 64, card) {
                Ok(g) => {
                    let got = g.runner.run(&input).unwrap();
                    let off = worst(&want, &got);
                    eprintln!("{} card: {} — check tile {:.1} ms (CPU {:.0} ms), off by {off:.1e}", card.id(), g.adapter, g.card_ms, g.cpu_ms);
                    assert!(off <= GPU_AGREE, "{}: {off:e}", g.adapter);
                    answers.push(got);
                }
                Err(why) => lcc_testkit::skip!(Gpu, "AI Denoise on the {} card: {why}", card.id()),
            }
            assert!(!marker_of(&path.with_file_name(GPU_SETUP_MARKER), card).exists(), "a set-up that ended leaves no marker");
        }
        if let [a, b] = answers.as_slice() {
            let off = worst(a, b);
            eprintln!("the two cards differ by {off:.1e}");
            assert!(off <= GPU_AGREE);
        }
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// The second card's set-up marker sits beside the card for editing's, so a crash setting up one keeps only it off.
    #[test]
    fn each_card_has_its_own_set_up_marker() {
        let base = Path::new("models").join("m").join(GPU_SETUP_MARKER);
        assert_eq!(card_marker(&base, false), base);
        assert_eq!(card_marker(&base, true), Path::new("models").join("m").join("gpu-setup-helper.marker"));
        let temp = Path::new("tmp").join("app-builtin-gpu-setup.marker");
        assert_eq!(card_marker(&temp, true), Path::new("tmp").join("app-builtin-gpu-setup-helper.marker"));
        assert_ne!(marker_of(&base, lcc_gpu::nn::Card::Second), marker_of(&base, lcc_gpu::nn::Card::Editing));
    }

    #[test]
    fn a_slow_card_is_left_out_unless_it_is_chosen() {
        let (_, cpu, path) = model("slow", 64);
        let slow = GpuSide { runner: Box::new(cpu.clone()), adapter: "Slow card".into(), card_ms: 490.0, cpu_ms: 1400.0 };
        let model = with_cards(&cpu, &path, one_card(Ok(slow)));
        assert_eq!(model.runner(RunOn::Auto, 16).1, 16, "sixteen threads beat it");
        let device = model.device(RunOn::Auto);
        assert_eq!((device["kind"].as_str(), device["reason"].as_str()), (Some("cpu"), Some("it is faster than Slow card on this computer")));
        assert_eq!(device["cardMs"], 490.0, "the times measured are reported");
        assert_eq!(device["retry"], false, "trying again would not make it faster");
        assert_eq!(model.runner(RunOn::Auto, 2).1, GPU_PARALLEL.min(2), "two threads do not");
        assert_eq!(model.device(RunOn::Auto)["kind"], "gpu");
        assert_eq!(model.runner(RunOn::Gpu, 16).1, GPU_PARALLEL, "chosen, it is used anyway");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_set_up_marker_left_behind_keeps_the_card_off() {
        let (model, _, path) = model("crashed", 64);
        let marker = path.with_file_name(GPU_SETUP_MARKER);
        std::fs::write(&marker, "setting up the graphics card for AI Denoise (unix time 0)").unwrap();
        let (_, at_once) = model.runner(RunOn::Gpu, 4);
        let all = model.device(RunOn::Gpu);
        // a second graphics card (when there is one) is not the card that crashed: it may do the work
        let device = all["cards"].as_array().and_then(|c| c.iter().find(|c| c["card"] == "editing")).cloned().unwrap_or(all.clone());
        if all["cards"].is_null() {
            assert_eq!(at_once, 4, "the CPU does the work");
        }
        assert_eq!((device["kind"].as_str(), device["reason"].as_str()), (Some("cpu"), Some(GPU_CRASHED)), "{all}");
        assert_eq!(device["retry"], true, "the interface offers to try again");
        assert!(marker.exists(), "it stays until the user chooses where denoise runs");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_set_up_runs_beside_the_cpu_and_removes_its_marker() {
        let dir = folder("setup");
        let marker = dir.join(GPU_SETUP_MARKER);
        let seen = Arc::new(AtomicBool::new(false));
        let (m, s) = (marker.clone(), seen.clone());
        let r = set_up_card(
            &marker,
            Duration::from_secs(10),
            move || {
                s.store(m.exists(), Ordering::Relaxed);
                Ok(7)
            },
            || "cpu",
        );
        assert_eq!(r, Ok((7, "cpu")));
        assert!(seen.load(Ordering::Relaxed), "the marker is there while the card is set up");
        assert!(!marker.exists(), "and gone after");
        // a set-up that fails or panics removes it too
        assert_eq!(set_up_card(&marker, Duration::from_secs(10), || Err::<(), _>("no card".into()), || ()), Err("no card".into()));
        assert!(!marker.exists());
        let r = set_up_card(&marker, Duration::from_secs(10), || -> Result<(), String> { panic!("driver") }, || ());
        assert!(r.is_err() && !marker.exists(), "{r:?}");
        // a marker found before the set-up starts means the last one took the app down: the card is not touched
        std::fs::write(&marker, "left behind").unwrap();
        let touched = Arc::new(AtomicBool::new(false));
        let t = touched.clone();
        let r = set_up_card(
            &marker,
            Duration::from_secs(10),
            move || {
                t.store(true, Ordering::Relaxed);
                Ok(())
            },
            || (),
        );
        assert_eq!(r, Err(GPU_CRASHED.to_string()));
        assert!(!touched.load(Ordering::Relaxed) && marker.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_set_up_makes_the_folder_of_its_marker() {
        let dir = folder("setup-folder").join("not").join("yet");
        let marker = dir.join(GPU_SETUP_MARKER);
        let (m, seen) = (marker.clone(), Arc::new(AtomicBool::new(false)));
        let s = seen.clone();
        let r = set_up_card(
            &marker,
            Duration::from_secs(10),
            move || {
                s.store(m.exists(), Ordering::Relaxed);
                Ok(())
            },
            || (),
        );
        assert_eq!(r, Ok(((), ())));
        assert!(seen.load(Ordering::Relaxed), "the crash guard is written even when the models folder was missing");
        let _ = std::fs::remove_dir_all(dir.ancestors().nth(2).unwrap_or(&dir));
    }

    #[test]
    fn a_set_up_that_takes_too_long_is_given_up_on() {
        let dir = folder("slowsetup");
        let marker = dir.join(GPU_SETUP_MARKER);
        let started = web_time::Instant::now();
        let slow = || -> Result<(), String> {
            std::thread::sleep(Duration::from_millis(600));
            Ok(())
        };
        let r = set_up_card(&marker, Duration::from_millis(50), slow, || ());
        assert!(r.as_ref().is_err_and(|e| e.contains("took longer") && worth_retrying(e)), "{r:?}");
        assert!(started.elapsed() < Duration::from_millis(500), "the caller does not wait for the card");
        // while the first is still stuck, another set-up of the same model does not start a second one
        let again = set_up_card(&marker, Duration::from_secs(5), || Ok(()), || ());
        assert!(again.is_err_and(|e| e.contains("still setting up") && worth_retrying(&e)));
        // once it ends its marker goes, and the card can be set up again
        while marker.exists() {
            assert!(started.elapsed() < Duration::from_secs(10), "the stuck set-up ended");
            std::thread::sleep(Duration::from_millis(20));
        }
        while SETTING_UP.lock().unwrap().iter().any(|m| m == &marker) {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(set_up_card(&marker, Duration::from_secs(5), || Ok(1), || 2), Ok((1, 2)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The built-in denoiser for one photo of `noise` `(a, b)`, its card state kept by a marker in a folder of its own.
    fn builtin(name: &str) -> (BuiltinModel, PathBuf) {
        let dir = folder(name);
        let m = BuiltinModel::new(&dir.join(GPU_SETUP_MARKER));
        let _ = m.runner.set(lcc_denoise::builtin::Builtin::with_noise(CHECK_NOISE));
        (m, dir)
    }

    /// Compare a tile from the card with the CPU's picture: equal on average, and with few values far off. (The demosaic
    /// chooses a direction at each pixel, so a tiny difference of the filter can change a pixel by a good deal.)
    fn agree(want: &[f32], got: &[f32]) {
        assert_eq!(want.len(), got.len());
        let scale = want.iter().fold(1e-9f32, |m, v| m.max(v.abs()));
        let mean = want.iter().zip(got).map(|(a, b)| (a - b).abs()).sum::<f32>() / want.len() as f32 / scale;
        let far = want.iter().zip(got).filter(|(a, b)| (*a - *b).abs() > 0.05 * scale).count() as f32 / want.len() as f32;
        assert!(mean < 1e-3 && far < 5e-3, "off by {mean:e} on average of the largest value, {far:e} of the values far off");
    }

    /// With the card chosen, the built-in denoiser's tile agrees with the processor's, a few tiles feed the card at once,
    /// and a set-up that ended leaves no marker; where there is no card it stays on the processor.
    #[test]
    fn the_built_in_denoiser_on_the_card_gives_the_cpus_tile() {
        use lcc_denoise::builtin::{Builtin, TILE};
        let (m, dir) = builtin("builtin-card");
        let tile = builtin_check_tile(TILE as usize);
        let want = Builtin::with_noise(CHECK_NOISE).run(&tile).unwrap();
        let (runner, at_once) = m.runner(RunOn::Gpu, 16);
        let device = m.device(RunOn::Gpu);
        eprintln!("device: {device}");
        match device["kind"].as_str() {
            Some("gpu") => {
                assert!(at_once <= GPU_PARALLEL, "a card is fed by a few tiles at once");
                agree(&want, &runner.run(&tile).unwrap());
                assert!(device["cardMs"].as_f64().is_some_and(|ms| ms > 0.0) && device["cpuMs"].as_f64().is_some_and(|ms| ms > 0.0), "{device}");
            }
            Some("cpu") => {
                lcc_testkit::skip!(Gpu, "the built-in denoiser stays on the CPU here: {}", device["reason"]);
                assert_eq!(at_once, 16);
            }
            other => panic!("the first photo decides where the work runs, not {other:?}"),
        }
        assert!(!dir.join(GPU_SETUP_MARKER).exists(), "a set-up that ended leaves no marker");
        forget_builtin_card(&dir.join(GPU_SETUP_MARKER));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// With the processor chosen, the built-in denoiser gives the processor's tile bit for bit, uses all the threads
    /// asked for and never sets the card up.
    #[test]
    fn with_the_processor_chosen_the_built_in_denoiser_runs_on_the_cpu_and_never_starts_the_card() {
        use lcc_denoise::builtin::{Builtin, TILE};
        let (m, dir) = builtin("builtin-cpu");
        let tile = builtin_check_tile(TILE as usize);
        let (runner, at_once) = m.runner(RunOn::Cpu, 16);
        assert_eq!(runner.run(&tile).unwrap(), Builtin::with_noise(CHECK_NOISE).run(&tile).unwrap(), "the CPU path is the CPU runner, bit for bit");
        assert_eq!(at_once, 16, "the threads asked for are the threads used");
        assert_eq!(m.device(RunOn::Cpu)["kind"], "cpu");
        assert_eq!(m.device(RunOn::Auto)["kind"], "pending", "the card was not even set up");
        forget_builtin_card(&dir.join(GPU_SETUP_MARKER));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A set-up marker left behind (the app closed while it ran) keeps the card off the built-in denoiser, with a reason
    /// and a retry, until choosing where denoise runs forgets it.
    #[test]
    fn a_set_up_marker_left_behind_keeps_the_card_off_the_built_in_denoiser_until_a_choice_is_made() {
        let (m, dir) = builtin("builtin-marker");
        let marker = dir.join(GPU_SETUP_MARKER);
        std::fs::write(&marker, "setting up the graphics card for AI Denoise (unix time 1)\n").unwrap();
        let (_, at_once) = m.runner(RunOn::Gpu, 6);
        let all = m.device(RunOn::Gpu);
        // a second graphics card (when there is one) has a marker of its own: it may do the work
        let device = all["cards"].as_array().and_then(|c| c.iter().find(|c| c["card"] == "editing")).cloned().unwrap_or(all.clone());
        if all["cards"].is_null() {
            assert_eq!(at_once, 6, "no card: the threads asked for");
        }
        assert_eq!(device["kind"], "cpu", "{all}");
        assert_eq!(device["retry"], true, "{all}");
        assert!(device["reason"].as_str().unwrap().contains("closed"), "{all}");
        // choosing where denoise runs forgets it (and the marker): the card is set up afresh for the next photo
        forget_gpu_set_ups(&dir);
        assert!(!marker.exists());
        let (again, _) = builtin("builtin-marker-again");
        assert_eq!(again.device(RunOn::Auto)["kind"], "pending");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The card's filter stays within the agreement limits of the processor's on the check tile, well inside them, and
    /// gives the same answer every time.
    #[test]
    fn the_cards_filter_agrees_with_the_cpus_on_the_check_tile_and_gives_the_same_answer_every_time() {
        use lcc_denoise::builtin::{Builtin, TILE};
        let tile = builtin_check_tile(TILE as usize);
        let b = Builtin::with_noise(CHECK_NOISE);
        let params = b.bm3d_params().unwrap();
        let gpu = match lcc_gpu::bm3d::Bm3dGpu::new(&params, TILE as usize) {
            Ok(g) => Arc::new(g),
            Err(why) => return lcc_testkit::skip!(Gpu, "no card for the built-in denoiser here: {why}"),
        };
        let runner = lcc_gpu::bm3d::BuiltinRunner::new(gpu, b).unwrap();
        let a = runner.agreement(&tile).unwrap();
        eprintln!("the card's filter is off by {:.3} at most and {:.1e} on average (noise deviations)", a.worst, a.mean);
        assert!(a.worst <= BUILTIN_AGREE_WORST && a.mean <= BUILTIN_AGREE_MEAN, "{a:?}");
        assert!(a.worst < 0.15 && a.mean < 1e-4, "a card within the limits but far from what it manages: {a:?}");
        let (first, again) = (runner.run(&tile).unwrap(), runner.run(&tile).unwrap());
        assert_eq!(first, again, "the card gives the same answer every time");
        agree(&Builtin::with_noise(CHECK_NOISE).run(&tile).unwrap(), &first);
    }

    /// A tile the card cannot do (another size than it was made for) is run on the processor, and after three of them
    /// the card is stopped for the built-in denoiser.
    #[test]
    fn a_tile_the_card_gets_wrong_is_run_on_the_processor_and_three_stop_the_card_for_the_built_in_denoiser() {
        use lcc_denoise::builtin::Builtin;
        let (m, dir) = builtin("builtin-wrong");
        let (_, _) = m.runner(RunOn::Gpu, 8);
        if m.device(RunOn::Gpu)["kind"] != "gpu" {
            lcc_testkit::skip!(Gpu, "no card for the built-in denoiser here: {}", m.device(RunOn::Gpu)["reason"]);
            forget_builtin_card(&dir.join(GPU_SETUP_MARKER));
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }
        // a tile of another size than the card was made for is one it cannot do: the CPU runs it, and three stop the card
        let small = 64;
        let input = builtin_check_tile(small);
        let want = Builtin::with_noise(CHECK_NOISE).run(&input).unwrap();
        // each card (two when there is a second one) is stopped by its own wrong tiles
        for _ in 0..lcc_gpu::nn::CARDS.len() {
            if m.device(RunOn::Gpu)["kind"] != "gpu" {
                break;
            }
            let (runner, _) = m.runner(RunOn::Gpu, 8);
            for _ in 0..GPU_MAX_FALLBACKS {
                assert_eq!(runner.run(&input).unwrap(), want, "a tile the card cannot do is the CPU's tile");
            }
        }
        let device = m.device(RunOn::Gpu);
        assert_eq!(device["kind"], "cpu", "{device}");
        assert!(device["reason"].as_str().unwrap().contains("tiles wrong"), "{device}");
        assert_eq!(device["fellBack"], GPU_MAX_FALLBACKS);
        let (_, at_once) = m.runner(RunOn::Gpu, 8);
        assert_eq!(at_once, 8, "the next photo runs on the CPU, even with the card chosen");
        forget_builtin_card(&dir.join(GPU_SETUP_MARKER));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
