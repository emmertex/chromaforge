//! Preparing an import as a stream (I/O spec §5.6, PR 4): the files of every source are read ahead
//! through the process's I/O pool, per volume, while planning takes them in source order, exactly
//! as batch by batch ([`ImportJob::prepare_files`]) would; copies and moves run through the pool as
//! they are planned, so several cards (or drives) feed one destination at once. Batches are handed
//! over in order, each once its last copy has its outcome.
//!
//! What keeps the results those of one batch after another:
//! - planning (duplicates, sidecars, names, the rename sequence, folders) is the same code, run in
//!   file order ([`ImportJob::sort_file`], [`ImportJob::plan_file`]);
//! - a file with the content of a copy still under way waits, in planning, for that copy's outcome
//!   (it is a duplicate only if the copy is ready); so does a browsed record joining the library;
//! - a copy whose name family (`IMG.JPG`, `IMG.NEF`, `IMG-1.JPG`…) has one under way into the same
//!   folder starts after it (names are chosen when copying);
//! - a batch's items keep their places, whatever order its copies end in.
//!
//! Bounds: a volume has at most [`Lookahead::per_volume`] files read ahead and not yet done with
//! (planned without a copy, or copied), so a copy's read of its source still finds it in memory;
//! planning stays within [`Lookahead::batches`] batches of the oldest batch not handed over.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering::Relaxed;
use std::time::Duration;

use lcc_catalog::PhotoId;
use lcc_catalog::folders::VolumeKey;

use super::{
    How, ImportJob, Next, Planned, Prepared, PreparedItem, Sorted, Transfer, import_stems, probe_outcome, transfer_ok, transfer_outcome, workers,
};
use crate::io_sched::pool::Stream;
use crate::io_sched::{IoContext, IoJob, JobOutcome};
use crate::media::ProbeInfo;

/// Batches planned ahead of the oldest one not yet handed over (its copies still under way).
pub(crate) const AHEAD_BATCHES: usize = 64;

/// How long a wait for a result blocks on one of the two streams before looking at the other.
const SLICE: Duration = Duration::from_millis(50);

/// How far preparing runs ahead.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Lookahead {
    /// Files per batch (each batch is handed over, and committed, as one).
    pub(crate) batch: usize,
    /// Files per volume read ahead and not yet done with: never fewer than a batch.
    pub(crate) per_volume: usize,
    /// Batches staged and planned ahead of the oldest one not handed over yet.
    pub(crate) batches: usize,
}

impl Lookahead {
    /// Today's batches ([`super::batch_size`]), two of them read ahead per volume (§5.6).
    pub(crate) fn new() -> Lookahead {
        let batch = super::batch_size();
        Lookahead { batch, per_volume: batch.saturating_mul(2), batches: AHEAD_BATCHES }
    }

    fn checked(self) -> Lookahead {
        let batch = self.batch.max(1);
        // (fewer than a batch could leave a batch waiting for its own files' reads)
        Lookahead { batch, per_volume: self.per_volume.max(batch), batches: self.batches.max(1) }
    }
}

type ProbeResult = Result<ProbeInfo, String>;

/// A read as planning takes it: a failure keeps whether its volume stopped responding.
type Probed = Result<ProbeInfo, super::ProbeError>;

/// A file read ahead (or to be): where it stands.
enum Read {
    /// Waiting for its volume's turn.
    Queued,
    /// Submitted: the stream index.
    Fed(usize),
    Arrived(Box<Probed>),
    /// Planning has its result.
    Taken,
    /// Planning passed it without needing it (a result still to come is dropped).
    Dropped,
}

struct Pos {
    path: String,
    vol: usize,
    read: Read,
    /// Holds a place in its volume's look-ahead.
    holds: bool,
}

struct VolFeed {
    key: VolumeKey,
    /// Positions waiting to be read, in file order (entries no longer queued are skipped).
    queue: VecDeque<usize>,
    /// Places held ([`Pos::holds`]).
    held: usize,
}

/// A batch staged: its files after the `beforeImport` hook, from position `first` on.
struct Staged {
    first: usize,
    files: Vec<String>,
    out: Prepared,
}

enum Slot {
    Item(PreparedItem),
    /// A transfer's outcome, by transfer id.
    Transfer(usize),
}

