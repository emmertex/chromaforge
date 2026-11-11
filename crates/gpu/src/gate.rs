//! Turns at the GPU for batch work (exports), so many photos rendering side by side share the
//! graphics card without over-committing its memory, and the window's own work (the loupe, the
//! grid, drawing frames) always comes first.
//!
//! A thread marks its renders as batch work with [`crate::as_batch`]. Each batch render then holds
//! a [`Permit`] for the device memory it needs ([`job_bytes`]) while it runs: it waits while the
//! renders already holding permits use the budget ([`crate::set_batch_limits`]; by default a share
//! of the card's memory, [`auto_limit`]) or the most jobs at once, while an interactive render is
//! on the GPU, and while the window asked for room ([`crate::hold_batch`], e.g. while it resizes).
//! One batch render may always run, whatever its size (a photo bigger than the budget still uses
//! the GPU, alone). Interactive renders never wait: they are only counted.
//!
//! Between its submissions a batch render waits for its previous one to finish ([`throttle`]), so
//! the queue never holds more than about one submission per export: the window's frames slot in
//! between instead of waiting behind seconds of export work (with 14–16 exports queued at once
//! the window drew no frame for 5 s, and resizing it failed with "Failed to wait for GPU to come
//! idle").
//!
//! The second graphics card (`helper.rs`) has a gate of its own: its batch renders share its
//! memory, never the editing card's (by default [`HELPER_SHARE_PERCENT`] of it: it draws no
//! window), and only the thumbnails sent there come first. A thread's renders use the second
//! card's gate inside [`on_helper`].
//!
//! **Both cards take batch photos** ([`order`]): each gate measures its card's speed for batch
//! work ([`measured_rate`]), and a photo goes to the card whose running photos plus this one would
//! finish soonest, so both cards work in proportion to their speed (the second card first on a
//! tie, and whenever it has none while the interface is tracked).
//!
//! **The editing card yields to the interface** once the app tracks it ([`track`]; the desktop
//! app does): batch work there waits until nothing was touched and no view rendered for [`idle`]
//! (~1 s), takes one job at a time while a second card works, and a batch render already running
//! there pauses between its submissions ([`yield_to_interaction`]) the moment a slider, brush, pan
//! or photo change starts, so it never holds the card for a whole render while a draft waits. The
//! second card draws no window: its batch work never waits for the interface.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

#[derive(Default)]
struct State {
    /// Device bytes the running batch renders hold.
    held: u64,
    /// Batch renders running.
    jobs: usize,
    /// Interactive renders running.
    interactive: usize,
    /// Batch renders wait until then (the window asked for the GPU).
    hold_until: Option<Instant>,
    /// Batch renders waiting for their turn now.
    waiting: usize,
    /// When the last interactive render ended.
    interactive_ended: Option<Instant>,
    /// Since when batch renders run without a break (`None`: none runs now).
    busy_since: Option<Instant>,
    /// Batch work done (the device bytes of the turns given back) and the time it took with batch
    /// renders running: the card's measured speed ([`rate`]), which shares batch photos out
    /// between the cards ([`order`]).
    done: u64,
    busy: Duration,
}

impl State {
    const fn new() -> State {
        State {
            held: 0,
            jobs: 0,
            interactive: 0,
            hold_until: None,
            waiting: 0,
            interactive_ended: None,
            busy_since: None,
            done: 0,
            busy: Duration::ZERO,
        }
    }
}

static STATE: Mutex<State> = Mutex::new(State::new());
static CV: Condvar = Condvar::new();
/// The second card's gate.
static HELPER_STATE: Mutex<State> = Mutex::new(State::new());
static HELPER_CV: Condvar = Condvar::new();
/// Device bytes batch renders may hold together (0: [`auto_limit`]).
static LIMIT: AtomicU64 = AtomicU64::new(0);
/// Most batch renders at once (0: no other limit than memory, at most [`MAX_JOBS`]).
static JOBS: AtomicUsize = AtomicUsize::new(0);
/// Batch renders that waited for their turn, and for how long in all (ms): diagnostics.
static WAITS: AtomicU64 = AtomicU64::new(0);
static WAITED_MS: AtomicU64 = AtomicU64::new(0);
/// Running batch renders on the editing card that paused for the interface, and for how long in
/// all (ms): diagnostics.
static PAUSES: AtomicU64 = AtomicU64::new(0);
static PAUSED_MS: AtomicU64 = AtomicU64::new(0);

/// The most batch renders on the GPU at once by default.
pub const MAX_JOBS: usize = 4;

/// Share of the card's memory batch renders may use (the window, the views and the driver keep the rest).
const SHARE_PERCENT: u64 = 55;
/// Share of the second card's memory: it draws no window and renders no view.
const HELPER_SHARE_PERCENT: u64 = 80;

