//! Originals read in the background (issue #1303): a Lightroom catalog import commits its photos
//! from the catalog first, each without its original read yet (`lcc_engine::cmd::read_originals`),
//! and this reads those originals afterwards, in batches, as one stoppable row on the activity
//! stack. Each batch is applied as soon as it is read, so a cancel, crash or sleep keeps what was
//! read; the rest is read the next time the library opens (or after the next import). A file that
//! can't be read (an offline drive) is not tried again until then.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use lcc_catalog::PhotoId;
use lcc_engine::cmd::read_originals;

use crate::LccApp;

/// The activity stack's label while originals are read.
pub const LABEL: &str = "Reading originals";

/// Files read per batch: what a cancel or crash can lose, and one journaled batch each.
pub const BATCH: usize = 500;

/// Where the background reading of one opened library stands.
#[derive(Clone, Debug, Default)]
pub struct OriginalsState {
    /// The library this is about ([`lcc_engine::Session::library_generation`]).
    library: Option<u64>,
    /// Photos tried in this library (read or not): never listed again while it stays open.
    tried: HashSet<PhotoId>,
    /// Nothing to do until [`wake`]: all read or tried, or the user stopped it.
    idle: bool,
    running: bool,
    /// Photos this run will read, counted when it starts, for the row's progress.
    total: u64,
    /// How many photos had been tried when this run started.
    run_start: usize,
    /// This run's reader and cancel flag, kept from batch to batch; dropped when the run stops.
    run: Option<ReaderRun>,
    /// Files per batch in tests ([`BATCH`] otherwise).
    #[cfg(test)]
    batch: Option<usize>,
}

/// One run's reader, which reads the batches and the next ones' files ahead meanwhile, and the
/// cancel flag of every batch of the run. One flag for the whole run, not one per batch: the
/// reads ahead are linked to the flag they started under, and a batch with another flag would
/// make the reader drop them (and read those files again).
#[derive(Clone, Debug)]
struct ReaderRun {
    reader: Arc<Mutex<read_originals::OriginalsReader>>,
    cancel: Arc<AtomicBool>,
}

/// Files per batch.
fn batch_size(state: &OriginalsState) -> usize {
    #[cfg(test)]
    if let Some(n) = state.batch {
        return n;
    }
    let _ = state;
    BATCH
}

/// Read in batches of `n` files from now on (tests).
#[cfg(test)]
pub(crate) fn use_batches_of(app: &mut LccApp, n: usize) {
    app.ui.originals = OriginalsState { library: Some(app.session.library_generation()), batch: Some(n), ..OriginalsState::default() };
}

/// Look again for originals to read (called when an import has added photos without reading them).
pub fn wake(app: &mut LccApp) {
    let state = &mut app.ui.originals;
    state.idle = false;
    if !state.running {
        state.total = 0;
    }
}

/// Every frame: read the next batch when there is one and none is under way.
pub fn tick(app: &mut LccApp) {
    let library = app.session.library_generation();
    if app.ui.originals.library != Some(library) {
        app.ui.originals = OriginalsState { library: Some(library), ..OriginalsState::default() };
    }
    let state = &app.ui.originals;
    if state.running || state.idle {
        return;
    }
    // a demo or browser session has no files to read
    let Some(probe) = read_originals::file_probe(&app.session) else {
        app.ui.originals.idle = true;
        return;
    };
    // one listing: this batch, then the photos after it, whose files are read ahead
    let size = batch_size(state);
    let mut batch = read_originals::unread(&app.session.catalog, &state.tried, size.saturating_mul(1 + read_originals::AHEAD_BATCHES));
    let next = batch.split_off(size.min(batch.len()));
    if batch.is_empty() {
        app.ui.originals.idle = true;
        app.ui.originals.total = 0;
        app.ui.originals.run = None;
        return;
    }
    let state = &mut app.ui.originals;
    if state.total == 0 {
        // (photos tried earlier in this library are still unread but not read again)
        let unread = read_originals::count_unread(&app.session.catalog);
        let tried_unread = state.tried.iter().filter(|id| app.session.catalog.photo(**id).is_some_and(|p| read_originals::is_unread(p))).count();
        state.total = unread.saturating_sub(tried_unread).max(batch.len()) as u64;
        state.run_start = state.tried.len();
    }
    let total = state.total;
    let done_before = (state.tried.len().saturating_sub(state.run_start) as u64).min(total);
    state.tried.extend(batch.iter().map(|u| u.id));
    state.running = true;
    let ReaderRun { reader, cancel } = state
        .run
        .get_or_insert_with(|| ReaderRun {
            reader: Arc::new(Mutex::new(read_originals::OriginalsReader::new(probe))),
            cancel: Arc::new(AtomicBool::new(false)),
        })
        .clone();
    let worker_cancel = cancel.clone();
    let stopped = cancel.clone();
    let work = move |task: lcc_engine::activity::TaskHandle| {
        task.progress(done_before, total);
        let mut reader = reader.lock().unwrap_or_else(PoisonError::into_inner);
        reader.read(batch, &next, &worker_cancel, &|done| task.progress(done_before.saturating_add(done as u64).min(total), total))
    };
    let done = move |app: &mut LccApp, _ctx: &egui::Context, results| {
        if app.session.library_generation() != library {
            return;
        }
        app.ui.originals.running = false;
        // stopped: the rest waits for the library's next opening or the next import
        if stopped.load(Ordering::Relaxed) {
            app.ui.originals.idle = true;
            app.ui.originals.total = 0;
            app.ui.originals.run = None;
        }
        match read_originals::apply(&mut app.session, results) {
            Ok(r) => {
                for (id, same) in r.duplicates {
                    log::info!("reading originals: photo {} has the same file content as photo {}", id.0, same.0);
                }
            }
            Err(e) => log::warn!("reading originals: {e}"),
        }
    };
    if let Err(e) = crate::tasks::spawn_tracked(app, LABEL, "readOriginals", cancel, work, done) {
        log::warn!("{e}");
        app.ui.originals.running = false;
        app.ui.originals.idle = true;
    }
}
