//! Originals read after their records are committed (issue #1303): a Lightroom catalog import adds
//! its photos from the catalog alone, each marked "original not yet read" (no content hash), so a
//! large archive on slow or external drives is browsable in minutes instead of days. The originals
//! are then read here, in the background and in batches, filling in only what needs the file: the
//! content hash, file size and dimensions, empty camera fields, embedded lens corrections and the
//! as-shot white balance — exactly what Reload from Disk would give the photo
//! ([`crate::cmd::convert::reload_ops`]) — and the SHA-1 the same read computes, so the background
//! SHA-1 back-fill need not read the file again. Ratings, keywords, develop settings and history stay as
//! the catalog gave them.
//!
//! The marker is the absence of a content hash, so the work resumes by itself: a photo whose file
//! was read has a hash and is not listed again; a cancelled, interrupted or crashed run leaves the
//! rest listed for the next one. A file that can't be read (an offline drive) changes nothing and
//! is tried again when the library next opens.
//!
//! The work is split like [`crate::cmd::recheck`]: [`unread`] lists photos (no file-system calls),
//! [`probe_cancellable_with_progress`] reads files without the session, and [`apply`] applies the
//! results as the app's own bookkeeping (journaled, not an undo step). Agents and the CLI use
//! `library.readOriginals`, which does all three at once.

use std::collections::HashSet;

use lcc_catalog::{Catalog, MediaKind, Op, Photo, PhotoId};
use serde_json::{Value, json};

use super::{CommandSpec, always, bad, cmd};
use crate::media::{FileProbe, ProbeInfo};
use crate::{Result, Session};

/// A photo whose original is still to be read, and the file it named when it was listed (so a
/// result for a photo relinked meanwhile is dropped).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unread {
    pub id: PhotoId,
    pub path: String,
}

impl Unread {
    /// The photo is still unread and names the same file.
    fn current(&self, p: &Photo) -> bool {
        p.content_hash.is_none() && crate::cmd::missing::checked_path(p) == Some(self.path.as_str())
    }
}

/// Whether a library photo's original is still to be read: a file of its own (not a virtual copy,
/// which follows its master), no content hash, and a still photo (a video's probe need not hash it,
/// so it would be listed forever).
pub fn is_unread(p: &Photo) -> bool {
    p.copy_of.is_none() && p.content_hash.is_none() && p.kind != MediaKind::Video && p.preview_only.is_none()
}

/// Up to `limit` library photos whose originals are still to be read, leaving out `skip` (those
/// already tried this session), in id order (the order an import added them). No file-system calls.
pub fn unread(cat: &Catalog, skip: &HashSet<PhotoId>, limit: usize) -> Vec<Unread> {
    let mut out: Vec<Unread> = cat
        .photos()
        .filter(|p| is_unread(p) && !skip.contains(&p.id))
        .filter_map(|p| crate::cmd::missing::checked_path(p).map(|path| Unread { id: p.id, path: path.to_string() }))
        .collect();
    out.sort_by_key(|u| u.id.0);
    out.truncate(limit);
    out
}

/// The next batch to read and the photos after it whose files are read ahead
/// ([`OriginalsReader::read`]): at most `batch` photos [`unread`] lists (leaving out `skip`), then
/// at most [`AHEAD_BATCHES`] batches' worth more, all within `limit` photos (`usize::MAX`: no
/// limit). One listing, so both come from the same library state. No file-system calls.
pub fn next_batch(cat: &Catalog, skip: &HashSet<PhotoId>, batch: usize, limit: usize) -> (Vec<Unread>, Vec<Unread>) {
    let n = batch.min(limit);
    let ahead = n.saturating_mul(AHEAD_BATCHES).min(limit.saturating_sub(n));
    let mut listed = unread(cat, skip, n.saturating_add(ahead));
    let next = listed.split_off(n.min(listed.len()));
    (listed, next)
}

/// How many library photos' originals are still to be read. No file-system calls.
pub fn count_unread(cat: &Catalog) -> usize {
    cat.photos().filter(|p| is_unread(p) && crate::cmd::missing::checked_path(p).is_some()).count()
}

/// The session's file probe; `None` when the session has no file access (a demo or browser
/// session), where nothing can be read.
pub fn file_probe(s: &Session) -> Option<FileProbe> {
    s.media.file_probe.clone()
}