// ---------------------------------------------------------------- the interface's idle clock

/// Whether batch work on the editing card waits for the interface to be idle ([`track`]).
static TRACK: AtomicBool = AtomicBool::new(false);
/// How long the interface must be untouched before batch work uses the editing card (ms).
static IDLE_MS: AtomicU64 = AtomicU64::new(1000);
/// With a second card ready, how long the interface must be untouched before batch work uses the
/// editing card too (ms): the second card takes it meanwhile, and the card drawing the window is
/// left to the photo being edited. (At [`IDLE_MS`] an export or a 1:1 preview build took the
/// window's card back between two slider drags; its work then held the window's frames up for up
/// to 5.7 s, CF-ZOOM-FREEZE 2026-11-10.)
static HELPER_IDLE_MS: AtomicU64 = AtomicU64::new(8000);
/// When the interface was last touched, ms after [`epoch`] (0: never).
static LAST_INPUT_MS: AtomicU64 = AtomicU64::new(0);

fn epoch() -> Instant {
    static E: OnceLock<Instant> = OnceLock::new();
    *E.get_or_init(Instant::now)
}

fn now_ms() -> u64 {
    // (+1: 0 means "never")
    u64::try_from(epoch().elapsed().as_millis()).unwrap_or(u64::MAX).saturating_add(1)
}

/// Turn the idle rule on (the desktop app) or off (the default: headless sessions, the CLI and
/// tests keep the old behaviour).
pub(crate) fn track(on: bool) {
    TRACK.store(on, Ordering::Relaxed);
    CV.notify_all();
}

pub(crate) fn tracking() -> bool {
    TRACK.load(Ordering::Relaxed)
}

/// The interface was touched now (input, a slider, a pan, a photo change).
pub(crate) fn note_interaction() {
    LAST_INPUT_MS.store(now_ms(), Ordering::Relaxed);
}

/// How long the interface must be untouched before batch work uses the editing card.
pub(crate) fn idle() -> Duration {
    Duration::from_millis(IDLE_MS.load(Ordering::Relaxed))
}

/// How long the interface must be untouched before batch work uses the editing card: [`idle`],
/// or longer while a second card takes batch work ([`HELPER_IDLE_MS`]).
pub(crate) fn editing_idle(second_card: bool) -> Duration {
    let idle = idle();
    if second_card { idle.max(Duration::from_millis(HELPER_IDLE_MS.load(Ordering::Relaxed))) } else { idle }
}

/// Set [`HELPER_IDLE_MS`] (tests).
#[cfg(test)]
pub(crate) fn set_helper_idle(d: Duration) {
    HELPER_IDLE_MS.store(u64::try_from(d.as_millis()).unwrap_or(u64::MAX), Ordering::Relaxed);
}

/// Set [`idle`] (tests).
pub(crate) fn set_idle(d: Duration) {
    IDLE_MS.store(u64::try_from(d.as_millis()).unwrap_or(u64::MAX), Ordering::Relaxed);
}

/// Whether the interface leaves the editing card to batch work now: no view rendering, and (when
/// tracked) nothing touched and no view rendered for [`idle`].
fn interface_quiet(s: &State, now: Instant) -> bool {
    if s.interactive > 0 {
        return false;
    }
    if !tracking() {
        return true;
    }
    let idle = editing_idle(crate::helper::current().is_some());
    let last = LAST_INPUT_MS.load(Ordering::Relaxed);
    let since_input = Duration::from_millis(now_ms().saturating_sub(last));
    let input_quiet = last == 0 || since_input >= idle;
    let views_quiet = s.interactive_ended.is_none_or(|t| now.saturating_duration_since(t) >= idle);
    input_quiet && views_quiet
}

/// Whether a batch render on the editing card would have to wait for the interface now.
pub(crate) fn interface_busy() -> bool {
    !interface_quiet(&state(), Instant::now())
}

fn state() -> std::sync::MutexGuard<'static, State> {
    state_of(false)
}

/// The editing card's gate, or (`helper`) the second card's.
fn state_of(helper: bool) -> std::sync::MutexGuard<'static, State> {
    if helper { &HELPER_STATE } else { &STATE }.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn cv_of(helper: bool) -> &'static Condvar {
    if helper { &HELPER_CV } else { &CV }
}

thread_local! {
    /// This thread's renders run on the second card ([`on_helper`]).
    static HELPER: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Run `f` with this thread's renders on the second card and its gate.
pub(crate) fn on_helper<R>(f: impl FnOnce() -> R) -> R {
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            let _ = HELPER.try_with(|c| c.set(self.0));
        }
    }
    let _restore = Restore(HELPER.with(|c| c.replace(true)));
    f()
}

