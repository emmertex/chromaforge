//! Worker threads that leave the desktop room (TEST-NOTES 2026-10-16 item 15: finishing a patch
//! outline kept 10–21 cores busy for over a minute and the mouse crawled across the whole screen).
//!
//! The shared rayon pool (every render's parallel stages) gets two cores fewer than the machine
//! has, and its threads and the render workers run below normal priority: the window, the mouse and
//! other apps always come first, and the app takes what is left.

/// Rayon worker threads on a machine with `cores` logical cores: all but two (all but one from
/// three to eight cores; one on one or two).
pub fn workers_for(cores: usize) -> usize {
    match cores {
        0..=2 => 1,
        3..=8 => cores - 1,
        _ => cores - 2,
    }
}

/// Set the shared pool up and lower the render workers' priority. Call once, before any render.
pub fn install() {
    let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    let n = workers_for(cores);
    let built = rayon::ThreadPoolBuilder::new()
        .num_threads(n)
        .thread_name(|i| format!("lc-rayon-{i}"))
        .start_handler(|_| {
            lcc_sysmem::lower_thread_priority();
        })
        .build_global();
    match built {
        Ok(()) => log::info!("workers: {n} of {cores} cores, below normal priority"),
        // already built (a test, or something used rayon first): it keeps its threads
        Err(e) => log::warn!("workers: the shared pool was already set up ({e})"),
    }
    lcc_preview::pool::set_worker_start(|| {
        lcc_sysmem::lower_thread_priority();
    });
}

#[cfg(test)]
mod tests {
    use super::workers_for;

    #[test]
    fn a_core_or_two_stays_free() {
        assert_eq!(workers_for(0), 1);
        assert_eq!(workers_for(1), 1);
        assert_eq!(workers_for(2), 1);
        assert_eq!(workers_for(4), 3);
        assert_eq!(workers_for(8), 7);
        assert_eq!(workers_for(16), 14);
        assert_eq!(workers_for(32), 30);
    }
}