/// Read each listed file, reporting completed files and stopping before starting more after
/// `cancel` is set. Files not reached keep an error result, which [`apply`] leaves unchanged.
pub fn probe_cancellable_with_progress(
    probe: &FileProbe,
    unread: Vec<Unread>,
    cancel: &std::sync::Arc<std::sync::atomic::AtomicBool>,
    progress: &dyn Fn(usize),
) -> Vec<(Unread, std::result::Result<ProbeInfo, String>)> {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let paths: Vec<String> = unread.iter().map(|u| u.path.clone()).collect();
    let done = AtomicUsize::new(0);
    #[cfg(not(target_arch = "wasm32"))]
    let probed = std::thread::scope(|scope| {
        let worker = scope.spawn(|| crate::import::probe_all(Some(probe), &paths, &done, cancel));
        while !worker.is_finished() {
            progress(done.load(Ordering::Relaxed));
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        worker.join().unwrap_or_else(|_| paths.iter().map(|path| Err(format!("{path}: could not be read"))).collect())
    });
    #[cfg(target_arch = "wasm32")]
    let probed = crate::import::probe_all(Some(probe), &paths, &done, cancel);
    progress(done.load(Ordering::Relaxed));
    unread.into_iter().zip(probed).collect()
}

/// Reads originals batch after batch while the files of the batches that follow are read ahead
/// (I/O spec §5.6): the batches stay what [`unread`] lists, applied in id order, so their
/// results, duplicate pairs and journal batches are what [`probe_cancellable_with_progress`] per
/// batch gives; only the reads are interleaved — across the volumes the next batches are on, and
/// while a batch is applied. A read ahead for a photo the next batch doesn't list (the library
/// changed meanwhile) is dropped; one the next batch lists is used as read, and the rest of the
/// batch is read then.
pub struct OriginalsReader {
    probe: FileProbe,
    #[cfg(not(target_arch = "wasm32"))]
    ahead: Ahead,
}

impl std::fmt::Debug for OriginalsReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OriginalsReader").finish_non_exhaustive()
    }
}

/// Files read ahead after a batch: at most this many batches' worth.
pub const AHEAD_BATCHES: usize = 2;

#[cfg(not(target_arch = "wasm32"))]
type ProbeResult = std::result::Result<ProbeInfo, String>;

/// A file read ahead: its read under way, or its result.
#[cfg(not(target_arch = "wasm32"))]
enum Ahead1 {
    Fed,
    Arrived(Box<ProbeResult>),
}

#[cfg(not(target_arch = "wasm32"))]
struct Ahead {
    ctx: std::sync::Arc<crate::io_sched::IoContext>,
    stream: Option<crate::io_sched::pool::Stream<ProbeResult>>,
    /// The cancel flag `stream` is linked to (the pool starts no read once it is set).
    linked: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    reads: std::collections::HashMap<(PhotoId, String), Ahead1>,
    by_index: std::collections::HashMap<usize, (PhotoId, String)>,
}