/// A batch planned, waiting for its transfers' outcomes before it is handed over.
struct Planning {
    out: Prepared,
    slots: Vec<Slot>,
}

struct Pending {
    hash: Option<String>,
    family: (String, String),
    pos: Option<usize>,
    path: String,
    dir: std::path::PathBuf,
    moving: bool,
}

/// Why preparing stopped early.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stop {
    Cancel,
    /// Nobody takes the batches any more.
    Refused,
}

/// What a wait is for: the stream to block on when nothing has come.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Wait {
    Probes,
    Transfers,
}

type Emit<'e> = &'e mut dyn FnMut(Prepared) -> bool;

struct Run<'a> {
    ctx: &'a Arc<IoContext>,
    look: Lookahead,
    cancel: &'a AtomicBool,
    files: Vec<String>,
    /// Batches staged so far (the next chunk of `files` to stage).
    staged_chunks: usize,
    next_pos: usize,
    staged: VecDeque<Staged>,
    /// Planned batches not yet handed over, oldest first.
    planned: VecDeque<Planning>,
    /// Batches handed over.
    emitted: usize,
    probes: Option<Stream<ProbeResult>>,
    pos: HashMap<usize, Pos>,
    vols: Vec<VolFeed>,
    probe_index: HashMap<usize, usize>,
    /// Probes a preceding scan left, by position, for the read that checks them ([`Run::need`]).
    cached: HashMap<usize, ProbeInfo>,
    transfers: Option<Stream<PreparedItem>>,
    next_tid: usize,
    transfer_index: HashMap<usize, usize>,
    pending: HashMap<usize, Pending>,
    outcomes: HashMap<usize, Option<PreparedItem>>,
    /// Transfers waiting for an earlier one of their family, per family.
    held: HashMap<(String, String), VecDeque<(usize, Box<Transfer>)>>,
    /// Pending transfers (under way or held) per family and per content hash.
    families: HashMap<(String, String), usize>,
    hashes: HashMap<String, usize>,
    stop: Option<Stop>,
    /// Copy as DNG's reader of the copied file (the session's).
    file_bytes: Option<crate::merge::ByteReader>,
    #[cfg(test)]
    fault: crate::import_move::Fault,
}

impl ImportJob {
    /// [`ImportJob::prepare_files`] over every file of an import, batch by batch, handing each
    /// batch to `emit` in file order (`false`: nobody takes them any more; what is not yet handed
    /// over is taken back, [`Prepared::rollback`]). The batches and their items are what one
    /// [`ImportJob::prepare_files`] call per [`super::batch_size`] files would give; the reads and
    /// copies of several volumes run at once meanwhile (I/O spec §5.6). Stops before the next file
    /// once `cancel` is set: the batches with items are still handed over (the last ones cut short).
    pub fn prepare_stream(&mut self, files: Vec<String>, cancel: &AtomicBool, mut emit: impl FnMut(Prepared) -> bool) {
        self.prepare_streamed(IoContext::global(), files, cancel, Lookahead::new(), &mut emit);
    }

    pub(crate) fn prepare_streamed(&mut self, ctx: &Arc<IoContext>, files: Vec<String>, cancel: &AtomicBool, look: Lookahead, emit: Emit<'_>) {
        if files.is_empty() {
            return;
        }
        // the library as it was, for the first batch's stem sidecars (as prepare_files gives it)
        let mut library = self.look_up_library();
        let mut run = Run::new(ctx, look.checked(), cancel, files, self.file_bytes.clone());
        let chunks = run.files.len().div_ceil(run.look.batch);
        for k in 0..chunks {
            // within the horizon: the oldest batch not handed over is at most `batches` behind
            while run.stop.is_none() && k >= run.emitted.saturating_add(run.look.batches) {
                run.pump(self, Some(Wait::Transfers), emit);
            }
            // (as one call per batch: a cancel before a batch begins leaves it out)
            if run.stop.is_none() && cancel.load(Relaxed) {
                run.halt(Stop::Cancel);
            }
            if run.stop.is_some() {
                break;
            }
            run.stage(self, run.emitted.saturating_add(run.look.batches));
            let Some(batch) = run.staged.pop_front() else { break };
            let planning = self.plan_streamed(&mut run, batch, library.take(), emit);
            run.planned.push_back(planning);
            if run.stop.is_some() {
                break;
            }
        }
        run.finish(self, emit);
    }