/// Whether this thread's renders run on the second card ([`on_helper`], or a turn taken there).
pub(crate) fn uses_helper() -> bool {
    HELPER.try_with(|c| c.get()).unwrap_or(false)
}

/// Whether this thread holds a turn for several renders ([`in_one_turn`], [`try_turn`]).
pub(crate) fn in_turn() -> bool {
    IN_TURN.try_with(|c| c.get()).unwrap_or(false)
}

thread_local! {
    static BATCH: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// This thread holds a turn for several renders ([`in_one_turn`]): they don't queue again.
    static IN_TURN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Run `f` (several batch renders, e.g. an export's SDR and HDR renditions of one photo, which
/// share their device stages) in one turn needing `bytes`. Outside batch work it is just `f`.
pub(crate) fn in_one_turn<R>(bytes: u64, vram: u64, stop: &dyn Fn() -> bool, f: impl FnOnce() -> R) -> R {
    if !is_batch() || IN_TURN.try_with(|c| c.get()).unwrap_or(false) {
        return f();
    }
    let turn = enter(bytes, vram, stop);
    /// Puts the thread's flag back however `f` ends.
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            let _ = IN_TURN.try_with(|c| c.set(self.0));
        }
    }
    let _restore = Restore(IN_TURN.with(|c| c.replace(turn.is_some())));
    let r = f();
    drop(turn);
    r
}

/// Run `f` with this thread's GPU renders counted as batch work.
pub(crate) fn as_batch<R>(f: impl FnOnce() -> R) -> R {
    /// Puts the thread's flag back however `f` ends (an export's panic is caught further up).
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            let _ = BATCH.try_with(|b| b.set(self.0));
        }
    }
    let _restore = Restore(BATCH.with(|b| b.replace(true)));
    f()
}

/// A turn taken without waiting ([`try_turn`]): this thread's renders run in it until it is
/// dropped, as inside [`in_one_turn`].
pub(crate) struct Turn {
    _permit: Option<Permit>,
    was: bool,
    /// The thread's [`on_helper`] flag before the turn.
    was_helper: bool,
}

impl Drop for Turn {
    fn drop(&mut self) {
        let (was, helper) = (self.was, self.was_helper);
        let _ = IN_TURN.try_with(|c| c.set(was));
        let _ = HELPER.try_with(|c| c.set(helper));
    }
}

/// [`in_one_turn`]'s turn if it is free now; `None` when it would have to wait for other
/// exports (the caller renders on the CPU instead: exports on the GPU and the processor at once).
/// Outside batch work, and inside a turn already taken, always a turn.
/// With `helper`, the turn is the second card's, and the thread's renders run there until it is
/// dropped.
pub(crate) fn try_turn(bytes: u64, vram: u64, helper: bool) -> Option<Turn> {
    let was = IN_TURN.try_with(|c| c.get()).unwrap_or(false);
    let was_helper = uses_helper();
    if !is_batch() || was {
        return Some(Turn { _permit: None, was, was_helper });
    }
    let _ = HELPER.try_with(|c| c.set(helper));
    let Some(permit) = enter(bytes, vram, &|| true) else {
        let _ = HELPER.try_with(|c| c.set(was_helper));
        return None;
    };
    let _ = IN_TURN.try_with(|c| c.set(true));
    Some(Turn { _permit: Some(permit), was, was_helper })
}

thread_local! {
    /// This thread's renders stay on the CPU ([`on_cpu`]).
    static CPU_ONLY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Run `f` with this thread's renders on the CPU (the GPU is busy with other photos).
pub(crate) fn on_cpu<R>(f: impl FnOnce() -> R) -> R {
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            let _ = CPU_ONLY.try_with(|c| c.set(self.0));
        }
    }
    let _restore = Restore(CPU_ONLY.with(|c| c.replace(true)));
    f()
}

/// Whether this thread's renders stay on the CPU ([`on_cpu`]).
pub(crate) fn cpu_only() -> bool {
    CPU_ONLY.try_with(|c| c.get()).unwrap_or(false)
}

/// Are this thread's renders batch work?
pub(crate) fn is_batch() -> bool {
    BATCH.try_with(|b| b.get()).unwrap_or(false)
}

/// Device memory a render of `out_px` output pixels from a source of `src_px` pixels holds at its
/// peak: the uploaded source (12 B/px), its framed copy and the stages it keeps while finishing
/// (measured: a 45 MP export peaks at ~[`OUT_BYTES_PER_PX`] B per output pixel).
pub(crate) fn job_bytes(src_px: usize, out_px: usize) -> u64 {
    (src_px as u64).saturating_mul(12).saturating_add((out_px as u64).saturating_mul(OUT_BYTES_PER_PX))
}

/// Device bytes per output pixel at a render's peak (see [`job_bytes`]).
pub(crate) const OUT_BYTES_PER_PX: u64 = 72;

