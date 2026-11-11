//! Device memory shared by concurrent renders (upstream issue #1764).
//!
//! Each render asks for room for what it is expected to hold on the device and waits while the
//! renders in flight leave too little; a render always gets in when nothing else is in flight,
//! however large it is (the device's own limits still apply). wgpu reports no memory size, so the
//! budget starts from the largest buffer the device allows and adapts to the device it meets:
//! it is halved when a render runs out of device memory and grows back a step with each render
//! that succeeds (the additive-increase / multiplicative-decrease rule of TCP congestion control).
//! A 2 GB card settles near what it holds instead of failing every render past its memory.
//! Renders get in in the order they asked (a fair semaphore): a large preview is not passed over
//! for ever by a stream of thumbnails that each fit.
//!
//! **The window's renders never wait for batch work** (exports, Build Previews, the background
//! preparation): an interactive render is measured against the other interactive renders only,
//! and goes ahead of every batch render still waiting. A batch render keeps its room for its whole
//! render, also while it pauses between submissions for the interface
//! (`gate::yield_to_interaction`); had the loupe's render waited for that room, each would have
//! waited for the other. On a 45 MP raw (2.9 GB each under a 3 GB budget) the loupe got no render
//! for 30 s and more while 1:1 previews built (CF-ZOOM-FREEZE 2026-11-10). A batch render waits for
//! every render in flight and for every interactive render waiting.

use std::collections::VecDeque;
use std::sync::{Condvar, Mutex, PoisonError};
use std::time::Duration;

/// The budget never shrinks below this: one render always fits anyway, so this only bounds how
/// far a run of failures can push it.
const FLOOR: u64 = 256 << 20;

/// Device memory a render is expected to hold: the source, two working copies and the output,
/// as float RGBA, at the source's size.
pub(crate) fn footprint(width: usize, height: usize) -> u64 {
    (width as u64).saturating_mul(height as u64).saturating_mul(16 * 4)
}

/// What a render of a `width` × `height` source is expected to hold: [`footprint`], or for a
/// `window` (a zoomed view's part) the source once as float RGBA (the copy every window of the
/// photo shares) plus the window's own working copies.
pub(crate) fn need(width: usize, height: usize, window: Option<(usize, usize)>) -> u64 {
    match window {
        Some((w, h)) => (width as u64).saturating_mul(height as u64).saturating_mul(16).saturating_add(footprint(w, h)).min(footprint(width, height)),
        None => footprint(width, height),
    }
}

/// Where the budget starts: 1.5 × the largest buffer the device allows (about the device's
/// memory on small cards), at least 1 GiB.
pub(crate) fn initial(max_buffer: u64) -> u64 {
    (max_buffer / 2).saturating_mul(3).max(1 << 30)
}

/// Renders in flight or waiting of one kind (interactive or batch).
#[derive(Default)]
struct Kind {
    /// Bytes held by the renders in flight.
    used: u64,
    /// Renders in flight.
    running: u32,
    /// Tickets of the renders waiting, in the order they asked.
    queue: VecDeque<u64>,
}

struct State {
    interactive: Kind,
    batch: Kind,
    /// Bytes renders may hold together now.
    limit: u64,
    /// What the budget grows back to.
    ceiling: u64,
    next_ticket: u64,
}

impl State {
    fn kind(&mut self, batch: bool) -> &mut Kind {
        if batch { &mut self.batch } else { &mut self.interactive }
    }

    /// Whether the render waiting with `ticket` may start now (see the module doc).
    fn admits(&self, ticket: u64, bytes: u64, alone: bool, batch: bool) -> bool {
        let (first, running, used) = if batch {
            (
                self.interactive.queue.is_empty() && self.batch.queue.front() == Some(&ticket),
                self.interactive.running.saturating_add(self.batch.running),
                self.interactive.used.saturating_add(self.batch.used),
            )
        } else {
            (self.interactive.queue.front() == Some(&ticket), self.interactive.running, self.interactive.used)
        };
        first && (running == 0 || (!alone && used.saturating_add(bytes) <= self.limit))
    }
}

pub(crate) struct Budget {
    state: Mutex<State>,
    room: Condvar,
}

/// Room on the device for one render, released when dropped.
pub(crate) struct Permit<'a> {
    budget: &'a Budget,
    bytes: u64,
    batch: bool,
}

impl Budget {
    pub fn new(ceiling: u64) -> Budget {
        Budget {
            state: Mutex::new(State { interactive: Kind::default(), batch: Kind::default(), limit: ceiling, ceiling, next_ticket: 0 }),
            room: Condvar::new(),
        }
    }