    /// Plan one staged batch in file order (as [`ImportJob::prepare_files`] does), its transfers
    /// submitted as they are planned. Stops early (the batch cut short) when the run stops.
    fn plan_streamed(&mut self, run: &mut Run<'_>, batch: Staged, library: Option<lcc_catalog::Catalog>, emit: Emit<'_>) -> Planning {
        let Staged { first, files, mut out } = batch;
        let mut todo: Vec<(usize, String)> = Vec::new();
        let mut follow: HashMap<String, PhotoId> = HashMap::new();
        for (i, f) in files.into_iter().enumerate() {
            let pos = first.saturating_add(i);
            // a browsed record joining the library is a duplicate of a copy ready before it
            if let Some(h) = self.promote_hash(&f).map(str::to_string)
                && !run.wait_hash(self, &h, emit)
            {
                out.cut_short = true;
                return Planning { out, slots: Vec::new() };
            }
            match self.sort_file(f, &mut follow) {
                Sorted::Item(it) => {
                    out.items.push(it);
                    run.release(pos);
                }
                Sorted::Todo(f) => todo.push((pos, f)),
            }
        }
        let paths: Vec<String> = todo.iter().map(|(_, f)| f.clone()).collect();
        // a probe a preceding scan left is checked against its file's disk by the file's own job
        // (on its volume, never here: a hung source can't hold planning, and cancel returns)
        let mut cached = self.take_cached(&paths);
        for (pos, path) in &todo {
            if let Some(info) = cached.remove(path) {
                run.cached.insert(*pos, info);
            }
            run.need(self, *pos, path);
        }
        let mut slots = Vec::new();
        // every read of the batch, before its first file is planned (the stem sidecars' owners)
        while run.stop.is_none() && todo.iter().any(|(pos, _)| !run.arrived(*pos)) {
            run.pump(self, Some(Wait::Probes), emit);
        }
        if run.stop.is_some() {
            out.cut_short = !todo.is_empty();
            return Planning { out, slots };
        }
        let probed: Vec<Probed> = todo.iter().map(|(pos, _)| run.take(*pos)).collect();
        crate::memory::release();
        let stems = import_stems(library.as_ref(), &paths, &probed);
        let mut listing = crate::sidecar::FolderListing::default();
        for ((pos, path), info) in todo.into_iter().zip(probed) {
            run.pump(self, None, emit);
            if run.stop.is_some() {
                out.cut_short = true;
                break;
            }
            let info = match info {
                Ok(i) => i,
                Err(e) => {
                    slots.push(Slot::Item(self.probe_failed(path, e)));
                    run.release(pos);
                    continue;
                }
            };
            let promote = follow.get(&path).copied();
            // a copy of a file whose transfer is still pending: a duplicate only if that one succeeds
            if !self.opts.local
                && let Some(h) = info.content_hash.clone()
                && !run.wait_hash(self, &h, emit)
            {
                out.cut_short = true;
                break;
            }
            let Planned { kept, next } = self.plan_file(path, info, promote, &stems, &mut listing);
            slots.extend(kept.map(Slot::Item));
            match next {
                Next::Transfer(t) => slots.push(Slot::Transfer(run.add_transfer(t, pos))),
                Next::Item(it) => {
                    slots.push(Slot::Item(it));
                    run.release(pos);
                }
            }
        }
        Planning { out, slots }
    }
}

/// The format-only result a session without file access gives every file (as [`super::probe_all`]).
fn unread_info(path: &str) -> ProbeInfo {
    ProbeInfo { format: std::path::Path::new(path).extension().map(|e| e.to_string_lossy().to_uppercase()).unwrap_or_default(), ..Default::default() }
}

