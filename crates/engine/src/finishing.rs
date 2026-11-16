//! Finishing an import survives a restart (ported from OneSlips' LR-IMP-WINDOW): what
//! [`Session::finish_import`] was asked to do (the photos, and which previews) is kept in the
//! library (`import-finishing.json`) until its work is done. A library opened with a plan left in
//! it (the app quit or crashed part-way through a large import) takes it up again with the next
//! [`Session::finishing_tick`]: the AI masks still missing are queued, Smooth Skin looks for the
//! skin it hasn't found, and the previews are built (those already built are quick to skip). A
//! finish the user stops (the ✕ on its preview row) is not taken up again.
//!
//! Driven by [`Session::finishing_tick`]: every frame in the app; cheap when there's nothing to do.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use lcc_catalog::PhotoId;
use serde::{Deserialize, Serialize};

use crate::Session;
use crate::cmd::previews::PreviewBuild;

/// The library file that keeps an unfinished plan across restarts.
pub const FILE: &str = "import-finishing.json";

/// The most photos a kept plan names (a library's own file, but read as hostile input).
const MAX_IDS: usize = 1 << 20;

/// What an import asked to be finished.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct FinishPlan {
    pub ids: Vec<u64>,
    /// `none` | `standard` | `full` (1:1), as `library.finishImport` takes it.
    pub previews: String,
}

/// A finish at work: its plan, and the preview build it started (if it asked for previews).
#[derive(Debug)]
pub struct Finishing {
    pub plan: FinishPlan,
    build: Option<Arc<PreviewBuild>>,
}

impl Session {
    /// Keep `plan` as the library's unfinished finish (joined with one at work, whose photos it
    /// takes along), with `build` its preview build. Called by [`Session::finish_import`] once
    /// its stages have started.
    pub(crate) fn finishing_started(&mut self, mut plan: FinishPlan, build: Option<Arc<PreviewBuild>>) {
        if let Some(old) = self.finishing.take() {
            let known: std::collections::HashSet<u64> = plan.ids.iter().copied().collect();
            plan.ids.extend(old.plan.ids.into_iter().filter(|id| !known.contains(id)));
        }
        self.finish_pending = None;
        self.save_finishing(Some(&plan));
        self.finishing = Some(Finishing { plan, build });
    }

    /// Advance the finish (cheap; call every frame): take up a plan the library kept, and forget
    /// the plan once its work is done or the user stopped it. → whether a finish is at work.
    pub fn finishing_tick(&mut self) -> bool {
        if self.finishing.is_none() && self.finish_pending.is_some() {
            // someone else's preview build: wait for it, then build ours
            if self.preview_build.as_ref().is_some_and(|b| !b.finished.load(Ordering::Relaxed)) {
                return true;
            }
            if let Some(plan) = self.finish_pending.take() {
                log::info!("resuming the unfinished import of {} photo(s)", plan.ids.len());
                match self.finish_import(&plan.ids, &plan.previews) {
                    // nothing was left to do: the plan is spent
                    Ok(_) if self.finishing.is_none() => self.save_finishing(None),
                    Ok(_) => {}
                    Err(e) => {
                        log::warn!("finishing import: {e}");
                        self.save_finishing(None);
                    }
                }
            }
        }
        let Some(f) = &self.finishing else { return false };
        let stopped = f.build.as_ref().is_some_and(|b| b.cancel.load(Ordering::Relaxed));
        let built = f.build.as_ref().is_none_or(|b| b.finished.load(Ordering::Relaxed));
        if !stopped && !(built && !self.finishing_analyses(&f.plan.ids)) {
            return true;
        }
        if stopped {
            log::info!("finishing import: stopped; not taken up again");
        }
        self.finishing = None;
        self.save_finishing(None);
        false
    }

    /// Whether the AI masks or the Smooth Skin analysis of photos `ids` are still being worked out.
    fn finishing_analyses(&self, ids: &[u64]) -> bool {
        self.segmenter.recomputing() || ids.iter().any(|id| self.skin_pending(PhotoId(*id)))
    }

    /// Keep (`Some`) or clear the plan in the library, for a restart. (Cleared by an empty plan: the
    /// library's store has no remove.)
    fn save_finishing(&mut self, plan: Option<&FinishPlan>) {
        let Some(lib) = self.library.as_mut() else { return };
        let empty = FinishPlan::default();
        let plan = plan.unwrap_or(&empty);
        // nothing kept and nothing to keep: no file is written
        if plan.ids.is_empty() && !matches!(lib.files.read(FILE), Ok(Some(b)) if !b.is_empty()) {
            return;
        }
        let bytes = match serde_json::to_vec_pretty(plan) {
            Ok(b) => b,
            Err(e) => {
                log::warn!("finishing import: {FILE}: {e}");
                return;
            }
        };
        if let Err(e) = lib.files.write_atomic(FILE, &bytes) {
            log::warn!("finishing import: {FILE}: {e}");
        }
    }

    /// When a library opens: an unfinished plan it kept, taken up by the next tick. An unreadable
    /// file is left alone (and said so in the log), never a failed open.
    pub(crate) fn load_finishing(&mut self) {
        self.finishing = None;
        self.finish_pending = None;
        let Some(lib) = self.library.as_mut() else { return };
        let Ok(Some(bytes)) = lib.files.read(FILE) else { return };
        if bytes.is_empty() {
            return;
        }
        match serde_json::from_slice::<FinishPlan>(&bytes) {
            Ok(mut plan) if !plan.ids.is_empty() => {
                plan.ids.truncate(MAX_IDS);
                self.finish_pending = Some(plan);
            }
            Ok(_) => {}
            Err(e) => log::warn!("finishing import: {FILE} unreadable, left alone: {e}"),
        }
    }
}