impl OriginalsReader {
    pub fn new(probe: FileProbe) -> OriginalsReader {
        #[cfg(not(target_arch = "wasm32"))]
        {
            OriginalsReader::in_context(probe, crate::io_sched::IoContext::global().clone())
        }
        #[cfg(target_arch = "wasm32")]
        {
            OriginalsReader { probe }
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn in_context(probe: FileProbe, ctx: std::sync::Arc<crate::io_sched::IoContext>) -> OriginalsReader {
        OriginalsReader {
            probe,
            ahead: Ahead { ctx, stream: None, linked: None, reads: std::collections::HashMap::new(), by_index: std::collections::HashMap::new() },
        }
    }

    /// Read `batch` (as [`probe_cancellable_with_progress`] would: its results in its order,
    /// `progress` with the files done, stopping before more once `cancel` is set) and start reading
    /// `next` (the photos [`unread`] would list after it, at most [`AHEAD_BATCHES`] batches of
    /// them) ahead. A cancel drops what was read ahead. `cancel` stays linked to the reads ahead:
    /// once it is set no further read starts, between calls too (a call with another flag drops
    /// what was read ahead and links its own; so a caller passes one flag for all of a run's
    /// batches, or its reads ahead are never used).
    pub fn read(
        &mut self,
        batch: Vec<Unread>,
        next: &[Unread],
        cancel: &std::sync::Arc<std::sync::atomic::AtomicBool>,
        progress: &dyn Fn(usize),
    ) -> Vec<(Unread, std::result::Result<ProbeInfo, String>)> {
        #[cfg(target_arch = "wasm32")]
        {
            let _ = next;
            probe_cancellable_with_progress(&self.probe, batch, cancel, progress)
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.read_ahead(batch, next, cancel, progress)
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn read_ahead(
        &mut self,
        batch: Vec<Unread>,
        next: &[Unread],
        cancel: &std::sync::Arc<std::sync::atomic::AtomicBool>,
        progress: &dyn Fn(usize),
    ) -> Vec<(Unread, std::result::Result<ProbeInfo, String>)> {
        use std::sync::atomic::Ordering::Relaxed;
        // (cancelled before it starts: nothing is read, as probe_all reads nothing)
        if cancel.load(Relaxed) {
            self.drop_ahead();
            return batch.into_iter().map(|u| (u, Err("not probed".to_string()))).collect();
        }
        // (reads ahead linked to another flag: that flag's cancel would stop them, this one's not)
        if self.ahead.linked.as_ref().is_some_and(|l| !std::sync::Arc::ptr_eq(l, cancel)) {
            self.drop_ahead();
        }
        let key = |u: &Unread| (u.id, u.path.clone());
        let limit = batch.len().saturating_mul(AHEAD_BATCHES);
        let next = next.get(..limit.min(next.len())).unwrap_or(next);
        // what was read ahead for photos listed neither now nor next is dropped
        let keep: std::collections::HashSet<(PhotoId, String)> = batch.iter().chain(next).map(key).collect();
        let a = &mut self.ahead;
        a.reads.retain(|k, _| keep.contains(k));
        a.by_index.retain(|_, k| keep.contains(k));
        let fresh: Vec<&Unread> = batch.iter().chain(next).filter(|u| !a.reads.contains_key(&key(u))).collect();
        if !fresh.is_empty() {
            let jobs = fresh.iter().map(|u| crate::import::probe_job(&self.probe, &u.path, None)).collect();
            let ctx = a.ctx.clone();
            a.linked = Some(cancel.clone());
            let stream = a.stream.get_or_insert_with(|| crate::import::probe_stream(&ctx, None, cancel));
            match stream.submit(jobs) {
                Ok(r) => {
                    for (k, u) in fresh.iter().enumerate() {
                        let i = r.start.saturating_add(k);
                        a.by_index.insert(i, key(u));
                        a.reads.insert(key(u), Ahead1::Fed);
                    }
                }
                Err(e) => {
                    for u in fresh {
                        a.reads.insert(key(u), Ahead1::Arrived(Box::new(Err(crate::import::refused_probe(&u.path, e).into_message()))));
                    }
                }
            }
        }
        let wanted: std::collections::HashSet<(PhotoId, String)> = batch.iter().map(key).collect();
        let (mut done, mut forwarded) = (0usize, false);
        let have = |a: &Ahead| wanted.iter().filter(|k| matches!(a.reads.get(*k), Some(Ahead1::Arrived(_)))).count();
        done += have(&self.ahead);
        progress(done);
        while have(&self.ahead) < wanted.len() {
            if !forwarded && cancel.load(Relaxed) {
                // the batch's reads not started never start, and nothing more is read ahead
                if let Some(s) = self.ahead.stream.as_mut() {
                    s.cancel();
                }
                forwarded = true;
            }
            let a = &mut self.ahead;
            let Some(s) = a.stream.as_mut() else { break };
            let Some((i, o, _)) = s.recv_timeout(a.ctx.tick) else {
                if s.left_behind() {
                    // cancelled, and the pool stopped: the reads still running are left behind
                    for (_, k) in a.by_index.drain().filter(|(_, k)| wanted.contains(k)) {
                        let r = crate::import::probe_outcome(crate::io_sched::JobOutcome::Cancelled, &k.1, "not probed");
                        a.reads.insert(k, Ahead1::Arrived(Box::new(r.map_err(crate::import::ProbeError::into_message))));
                    }
                    break;
                }
                continue;
            };
            let counted = o.counts_as_done();
            let Some(k) = a.by_index.remove(&i) else { continue };
            let r = crate::import::probe_outcome(o, &k.1, "not probed").map_err(crate::import::ProbeError::into_message);
            if wanted.contains(&k) && counted {
                done += 1;
                progress(done);
            }
            a.reads.insert(k, Ahead1::Arrived(Box::new(r)));
        }
        let out = batch
            .into_iter()
            .map(|u| {
                let r = match self.ahead.reads.remove(&key(&u)) {
                    Some(Ahead1::Arrived(r)) => *r,
                    _ => Err("not probed".to_string()),
                };
                (u, r)
            })
            .collect();
        if forwarded {
            self.drop_ahead();
        }
        out
    }

    /// Forget what was read ahead (a cancelled stream reads nothing more: the next read opens a
    /// new one).
    #[cfg(not(target_arch = "wasm32"))]
    fn drop_ahead(&mut self) {
        self.ahead.stream = None;
        self.ahead.linked = None;
        self.ahead.reads.clear();
        self.ahead.by_index.clear();
    }

    /// Files read ahead and not used yet (tests).
    #[cfg(all(test, not(target_arch = "wasm32")))]
    fn ahead_len(&self) -> usize {
        self.ahead.reads.len()
    }
}

/// What [`apply`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReadOriginals {
    /// Photos brought up to date with their file (virtual copies included).
    pub updated: Vec<PhotoId>,
    /// Listed photos whose file could not be read; they stay unread.
    pub unreadable: usize,
    /// `(read, already)`: a photo whose file, once read, has the same content as another library
    /// photo (an ordinary import would have skipped it as a duplicate). Both records are kept, since
    /// the newly read one may carry the catalog's edits; the pair is reported so the user can choose.
    pub duplicates: Vec<(PhotoId, PhotoId)>,
}

/// Apply the probes: each photo gets the ops Reload would give it, all in one batch that is
/// journaled but isn't an undo step. A photo that changed since it was listed (relinked, read by
/// another run, removed) is skipped.
pub fn apply(s: &mut Session, results: Vec<(Unread, std::result::Result<ProbeInfo, String>)>) -> Result<ReadOriginals> {
    let (mut ops, mut hashed) = (Vec::new(), Vec::new());
    let mut out = ReadOriginals::default();
    let mut hashes: std::collections::HashMap<String, PhotoId> = s
        .catalog
        .photos()
        .filter(|p| p.copy_of.is_none())
        .filter_map(|p| p.content_hash.as_deref().map(|h| (crate::sync::base_hash(h).to_string(), p.id)))
        .collect();
    for (item, info) in results {
        let Ok(info) = info else {
            out.unreadable = out.unreadable.saturating_add(1);
            continue;
        };
        if !s.catalog.photo(item.id).is_some_and(|p| item.current(p)) {
            continue;
        }
        if let Some(h) = info.content_hash.as_deref() {
            let key = crate::sync::base_hash(h).to_string();
            match hashes.get(&key) {
                Some(other) if *other != item.id => out.duplicates.push((item.id, *other)),
                _ => {
                    hashes.insert(key, item.id);
                }
            }
        }
        let reach = crate::cmd::convert::Reach::Family;
        crate::cmd::convert::reload_ops(&s.catalog, item.id, &info, reach, &mut ops, &mut out.updated, &mut hashed);
    }
    if !ops.is_empty() {
        s.apply_system(Op::Batch { ops })?;
    }
    crate::cmd::convert::queue_link_check(s, &hashed);
    for id in &out.updated {
        s.media.forget(*id);
    }
    Ok(out)
}

/// Batch size of `library.readOriginals`: one journaled batch per this many files, so an interrupted
/// run keeps what it read.
const BATCH: usize = 256;

/// Files `library.readOriginals` reads when no `limit` is given: the command runs on the session's
/// thread, so over `--connect` an unbounded run would hold the app for a large library. Callers
/// loop on `remaining`.
const DEFAULT_LIMIT: usize = 1024;

fn read_originals(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "library.readOriginals";
    // without file access every probe "succeeds" with nothing known
    let fp = file_probe(s).ok_or_else(|| bad(C, "this session has no file access"))?;
    let limit = match p.get("limit") {
        None | Some(Value::Null) => DEFAULT_LIMIT,
        Some(v) => v.as_u64().and_then(|n| usize::try_from(n).ok()).ok_or_else(|| bad(C, "`limit` must be a whole number ≥ 0"))?,
    };
    let mut tried = HashSet::new();
    let (mut updated, mut unreadable, mut checked, mut duplicates) = (Vec::new(), 0usize, 0usize, Vec::new());
    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    // the next batches' files are read while one is applied (and across their volumes)
    let mut reader = OriginalsReader::new(fp);
    while checked < limit {
        let (batch, next) = next_batch(&s.catalog, &tried, BATCH, limit.saturating_sub(checked));
        if batch.is_empty() {
            break;
        }
        checked = checked.saturating_add(batch.len());
        tried.extend(batch.iter().map(|u| u.id));
        let r = apply(s, reader.read(batch, &next, &cancel, &|_| {}))?;
        updated.extend(r.updated);
        unreadable = unreadable.saturating_add(r.unreadable);
        duplicates.extend(r.duplicates);
    }
    Ok(json!({
        "checked": checked,
        "updated": updated.iter().map(|i| i.0).collect::<Vec<_>>(),
        "unreadable": unreadable,
        "duplicates": duplicates.iter().map(|(a, b)| json!({"id": a.0, "sameAs": b.0})).collect::<Vec<_>>(),
        "remaining": count_unread(&s.catalog),
    }))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        "library.readOriginals",
        "Read Originals Not Yet Read",
        [],
        None,
        "{limit?: int}: read the originals of photos added without reading their file (a Lightroom catalog import adds its photos from the catalog first; the app then reads their originals in the background), at most `limit` of them (default 1024; call again while `remaining` > 0), and fill in what needs the file as Reload from Disk would: content hash, SHA-1, file size, dimensions, empty camera fields, embedded lens corrections, as-shot white balance; not an undo step, ratings, keywords, develop settings and history are kept; files that can't be read (an offline drive) are left for next time; a read file whose content matches another library photo is kept and reported in `duplicates` → {checked, updated: [ids], unreadable, duplicates: [{id, sameAs}], remaining}",
        always,
        read_originals
    )]
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use super::*;
    use lcc_catalog::Source;

    fn unread_photo(id: u64, path: &str) -> Photo {
        let mut p = Photo::new(PhotoId(id), Source::File { path: path.into() }, "a.NEF", "NEF", 30, 20, "2026-01-01");
        p.kind = MediaKind::Raw;
        p.rating = 4;
        p
    }

    fn session_with(photos: Vec<Photo>, probe: FileProbe) -> Session {
        let mut s = Session::new();
        for p in photos {
            s.catalog.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
        }
        s.media.file_probe = Some(probe);
        s
    }

    fn probe_hashing(calls: Arc<AtomicUsize>) -> FileProbe {
        Arc::new(move |path: &str| {
            calls.fetch_add(1, Ordering::Relaxed);
            if path.contains("offline") {
                return Err(format!("{path}: not found"));
            }
            Ok(ProbeInfo {
                width: 60,
                height: 40,
                file_size: 1234,
                format: "NEF".into(),
                content_hash: Some(format!("hash-{path}")),
                ..Default::default()
            })
        })
    }

    #[test]
    fn reading_fills_file_facts_and_keeps_catalog_data_then_lists_nothing_more() {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut s = session_with(vec![unread_photo(1, "/x/a.NEF"), unread_photo(2, "/offline/b.NEF")], probe_hashing(calls.clone()));
        assert_eq!(count_unread(&s.catalog), 2);
        let r = s.execute("library.readOriginals", &json!({})).unwrap();
        assert_eq!(r["updated"], json!([1]), "{r}");
        assert_eq!(r["unreadable"], 1, "{r}");
        let p = s.catalog.photo(PhotoId(1)).unwrap();
        assert_eq!(p.content_hash.as_deref(), Some("hash-/x/a.NEF"));
        assert_eq!((p.width, p.height, p.file_size), (60, 40, 1234));
        assert_eq!(p.rating, 4, "catalog data is kept");
        assert_eq!(r["remaining"], 1, "the offline original stays unread for next time: {r}");
        assert!(s.undo.is_empty(), "reading originals is not an undo step");
        // a second run re-reads only what is still unread
        let before = calls.load(Ordering::Relaxed);
        s.execute("library.readOriginals", &json!({})).unwrap();
        assert_eq!(calls.load(Ordering::Relaxed) - before, 1);
    }

    #[test]
    fn a_read_original_matching_another_photo_is_reported_as_a_duplicate_and_kept() {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut have = unread_photo(1, "/x/a.NEF");
        have.content_hash = Some("hash-/x/a.NEF".into());
        // the same file under another spelling hashes the same
        let probe: FileProbe = Arc::new(move |_path: &str| {
            calls.fetch_add(1, Ordering::Relaxed);
            Ok(ProbeInfo { width: 6, height: 4, format: "NEF".into(), content_hash: Some("hash-/x/a.NEF".into()), ..Default::default() })
        });
        let mut s = session_with(vec![have, unread_photo(2, "/y/a.NEF")], probe);
        let r = s.execute("library.readOriginals", &json!({})).unwrap();
        assert_eq!(r["duplicates"], json!([{"id": 2, "sameAs": 1}]), "{r}");
        assert_eq!(s.catalog.len(), 2, "both records are kept");
        assert_eq!(r["remaining"], 0, "the duplicate is read, not listed again: {r}");
    }

    #[test]
    fn without_a_limit_a_run_stops_at_the_default_and_reports_the_rest() {
        let calls = Arc::new(AtomicUsize::new(0));
        let n = DEFAULT_LIMIT as u64 + 3;
        let photos = (1..=n).map(|i| unread_photo(i, &format!("/x/{i}.NEF"))).collect();
        let mut s = session_with(photos, probe_hashing(calls));
        let r = s.execute("library.readOriginals", &json!({})).unwrap();
        assert_eq!(r["checked"], DEFAULT_LIMIT, "{}", r["checked"]);
        assert_eq!(r["remaining"], 3);
    }

    #[test]
    fn a_limit_reads_part_and_the_rest_resumes() {
        let calls = Arc::new(AtomicUsize::new(0));
        let photos = (1..=5).map(|i| unread_photo(i, &format!("/x/{i}.NEF"))).collect();
        let mut s = session_with(photos, probe_hashing(calls));
        let r = s.execute("library.readOriginals", &json!({"limit": 2})).unwrap();
        assert_eq!(r["checked"], 2);
        assert_eq!(r["remaining"], 3);
        let r = s.execute("library.readOriginals", &json!({})).unwrap();
        assert_eq!(r["remaining"], 0, "{r}");
    }

    #[test]
    fn hostile_params_and_photos_are_refused_or_skipped_without_panicking() {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut copy = unread_photo(2, "/x/a.NEF");
        copy.copy_of = Some(PhotoId(1));
        let mut video = unread_photo(3, "/x/v.mov");
        video.kind = MediaKind::Video;
        let mut s = session_with(vec![unread_photo(1, "/x/a.NEF"), copy, video, unread_photo(4, "")], probe_hashing(calls));
        for limit in [json!(-1), json!(1.5), json!("x"), json!(u64::MAX)] {
            let r = s.execute("library.readOriginals", &json!({ "limit": limit }));
            assert!(r.is_ok() || r.is_err(), "no panic for {limit}");
        }
        assert!(s.execute("library.readOriginals", &json!({"limit": -1})).is_err());
        let listed = unread(&s.catalog, &HashSet::new(), usize::MAX);
        assert!(listed.iter().all(|u| u.id != PhotoId(2) && u.id != PhotoId(3)), "copies and videos are not listed: {listed:?}");
        // a session without file access refuses instead of marking everything read
        let mut bare = Session::new();
        assert!(bare.execute("library.readOriginals", &json!({})).is_err());
    }

    /// A probe giving files under `dup` one content (duplicates of each other), failing `offline`
    /// files, counting its calls per path.
    #[cfg(not(target_arch = "wasm32"))]
    fn probe_counting(calls: Arc<std::sync::Mutex<Vec<String>>>) -> FileProbe {
        Arc::new(move |path: &str| {
            calls.lock().unwrap().push(path.to_string());
            if path.contains("offline") {
                return Err(format!("{path}: not found"));
            }
            let hash = if path.contains("dup") { "hash-dup".to_string() } else { format!("hash-{path}") };
            Ok(ProbeInfo { width: 60, height: 40, file_size: 1234, format: "NEF".into(), content_hash: Some(hash), ..Default::default() })
        })
    }

    /// Photos on two volumes (`/a`, `/b`), in id order alternating runs of each, with duplicates
    /// and offline files among them.
    #[cfg(not(target_arch = "wasm32"))]
    fn two_volume_photos() -> Vec<Photo> {
        (1..=23u64)
            .map(|i| {
                let vol = if (i / 4) % 2 == 0 { "a" } else { "b" };
                let name = match i {
                    3 | 11 | 19 => format!("dup{i}"),
                    7 | 16 => format!("offline{i}"),
                    _ => format!("p{i}"),
                };
                unread_photo(i, &format!("/{vol}/{name}.NEF"))
            })
            .collect()
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn context() -> Arc<crate::io_sched::IoContext> {
        use crate::io_sched::clock::{FakeClock, SEC};
        let clock = Arc::new(FakeClock::default());
        clock.advance(1000 * SEC);
        let points = vec!["/".to_string(), "/a".to_string(), "/b".to_string()];
        let reader: crate::io_sched::mounts::MountReader = Arc::new(move || Some(lcc_catalog::folders::Mounts::from_points(points.clone(), false)));
        Arc::new(crate::io_sched::IoContext::new(clock, reader, std::time::Duration::from_millis(1)))
    }

    /// The batches' results (updated, unreadable, duplicate pairs) and the photos' hashes after.
    #[cfg(not(target_arch = "wasm32"))]
    fn read_all(s: &mut Session, batch: usize, reader: Option<&mut OriginalsReader>, probe: &FileProbe) -> (Vec<ReadOriginals>, Vec<Option<String>>) {
        let mut tried = HashSet::new();
        let mut out = Vec::new();
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let mut reader = reader;
        loop {
            let (listed, next) = next_batch(&s.catalog, &tried, batch, usize::MAX);
            if listed.is_empty() {
                break;
            }
            tried.extend(listed.iter().map(|u| u.id));
            let results = match reader.as_deref_mut() {
                Some(r) => r.read(listed, &next, &cancel, &|_| {}),
                None => probe_cancellable_with_progress(probe, listed, &cancel, &|_| {}),
            };
            out.push(apply(s, results).unwrap());
        }
        let hashes = s.catalog.photos().map(|p| p.content_hash.clone()).collect();
        (out, hashes)
    }

    /// The batch and its look-ahead stay within the limit (at, just below, just above the batch,
    /// and the batch plus its look-ahead): the batch first, then at most two batches more.
    #[test]
    fn the_next_batch_and_its_look_ahead_stay_within_the_limit() {
        let photos = (1..=20u64).map(|i| unread_photo(i, &format!("/x/{i}.NEF"))).collect();
        let s = session_with(photos, probe_hashing(Arc::new(AtomicUsize::new(0))));
        let none = HashSet::new();
        let lens = |limit| {
            let (b, n) = next_batch(&s.catalog, &none, 3, limit);
            (b.len(), n.len())
        };
        for (limit, want) in [(0, (0, 0)), (2, (2, 0)), (3, (3, 0)), (4, (3, 1)), (8, (3, 5)), (9, (3, 6)), (10, (3, 6)), (usize::MAX, (3, 6))] {
            assert_eq!(lens(limit), want, "limit {limit}");
        }
        let (b, n) = next_batch(&s.catalog, &none, 3, usize::MAX);
        let ids: Vec<u64> = b.iter().chain(&n).map(|u| u.id.0).collect();
        assert_eq!(ids, (1..=9).collect::<Vec<_>>(), "in id order, the batch first");
        // fewer photos than the look-ahead: what there is
        let skip: HashSet<PhotoId> = (1..=16).map(PhotoId).collect();
        let (b, n) = next_batch(&s.catalog, &skip, 3, usize::MAX);
        assert_eq!((b.len(), n.len()), (3, 1));
    }

    /// The batches, their results, duplicate pairs and the photos' facts are those of one batch
    /// after another; only the reads run ahead (each file read once).
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn reading_ahead_gives_the_batches_of_one_after_another() {
        for batch in [1, 3, 4, 7] {
            let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
            let probe = probe_counting(calls.clone());
            let mut old = session_with(two_volume_photos(), probe.clone());
            let want = read_all(&mut old, batch, None, &probe);
            let mut new = session_with(two_volume_photos(), probe.clone());
            let before = calls.lock().unwrap().len();
            let mut reader = OriginalsReader::in_context(probe.clone(), context());
            let got = read_all(&mut new, batch, Some(&mut reader), &probe);
            assert_eq!(got, want, "batches of {batch}");
            assert!(want.0.iter().any(|r| !r.duplicates.is_empty()), "duplicate pairs are found");
            let mut read: Vec<String> = calls.lock().unwrap()[before..].to_vec();
            read.sort();
            let n = read.len();
            read.dedup();
            assert_eq!((n, read.len()), (23, 23), "batches of {batch}: each file read once");
        }
    }

    /// At most `AHEAD_BATCHES` batches' worth of files are read ahead (at, just below, just above).
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn reads_ahead_at_most_two_batches() {
        let probe = probe_hashing(Arc::new(AtomicUsize::new(0)));
        let batch: Vec<Unread> = (1..=3).map(|i| Unread { id: PhotoId(i), path: format!("/a/{i}.NEF") }).collect();
        let limit = batch.len() * AHEAD_BATCHES;
        for (n, want) in [(limit - 1, limit - 1), (limit, limit), (limit + 1, limit)] {
            let next: Vec<Unread> = (100..100 + n as u64).map(|i| Unread { id: PhotoId(i), path: format!("/b/{i}.NEF") }).collect();
            let mut reader = OriginalsReader::in_context(probe.clone(), context());
            let r = reader.read(batch.clone(), &next, &Arc::new(AtomicBool::new(false)), &|_| {});
            assert!(r.iter().all(|(_, x)| x.is_ok()));
            assert_eq!(reader.ahead_len(), want, "{n} listed next");
        }
    }

    /// A read ahead for a photo the next batch doesn't list is dropped (the library changed); the
    /// next batch's own photos are read then. A cancel drops what was read ahead.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn a_stale_read_ahead_is_dropped_and_cancel_drops_the_rest() {
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        let probe = probe_counting(calls.clone());
        let u = |i: u64, p: &str| Unread { id: PhotoId(i), path: p.into() };
        let mut reader = OriginalsReader::in_context(probe, context());
        let cancel = Arc::new(AtomicBool::new(false));
        reader.read(vec![u(1, "/a/1.NEF")], &[u(2, "/a/2.NEF"), u(3, "/b/3.NEF")], &cancel, &|_| {});
        // photo 2 was relinked meanwhile: listed with another file, read again; 3 is used as read
        let r = reader.read(vec![u(2, "/b/2-moved.NEF"), u(3, "/b/3.NEF")], &[], &cancel, &|_| {});
        assert_eq!(r[1].1.as_ref().unwrap().content_hash.as_deref(), Some("hash-/b/3.NEF"));
        let read = calls.lock().unwrap().clone();
        assert_eq!(read.iter().filter(|p| p.ends_with("3.NEF")).count(), 1, "{read:?}");
        assert!(read.contains(&"/b/2-moved.NEF".to_string()));
        assert_eq!(reader.ahead_len(), 0);
        // cancelled: nothing is read, and what was read ahead is forgotten
        reader.read(vec![u(4, "/a/4.NEF")], &[u(5, "/a/5.NEF")], &cancel, &|_| {});
        cancel.store(true, Ordering::SeqCst);
        let r = reader.read(vec![u(5, "/a/5.NEF")], &[], &cancel, &|_| {});
        assert!(r[0].1.is_err(), "cancelled: not read");
        assert_eq!(reader.ahead_len(), 0);
    }

    /// The cancel flag stays linked to the reads ahead between calls: set while the caller is busy
    /// applying a batch, the reads ahead not yet started never start (here: reads past the ones
    /// under way, held at a gate until it is set). A call with another flag drops what was read
    /// ahead (the old flag's cancel would stop those, the new one's not) and reads its batch then.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn cancel_stays_linked_to_the_reads_ahead_between_calls() {
        use std::sync::atomic::Ordering::SeqCst;
        let (gate, cancel, early, late) =
            (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)), Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let (g, c, e, l) = (gate.clone(), cancel.clone(), early.clone(), late.clone());
        let probe: FileProbe = Arc::new(move |path: &str| {
            if c.load(SeqCst) { &l } else { &e }.fetch_add(1, SeqCst);
            let start = std::time::Instant::now();
            while path.contains("/ahead/") && !g.load(SeqCst) && start.elapsed() < std::time::Duration::from_secs(20) {
                std::thread::sleep(std::time::Duration::from_micros(200));
            }
            Ok(ProbeInfo { format: "NEF".into(), content_hash: Some(format!("hash-{path}")), ..Default::default() })
        });
        let ctx = context();
        let mut reader = OriginalsReader::in_context(probe, ctx.clone());
        let u = |i: u64, p: String| Unread { id: PhotoId(i), path: p };
        let batch: Vec<Unread> = (1..=6).map(|i| u(i, format!("/a/{i}.NEF"))).collect();
        let next: Vec<Unread> = (100..112).map(|i| u(i, format!("/a/ahead/{i}.NEF"))).collect();
        let r = reader.read(batch, &next, &cancel, &|_| {});
        assert!(r.iter().all(|(_, x)| x.is_ok()));
        // the reads under way have begun (the rest queued behind them)
        let w = crate::import::workers(usize::MAX);
        let under_way = 6 + w.min(next.len());
        let start = std::time::Instant::now();
        while early.load(SeqCst) < under_way {
            assert!(start.elapsed() < std::time::Duration::from_secs(20), "reads ahead under way");
            std::thread::sleep(std::time::Duration::from_micros(200));
        }
        // between calls: cancelled, then the gate opens
        cancel.store(true, SeqCst);
        gate.store(true, SeqCst);
        let start = std::time::Instant::now();
        while ctx.pending.load(SeqCst) > 0 {
            assert!(start.elapsed() < std::time::Duration::from_secs(20), "the pool never settled");
            std::thread::sleep(std::time::Duration::from_micros(200));
        }
        assert_eq!(late.load(SeqCst), 0, "no read ahead started after the cancel");
        assert_eq!(early.load(SeqCst), under_way);

