//! Auto Mask strokes kept on the device between renders: each finished stroke's coverage (stamped,
//! then snapped to the photo's edges) in 16 bits, the GPU twin of
//! `lcc_pipeline::masks::auto_stroke`'s cache.
//!
//! A stroke is kept in the coordinates of the whole output frame, cut into [`BLOCK`]-pixel blocks
//! of it, so the windows of one frame share it: a zoomed view renders each of its tiles as a window
//! of the frame with a wide margin, and a dozen big strokes reach most of every tile. Kept per
//! window, the tiles worked out every stroke again and pushed each other's strokes out (a return to
//! 100 % found none of them). A block is kept only where the window that worked it out saw all the
//! pixels its refine reads, so it holds what any window of the frame works out there.
//!
//! What a stroke's coverage depends on: the stroke and its brush, the frame (its geometry and size)
//! and the pixels it reads, the frame's linear image ([`lcc_pipeline::Plan::frame_lin_key`]), on
//! the source photo (by identity) and device. Fit → 100 % with nothing edited takes every block of
//! the tiles from here.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::{Duration, Instant};

use lcc_raster::Rgb32f;

use crate::ctx::Buf;

/// The most device bytes of kept strokes (all cards) on a card whose memory isn't known: a dozen
/// strokes of a 490-pixel brush at 100 % of a 45 MP photo (each reaching about 4500 × 1700 pixels
/// of the frame, 15 MB in 16 bits, and the whole strokes of a window without margin beside them)
/// and the same strokes at Fit.
pub(crate) const STORE_BYTES: usize = 512 << 20;

/// The most a card with `vram` bytes keeps: an eighth of its memory, from [`STORE_BYTES`] to 1 GB
/// (two sets of a photo's strokes while one replaces the other).
pub(crate) fn store_bytes(vram: Option<u64>) -> usize {
    vram.map_or(STORE_BYTES, |v| usize::try_from(v / 8).unwrap_or(usize::MAX).clamp(STORE_BYTES, 1 << 30))
}

/// The side of the blocks of the frame strokes are kept in.
pub(crate) const BLOCK: usize = 512;

/// How long blocks of the view being rendered stay safe from each other: its tiles render one
/// after another, a few seconds apart at most; a block unused for longer belongs to a stroke that
/// was removed or changed.
const VIEW_KEEPS: Duration = Duration::from_secs(10);

/// A block of a finished stroke: its coverage over `rect` (x0, y0, x1, y1 of the whole frame;
/// exclusive ends), two 16-bit values per word (`pack2x16unorm`, pixel `2k` in the low half).
pub(crate) struct Kept {
    pub rect: (usize, usize, usize, usize),
    pub packed: Buf,
}

struct Entry {
    key: u64,
    /// The frame it belongs to ([`view`]).
    view: u64,
    /// The render pass that last used it.
    pass: u64,
    /// When a render last used it.
    used: Instant,
    /// Only batch renders used it (work made ahead in the background, `gate::is_batch`): those
    /// never evict what an interactive render keeps.
    batch: bool,
    /// A whole stroke over one window's plane ([`put_loose`]), not a block of the frame.
    loose: bool,
    /// The photo it was worked out on (its identity: a live `Weak` keeps the address from reuse).
    src: Weak<Rgb32f>,
    kept: Arc<Kept>,
}