/// The default budget of batch renders on a card with `vram` bytes: [`SHARE_PERCENT`] of it.
pub(crate) fn auto_limit(vram: u64) -> u64 {
    vram / 100 * SHARE_PERCENT
}

/// [`auto_limit`] for the second card ([`HELPER_SHARE_PERCENT`]: it draws no window).
pub(crate) fn auto_limit_helper(vram: u64) -> u64 {
    vram / 100 * HELPER_SHARE_PERCENT
}

/// Set the budget and the most jobs (`None`: automatic).
pub(crate) fn set_limits(bytes: Option<u64>, jobs: Option<usize>) {
    LIMIT.store(bytes.unwrap_or(0), Ordering::Relaxed);
    JOBS.store(jobs.unwrap_or(0), Ordering::Relaxed);
    CV.notify_all();
    HELPER_CV.notify_all();
}

/// The budget and the most jobs in force for the editing card with `vram` bytes.
pub(crate) fn limits(vram: u64) -> (u64, usize) {
    limits_of(false, vram)
}

/// The budget and the most jobs in force on the editing card (or, `helper`, the second card)
/// with `vram` bytes. With the interface tracked and a second card working, the editing card
/// takes one batch job at a time.
pub(crate) fn limits_of(helper: bool, vram: u64) -> (u64, usize) {
    let bytes = match LIMIT.load(Ordering::Relaxed) {
        0 if helper => auto_limit_helper(vram),
        0 => auto_limit(vram),
        b => b,
    };
    let jobs = match JOBS.load(Ordering::Relaxed) {
        0 => MAX_JOBS,
        j => j,
    };
    let jobs = if !helper && tracking() && crate::helper::current().is_some() { jobs.min(1) } else { jobs };
    (bytes, jobs)
}

// ------------------------------------------------------------ sharing photos between cards

/// How much batch work time a card's measured speed ([`rate`]) remembers (roughly; older work
/// counts half each time this much more has passed).
const RATE_WINDOW: Duration = Duration::from_secs(120);

/// Card `s`'s measured speed for batch work: device bytes of turns given back per second with
/// batch renders running (several at once included). `None` before it finished any.
fn rate(s: &State, now: Instant) -> Option<f64> {
    let busy = s.busy.saturating_add(s.busy_since.map_or(Duration::ZERO, |t| now.saturating_duration_since(t)));
    let secs = busy.as_secs_f64();
    (s.done > 0 && secs > 0.001).then(|| s.done as f64 / secs).filter(|r| r.is_finite() && *r > 0.0)
}

/// The cards to try, in order, for a batch photo needing `bytes` (`true`: the second card,
/// `false`: the editing card; `helper`: the second card is ready): the one that would finish it
/// soonest (its photos running now plus this one, at its measured speed, [`rate`]; both alike
/// until each has finished some), so both cards work in proportion to their speed and a build of
/// two photos at a time still uses both. On a tie, the second card. While the app tracks the
/// interface the second card takes a photo first whenever it has none: the editing card only gets
/// one when the second card is working already (and its gate hands it out only while the
/// interface is idle, one at a time).
pub(crate) fn order(bytes: u64, helper: bool) -> Vec<bool> {
    if !helper {
        return vec![false];
    }
    let now = Instant::now();
    let (h_held, h_jobs, h_rate) = {
        let s = state_of(true);
        (s.held, s.jobs, rate(&s, now))
    };
    let (p_held, p_rate) = {
        let s = state_of(false);
        (s.held, rate(&s, now))
    };
    if tracking() && h_jobs == 0 {
        return vec![true, false];
    }
    let (h_rate, p_rate) = match (h_rate, p_rate) {
        (Some(h), Some(p)) => (h, p),
        _ => (1.0, 1.0),
    };
    let finish = |held: u64, rate: f64| held.saturating_add(bytes) as f64 / rate;
    if finish(h_held, h_rate) <= finish(p_held, p_rate) { vec![true, false] } else { vec![false, true] }
}

/// The editing card's (or, `helper`, the second card's) measured speed for batch work (device
/// bytes per second; `None` before it finished any).
pub(crate) fn measured_rate(helper: bool) -> Option<f64> {
    rate(&state_of(helper), Instant::now())
}

/// Forget the cards' measured speeds (tests).
#[cfg(test)]
fn forget_rates() {
    for h in [false, true] {
        let mut s = state_of(h);
        s.done = 0;
        s.busy = Duration::ZERO;
        s.busy_since = None;
    }
}

/// Record a measured speed for a card (tests: fake cards).
#[cfg(test)]
fn set_rate(helper: bool, done: u64, busy: Duration) {
    let mut s = state_of(helper);
    s.done = done;
    s.busy = busy;
}

