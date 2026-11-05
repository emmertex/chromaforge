//! Auto Mask strokes kept on the device between renders: each finished stroke's coverage (stamped,
//! then snapped to the photo's edges) over the rectangle it can reach, in 16 bits, the GPU twin of
//! `lcc_pipeline::masks::auto_stroke`'s cache.
//!
//! A stroke's coverage depends on its dabs as placed in the plane and its brush
//! (`masks::auto_stroke_key`) and on the pixels it reads: the view's linear image, which the
//! source photo (by identity), its geometry key and its linear-stage key fix (the keys the stage
//! caches use). Fit → 100 % with nothing edited takes every stroke from here (the kernel refined a
//! dozen strokes of a big brush over the whole window again on every return to 100 %).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, Weak};

use lcc_raster::Rgb32f;

use crate::ctx::Buf;

/// The most device bytes of kept strokes (all cards): a dozen strokes of a 490-pixel brush over
/// a 4K window at 100 % of a 45 MP photo (each one reaching 5736 × 2900 pixels, 33 MB in 16 bits)
/// and the same strokes at Fit.
pub(crate) const STORE_BYTES: usize = 512 << 20;

/// One finished stroke: its coverage over `rect` (x0, y0, x1, y1 of the plane; exclusive ends),
/// zero everywhere else, two 16-bit values per word (`pack2x16unorm`, pixel `2k` in the low half).
pub(crate) struct Kept {
    pub rect: (usize, usize, usize, usize),
    pub packed: Buf,
}

struct Entry {
    key: u64,
    /// The render pass that last used it.
    pass: u64,
    /// Only batch renders used it (the windows around the one on screen, made ahead): those never
    /// evict what an interactive render keeps.
    batch: bool,
    /// The photo it was worked out on (its identity: a live `Weak` keeps the address from reuse).
    src: Weak<Rgb32f>,
    kept: Arc<Kept>,
}

static STORE: Mutex<VecDeque<Entry>> = Mutex::new(VecDeque::new());
static PASSES: AtomicU64 = AtomicU64::new(0);

/// A new brush render's pass number. A pass never evicts the strokes it used itself: when its
/// strokes outgrow [`STORE_BYTES`], the ones that fit stay and the rest aren't kept (evicting the
/// oldest would push out each stroke just before the next render asks for it).
pub(crate) fn pass() -> u64 {
    PASSES.fetch_add(1, Ordering::Relaxed).wrapping_add(1)
}

/// The key of one stroke (`masks::auto_stroke_key`) on the pixels of geometry `geo` and linear
/// stage `lin_key` on the device of generation `generation` (each device, the helper card's too,
/// has its own).
pub(crate) fn key(stroke: u64, geo: u64, lin_key: u64, generation: u64) -> u64 {
    use std::hash::{BuildHasher, BuildHasherDefault, DefaultHasher};
    BuildHasherDefault::<DefaultHasher>::default().hash_one((stroke, geo, lin_key, generation))
}

fn bytes_of(k: &Kept) -> usize {
    k.packed.len.saturating_mul(4)
}

fn lock() -> std::sync::MutexGuard<'static, VecDeque<Entry>> {
    STORE.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The stroke kept under `key` for photo `src` on device generation `generation`, marked used by
/// `pass` (of a batch render: `batch`).
pub(crate) fn get(src: &Arc<Rgb32f>, key: u64, pass: u64, generation: u64, batch: bool) -> Option<Arc<Kept>> {
    let mut store = lock();
    let me = Arc::downgrade(src);
    let i = store.iter().position(|e| e.key == key && Weak::ptr_eq(&e.src, &me) && e.kept.packed.generation() == generation)?;
    let mut e = store.remove(i)?;
    e.pass = pass;
    e.batch &= batch;
    let kept = e.kept.clone();
    store.push_back(e);
    Some(kept)
}

/// Keep `kept` under `key` for photo `src` (worked out by a batch render: `batch`): strokes of
/// photos no longer open go first, then the least recently used ones a batch render kept, then
/// (for an interactive render only) the least recently used others; never one `pass` used (then
/// `kept` isn't kept).
///
/// At 100 % the loupe makes the eight windows around the one on screen ahead, as batch renders:
/// each one's dozen big strokes filled the store, so returning to 100 % found none of the window
/// on screen.
pub(crate) fn put(src: &Arc<Rgb32f>, key: u64, pass: u64, batch: bool, kept: Arc<Kept>) {
    let mut store = lock();
    store.retain(|e| e.src.strong_count() > 0);
    store.retain(|e| e.key != key);
    keep(&mut store, Entry { key, pass, batch, src: Arc::downgrade(src), kept }, STORE_BYTES);
}

