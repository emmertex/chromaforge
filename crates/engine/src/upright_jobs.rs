//! Upright analysed on each photo, as Lightroom does (ported from OneSlips, test notes 2026-10-24 note 4):
//! an automatic Upright mode (Auto, Level, Vertical, Full) that reaches a photo by Auto Sync, Sync
//! Settings or Paste is a recipe, and the photo's own pixels decide the correction. Those paths drop the
//! correction worked out on the other photo; after the commit, the photos whose settings name such a mode
//! but hold no correction are analysed: with [`UprightJobs::background`] (the desktop app) on a worker
//! thread with one activity row for the batch, the results merged into the photos' current settings as
//! they arrive ([`Session::upright_poll`], no undo step of their own); without it (CLI, MCP, tests) at
//! once. Until its correction arrives a photo renders with one worked out for the render
//! (`lcc_pipeline::upright::resolve`), so nothing waits on the analysis.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::atomic::Ordering;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::mpsc::channel;
use std::sync::mpsc::{Receiver, Sender};

use lcc_catalog::{Op, PhotoId};
use lcc_develop::{DevelopSettings, Upright};
use lcc_geom::Homography;

use crate::Session;
#[cfg(not(target_arch = "wasm32"))]
use crate::activity::Cancel;
use crate::activity::TaskGuard;
use crate::media::SourceLevel;

/// Whether settings `d` name an automatic Upright mode but hold no correction for it.
pub(crate) fn needs_analysis(d: &DevelopSettings) -> bool {
    matches!(d.geometry.upright, Upright::Auto | Upright::Level | Upright::Vertical | Upright::Full)
        && d.geometry.upright_transform.is_none()
        && d.section_enabled("geometry")
}

/// Settings `synced` that another photo's edit gave photo settings `own` (Auto Sync, Sync, Paste): an
/// automatic Upright mode travels as a recipe, without the other photo's correction (worked out on this
/// photo after the commit). Guided Upright is drawn on one photo's lines, so it doesn't travel: the photo
/// keeps the Upright it has (mode, correction, guides); the Transform sliders still travel (OneSlips,
/// decision 2026-10-25 item 12).
pub(crate) fn as_recipe(own: &DevelopSettings, synced: &mut DevelopSettings) {
    let (a, b) = (&own.geometry, &synced.geometry);
    if a.upright == b.upright && a.upright_transform == b.upright_transform && a.guides == b.guides {
        return;
    }
    if b.upright == Upright::Guided {
        synced.geometry.upright = a.upright;
        synced.geometry.upright_transform = a.upright_transform;
        synced.geometry.guides = a.guides.clone();
        return;
    }
    synced.geometry.upright_transform = None;
    synced.geometry.guides = a.guides.clone();
}

/// What a worker found for a photo.
struct Found {
    id: PhotoId,
    mode: Upright,
    result: Result<Homography, String>,
}

/// The Upright analyses in flight.
#[derive(Default)]
pub struct UprightJobs {
    /// Analyse on a worker thread (the desktop app); else at once.
    pub background: bool,
    /// Photos queued or being analysed, and for which mode.
    pending: HashMap<PhotoId, Upright>,
    rx: Option<Receiver<Found>>,
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    tx: Option<Sender<Found>>,
    /// The batch's activity row, while photos are pending.
    guard: Option<TaskGuard>,
    cancel: Option<Arc<AtomicBool>>,
}

impl UprightJobs {
    /// Photos still waiting for their analysis.
    pub fn pending(&self) -> usize {
        self.pending.len()
    }
}

/// Photos whose settings `op` sets, with those settings (batches nested at most 16 deep).
fn developed(op: &Op, out: &mut Vec<(PhotoId, Arc<DevelopSettings>)>, depth: usize) {
    match op {
        Op::SetDevelop { id, settings, .. } => out.push((*id, settings.clone())),
        Op::Batch { ops } if depth < 16 => ops.iter().for_each(|o| developed(o, out, depth + 1)),
        _ => {}
    }
}

impl Session {
    /// After `op` was committed: analyse the photos it left with an automatic Upright mode and no
    /// correction.
    pub(crate) fn upright_after(&mut self, op: &Op) {
        let mut set = Vec::new();
        developed(op, &mut set, 0);
        let todo: Vec<(PhotoId, Upright)> = set
            .into_iter()
            .filter(|(id, d)| needs_analysis(d) && self.upright_jobs.pending.get(id) != Some(&d.geometry.upright))
            .map(|(id, d)| (id, d.geometry.upright))
            .collect();
        if todo.is_empty() {
            return;
        }
        if self.upright_jobs.background && cfg!(not(target_arch = "wasm32")) {
            self.upright_queue(todo);
        } else {
            for (id, mode) in todo {
                let result = self.upright_now(id, mode);
                self.upright_land(Found { id, mode, result });
            }
        }
    }

