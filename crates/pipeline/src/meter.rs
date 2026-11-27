//! A stage meter for benchmarks (`cargo xtask bench`'s export probe, #1377): off unless a benchmark switches it on
//! ([`set_on`]); while on, [`add`] sums the time each named stage took, over every thread, so photos side by side
//! add up ("stage-seconds", not wall time). [`take`] hands the sums over and starts again.
//!
//! Off, recording costs one relaxed atomic load: callers read the clock only after [`on`] said yes (never
//! otherwise: `Instant::now` panics on wasm32).

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

static ON: AtomicBool = AtomicBool::new(false);
static SUMS: Mutex<BTreeMap<String, Stage>> = Mutex::new(BTreeMap::new());

/// One stage's total: the time all its runs took together, and how many runs.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Stage {
    pub total: Duration,
    pub runs: u64,
}

/// Switch the meter on or off (off keeps the sums until [`take`]).
pub fn set_on(on: bool) {
    ON.store(on, Ordering::Relaxed);
}

/// Whether the meter records.
pub fn on() -> bool {
    ON.load(Ordering::Relaxed)
}

/// Add one run of `stage` that took `d` (nothing while the meter is off).
pub fn add(stage: &str, d: Duration) {
    if !on() {
        return;
    }
    let mut sums = SUMS.lock().unwrap_or_else(PoisonError::into_inner);
    let s = sums.entry(stage.to_string()).or_default();
    s.total = s.total.saturating_add(d);
    s.runs = s.runs.saturating_add(1);
}

/// A clock to time a stage with, `None` while the meter is off (pass it to [`stop`]).
pub fn start() -> Option<Instant> {
    on().then(Instant::now)
}

/// Record `stage` as the time since `started` ([`start`]); nothing when it is `None`.
pub fn stop(stage: &str, started: Option<Instant>) {
    if let Some(t) = started {
        add(stage, t.elapsed());
    }
}

/// Run `f` as one run of `stage`.
pub fn time<R>(stage: &str, f: impl FnOnce() -> R) -> R {
    let t = start();
    let r = f();
    stop(stage, t);
    r
}

/// The sums so far, by stage name, and start again from zero.
pub fn take() -> BTreeMap<String, Stage> {
    std::mem::take(&mut *SUMS.lock().unwrap_or_else(PoisonError::into_inner))
}

#[cfg(test)]
mod tests {
    use super::*;

    // (one test: the meter is process-wide)
    #[test]
    fn records_only_while_on_and_take_starts_again() {
        set_on(false);
        let _ = take();
        add("off", Duration::from_millis(5));
        assert_eq!(time("off too", || 7), 7);
        assert!(take().is_empty(), "nothing recorded while off");
        set_on(true);
        add("decode", Duration::from_millis(5));
        add("decode", Duration::from_millis(7));
        let t = start();
        assert!(t.is_some());
        stop("encode", t);
        stop("never", None);
        let sums = take();
        set_on(false);
        assert_eq!(sums["decode"], Stage { total: Duration::from_millis(12), runs: 2 });
        assert_eq!(sums["encode"].runs, 1);
        assert!(!sums.contains_key("never"));
        assert!(take().is_empty(), "take starts again");
    }
}
