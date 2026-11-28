//! How long the GPU had work: a meter for benchmarks (`cargo xtask bench`'s export probe, #1377), off by default.
//!
//! While metering ([`set_metering`]), each submission is counted in flight from [`submitted`] until wgpu reports it
//! done (`Queue::on_submitted_work_done`, which runs when the device is next polled); [`busy_time`] is the time at
//! least one submission was in flight. That is an upper bound on the time the device itself was working (a
//! submission waits in the queue behind others, and its callback can run a little after it finished, at the next
//! poll), not a hardware utilisation counter; on a machine whose exports keep the card fed, the two agree closely.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

static ON: AtomicBool = AtomicBool::new(false);
/// Holders of [`hold`]: they read differences of [`busy_time`], so metering for them never resets the total.
static HOLDERS: AtomicUsize = AtomicUsize::new(0);
static STATE: Mutex<State> = Mutex::new(State { in_flight: 0, since: None, busy: Duration::ZERO });

struct State {
    in_flight: usize,
    /// Since when at least one submission is in flight.
    since: Option<Instant>,
    /// Time with work in flight, before `since`.
    busy: Duration,
}

/// Start or stop metering. Starting resets the total.
pub fn set_metering(on: bool) {
    let mut s = STATE.lock().unwrap_or_else(PoisonError::into_inner);
    if on {
        s.busy = Duration::ZERO;
        s.since = (s.in_flight > 0).then(Instant::now);
    }
    ON.store(on, Ordering::Relaxed);
}

/// Whether submissions are metered.
pub fn metering() -> bool {
    ON.load(Ordering::Relaxed) || HOLDERS.load(Ordering::Relaxed) > 0
}

/// Meter submissions while the returned guard lives, without resetting the total: for a reader of how [`busy_time`]
/// changes over a stretch of work (an export batch sizing its lanes, #1377).
pub fn hold() -> Hold {
    HOLDERS.fetch_add(1, Ordering::Relaxed);
    Hold(())
}

/// Metering held on ([`hold`]).
pub struct Hold(());

impl Drop for Hold {
    fn drop(&mut self) {
        HOLDERS.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Time the GPU had work in flight since metering started.
pub fn busy_time() -> Duration {
    let s = STATE.lock().unwrap_or_else(PoisonError::into_inner);
    s.busy.saturating_add(s.since.map_or(Duration::ZERO, |t| t.elapsed()))
}

/// A submission was just made on `queue`: count it in flight until it is done.
pub(crate) fn submitted(queue: &wgpu::Queue) {
    if !metering() {
        return;
    }
    begin(Instant::now());
    queue.on_submitted_work_done(|| end(Instant::now()));
}

fn begin(now: Instant) {
    let mut s = STATE.lock().unwrap_or_else(PoisonError::into_inner);
    if s.in_flight == 0 {
        s.since = Some(now);
    }
    s.in_flight = s.in_flight.saturating_add(1);
}

fn end(now: Instant) {
    let mut s = STATE.lock().unwrap_or_else(PoisonError::into_inner);
    s.in_flight = s.in_flight.saturating_sub(1);
    if s.in_flight == 0
        && let Some(t) = s.since.take()
    {
        s.busy = s.busy.saturating_add(now.saturating_duration_since(t));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // (one test: the meter is process-wide)
    #[test]
    fn overlapping_submissions_count_once_and_idle_gaps_not_at_all() {
        set_metering(true);
        let t0 = Instant::now();
        let ms = |n| t0 + Duration::from_millis(n);
        begin(ms(0));
        begin(ms(5)); // overlaps the first
        end(ms(10));
        end(ms(20)); // 0..20 busy
        begin(ms(50)); // 20..50 idle
        end(ms(60)); // 50..60 busy
        assert_eq!(busy_time(), Duration::from_millis(30));
        end(ms(70)); // a stray callback never goes below zero or adds time
        assert_eq!(busy_time(), Duration::from_millis(30));
        set_metering(true);
        assert_eq!(busy_time(), Duration::ZERO, "starting again resets");
        set_metering(false);
    }
}
