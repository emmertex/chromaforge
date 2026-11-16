//! Metadata ▸ Update DNG Preview & Metadata: for each selected DNG, render a JPEG preview of the
//! current edit and rewrite the file with that preview and the photo's current XMP embedded, the
//! raw data untouched ([`lcc_raw::dngupdate`] makes the new bytes; this module sets the work up,
//! merges the XMP and replaces the file).
//!
//! Split like [`crate::render_dng`], so the desktop app can run the heavy part off the UI thread:
//! [`prepare`] (on the session: checks the params and sets each photo's preview render up),
//! [`UpdateJob::run`] (no session: reads, renders, rewrites), [`finish`] (on the session: drops
//! what the session cached of the files). `photo.updateDngPreview` runs all three in a row.
//!
//! The file is replaced atomically (a temp file next to it, synced, renamed over it), and only
//! when it is still what was read: a file that changed meanwhile is left alone. DNGs this will not
//! touch (Adobe-written ones, unknown structures) are reported with the reason, never patched.
//! Not a DNG: reported as skipped.

use std::path::Path;

use lcc_catalog::{PhotoId, Source};
use serde_json::{Value, json};

use crate::export::{ExportOptions, PreparedExport};
use crate::{EngineError, Result, Session};

pub const COMMAND: &str = "photo.updateDngPreview";

/// Most photos one update takes.
pub const MAX_PHOTOS: usize = 500;
/// Default and range of the preview's long edge (pixels).
pub const DEFAULT_EDGE: u32 = 1024;
pub const EDGE_RANGE: (u32, u32) = (256, 4096);
/// Default JPEG quality of the preview.
pub const DEFAULT_QUALITY: u8 = 85;
/// The biggest file read: offsets in the rewritten file are 32-bit.
const MAX_FILE: u64 = (1 << 32) - (1 << 26);

fn bad(msg: impl Into<String>) -> EngineError {
    EngineError::BadParams { cmd: COMMAND.into(), msg: msg.into() }
}

struct Item {
    id: PhotoId,
    name: String,
    path: String,
    /// The preview's render; `None` for a photo that is not a DNG on disk (`skipped`).
    export: Option<PreparedExport>,
    /// The photo's XMP as the app writes it (merged with the packet in the file when it runs).
    xmp: String,
    skipped: Option<String>,
}

/// The updates of one command, set up from the session ([`prepare`]).
pub struct UpdateJob {
    items: Vec<Item>,
}

/// What [`UpdateJob::run`] did with one photo.
#[derive(Clone, Debug)]
pub struct Updated {
    pub id: PhotoId,
    pub name: String,
    pub path: String,
    pub result: std::result::Result<Done, Refusal>,
}

/// A rewritten file.
#[derive(Clone, Debug)]
pub struct Done {
    pub bytes: u64,
    pub preview: (u32, u32),
    /// Previews of this app's that were replaced.
    pub replaced: usize,
}

/// Why a file was left as it was: not a DNG (`skipped`), or refused / failed (`error`).
#[derive(Clone, Debug)]
pub enum Refusal {
    Skipped(String),
    Error(String),
}

impl Refusal {
    fn message(&self) -> &str {
        match self {
            Self::Skipped(m) | Self::Error(m) => m,
        }
    }
}

pub(crate) fn is_dng(path: &str) -> bool {
    Path::new(path).extension().is_some_and(|e| e.eq_ignore_ascii_case("dng"))
}

/// Check `p` (`ids?`, `longEdge?`, `quality?`) and set up each target photo's preview render.
pub fn prepare(s: &mut Session, p: &Value) -> Result<UpdateJob> {
    let mut ids = s.targets(p)?;
    ids.dedup();
    if ids.is_empty() {
        return Err(bad("no photo selected"));
    }
    if ids.len() > MAX_PHOTOS {
        return Err(bad(format!("{} photos selected: update at most {MAX_PHOTOS} at a time", ids.len())));
    }
    let edge = match p.get("longEdge") {
        None | Some(Value::Null) => DEFAULT_EDGE,
        Some(v) => v
            .as_u64()
            .and_then(|e| u32::try_from(e).ok())
            .filter(|e| (EDGE_RANGE.0..=EDGE_RANGE.1).contains(e))
            .ok_or_else(|| bad(format!("longEdge must be a whole number from {} to {}", EDGE_RANGE.0, EDGE_RANGE.1)))?,
    };
    let quality = match p.get("quality") {
        None | Some(Value::Null) => DEFAULT_QUALITY,
        Some(v) => {
            v.as_u64().filter(|q| (1..=100).contains(q)).map(|q| q as u8).ok_or_else(|| bad("quality must be a whole number from 1 to 100"))?
        }
    };
    // sRGB, no metadata of its own: the preview is pixels, the metadata is the XMP packet
    let opts = ExportOptions::from_json(&json!({"format": "jpeg", "quality": quality, "longEdge": edge, "colorSpace": "srgb", "metadata": "none"}));
    let mut items = Vec::with_capacity(ids.len());
    for id in ids {
        let ph = s.catalog.photo(id).ok_or_else(|| bad(format!("no photo {}", id.0)))?;
        let name = ph.file_name.clone();
        let path = match &ph.source {
            Source::File { path } if is_dng(path) => path.clone(),
            _ => {
                items.push(Item { id, name, path: String::new(), export: None, xmp: String::new(), skipped: Some("it is not a DNG file".into()) });
                continue;
            }
        };
        let xmp = crate::sidecar::sidecar_packet(ph, &s.catalog);
        let export = crate::export::prepare_export(s, id, &opts, 1).map_err(bad)?;
        items.push(Item { id, name, path, export: Some(export), xmp, skipped: None });
    }
    if items.iter().all(|i| i.skipped.is_some()) {
        return Err(bad("none of the selected photos is a DNG file"));
    }
    Ok(UpdateJob { items })
}

