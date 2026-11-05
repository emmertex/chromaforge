//! Background results never overwrite edits (ported from OneSlips 1c785da00, test notes 2026-10-24 note 18, "the
//! edit on top is queued, never dropped"): what an AI mask recompute, a Retouch parts analysis or a Smooth Skin
//! analysis finds is MERGED into the photo's settings as they are when it arrives, only into its own fields, and
//! only where what it was computed for is still there. It also goes into every undo and redo step, a running
//! interaction's starting points and the photo's History steps that hold the same thing, so an Undo, a cancelled
//! drag or a click on a History step made while it ran doesn't drop it, and History keeps matching the settings.

use std::sync::Arc;

use lcc_catalog::{HistoryStep, Op, PhotoId};
use lcc_develop::DevelopSettings;

use crate::Session;

/// Deepest nesting of batches walked.
const MAX_DEPTH: usize = 16;

/// `patch` on a settings value held in an `Arc`, cloned only when it changes something.
fn patch_arc(s: &mut Arc<DevelopSettings>, patch: &dyn Fn(&mut DevelopSettings) -> bool) -> bool {
    let mut d = (**s).clone();
    if patch(&mut d) {
        *s = Arc::new(d);
        true
    } else {
        false
    }
}

fn patch_steps(steps: &mut [HistoryStep], patch: &dyn Fn(&mut DevelopSettings) -> bool) -> bool {
    let mut any = false;
    for st in steps {
        any |= patch_arc(&mut st.settings, patch);
    }
    any
}

/// `patch` on every settings value of photo `id` in `op` (its develop settings, the History steps it pushes or puts
/// back).
fn patch_op(op: &mut Op, id: PhotoId, patch: &dyn Fn(&mut DevelopSettings) -> bool, depth: usize) {
    match op {
        Op::SetDevelop { id: p, settings, .. } if *p == id => {
            patch_arc(settings, patch);
        }
        Op::PushHistory { id: p, step } if *p == id => {
            patch_arc(&mut step.settings, patch);
        }
        Op::SetHistory { id: p, history } if *p == id => {
            patch_steps(history, patch);
        }
        Op::Batch { ops } if depth < MAX_DEPTH => {
            for o in ops {
                patch_op(o, id, patch, depth.saturating_add(1));
            }
        }
        _ => {}
    }
}

impl Session {
    /// Give photo `id` a background result: `patch` puts it into a settings value and says whether it changed
    /// anything (it touches only its own fields, and only where what it was computed for is still there). Into the
    /// current settings (edits made meanwhile are kept), every undo and redo step, a running interaction's starting
    /// points and the photo's History steps; no undo step of its own (saved and written to the sidecar like any
    /// change). `touch_edited`: the photo counts as edited now (an AI mask's selection; an analysis isn't an edit). →
    /// whether the current settings changed.
    pub(crate) fn merge_background(
        &mut self,
        id: PhotoId,
        label: &str,
        touch_edited: bool,
        patch: &dyn Fn(&mut DevelopSettings) -> bool,
    ) -> crate::Result<bool> {
        for e in self.undo.iter_mut().chain(self.redo.iter_mut()) {
            patch_op(&mut e.op, id, patch, 0);
        }
        if let Some(i) = self.interaction.as_mut() {
            if i.photo == id {
                patch_arc(&mut i.original, patch);
            }
            for (o, d) in i.others.iter_mut().chain(i.along.iter_mut()) {
                if *o == id {
                    patch_arc(d, patch);
                }
            }
        }
        let Some(p) = self.catalog.photo(id) else { return Ok(false) };
        let mut history = p.history.clone();
        let edited = if touch_edited { Some((self.clock)()) } else { p.edited.clone() };
        let mut d = (*p.develop).clone();
        let changed = patch(&mut d);
        if patch_steps(&mut history, patch)
            && let Err(e) = self.apply_unrecorded(Op::SetHistory { id, history })
        {
            log::warn!("{label}: photo {}: the History steps were not updated: {e}", id.0);
        }
        if !changed {
            return Ok(false);
        }
        self.apply_unrecorded(Op::SetDevelop { id, settings: Arc::new(d), label: label.into(), edited })?;
        Ok(true)
    }
}