        // another flag: what was read ahead is dropped and its batch read then
        let (first, other) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
        let mut reader = OriginalsReader::in_context(probe_hashing(Arc::new(AtomicUsize::new(0))), context());
        reader.read(vec![u(1, "/a/1.NEF".into())], &[u(2, "/a/2.NEF".into())], &first, &|_| {});
        let r = reader.read(vec![u(2, "/a/2.NEF".into())], &[], &other, &|_| {});
        assert!(r[0].1.is_ok());
        first.store(true, SeqCst);
        let r = reader.read(vec![u(3, "/b/3.NEF".into())], &[], &other, &|_| {});
        assert!(r[0].1.is_ok(), "the first flag no longer stops this reader");
    }

    /// After the pool's supervisor stopped (a panic) no rule watches a read hung on a dead drive:
    /// a cancel still ends the batch (the hung file not read), instead of waiting for ever.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn cancel_ends_a_batch_whose_read_hangs_after_the_pool_stopped() {
        use std::sync::atomic::Ordering::SeqCst;
        let (gate, started) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicUsize::new(0)));
        let (g, st) = (gate.clone(), started.clone());
        let probe: FileProbe = Arc::new(move |path: &str| {
            st.fetch_add(1, SeqCst);
            let start = std::time::Instant::now();
            while path.contains("hung") && !g.load(SeqCst) && start.elapsed() < std::time::Duration::from_secs(60) {
                std::thread::sleep(std::time::Duration::from_micros(200));
            }
            Ok(ProbeInfo { format: "NEF".into(), content_hash: Some(format!("hash-{path}")), ..Default::default() })
        });
        let ctx = context();
        let mut reader = OriginalsReader::in_context(probe, ctx.clone());
        let cancel = Arc::new(AtomicBool::new(false));
        let u = |i: u64, p: &str| Unread { id: PhotoId(i), path: p.into() };
        let c = cancel.clone();
        let h = std::thread::spawn(move || reader.read(vec![u(1, "/a/hung.NEF"), u(2, "/b/2.NEF")], &[], &c, &|_| {}));
        let start = std::time::Instant::now();
        while started.load(SeqCst) < 2 {
            assert!(start.elapsed() < std::time::Duration::from_secs(20), "the reads to start");
            std::thread::sleep(std::time::Duration::from_micros(200));
        }
        // (the other volume's read delivered)
        std::thread::sleep(std::time::Duration::from_millis(100));
        ctx.panic_supervisor.store(true, SeqCst);
        let start = std::time::Instant::now();
        while ctx.pool_up() {
            assert!(start.elapsed() < std::time::Duration::from_secs(20), "the supervisor to stop");
            std::thread::sleep(std::time::Duration::from_micros(200));
        }
        cancel.store(true, SeqCst);
        let start = std::time::Instant::now();
        while !h.is_finished() && start.elapsed() < std::time::Duration::from_secs(10) {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let finished = h.is_finished();
        gate.store(true, SeqCst);
        assert!(finished, "the batch never returned after the cancel");
        let r = h.join().unwrap();
        assert_eq!(r[0].1.as_ref().err().map(String::as_str), Some("cancelled"), "the hung read is left behind");
        assert!(r[1].1.is_ok(), "the other volume's read was done");
    }

    #[test]
    fn a_result_for_a_photo_relinked_meanwhile_is_dropped_and_cancel_reads_nothing() {
        let calls = Arc::new(AtomicUsize::new(0));
        let probe = probe_hashing(calls.clone());
        let mut s = session_with(vec![unread_photo(1, "/x/a.NEF")], probe.clone());
        let listed = unread(&s.catalog, &HashSet::new(), 10);
        let cancelled = probe_cancellable_with_progress(&probe, listed.clone(), &std::sync::Arc::new(AtomicBool::new(true)), &|_| {});
        assert_eq!(calls.load(Ordering::Relaxed), 0, "a cancelled run reads no file");
        assert!(apply(&mut s, cancelled).unwrap().updated.is_empty());
        let results = probe_cancellable_with_progress(&probe, listed, &std::sync::Arc::new(AtomicBool::new(false)), &|_| {});
        s.catalog
            .apply(Op::Relink { id: PhotoId(1), file_name: "a.NEF".into(), source: Source::File { path: "/y/a.NEF".into() }, format: None })
            .unwrap();
        assert!(apply(&mut s, results).unwrap().updated.is_empty(), "the relinked photo is not given another file's facts");
        assert!(s.catalog.photo(PhotoId(1)).unwrap().content_hash.is_none());
    }
}