impl UpdateJob {
    pub fn len(&self) -> usize {
        self.items.len()
    }
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Rewrite each DNG. `progress(done)` is asked before each photo; `false` stops there
    /// (cancelled). Returns the outcomes and whether it was cancelled.
    pub fn run(self, progress: &mut dyn FnMut(usize) -> bool) -> (Vec<Updated>, bool) {
        let mut out = Vec::with_capacity(self.items.len());
        for (i, item) in self.items.into_iter().enumerate() {
            if !progress(i) {
                return (out, true);
            }
            let (id, name, path) = (item.id, item.name.clone(), item.path.clone());
            let result = match item.skipped {
                Some(why) => Err(Refusal::Skipped(why)),
                None => update_one(item).map_err(Refusal::Error),
            };
            out.push(Updated { id, name, path, result });
        }
        (out, false)
    }
}

/// Merge the app's packet into the one the file has: another writer's data in it stays; a packet that
/// cannot be read is a refusal, never a silent replacement.
fn merged_xmp(existing: Option<Vec<u8>>, fresh: &str) -> std::result::Result<String, String> {
    match existing {
        None => Ok(fresh.to_string()),
        Some(bytes) => {
            let text = String::from_utf8(bytes).map_err(|_| "its embedded XMP is not text this can read".to_string())?;
            crate::sidecar::merge_packet(&text, fresh).map_err(|e| format!("its embedded XMP could not be read ({e})"))
        }
    }
}

/// One photo: read, check, render, build, replace.
fn update_one(item: Item) -> std::result::Result<Done, String> {
    let path = Path::new(&item.path);
    let before = std::fs::metadata(path).map_err(|e| format!("{}: {e}", item.path))?;
    let file = crate::bounded_read::read_bounded(path, MAX_FILE).map_err(|e| format!("{}: {e}", item.path))?;
    // refuse before rendering anything
    lcc_raw::dngupdate::check(&file).map_err(|e| e.to_string())?;
    let existing = lcc_raw::dngupdate::embedded_xmp(&file).map_err(|e| e.to_string())?;
    let xmp = merged_xmp(existing, &item.xmp)?;
    let export = item.export.ok_or("no preview was prepared")?;
    let preview = export.run()?;
    let made = lcc_raw::dngupdate::update(&file, &preview.bytes, xmp.as_bytes()).map_err(|e| e.to_string())?;
    // only over the file that was read: one that changed meanwhile (a camera tool, a sync) is left alone
    let now = std::fs::metadata(path).map_err(|e| format!("{}: {e}", item.path))?;
    if now.len() != before.len() || now.modified().ok() != before.modified().ok() {
        return Err("the file changed while its preview was being made; try again".into());
    }
    lcc_catalog::safe_file::write_atomic(path, &made.bytes).map_err(|e| format!("{}: {e}", item.path))?;
    Ok(Done { bytes: made.bytes.len() as u64, preview: made.preview_size, replaced: made.replaced_previews })
}

/// Forget what the session cached of the rewritten files (their embedded preview and size
/// changed) → `{updated: [{id, path, bytes, preview: [w, h], replacedPreviews}], skipped: [{id, reason}],
/// failed: [{id, error}], cancelled}`. A single photo that failed is an error.
pub fn finish(s: &mut Session, done: Vec<Updated>, cancelled: bool) -> Result<Value> {
    let single = done.len() == 1 && !cancelled;
    let (mut updated, mut skipped, mut failed) = (Vec::new(), Vec::new(), Vec::new());
    for u in done {
        match u.result {
            Ok(d) => {
                s.media.forget(u.id);
                updated.push(
                    json!({"id": u.id.0, "path": u.path, "bytes": d.bytes, "preview": [d.preview.0, d.preview.1], "replacedPreviews": d.replaced}),
                );
            }
            Err(r @ Refusal::Error(_)) if single => return Err(bad(format!("{}: {}", u.name, r.message()))),
            Err(Refusal::Skipped(why)) => skipped.push(json!({"id": u.id.0, "name": u.name, "reason": why})),
            Err(Refusal::Error(e)) => failed.push(json!({"id": u.id.0, "name": u.name, "error": e})),
        }
    }
    Ok(json!({"updated": updated, "skipped": skipped, "failed": failed, "cancelled": cancelled}))
}

/// `photo.updateDngPreview` all in one go (a batch shows in the activity stack and can be cancelled
/// between photos); the desktop app runs [`UpdateJob::run`] on a worker instead.
pub fn run_command(s: &mut Session, p: &Value) -> Result<Value> {
    let job = prepare(s, p)?;
    let n = job.len();
    let task = Some(s.activity.start("dngUpdate", "Updating DNG previews", crate::activity::Cancel::Yes));
    let (done, cancelled) = job.run(&mut |i| {
        let Some(t) = &task else { return true };
        t.progress(i as u64, n as u64);
        !t.is_cancelled()
    });
    drop(task);
    finish(s, done, cancelled)
}