/// Batch renders wait at least `d` from now before they start (the window needs the GPU).
pub(crate) fn hold(d: Duration) {
    let until = Instant::now() + d;
    let mut s = state();
    s.hold_until = Some(s.hold_until.map_or(until, |t| t.max(until)));
}

/// A render's turn at the GPU; given back when dropped.
pub(crate) struct Permit {
    bytes: u64,
    batch: bool,
    /// Counted at the gate (not a render inside a turn already taken).
    held: bool,
    /// At the second card's gate.
    helper: bool,
}

impl Drop for Permit {
    fn drop(&mut self) {
        if !self.held {
            return;
        }
        let mut s = state_of(self.helper);
        if self.batch {
            s.held = s.held.saturating_sub(self.bytes);
            s.jobs = s.jobs.saturating_sub(1);
            s.done = s.done.saturating_add(self.bytes);
            if s.jobs == 0
                && let Some(t) = s.busy_since.take()
            {
                s.busy = s.busy.saturating_add(t.elapsed());
            }
            // (the recent past counts most: halve the record now and then)
            if s.busy > RATE_WINDOW {
                s.busy /= 2;
                s.done /= 2;
            }
        } else {
            s.interactive = s.interactive.saturating_sub(1);
            s.interactive_ended = Some(Instant::now());
        }
        drop(s);
        cv_of(self.helper).notify_all();
    }
}

/// Take a turn for a render needing `bytes` on a card with `vram` bytes: at once for interactive
/// work, else when it fits (see the module doc). `None` when `stop()` says to give up while
/// waiting (the app is closing): the render then runs on the CPU.
pub(crate) fn enter(bytes: u64, vram: u64, stop: &dyn Fn() -> bool) -> Option<Permit> {
    // (a render inside a turn already taken: nothing more to hold)
    let helper = uses_helper();
    if is_batch() && IN_TURN.try_with(|c| c.get()).unwrap_or(false) {
        return Some(Permit { bytes: 0, batch: false, held: false, helper });
    }
    let mut s = state_of(helper);
    if !is_batch() {
        s.interactive += 1;
        return Some(Permit { bytes, batch: false, held: true, helper });
    }
    let start = Instant::now();
    let mut waited = false;
    loop {
        let (limit, jobs) = limits_of(helper, vram);
        let now = Instant::now();
        let held_ok = s.jobs == 0 || s.held.saturating_add(bytes) <= limit;
        let jobs_ok = s.jobs < jobs.max(1);
        // the second card has no window: only the thumbnails sent there come first; the editing
        // card's batch work also waits for the interface (and a hold the window asked for)
        let window_ok = if helper { s.interactive == 0 } else { interface_quiet(&s, now) && s.hold_until.is_none_or(|t| now >= t) };
        if held_ok && jobs_ok && window_ok {
            if s.jobs == 0 {
                s.busy_since = Some(now);
            }
            s.held = s.held.saturating_add(bytes);
            s.jobs += 1;
            if waited {
                s.waiting = s.waiting.saturating_sub(1);
                WAITS.fetch_add(1, Ordering::Relaxed);
                WAITED_MS.fetch_add(start.elapsed().as_millis() as u64, Ordering::Relaxed);
            }
            return Some(Permit { bytes, batch: true, held: true, helper });
        }
        if !waited {
            waited = true;
            s.waiting += 1;
        }
        if stop() {
            s.waiting = s.waiting.saturating_sub(1);
            return None;
        }
        // (woken by a permit given back; the timeout re-checks a hold or an idle clock that ran
        // out, and `stop`)
        s = cv_of(helper).wait_timeout(s, Duration::from_millis(50)).map(|(g, _)| g).unwrap_or_else(|e| e.into_inner().0);
    }
}

/// The gate's state now: (batch renders running, device bytes they hold, renders waiting, batch
/// renders that waited so far and their total wait in ms).
pub(crate) fn usage() -> (usize, u64, usize, u64, u64) {
    let s = state();
    (s.jobs, s.held, s.waiting, WAITS.load(Ordering::Relaxed), WAITED_MS.load(Ordering::Relaxed))
}

/// The second card's gate now: (batch renders running, device bytes they hold, renders waiting).
pub(crate) fn helper_usage() -> (usize, u64, usize) {
    let s = state_of(true);
    (s.jobs, s.held, s.waiting)
}

/// Running batch renders on the editing card that paused for the interface so far, and their
/// total pause in ms.
pub(crate) fn pauses() -> (u64, u64) {
    (PAUSES.load(Ordering::Relaxed), PAUSED_MS.load(Ordering::Relaxed))
}

