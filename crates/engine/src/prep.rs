//! The pictures on screen come first: while a grid or filmstrip cell, or the loupe, still lacks
//! its first picture (not even the cheapest, a raw's embedded JPEG or a cached thumbnail), the
//! frontend says so every frame ([`pictures_missing`]) and background work that reads raws waits
//! ([`wait_for_pictures`]): eye finding, culling scans. The signal holds for a moment after the
//! last call, so a frontend that stops saying it (the pictures arrived, or it closed) never holds
//! anything back for long.
//!
//! Sources: own design (OneSlips, PERF-CULL-TIERS: "lowest and fastest resolution first, then the
//! next cheap one, then anything bigger").

use std::sync::atomic::{AtomicU64, Ordering};

/// How long one [`pictures_missing`] holds background work back (ms): a few frames.
const HOLD_MS: u64 = 150;

/// Until when (ms on [`clock_ms`]) something on screen has no picture yet.
static PICTURES_UNTIL: AtomicU64 = AtomicU64::new(0);

fn clock_ms() -> u64 {
    static START: std::sync::OnceLock<web_time::Instant> = std::sync::OnceLock::new();
    let start = START.get_or_init(web_time::Instant::now);
    u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX).saturating_add(1)
}

/// The frontend says a cell on screen or the loupe has no picture yet, its first one on its way:
/// background work waits until each has one. Say it every frame it is so; it holds for a moment.
pub fn pictures_missing() {
    PICTURES_UNTIL.store(clock_ms().saturating_add(HOLD_MS), Ordering::Relaxed);
}

/// Whether something on screen still lacks its first picture ([`pictures_missing`]).
pub fn pictures_busy() -> bool {
    clock_ms() < PICTURES_UNTIL.load(Ordering::Relaxed)
}

/// Wait while something on screen lacks its first picture (background work that reads raws).
/// `true`: `stop()` said so meanwhile. Waits at most `max`, so the photo on screen's own
/// background work (its eyes) still comes.
pub fn wait_for_pictures(stop: &dyn Fn() -> bool, max: std::time::Duration) -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let t0 = web_time::Instant::now();
        while pictures_busy() && t0.elapsed() < max {
            if stop() {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
    #[cfg(target_arch = "wasm32")]
    let _ = max;
    stop()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A picture missing on screen holds background work back until the frontend stops saying
    /// so, and never longer than the caller's bound; a stop request ends the wait.
    #[test]
    fn a_picture_missing_on_screen_holds_background_work_back_for_a_moment() {
        pictures_missing();
        assert!(pictures_busy());
        let t = web_time::Instant::now();
        assert!(!wait_for_pictures(&|| false, std::time::Duration::from_secs(5)), "eye finding waits for it");
        assert!(t.elapsed().as_millis() >= 100, "{:?}", t.elapsed());
        assert!(t.elapsed().as_secs() < 5, "the hold ends once the frontend stops saying it");
        assert!(!pictures_busy());
        pictures_missing();
        let t = web_time::Instant::now();
        assert!(!wait_for_pictures(&|| false, std::time::Duration::from_millis(30)));
        assert!(t.elapsed().as_millis() < 140, "bounded by the caller");
        pictures_missing();
        assert!(wait_for_pictures(&|| true, std::time::Duration::from_secs(5)), "a stop ends the wait");
    }
}