impl<'a> Run<'a> {
    fn new(
        ctx: &'a Arc<IoContext>,
        look: Lookahead,
        cancel: &'a AtomicBool,
        files: Vec<String>,
        file_bytes: Option<crate::merge::ByteReader>,
    ) -> Run<'a> {
        Run {
            ctx,
            look,
            cancel,
            files,
            staged_chunks: 0,
            next_pos: 0,
            staged: VecDeque::new(),
            planned: VecDeque::new(),
            emitted: 0,
            probes: None,
            pos: HashMap::new(),
            vols: Vec::new(),
            probe_index: HashMap::new(),
            cached: HashMap::new(),
            transfers: None,
            next_tid: 0,
            transfer_index: HashMap::new(),
            pending: HashMap::new(),
            outcomes: HashMap::new(),
            held: HashMap::new(),
            families: HashMap::new(),
            hashes: HashMap::new(),
            stop: None,
            file_bytes,
            #[cfg(test)]
            fault: crate::import_move::injected_fault(),
        }
    }

    /// Stage the chunks before `upto` (the `beforeImport` hook; which files may be read ahead).
    fn stage(&mut self, job: &mut ImportJob, upto: usize) {
        while self.staged_chunks < upto {
            let start = self.staged_chunks.saturating_mul(self.look.batch);
            let Some(chunk) = self.files.get(start..).map(|rest| rest.iter().take(self.look.batch).cloned().collect::<Vec<_>>()) else { break };
            if chunk.is_empty() {
                break;
            }
            self.staged_chunks += 1;
            let (files, out) = job.filter_batch(chunk, self.cancel);
            let first = self.next_pos;
            self.next_pos = self.next_pos.saturating_add(files.len());
            if job.probe.is_some() {
                for (i, f) in files.iter().enumerate() {
                    if job.may_read(f) {
                        self.queue(first.saturating_add(i), f);
                    }
                }
            }
            self.staged.push_back(Staged { first, files, out });
        }
        self.feed(job);
    }

    fn probe_stream(&mut self) -> &mut Stream<ProbeResult> {
        let ctx = self.ctx;
        self.probes.get_or_insert_with(|| Stream::open(ctx, workers(usize::MAX), Result::is_ok))
    }

    fn vol_of(&mut self, path: &str) -> usize {
        let key = self.probe_stream().volume_of(path);
        match self.vols.iter().position(|v| v.key == key) {
            Some(i) => i,
            None => {
                self.vols.push(VolFeed { key, queue: VecDeque::new(), held: 0 });
                self.vols.len() - 1
            }
        }
    }

    /// Queue position `p` (file `path`) to be read ahead on its volume.
    fn queue(&mut self, p: usize, path: &str) {
        let vol = self.vol_of(path);
        if let Some(v) = self.vols.get_mut(vol) {
            v.queue.push_back(p);
        }
        self.pos.insert(p, Pos { path: path.to_string(), vol, read: Read::Queued, holds: false });
    }

    /// Read ahead what each volume has room for, in file order, every volume's reads in one submit
    /// (so the pool sees every volume's work at once and shares its slots between them, rather than
    /// giving the first volume submitted every slot).
    fn feed(&mut self, job: &ImportJob) {
        if self.stop.is_some() {
            return;
        }
        let mut picks = Vec::new();
        for v in 0..self.vols.len() {
            while let Some(vf) = self.vols.get_mut(v)
                && vf.held < self.look.per_volume
                && let Some(p) = vf.queue.pop_front()
            {
                if let Some(e) = self.pos.get_mut(&p).filter(|e| matches!(e.read, Read::Queued)) {
                    e.holds = true;
                    vf.held = vf.held.saturating_add(1);
                    picks.push(p);
                }
            }
        }
        self.submit_probes(job, picks);
    }

    /// Read positions `ps` (their places already held).
    fn submit_probes(&mut self, job: &ImportJob, ps: Vec<usize>) {
        let Some(probe) = job.probe.clone() else { return };
        let mut jobs = Vec::with_capacity(ps.len());
        for p in &ps {
            let Some(path) = self.pos.get(p).map(|e| e.path.clone()) else { continue };
            let (probe, read, cached) = (probe.clone(), path.clone(), self.cached.remove(p));
            // (a cached probe that still holds comes back unread)
            let run = move || match cached {
                Some(info) if super::keep_import_probe(&read, &info) => Ok(info),
                _ => probe(&read),
            };
            jobs.push(IoJob::read(path, Box::new(run) as Box<dyn FnOnce() -> ProbeResult + Send>));
        }
        if jobs.is_empty() {
            return;
        }
        job.read_total.fetch_add(jobs.len(), Relaxed);
        let submitted = self.probe_stream().submit(jobs);
        for (k, p) in ps.into_iter().enumerate() {
            let Some(e) = self.pos.get_mut(&p) else { continue };
            match &submitted {
                Ok(r) => {
                    let index = r.start.saturating_add(k);
                    e.read = Read::Fed(index);
                    self.probe_index.insert(index, p);
                }
                // (from inside a file job: refused, as probe_all refuses it)
                Err(err) => e.read = Read::Arrived(Box::new(Err(format!("{}: {err}", e.path).into()))),
            }
        }
    }

    /// Planning needs position `p`'s read (`path`): queued ones wait for their volume's turn, one
    /// not read ahead (planning's own guess was wrong) is read now.
    fn need(&mut self, job: &ImportJob, p: usize, path: &str) {
        if job.probe.is_none() {
            // (no file access: a cached probe is used as it is, as `probe_all_cached` does)
            let info = self.cached.remove(&p).unwrap_or_else(|| unread_info(path));
            self.pos.insert(p, Pos { path: path.to_string(), vol: usize::MAX, read: Read::Arrived(Box::new(Ok(info))), holds: false });
            return;
        }
        if self.pos.contains_key(&p) {
            return;
        }
        let vol = self.vol_of(path);
        if let Some(vf) = self.vols.get_mut(vol) {
            vf.held = vf.held.saturating_add(1);
        }
        self.pos.insert(p, Pos { path: path.to_string(), vol, read: Read::Queued, holds: true });
        if self.stop.is_none() {
            self.submit_probes(job, vec![p]);
        } else {
            self.pos
                .insert(p, Pos { path: path.to_string(), vol, read: Read::Arrived(Box::new(Err(String::from("not probed").into()))), holds: false });
            if let Some(vf) = self.vols.get_mut(vol) {
                vf.held = vf.held.saturating_sub(1);
            }
        }
    }

    fn arrived(&self, p: usize) -> bool {
        self.pos.get(&p).is_some_and(|e| matches!(e.read, Read::Arrived(_)))
    }

    fn take(&mut self, p: usize) -> Probed {
        match self.pos.get_mut(&p) {
            Some(e) => match std::mem::replace(&mut e.read, Read::Taken) {
                Read::Arrived(r) => *r,
                _ => Err(String::from("not probed").into()),
            },
            None => Err(String::from("not probed").into()),
        }
    }

    /// Planning is done with position `p` (or a transfer of it has its outcome): its volume's
    /// place is free.
    fn release(&mut self, p: usize) {
        let Some(e) = self.pos.get_mut(&p) else { return };
        if e.holds {
            e.holds = false;
            if let Some(vf) = self.vols.get_mut(e.vol) {
                vf.held = vf.held.saturating_sub(1);
            }
        }
        if matches!(e.read, Read::Fed(_)) {
            e.read = Read::Dropped;
        } else {
            self.pos.remove(&p);
        }
    }

    /// A transfer planned at position `p`: under way at once, or after the pending one of its
    /// name family. Its id.
    fn add_transfer(&mut self, t: Box<Transfer>, p: usize) -> usize {
        let tid = self.next_tid;
        self.next_tid = self.next_tid.saturating_add(1);
        let hash = t.info.content_hash.clone();
        if let Some(h) = &hash {
            *self.hashes.entry(h.clone()).or_default() += 1;
        }
        let family = t.family.clone();
        let moving = matches!(t.how, How::Place { .. });
        self.pending.insert(tid, Pending { hash, family: family.clone(), pos: Some(p), path: t.path.clone(), dir: t.dir.clone(), moving });
        let busy = self.families.entry(family.clone()).or_default();
        *busy += 1;
        if *busy > 1 {
            self.held.entry(family).or_default().push_back((tid, t));
        } else {
            self.submit_transfer(tid, t);
        }
        tid
    }

    fn submit_transfer(&mut self, tid: usize, t: Box<Transfer>) {
        let ctx = self.ctx;
        #[cfg(test)]
        let fault = self.fault;
        let (source, dest, skippable) = (t.path.clone(), Some(t.dir.to_string_lossy().into_owned()), t.dng);
        let file_bytes = self.file_bytes.clone();
        let run = move || {
            #[cfg(test)]
            crate::import_move::inject(fault);
            let path = t.path.clone();
            // last-resort guard: a panic is this file's failure, not the import's
            crate::guard::catch("import", || super::run_transfer(*t, file_bytes.as_ref())).unwrap_or_else(|e| PreparedItem::Failed(path, e))
        };
        let job = IoJob { source, dest, skippable, run: Box::new(run) as Box<dyn FnOnce() -> PreparedItem + Send> };
        let stream = self.transfers.get_or_insert_with(|| Stream::open(ctx, workers(usize::MAX), transfer_ok));
        match stream.submit(vec![job]) {
            Ok(r) => {
                self.transfer_index.insert(r.start, tid);
            }
            Err(e) => {
                let path = self.pending.get(&tid).map(|p| p.path.clone()).unwrap_or_default();
                self.resolved(tid, Some(PreparedItem::Failed(path, format!("{e}; the original is kept"))));
            }
        }
    }

    /// Transfer `tid` has its outcome (`None`: not started): the next of its family may start.
    fn resolved(&mut self, tid: usize, item: Option<PreparedItem>) {
        self.outcomes.insert(tid, item);
        let Some(p) = self.pending.remove(&tid) else { return };
        if let Some(h) = &p.hash
            && let Some(n) = self.hashes.get_mut(h)
        {
            *n = n.saturating_sub(1);
            if *n == 0 {
                self.hashes.remove(h);
            }
        }
        if let Some(n) = self.families.get_mut(&p.family) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                self.families.remove(&p.family);
            }
        }
        if let Some(pos) = p.pos {
            self.release(pos);
        }
        let next = self.held.get_mut(&p.family).and_then(VecDeque::pop_front);
        if let Some((t2, t)) = next {
            if self.stop.is_some() {
                // (stopping: it never starts)
                self.resolved(t2, None);
            } else {
                self.submit_transfer(t2, t);
            }
        }
    }

    /// Wait until no transfer with content `h` is pending; `false` if the run stopped first.
    fn wait_hash(&mut self, job: &mut ImportJob, h: &str, emit: Emit<'_>) -> bool {
        while self.stop.is_none() && self.hashes.contains_key(h) {
            self.pump(job, Some(Wait::Transfers), emit);
        }
        self.stop.is_none()
    }

    fn on_probe(&mut self, job: &ImportJob, index: usize, outcome: JobOutcome<ProbeResult>) {
        if !matches!(outcome, JobOutcome::NotStarted | JobOutcome::Cancelled) {
            job.read_done.fetch_add(1, Relaxed);
        }
        let Some(p) = self.probe_index.remove(&index) else { return };
        let Some(e) = self.pos.get_mut(&p) else { return };
        match e.read {
            Read::Fed(i) if i == index => e.read = Read::Arrived(Box::new(probe_outcome(outcome, &e.path, "not probed"))),
            Read::Dropped => {
                self.pos.remove(&p);
            }
            _ => {}
        }
    }

    fn on_transfer(&mut self, job: &mut ImportJob, index: usize, outcome: JobOutcome<PreparedItem>) {
        let Some(tid) = self.transfer_index.remove(&index) else { return };
        if !matches!(outcome, JobOutcome::NotStarted | JobOutcome::Cancelled) {
            job.done.fetch_add(1, Relaxed);
        }
        let item = match self.pending.get(&tid) {
            Some(p) => transfer_outcome(outcome, p.path.clone(), &p.dir, p.moving),
            None => None,
        };
        if let Some(it) = &item {
            job.note_transfer(it);
        }
        self.resolved(tid, item);
    }

    /// Take every result that has come, read ahead, hand over the batches that are complete; with
    /// `wait`, block a little on that stream first when nothing has come. Notices cancel.
    fn pump(&mut self, job: &mut ImportJob, wait: Option<Wait>, emit: Emit<'_>) {
        if self.stop.is_none() && self.cancel.load(Relaxed) {
            self.halt(Stop::Cancel);
        }
        let got = self.drain(job);
        if !got && let Some(w) = wait {
            match w {
                Wait::Probes if self.probes.is_some() => self.drain_probes(job, SLICE),
                Wait::Transfers if self.transfers.is_some() => self.drain_transfers(job, SLICE),
                // (nothing can come on a stream never opened: look at the other one)
                Wait::Probes => self.drain_transfers(job, SLICE),
                Wait::Transfers => self.drain_probes(job, SLICE),
            };
        }
        self.feed(job);
        if self.stop.is_none() {
            self.emit_ready(emit);
        }
    }

    fn drain(&mut self, job: &mut ImportJob) -> bool {
        let a = self.drain_probes(job, Duration::ZERO);
        let b = self.drain_transfers(job, Duration::ZERO);
        a || b
    }

    /// Take the probe results that have come (waiting up to `wait` for the first).
    fn drain_probes(&mut self, job: &ImportJob, wait: Duration) -> bool {
        let mut got = false;
        let mut w = wait;
        while let Some(s) = self.probes.as_mut() {
            let Some((i, o, _)) = s.recv_timeout(w) else { break };
            self.on_probe(job, i, o);
            got = true;
            w = Duration::ZERO;
        }
        got
    }

    fn drain_transfers(&mut self, job: &mut ImportJob, wait: Duration) -> bool {
        let mut got = false;
        let mut w = wait;
        while let Some(s) = self.transfers.as_mut() {
            let Some((i, o, _)) = s.recv_timeout(w) else { break };
            self.on_transfer(job, i, o);
            got = true;
            w = Duration::ZERO;
        }
        got
    }

    /// Hand over the oldest batches whose transfers all have outcomes.
    fn emit_ready(&mut self, emit: Emit<'_>) {
        while self.planned.front().is_some_and(|b| self.complete(b)) {
            let Some(b) = self.planned.pop_front() else { break };
            let out = self.assemble(b);
            self.emitted = self.emitted.saturating_add(1);
            if !emit(out) {
                self.halt(Stop::Refused);
                return;
            }
        }
    }

    fn complete(&self, b: &Planning) -> bool {
        b.slots.iter().all(|s| match s {
            Slot::Item(_) => true,
            Slot::Transfer(t) => self.outcomes.contains_key(t),
        })
    }

    /// A planned batch's items in file order (a transfer not started leaves its batch cut short).
    fn assemble(&mut self, b: Planning) -> Prepared {
        let Planning { mut out, slots } = b;
        for s in slots {
            match s {
                Slot::Item(it) => out.items.push(it),
                Slot::Transfer(t) => match self.outcomes.remove(&t).flatten() {
                    Some(it) => out.items.push(it),
                    None => out.cut_short = true,
                },
            }
        }
        out
    }

    fn halt(&mut self, why: Stop) {
        if self.stop.is_some() {
            return;
        }
        self.stop = Some(why);
        if let Some(s) = self.probes.as_mut() {
            s.cancel();
        }
        if let Some(s) = self.transfers.as_mut() {
            s.cancel();
        }
    }

    /// Wait for every transfer's outcome (and every read under way), then hand over what remains,
    /// in order; after a refusal, take it back instead.
    fn finish(&mut self, job: &mut ImportJob, emit: Emit<'_>) {
        if self.stop.is_some() {
            // transfers still held behind their family never start
            let held: Vec<usize> = self.held.drain().flat_map(|(_, q)| q.into_iter().map(|(t, _)| t)).collect();
            for t in held {
                self.resolved(t, None);
            }
        }
        // (reads ahead that nobody needs any more are not waited for: the stream, dropped, cancels
        // them, and the pool sees to the ones under way)
        loop {
            if self.stop.is_none() && self.cancel.load(Relaxed) {
                self.halt(Stop::Cancel);
                let held: Vec<usize> = self.held.drain().flat_map(|(_, q)| q.into_iter().map(|(t, _)| t)).collect();
                for t in held {
                    self.resolved(t, None);
                }
            }
            if self.stop.is_none() {
                self.emit_ready(emit);
            }
            if self.transfer_index.is_empty() {
                // (nothing more can come: a transfer still held has no earlier one under way)
                let held: Vec<usize> = self.held.drain().flat_map(|(_, q)| q.into_iter().map(|(t, _)| t)).collect();
                if held.is_empty() {
                    break;
                }
                for t in held {
                    self.resolved(t, None);
                }
                continue;
            }
            self.drain_probes(job, Duration::ZERO);
            self.drain_transfers(job, SLICE);
        }
        // stopped: what is planned goes over as it stands (cut short), or is taken back
        while let Some(b) = self.planned.pop_front() {
            let out = self.assemble(b);
            if self.stop == Some(Stop::Refused) {
                out.rollback();
                continue;
            }
            if !emit(out) {
                self.stop = Some(Stop::Refused);
            }
        }
        self.probes = None;
        if let Some(s) = self.transfers.as_mut() {
            s.close();
        }
    }
}