/// Between a batch render's submissions: wait for its previous one (`prev`) to finish first, so
/// the queue stays short for the window (see the module doc). `false`: the wait failed (timeout,
/// a lost device): the caller fails the render.
pub(crate) fn throttle(gpu: &crate::ctx::Gpu, prev: Option<wgpu::SubmissionIndex>) -> bool {
    if !is_batch() {
        return true;
    }
    // (`Gpu::poll`: a lost device is an error here, never a panic)
    if let Some(prev) = prev
        && gpu.poll(wgpu::PollType::Wait { submission_index: Some(prev), timeout: Some(THROTTLE_TIMEOUT) }).is_err()
    {
        return false;
    }
    // (the second card draws no window: nothing to yield to there)
    crate::helper::is_helper(gpu.generation) || yield_to_interaction(&crate::shutting_down)
}

/// A batch render on the editing card between two submissions: while the interface is busy (a
/// view renders, or it was touched within [`idle`], only when tracked), wait. `false` when
/// `stop()` says to give up (the app is closing). Interactive renders never come here.
pub(crate) fn yield_to_interaction(stop: &dyn Fn() -> bool) -> bool {
    if !is_batch() || !tracking() {
        return true;
    }
    let mut s = state_of(false);
    let start = Instant::now();
    let mut paused = false;
    while !interface_quiet(&s, Instant::now()) {
        if stop() {
            return false;
        }
        paused = true;
        s = CV.wait_timeout(s, Duration::from_millis(25)).map(|(g, _)| g).unwrap_or_else(|e| e.into_inner().0);
    }
    if paused {
        PAUSES.fetch_add(1, Ordering::Relaxed);
        PAUSED_MS.fetch_add(start.elapsed().as_millis() as u64, Ordering::Relaxed);
    }
    true
}

/// Longest a batch render waits for its previous submission before it gives the GPU up.
const THROTTLE_TIMEOUT: Duration = Duration::from_secs(60);

#[cfg(test)]
mod tests {
    use super::*;

    /// The gate's statics are shared: tests that use them take turns.
    static SERIAL: Mutex<()> = Mutex::new(());

    #[test]
    fn job_memory_grows_with_the_pixels_and_never_overflows() {
        assert_eq!(job_bytes(0, 0), 0);
        assert!(job_bytes(45_000_000, 45_000_000) > 3 << 30);
        assert!(job_bytes(45_000_000, 2_000_000) < job_bytes(45_000_000, 45_000_000));
        assert_eq!(job_bytes(usize::MAX, usize::MAX), u64::MAX);
        assert_eq!(auto_limit(12 << 30), (12u64 << 30) / 100 * SHARE_PERCENT);
        // the second card draws no window: a bigger share
        assert!(auto_limit_helper(12 << 30) > auto_limit(12 << 30));
    }

    #[test]
    fn batch_renders_share_the_budget_and_one_always_runs() {
        let _s = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        set_limits(Some(10 << 30), Some(3));
        let never = || false;
        as_batch(|| {
            // bigger than the budget: alone, it still runs
            let big = enter(20 << 30, 0, &never).unwrap();
            assert_eq!(usage().0, 1);
            drop(big);
            let a = enter(4 << 30, 0, &never).unwrap();
            let b = enter(4 << 30, 0, &never).unwrap();
            assert_eq!(usage().1, 8 << 30);
            // a third one doesn't fit: it waits until one is given back
            let started = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let flag = started.clone();
            let t = std::thread::spawn(move || {
                as_batch(|| {
                    let p = enter(4 << 30, 0, &|| false);
                    flag.store(true, Ordering::SeqCst);
                    drop(p);
                })
            });
            std::thread::sleep(Duration::from_millis(250));
            assert!(!started.load(Ordering::SeqCst), "over the budget: waits");
            drop(a);
            t.join().unwrap();
            assert!(started.load(Ordering::SeqCst));
            drop(b);
        });
        assert_eq!(usage().0, 0);
        set_limits(None, None);
    }

    #[test]
    fn the_window_comes_first() {
        let _s = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        set_limits(Some(10 << 30), None);
        // an interactive render never waits, and holds batch work back while it runs
        let view = enter(9 << 30, 0, &|| false).unwrap();
        let gave_up = as_batch(|| {
            let t = Instant::now();
            enter(1 << 20, 0, &|| t.elapsed() > Duration::from_millis(300)).is_none()
        });
        assert!(gave_up, "a batch render waits while the view renders");
        drop(view);
        assert!(as_batch(|| enter(1 << 20, 0, &|| false)).is_some());
        // a hold asked for by the window
        hold(Duration::from_millis(300));
        let t = Instant::now();
        let p = as_batch(|| enter(1 << 20, 0, &|| false));
        assert!(p.is_some() && t.elapsed() >= Duration::from_millis(250), "waited for the hold: {:?}", t.elapsed());
        drop(p);
        set_limits(None, None);
    }