    /// Wait for room for `bytes`; `alone` waits until nothing else is in flight (the retry of a
    /// render that ran out of memory; for an interactive render, nothing else interactive).
    /// `batch`: an export's or a background render (see the module doc). `None` when `stop` turns
    /// true while waiting (the process is ending): the caller renders on the CPU.
    pub fn admit(&self, bytes: u64, alone: bool, batch: bool, stop: impl Fn() -> bool) -> Option<Permit<'_>> {
        let mut st = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let ticket = st.next_ticket;
        st.next_ticket = st.next_ticket.wrapping_add(1);
        st.kind(batch).queue.push_back(ticket);
        while !st.admits(ticket, bytes, alone, batch) {
            if stop() {
                st.kind(batch).queue.retain(|t| *t != ticket);
                // the render behind may be the next to fit
                self.room.notify_all();
                return None;
            }
            st = self.room.wait_timeout(st, Duration::from_millis(100)).unwrap_or_else(PoisonError::into_inner).0;
        }
        let k = st.kind(batch);
        k.queue.pop_front();
        k.used = k.used.saturating_add(bytes);
        k.running = k.running.saturating_add(1);
        // the next in line may fit beside this one
        self.room.notify_all();
        Some(Permit { budget: self, bytes, batch })
    }

    /// A render ran out of device memory: halve the budget (not below what one render needs).
    pub fn out_of_memory(&self) {
        let mut st = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        st.limit = (st.limit / 2).max(FLOOR);
    }

    /// A render succeeded: grow the budget back by a sixteenth of its ceiling.
    pub fn succeeded(&self) {
        let mut st = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        st.limit = st.limit.saturating_add(st.ceiling / 16).min(st.ceiling);
    }

    /// (bytes in flight, renders in flight, current budget).
    #[cfg(test)]
    fn snapshot(&self) -> (u64, u32, u64) {
        let st = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        (st.interactive.used.saturating_add(st.batch.used), st.interactive.running.saturating_add(st.batch.running), st.limit)
    }
}