fn keep(store: &mut VecDeque<Entry>, entry: Entry, cap: usize) {
    let mut total = store.iter().map(|e| bytes_of(&e.kept)).fold(bytes_of(&entry.kept), usize::saturating_add);
    while total > cap {
        let other = |e: &Entry| e.pass != entry.pass;
        let Some(i) = store.iter().position(|e| other(e) && e.batch).or_else(|| (!entry.batch).then(|| store.iter().position(other)).flatten())
        else {
            break;
        };
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

    /// More strokes than fit, rendered again and again: the ones that fit stay, and a photo that
    /// was closed gives its strokes back.
    #[test]
    fn strokes_that_outgrow_the_store_still_hit_on_the_next_render() {
        let src = Arc::new(Rgb32f::new(1, 1));
        let mut store = VecDeque::new();
        let mut hits = 0;
        // a cap of 3 strokes, 4 strokes per render
        for pass in 1..=3u64 {
            for key in 0..4u64 {
                if let Some(i) = store.iter().position(|e: &Entry| e.key == key)
                    && let Some(mut e) = store.remove(i)
                {
                    hits += 1;
                    e.pass = pass;
                    store.push_back(e);
                } else {
                    keep(&mut store, Entry { key, pass, batch: false, src: Arc::downgrade(&src), kept: kept(10) }, 3 * 40);
                }
            }
        }
        assert_eq!(hits, 6, "three of four strokes hit on each later render");
        keep(&mut store, Entry { key: 9, pass: 4, batch: false, src: Arc::downgrade(&src), kept: kept(10) }, 3 * 40);
        assert!(store.iter().any(|e| e.key == 9), "an old pass's strokes make way");
    }

    /// The windows made ahead around the one on screen (batch renders) don't push its strokes out:
    /// they make way for each other only.
    #[test]
    fn strokes_made_ahead_never_evict_the_windows_on_screen() {
        let src = Arc::new(Rgb32f::new(1, 1));
        let e = |key, pass, batch| Entry { key, pass, batch, src: Arc::downgrade(&src), kept: kept(10) };
        let mut store = VecDeque::new();
        // the window on screen: three strokes, the cap
        for key in 0..3 {
            keep(&mut store, e(key, 1, false), 3 * 40);
        }
        // a window made ahead: none of its strokes is kept
        for key in 10..13 {
            keep(&mut store, e(key, 2, true), 3 * 40);
        }
        assert_eq!(store.iter().map(|e| e.key).collect::<Vec<_>>(), [0, 1, 2]);
        // room for one: windows made ahead take turns with it
        store.pop_back();
        keep(&mut store, e(20, 3, true), 3 * 40);
        keep(&mut store, e(21, 4, true), 3 * 40);
        assert_eq!(store.iter().map(|e| e.key).collect::<Vec<_>>(), [0, 1, 21]);
        // the screen's next window takes the batch one's place, then the oldest of its own
        keep(&mut store, e(30, 5, false), 3 * 40);
        keep(&mut store, e(31, 5, false), 3 * 40);
        assert_eq!(store.iter().map(|e| e.key).collect::<Vec<_>>(), [1, 30, 31]);
    }

    #[test]
    fn a_stroke_is_found_only_for_its_photo_and_device() {
        let (a, b) = (Arc::new(Rgb32f::new(1, 1)), Arc::new(Rgb32f::new(1, 1)));
        let k = key(1, 2, 3, 0);
        put(&a, k, pass(), false, kept(4));
        assert!(get(&a, k, pass(), 0, false).is_some());
        assert!(get(&b, k, pass(), 0, false).is_none(), "another photo");
        assert!(get(&a, k, pass(), 7, false).is_none(), "another device");
        assert_ne!(key(1, 2, 3, 1), k, "another device");
        drop(a);
        put(&b, key(5, 2, 3, 0), pass(), false, kept(4));
        assert!(lock().iter().all(|e| e.src.strong_count() > 0), "a closed photo's strokes go");
        clear();
    }
}