    /// Renders inside one turn don't queue again: a gain map's two renditions run even with room
    /// for one job only, and the turn is given back after.
    #[test]
    fn renders_in_one_turn_hold_it_once() {
        let _s = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        set_limits(Some(1 << 30), Some(1));
        as_batch(|| {
            in_one_turn(1 << 20, 0, &|| false, || {
                assert_eq!(usage().0, 1);
                let a = enter(1 << 20, 0, &|| true).expect("inside the turn");
                let b = enter(1 << 20, 0, &|| true).expect("inside the turn");
                assert_eq!(usage().0, 1, "not counted again");
                drop((a, b));
            });
        });
        assert_eq!(usage().0, 0);
        set_limits(None, None);
    }

    #[test]
    fn a_turn_is_taken_only_when_free() {
        let _s = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        set_limits(Some(10 << 30), Some(1));
        as_batch(|| {
            let t = try_turn(1 << 20, 0, false).expect("free");
            assert_eq!(usage().0, 1);
            // the renders inside it don't queue again
            assert!(enter(1 << 20, 0, &|| true).is_some());
            assert_eq!(usage().0, 1);
            // another thread's photo finds it taken: no waiting
            let other = std::thread::spawn(|| as_batch(|| try_turn(1 << 20, 0, false).is_none())).join().unwrap();
            assert!(other, "taken: the other photo renders on the CPU");
            drop(t);
            assert_eq!(usage().0, 0);
            assert!(!IN_TURN.with(|c| c.get()));
        });
        set_limits(None, None);
    }

    /// The second card's batch renders have a gate of their own: a view rendering on the editing
    /// card doesn't hold them back, and their memory isn't the editing card's.
    #[test]
    fn the_second_card_has_its_own_gate() {
        let _s = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        set_limits(Some(10 << 30), Some(1));
        let view = enter(9 << 30, 0, &|| false).unwrap();
        let on_edit = as_batch(|| enter(1 << 20, 0, &|| true));
        assert!(on_edit.is_none(), "the editing card's batch waits for the view");
        let on_helper = as_batch(|| on_helper(|| enter(1 << 20, 0, &|| true)));
        assert!(on_helper.is_some(), "the second card's doesn't");
        assert_eq!(helper_usage().0, 1);
        assert_eq!(usage().0, 0);
        // a turn taken there keeps the thread's renders there until it is given back
        drop(on_helper);
        as_batch(|| {
            let t = try_turn(1 << 20, 0, true).expect("free");
            assert!(uses_helper() && in_turn());
            drop(t);
            assert!(!uses_helper() && !in_turn());
        });
        drop(view);
        assert_eq!(helper_usage().0, 0);
        set_limits(None, None);
    }

    /// Take a permit for a batch photo on the card [`order`] picks first that has room (as
    /// `batch_turn_any` does with the real cards).
    fn take(bytes: u64) -> Option<(bool, Permit)> {
        order(bytes, true).into_iter().find_map(|h| {
            let p = if h { on_helper(|| enter(bytes, 0, &|| true)) } else { enter(bytes, 0, &|| true) };
            p.map(|p| (h, p))
        })
    }

    /// Two fake cards: a build of two photos at a time uses both while the interface is idle,
    /// only the second card while it is in use, and photos go to the cards in proportion to their
    /// speed (every photo going to the second card while it had room made two cards slower than
    /// the faster one alone).
    #[test]
    fn photos_go_to_both_cards_in_proportion_to_their_speed() {
        let _s = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        set_limits(Some(1 << 40), Some(4));
        forget_rates();
        let mb = 1u64 << 20;
        const H: bool = true;
        const P: bool = false;
        as_batch(|| {
            // nothing measured yet, untracked (the CLI): the first photo to the second card, the
            // second to the editing card
            let a = take(mb).unwrap();
            let b = take(mb).unwrap();
            assert_eq!((a.0, b.0), (H, P), "two lanes: one photo on each card");
            drop((a, b));

            // the app, interface idle: the second card first, then the editing card
            forget_rates();
            set_idle(Duration::from_millis(200));
            track(true);
            note_interaction();
            std::thread::sleep(Duration::from_millis(250));
            assert!(!interface_busy());
            let a = take(mb).unwrap();
            let b = take(mb).unwrap();
            assert_eq!((a.0, b.0), (H, P), "idle: both cards work");
            drop(b);
            // in use: the editing card takes nothing, the second card everything
            note_interaction();
            let c = take(mb).unwrap();
            let d = take(mb).unwrap();
            assert_eq!((c.0, d.0), (H, H), "the interface is in use: only the second card");
            drop((a, c, d));
            track(false);
            set_idle(Duration::from_millis(1000));

            // measured: the editing card twice as fast: two photos there for each on the second
            forget_rates();
            set_rate(P, 2000 * mb, Duration::from_secs(1));
            set_rate(H, 1000 * mb, Duration::from_secs(1));
            let held: Vec<_> = (0..6).map(|_| take(mb).unwrap()).collect();
            let on_p = held.iter().filter(|(h, _)| *h == P).count();
            assert_eq!(on_p, 4, "4 of 6 on the faster card: {:?}", held.iter().map(|h| h.0).collect::<Vec<_>>());
            assert_eq!(held.first().map(|h| h.0), Some(P), "alone, a photo goes to the faster card");
            drop(held);
        });
        // without a second card: the editing card only
        assert_eq!(order(1, false), vec![P]);
        forget_rates();
        set_limits(None, None);
    }