impl Drop for Permit<'_> {
    fn drop(&mut self) {
        let mut st = self.budget.state.lock().unwrap_or_else(PoisonError::into_inner);
        let k = st.kind(self.batch);
        k.used = k.used.saturating_sub(self.bytes);
        k.running = k.running.saturating_sub(1);
        self.budget.room.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    const GIB: u64 = 1 << 30;

    #[test]
    fn renders_wait_for_room_and_a_lone_render_always_gets_in() {
        let b = Arc::new(Budget::new(4 * GIB));
        let first = b.admit(3 * GIB, false, false, || false).unwrap();
        let waiter = {
            let b = Arc::clone(&b);
            std::thread::spawn(move || {
                let p = b.admit(2 * GIB, false, false, || false).unwrap();
                let seen = b.snapshot();
                drop(p);
                seen
            })
        };
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(b.snapshot().1, 1, "3 + 2 GiB is over the 4 GiB budget: the second render waits");
        drop(first);
        assert_eq!(waiter.join().unwrap().0, 2 * GIB);
        assert_eq!(b.snapshot().0, 0);
        // nothing in flight: a render bigger than the whole budget still runs
        let big = b.admit(10 * GIB, false, false, || false).unwrap();
        assert_eq!(b.snapshot().0, 10 * GIB);
        drop(big);
    }

    #[test]
    fn a_retry_alone_waits_for_every_other_render() {
        let b = Arc::new(Budget::new(4 * GIB));
        let small = b.admit(GIB, false, false, || false).unwrap();
        let retry = {
            let b = Arc::clone(&b);
            std::thread::spawn(move || b.admit(GIB, true, false, || false).map(|_| ()).is_some())
        };
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(b.snapshot().1, 1, "the retry must not run beside another render, even with room");
        drop(small);
        assert!(retry.join().unwrap());
    }

    #[test]
    fn renders_get_in_in_the_order_they_asked() {
        let b = Arc::new(Budget::new(4 * GIB));
        let held = b.admit(3 * GIB, false, false, || false).unwrap();
        // a big render waits for room...
        let big = {
            let b = Arc::clone(&b);
            std::thread::spawn(move || {
                let p = b.admit(3 * GIB, false, false, || false).unwrap();
                let seen = b.snapshot().1;
                drop(p);
                seen
            })
        };
        std::thread::sleep(Duration::from_millis(50));
        // ...and a small one that would fit now queues behind it instead of passing it
        let small = {
            let b = Arc::clone(&b);
            std::thread::spawn(move || b.admit(GIB / 2, false, false, || false).map(|_| ()).is_some())
        };
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(b.snapshot().1, 1, "the small render must not pass the big one waiting before it");
        drop(held);
        assert!(big.join().unwrap() >= 1);
        assert!(small.join().unwrap());
    }

    #[test]
    fn waiting_ends_when_the_process_stops() {
        let b = Budget::new(GIB);
        let _held = b.admit(GIB, false, false, || false).unwrap();
        assert!(b.admit(GIB, false, false, || true).is_none());
        assert_eq!(b.snapshot().1, 1, "a refused render holds nothing");
    }

    #[test]
    fn the_budget_halves_on_out_of_memory_and_grows_back_on_success() {
        let b = Budget::new(4 * GIB);
        b.out_of_memory();
        assert_eq!(b.snapshot().2, 2 * GIB);
        for _ in 0..10 {
            b.out_of_memory();
        }
        assert_eq!(b.snapshot().2, FLOOR, "never below the floor");
        for _ in 0..100 {
            b.succeeded();
        }
        assert_eq!(b.snapshot().2, 4 * GIB, "never above where it started");
    }

    #[test]
    fn sizes_are_saturating_and_sized_for_big_raws() {
        // a 60-megapixel raw is about 3.8 GB: two never run together under a 4 GiB budget
        let one = footprint(9504, 6336);
        assert!(one > 3 * GIB && one < 4 * GIB, "{one}");
        assert_eq!(footprint(usize::MAX, usize::MAX), u64::MAX);
        assert_eq!(initial(2 * GIB), 3 * GIB);
        assert_eq!(initial(128 << 20), GIB, "small limits start at 1 GiB");
    }

    #[test]
    fn a_window_of_a_big_raw_needs_far_less_than_the_whole_render() {
        let whole = need(8192, 5464, None);
        let tile = need(8192, 5464, Some((1300, 1300)));
        assert_eq!(whole, footprint(8192, 5464));
        // three 1:1 tiles of a 45 MP raw fit a 3 GB budget together (one did before)
        assert!(tile.saturating_mul(3) <= 3 * GIB, "{tile}");
        assert!(need(100, 100, Some((5000, 5000))) <= footprint(100, 100), "never more than the whole render");
        assert_eq!(need(usize::MAX, usize::MAX, Some((usize::MAX, 1))), u64::MAX);
    }

    #[test]
    fn an_interactive_render_never_waits_for_a_batch_render() {
        // a 45 MP raw: 2.9 GB each under a 3 GB budget
        let b = Arc::new(Budget::new(3 * GIB));
        let big = footprint(8192, 5464);
        let export = b.admit(big, false, true, || false).unwrap();
        // the batch render pauses for the interface while it keeps its room: the view's render
        // gets in at once, though both together are over the budget (before: each waited for the
        // other, for ever)
        let (tx, rx) = std::sync::mpsc::channel();
        let view = {
            let b = Arc::clone(&b);
            std::thread::spawn(move || {
                let got = b.admit(big, false, false, || false).is_some();
                let _ = tx.send(());
                got
            })
        };
        assert!(rx.recv_timeout(Duration::from_secs(5)).is_ok(), "the view's render waited for the batch render's room");
        assert!(view.join().unwrap());
        drop(export);
    }

    #[test]
    fn interactive_renders_go_ahead_of_waiting_batch_renders() {
        let b = Arc::new(Budget::new(3 * GIB));
        let big = footprint(8192, 5464);
        let running = b.admit(big, false, true, || false).unwrap();
        // a second batch render waits for room…
        let order = Arc::new(Mutex::new(Vec::new()));
        let batch = {
            let (b, order) = (Arc::clone(&b), Arc::clone(&order));
            std::thread::spawn(move || {
                let p = b.admit(big, false, true, || false);
                order.lock().unwrap().push("batch");
                drop(p);
            })
        };
        std::thread::sleep(Duration::from_millis(50));
        // …an interactive one asking later starts first, and the batch render waits while it runs
        let view = b.admit(big, false, false, || false).unwrap();
        order.lock().unwrap().push("view");
        drop(running);
        std::thread::sleep(Duration::from_millis(250));
        assert_eq!(*order.lock().unwrap(), vec!["view"], "a batch render started beside the view's");
        drop(view);
        batch.join().unwrap();
        assert_eq!(*order.lock().unwrap(), vec!["view", "batch"]);
    }

    #[test]
    fn interactive_renders_still_share_the_budget_among_themselves() {
        let b = Arc::new(Budget::new(4 * GIB));
        let _batch = b.admit(3 * GIB, false, true, || false).unwrap();
        let first = b.admit(3 * GIB, false, false, || false).unwrap();
        let second = {
            let b = Arc::clone(&b);
            std::thread::spawn(move || b.admit(3 * GIB, false, false, || false).is_some())
        };
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(b.snapshot().1, 2, "3 + 3 GiB of views is over the budget: the second view waits");
        drop(first);
        assert!(second.join().unwrap());
    }
}