    /// Photo `id`'s correction for `mode`, worked out now.
    pub(crate) fn upright_now(&mut self, id: PhotoId, mode: Upright) -> Result<Homography, String> {
        let src = self.source_now(id, SourceLevel::Preview)?;
        let info = self.source_info(id);
        let d = self.develop_of(id).unwrap_or_default();
        let aux = lcc_pipeline::source_stage::aux_source(&src, &info, &d);
        let a = lcc_pipeline::upright::analyze_source(aux.as_deref().unwrap_or(&src), &info, &d);
        Ok(lcc_pipeline::upright::solve(mode, &a))
    }

    /// Give photo `f.id` what its analysis found, when its settings still ask for that mode without a
    /// correction (an edit made meanwhile wins). No undo step: the app worked it out, not the user.
    fn upright_land(&mut self, f: Found) {
        let t = match f.result {
            Ok(t) => t,
            Err(e) => {
                log::warn!("Upright on photo {}: {e}", f.id.0);
                return;
            }
        };
        let Some(photo) = self.catalog.photo(f.id) else { return };
        let mut d = (*photo.develop).clone();
        if d.geometry.upright != f.mode || d.geometry.upright_transform.is_some() {
            return;
        }
        d.geometry.upright_transform = Some(t);
        let d = self.fit_constrained_crop(f.id, d);
        let op = Op::SetDevelop { id: f.id, settings: Arc::new(d), label: "Upright".into(), edited: photo.edited.clone() };
        if let Err(e) = self.apply_unrecorded(op) {
            log::warn!("Upright on photo {}: {e}", f.id.0);
        }
    }

    /// Analyse `todo` on a worker thread (one activity row for all of them).
    #[cfg(not(target_arch = "wasm32"))]
    fn upright_queue(&mut self, todo: Vec<(PhotoId, Upright)>) {
        let mut jobs = Vec::new();
        for (id, mode) in todo {
            let Some(p) = self.catalog.photo(id).cloned() else { continue };
            let d = self.develop_of(id).unwrap_or_default();
            let src = self.media.source_ref(&p, SourceLevel::Preview);
            let info = self.source_info(id);
            jobs.push((id, mode, src, info, d));
        }
        if jobs.is_empty() {
            return;
        }
        let uj = &mut self.upright_jobs;
        if uj.tx.is_none() {
            let (tx, rx) = channel();
            uj.tx = Some(tx);
            uj.rx = Some(rx);
        }
        let Some(tx) = uj.tx.clone() else { return };
        for (id, mode, ..) in &jobs {
            uj.pending.insert(*id, *mode);
        }
        let cancel = uj.cancel.get_or_insert_with(|| Arc::new(AtomicBool::new(false))).clone();
        let total = uj.pending.len() as u64;
        let guard = uj.guard.get_or_insert_with(|| self.activity.start_batch("upright", "Upright", Cancel::Flag(cancel.clone())));
        guard.progress(0, total);
        let spawned = std::thread::Builder::new().name("upright".into()).spawn(move || {
            for (id, mode, src, info, d) in jobs {
                if cancel.load(Ordering::Relaxed) {
                    let _ = tx.send(Found { id, mode, result: Err("cancelled".into()) });
                    continue;
                }
                let result = src.load_source().map(|r| {
                    let aux = lcc_pipeline::source_stage::aux_source(&r.image, &info, &d);
                    let a = lcc_pipeline::upright::analyze_source(aux.as_deref().unwrap_or(&r.image), &info, &d);
                    lcc_pipeline::upright::solve(mode, &a)
                });
                if tx.send(Found { id, mode, result }).is_err() {
                    return;
                }
            }
        });
        if let Err(e) = spawned {
            log::warn!("Upright: no worker thread ({e}); analysing now");
            let ids: Vec<(PhotoId, Upright)> = self.upright_jobs.pending.drain().collect();
            self.upright_jobs.guard = None;
            for (id, mode) in ids {
                let result = self.upright_now(id, mode);
                self.upright_land(Found { id, mode, result });
            }
        }
    }

    #[cfg(target_arch = "wasm32")]
    fn upright_queue(&mut self, todo: Vec<(PhotoId, Upright)>) {
        for (id, mode) in todo {
            let result = self.upright_now(id, mode);
            self.upright_land(Found { id, mode, result });
        }
    }

    /// Apply the background analyses that finished (the desktop app calls this every frame) → whether
    /// a photo's settings changed.
    pub fn upright_poll(&mut self) -> bool {
        let mut found = Vec::new();
        if let Some(rx) = &self.upright_jobs.rx {
            while let Ok(f) = rx.try_recv() {
                found.push(f);
            }
        }
        let changed = !found.is_empty();
        for f in found {
            if self.upright_jobs.pending.get(&f.id) == Some(&f.mode) {
                self.upright_jobs.pending.remove(&f.id);
            }
            if let Some(g) = &self.upright_jobs.guard {
                match &f.result {
                    Ok(_) => g.item_done(),
                    Err(e) if e == "cancelled" => {}
                    Err(e) => g.item_failed(e),
                }
            }
            self.upright_land(f);
        }
        if self.upright_jobs.pending.is_empty() {
            self.upright_jobs.guard = None;
            self.upright_jobs.cancel = None;
        }
        changed
    }
}