static STORE: Mutex<VecDeque<Entry>> = Mutex::new(VecDeque::new());
static PASSES: AtomicU64 = AtomicU64::new(0);
thread_local! {
    /// Blocks this thread's renders took from the store ([`reused`]).
    static REUSED: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    /// Strokes this thread's renders refined ([`refined`]).
    static REFINED: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// A render refined (part of) a stroke: it wasn't all kept.
pub(crate) fn note_refined() {
    REFINED.with(|r| r.set(r.get().saturating_add(1)));
}

/// Strokes this thread's renders refined so far, whole or in part.
pub(crate) fn refined() -> u64 {
    REFINED.with(std::cell::Cell::get)
}

/// A new brush render's pass number. A pass never evicts the blocks it used itself.
pub(crate) fn pass() -> u64 {
    PASSES.fetch_add(1, Ordering::Relaxed).wrapping_add(1)
}

/// The key of the frame strokes are kept for: its linear image (`frame_lin_key`, which holds the
/// frame's geometry and size) on the device of generation `generation` (each device, the helper
/// card's too, has its own).
pub(crate) fn view(frame_lin_key: u64, generation: u64) -> u64 {
    hash((frame_lin_key, generation))
}

/// The key of a whole stroke `stroke` over the plane of one window (`plane`: its geometry and
/// linear-stage keys) of frame `view` ([`put_loose`]).
pub(crate) fn loose_key(view: u64, stroke: u64, plane: (u64, u64)) -> u64 {
    hash(("window", view, stroke, plane))
}

/// The key of block (`bx`, `by`) of stroke `stroke` (its own key) in frame `view`.
pub(crate) fn key(view: u64, stroke: u64, bx: usize, by: usize) -> u64 {
    hash((view, stroke, bx, by))
}

fn hash<T: std::hash::Hash>(v: T) -> u64 {
    use std::hash::{BuildHasher, BuildHasherDefault, DefaultHasher};
    BuildHasherDefault::<DefaultHasher>::default().hash_one(v)
}

fn bytes_of(k: &Kept) -> usize {
    k.packed.len.saturating_mul(4)
}

fn lock() -> std::sync::MutexGuard<'static, VecDeque<Entry>> {
    STORE.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The block kept under `key` for photo `src` on device generation `generation`, marked used by
/// `pass` (of a batch render: `batch`).
pub(crate) fn get(src: &Arc<Rgb32f>, key: u64, pass: u64, generation: u64, batch: bool) -> Option<Arc<Kept>> {
    let mut store = lock();
    let me = Arc::downgrade(src);
    let i = store.iter().position(|e| e.key == key && Weak::ptr_eq(&e.src, &me) && e.kept.packed.generation() == generation)?;
    let mut e = store.remove(i)?;
    e.pass = pass;
    e.used = Instant::now();
    e.batch &= batch;
    let kept = e.kept.clone();
    store.push_back(e);
    REUSED.with(|r| r.set(r.get().saturating_add(1)));
    Some(kept)
}

/// Keep `kept` under `key` (a block of frame `view`) for photo `src`, worked out by a batch render
/// (`batch`) or not: blocks of photos no longer open go first, then the least recently used ones
/// of other frames (those a batch render kept first; a batch render takes no others), then blocks
/// of this frame no render used for [`VIEW_KEEPS`]. Never one this `pass` used, nor one of this
/// frame used lately: the tiles of the view on screen don't push each other's strokes out (when
/// they outgrow the store, the blocks that fit stay and the rest aren't kept).
pub(crate) fn put(src: &Arc<Rgb32f>, key: u64, view: u64, pass: u64, batch: bool, cap: usize, kept: Arc<Kept>) {
    put_entry(src, key, (view, pass), batch, false, cap, kept);
}

/// Keep `kept`, a whole stroke over the plane of one window (`key`: [`loose_key`]), whose blocks
/// near the window's edges couldn't be kept (the window saw too little around them): a window
/// with no margin around it (a slider drag's, an export's) still finds its strokes on the next
/// render. These go before any block, and take the place of no block.
pub(crate) fn put_loose(src: &Arc<Rgb32f>, key: u64, view: u64, pass: u64, batch: bool, cap: usize, kept: Arc<Kept>) {
    put_entry(src, key, (view, pass), batch, true, cap, kept);
}

fn put_entry(src: &Arc<Rgb32f>, key: u64, (view, pass): (u64, u64), batch: bool, loose: bool, cap: usize, kept: Arc<Kept>) {
    let mut store = lock();
    store.retain(|e| e.src.strong_count() > 0);
    store.retain(|e| e.key != key);
    let now = Instant::now();
    keep(&mut store, Entry { key, view, pass, used: now, batch, loose, src: Arc::downgrade(src), kept }, cap, now);
}

fn keep(store: &mut VecDeque<Entry>, entry: Entry, cap: usize, now: Instant) {
    let mut total = store.iter().map(|e| bytes_of(&e.kept)).fold(bytes_of(&entry.kept), usize::saturating_add);
    while total > cap {
        let free = |e: &Entry| e.pass != entry.pass;
        let other = |e: &Entry| free(e) && e.view != entry.view;
        let stale = |e: &Entry| free(e) && e.view == entry.view && now.saturating_duration_since(e.used) >= VIEW_KEEPS;
        // whole strokes of single windows first; a whole stroke makes way for nothing else
        let loose = store.iter().position(|e| free(e) && e.loose);
        let pick = if entry.loose {
            loose
        } else {
            loose
                .or_else(|| store.iter().position(|e| other(e) && e.batch))
                .or_else(|| (!entry.batch).then(|| store.iter().position(other)).flatten())
                .or_else(|| store.iter().position(stale))
        };
        let Some(i) = pick else { break };
        let Some(old) = store.remove(i) else { break };
        total = total.saturating_sub(bytes_of(&old.kept));
    }
    if total <= cap {
        store.push_back(entry);
    }
}

/// Device bytes of the kept strokes.
pub(crate) fn bytes() -> usize {
    lock().iter().map(|e| bytes_of(&e.kept)).fold(0, usize::saturating_add)
}

/// Blocks this thread's renders took from the store so far (renders that found a stroke's block
/// worked out before).
pub(crate) fn reused() -> u64 {
    REUSED.with(std::cell::Cell::get)
}

/// Drop every kept stroke (the app idle, memory wanted).
pub(crate) fn clear() {
    let old = std::mem::take(&mut *lock());
    drop(old);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kept(words: usize) -> Arc<Kept> {
        Arc::new(Kept { rect: (0, 0, 0, 0), packed: Buf::detached(words) })
    }

    fn entry(src: &Arc<Rgb32f>, key: u64, view: u64, pass: u64, batch: bool, used: Instant) -> Entry {
        Entry { key, view, pass, used, batch, loose: false, src: Arc::downgrade(src), kept: kept(10) }
    }

    fn keys(store: &VecDeque<Entry>) -> Vec<u64> {
        store.iter().map(|e| e.key).collect()
    }

    /// The tiles of the view on screen (one frame, one render each) don't push each other's
    /// blocks out: what fits stays, and the next tiles' blocks aren't kept until those go stale.
    #[test]
    fn tiles_of_the_view_on_screen_never_evict_each_other() {
        let src = Arc::new(Rgb32f::new(1, 1));
        let now = Instant::now();
        let mut store = VecDeque::new();
        // a cap of 3 blocks; tile 1 keeps three, tile 2 tries two more
        for key in 0..3 {
            keep(&mut store, entry(&src, key, 7, 1, false, now), 3 * 40, now);
        }
        for key in 10..12 {
            keep(&mut store, entry(&src, key, 7, 2, false, now), 3 * 40, now);
        }
        assert_eq!(keys(&store), [0, 1, 2]);
        // another frame (Fit) makes way for this one's
        let mut store = VecDeque::new();
        keep(&mut store, entry(&src, 20, 8, 1, false, now), 3 * 40, now);
        for key in 0..3 {
            keep(&mut store, entry(&src, key, 7, 2, false, now), 3 * 40, now);
        }
        assert_eq!(keys(&store), [0, 1, 2]);
        // blocks of this frame nobody used for a while (a stroke removed) make way too
        let later = now + VIEW_KEEPS + Duration::from_secs(1);
        keep(&mut store, entry(&src, 30, 7, 3, false, later), 3 * 40, later);
        assert_eq!(keys(&store), [1, 2, 30]);
    }

    /// Batch renders (made ahead in the background) don't push the view's blocks out: they make
    /// way for each other only.
    #[test]
    fn blocks_made_ahead_never_evict_the_view_on_screen() {
        let src = Arc::new(Rgb32f::new(1, 1));
        let now = Instant::now();
        let mut store = VecDeque::new();
        for key in 0..3 {
            keep(&mut store, entry(&src, key, 7, 1, false, now), 3 * 40, now);
        }
        for key in 10..13 {
            keep(&mut store, entry(&src, key, 9, 2, true, now), 3 * 40, now);
        }
        assert_eq!(keys(&store), [0, 1, 2]);
        store.pop_back();
        keep(&mut store, entry(&src, 20, 9, 3, true, now), 3 * 40, now);
        keep(&mut store, entry(&src, 21, 10, 4, true, now), 3 * 40, now);
        assert_eq!(keys(&store), [0, 1, 21]);
        // an interactive render of another frame takes the batch block's place, then the oldest
        keep(&mut store, entry(&src, 30, 8, 5, false, now), 3 * 40, now);
        keep(&mut store, entry(&src, 31, 8, 5, false, now), 3 * 40, now);
        assert_eq!(keys(&store), [1, 30, 31]);
    }

    /// Whole strokes of single windows go before any block, and push none out.
    #[test]
    fn whole_strokes_of_one_window_make_way_for_blocks() {
        let src = Arc::new(Rgb32f::new(1, 1));
        let now = Instant::now();
        let mut store = VecDeque::new();
        let mut w = entry(&src, 1, 7, 1, false, now);
        w.loose = true;
        keep(&mut store, w, 3 * 40, now);
        for key in 10..12 {
            keep(&mut store, entry(&src, key, 7, 1, false, now), 3 * 40, now);
        }
        // a block of the next tile: the whole stroke goes, not a block
        keep(&mut store, entry(&src, 20, 7, 2, false, now), 3 * 40, now);
        assert_eq!(keys(&store), [10, 11, 20]);
        let mut w = entry(&src, 2, 7, 3, false, now);
        w.loose = true;
        keep(&mut store, w, 3 * 40, now);
        assert_eq!(keys(&store), [10, 11, 20], "a whole stroke takes no block's place");
    }

    #[test]
    fn a_block_is_found_only_for_its_photo_and_device() {
        let (a, b) = (Arc::new(Rgb32f::new(1, 1)), Arc::new(Rgb32f::new(1, 1)));
        let v = view(3, 0);
        let k = key(v, 1, 0, 0);
        put(&a, k, v, pass(), false, STORE_BYTES, kept(4));
        assert!(get(&a, k, pass(), 0, false).is_some());
        assert!(get(&b, k, pass(), 0, false).is_none(), "another photo");
        assert!(get(&a, k, pass(), 7, false).is_none(), "another device");
        assert_ne!(key(view(3, 1), 1, 0, 0), k, "another device's frame");
        assert_ne!(key(v, 1, 1, 0), k, "another block");
        drop(a);
        put(&b, key(v, 5, 0, 0), v, pass(), false, STORE_BYTES, kept(4));
        assert_eq!(store_bytes(None), STORE_BYTES);
        assert_eq!(store_bytes(Some(12 << 30)), 1 << 30, "an eighth of 12 GB, at most 1 GB");
        assert_eq!(store_bytes(Some(2 << 30)), STORE_BYTES);
        assert!(lock().iter().all(|e| e.src.strong_count() > 0), "a closed photo's blocks go");
        clear();
    }
}