    /// Finished batch work measures the card's speed; it stays finite and positive.
    #[test]
    fn a_cards_speed_is_measured_from_its_batch_work() {
        let _s = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        forget_rates();
        assert!(measured_rate(true).is_none(), "nothing done yet");
        as_batch(|| {
            let p = on_helper(|| enter(64 << 20, 0, &|| false)).unwrap();
            std::thread::sleep(Duration::from_millis(20));
            drop(p);
        });
        let r = measured_rate(true).expect("measured");
        assert!(r.is_finite() && r > 0.0 && r < (64u64 << 20) as f64 / 0.019, "{r}");
        forget_rates();
    }

    /// With the interface tracked, batch work on the editing card waits until nothing was touched
    /// for the idle time, a running batch render pauses between submissions the moment
    /// interaction starts and goes on once it is idle again. The second card never waits.
    #[test]
    fn the_editing_card_yields_to_interaction() {
        let _s = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        set_idle(Duration::from_millis(300));
        track(true);
        note_interaction();
        assert!(interface_busy());
        as_batch(|| {
            // just touched: no turn on the editing card, one at once on the second card
            assert!(try_turn(1 << 20, 0, false).is_none(), "touched within the idle time: the editing card is the interface's");
            assert!(try_turn(1 << 20, 0, true).is_some());
            // waiting for it: granted once idle
            let t = Instant::now();
            let p = enter(1 << 20, 0, &|| false);
            assert!(p.is_some() && t.elapsed() >= Duration::from_millis(200), "waited for the idle time: {:?}", t.elapsed());
            drop(p);
            // a running batch render pauses between submissions while the interface is busy
            note_interaction();
            let before = pauses().0;
            let t = Instant::now();
            assert!(yield_to_interaction(&|| false));
            assert!(t.elapsed() >= Duration::from_millis(200), "paused until idle: {:?}", t.elapsed());
            assert!(pauses().0 > before, "the pause is counted");
            // idle: no pause
            let t = Instant::now();
            assert!(yield_to_interaction(&|| false));
            assert!(t.elapsed() < Duration::from_millis(100));
            // the app closing ends a pause
            note_interaction();
            assert!(!yield_to_interaction(&|| true));
        });
        // views and the untracked default never pause
        assert!(yield_to_interaction(&|| false), "not batch work");
        track(false);
        note_interaction();
        assert!(!interface_busy(), "untracked: only running views count");
        assert!(as_batch(|| try_turn(1 << 20, 0, false)).is_some());
        set_idle(Duration::from_millis(1000));
    }

    /// With a second card taking batch work, the editing card (the window's) waits far longer
    /// for the interface to be idle: between two slider drags an export or a 1:1 preview build
    /// must not take it back (CF-ZOOM-FREEZE 2026-11-10, P2 / P3).
    #[test]
    fn with_a_second_card_the_window_card_waits_longer_for_batch_work() {
        let _s = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        set_idle(Duration::from_millis(1000));
        assert_eq!(editing_idle(false), Duration::from_millis(1000), "one card: the usual idle time");
        assert!(editing_idle(true) >= Duration::from_secs(5), "{:?}", editing_idle(true));
        set_helper_idle(Duration::from_millis(500));
        assert_eq!(editing_idle(true), Duration::from_millis(1000), "never shorter than the usual idle time");
        set_helper_idle(Duration::from_millis(8000));
        assert_eq!(editing_idle(true), Duration::from_millis(8000));
    }

    #[test]
    fn renders_on_the_cpu_stay_there_inside_and_not_after() {
        assert!(!cpu_only());
        on_cpu(|| assert!(cpu_only()));
        assert!(!cpu_only());
    }

    #[test]
    fn a_closing_app_stops_the_wait() {
        let _s = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        set_limits(Some(1 << 20), Some(1));
        as_batch(|| {
            let held = enter(1 << 20, 0, &|| false).unwrap();
            assert!(enter(1 << 20, 0, &|| true).is_none());
            drop(held);
        });
        set_limits(None, None);
    }
}
