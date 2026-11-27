//! Navigation on the undo stack: choosing another source, changing the filter and changing the
//! selection are undo steps, so Undo never reverts a photo the person can't see.
//!
//! A step holds the view before and after ([`ViewState`]); Undo puts the first back, Redo the
//! second. It changes nothing in the catalog and writes nothing to the library's log.
//! [`Session::run_command`] records one when a command left the view changed and committed no
//! step of its own (an import that selects its photos is one step, not two).

use std::sync::Arc;

use lcc_catalog::{Filter, Op};

use crate::view::{Browse, LibrarySource, Selection};
use crate::{Session, UndoEntry};

/// What the grid shows and what is selected: everything a navigation step puts back.
#[derive(Clone, Debug, PartialEq)]
pub struct ViewState {
    pub source: LibrarySource,
    pub browse: Option<Browse>,
    pub library_folder: Option<String>,
    pub folder_subfolders: bool,
    pub filter: Filter,
    pub selection: Selection,
    /// The module on screen (the key of `ui-egui`'s module: `library`, `develop`…), which the app
    /// reports ([`Session::module_switched`]) and follows after an Undo or Redo
    /// ([`Session::take_module_request`]).
    pub module: String,
}

/// The two ends of a navigation step. `apply` is what this entry puts back when it is taken;
/// `inverse` is what the entry for the opposite stack puts back.
#[derive(Clone, Debug)]
pub(crate) struct ViewStep {
    pub apply: Arc<ViewState>,
    pub inverse: Arc<ViewState>,
    /// The step is a text filter being typed: the next keystroke extends it, until
    /// [`Session::end_typing`].
    pub typing: bool,
}

impl UndoEntry {
    /// Whether this step only moves the view (a source, filter, selection or module): it changes nothing in the catalog.
    pub fn is_navigation(&self) -> bool {
        self.view.is_some()
    }
}

impl ViewStep {
    pub(crate) fn reversed(&self) -> ViewStep {
        ViewStep { apply: self.inverse.clone(), inverse: self.apply.clone(), typing: false }
    }

    /// Selected photos this step keeps in memory.
    fn weight(&self) -> usize {
        self.apply.selection.ids.len().saturating_add(self.inverse.selection.ids.len())
    }
}

/// Photo ids the navigation steps may hold between them (selecting everything in a very large
/// library is one step, and a thousand of them must not cost gigabytes): the oldest go first.
const VIEW_ID_BUDGET: usize = 2_000_000;

impl Session {
    pub fn view_state(&self) -> ViewState {
        ViewState {
            source: self.source,
            browse: self.browse.clone(),
            library_folder: self.library_folder.clone(),
            folder_subfolders: self.folder_subfolders,
            filter: self.filter.clone(),
            selection: self.selection.clone(),
            module: self.module.clone(),
        }
    }

    /// Show `v` again. What no longer exists (a deleted collection, a removed photo) is dropped
    /// the way a command's own result is ([`Session::reconcile_view`]).
    pub(crate) fn apply_view_state(&mut self, v: &ViewState) {
        let active = self.selection.active;
        self.source = v.source;
        self.browse = v.browse.clone();
        self.library_folder = v.library_folder.clone();
        self.folder_subfolders = v.folder_subfolders;
        self.filter = v.filter.clone();
        self.selection = v.selection.clone();
        if self.module != v.module {
            self.module = v.module.clone();
            self.module_request = Some(v.module.clone());
        }
        let catalog = &self.catalog;
        self.selection.ids.retain(|&id| catalog.photo(id).is_some());
        if self.selection.active.is_some_and(|a| !self.selection.ids.contains(&a)) {
            self.selection.active = self.selection.ids.first().copied();
        }
        if self.selection.active != active {
            self.active_mask = None;
            self.active_spot = None;
        }
        self.shelved_filter = None;
        self.reconcile_view();
    }

    /// The command that just ran left the view different from `before`: record it as one step,
    /// unless it committed (or undid) a step itself.
    pub(crate) fn view_changed(&mut self, id: &str, before: ViewState) {
        if !self.record_view_steps {
            return;
        }
        let after = self.view_state();
        if after == before {
            return;
        }
        let (label, text_only) = label_for(id, &before, &after);
        // typing: the next keystroke of the same word extends the step the last one made
        if text_only
            && let Some(top) = self.undo.last_mut()
            && let Some(step) = top.view.as_mut().filter(|s| s.typing && *s.inverse == before)
        {
            step.inverse = Arc::new(after);
            return;
        }
        let step = ViewStep { apply: Arc::new(before), inverse: Arc::new(after), typing: text_only };
        self.undo.push(UndoEntry {
            label: label.to_string(),
            op: Op::Batch { ops: Vec::new() },
            folder: None,
            profile: None,
            retry_inverse: Vec::new(),
            retry_sidecars: Vec::new(),
            view: Some(step),
        });
        self.trim_view_steps();
        // navigation is the cheap part of the history: past the cap it goes before the person's edits do
        if self.undo.len() > crate::UNDO_STEPS
            && self.undo_hold.strong_count() == 0
            && let Some(i) = self.undo.iter().position(UndoEntry::is_navigation)
        {
            self.undo.remove(i);
            self.undo_dropped = self.undo_dropped.wrapping_add(1);
        }
        self.trim_undo();
        // a new step ends the Redo chain, as an edit does
        for mut dropped in self.redo.drain(..) {
            self.pending_sidecar_repairs.append(&mut dropped.retry_sidecars);
        }
    }

    /// Change the view by hand (no command does it) as one step: what `f` changes in the source,
    /// filter or selection is taken back by Undo, as for a command.
    pub fn navigating<R>(&mut self, f: impl FnOnce(&mut Session) -> R) -> R {
        let before = self.view_state();
        let r = f(self);
        self.view_changed("library.select", before);
        r
    }

    /// Whether changes of the view are steps; returns what it was. Off, the app moves the view by itself (an
    /// import showing the photos it brought in), which is not the person's navigation.
    pub fn record_navigation(&mut self, on: bool) -> bool {
        std::mem::replace(&mut self.record_view_steps, on)
    }

    /// The app switched to module `to` (`library`, `develop`…): one step, unless the session
    /// already knows it (an Undo or a Redo brought it here).
    pub fn module_switched(&mut self, to: &str) {
        if self.module == to {
            return;
        }
        let before = self.view_state();
        self.module = to.to_string();
        self.view_changed("module.switch", before);
    }

    /// The module on screen changed some way that is no step (a start-up, a flow that restores
    /// its own): the session follows, so the next switch is measured from here.
    pub fn follow_module(&mut self, to: &str) {
        if self.module != to && self.module_request.is_none() {
            self.module = to.to_string();
        }
    }

    /// The module an Undo or Redo brought back, for the app to switch to (once).
    pub fn take_module_request(&mut self) -> Option<String> {
        self.module_request.take()
    }

    /// The word being typed into the filter is finished (Enter, or the field lost focus): the
    /// next keystroke starts a new step.
    pub fn end_typing(&mut self) {
        if let Some(step) = self.undo.last_mut().and_then(|e| e.view.as_mut()) {
            step.typing = false;
        }
    }

    /// Take a navigation step off `from` (Undo: the undo stack) and put its view on screen; the
    /// reverse goes on the other stack.
    pub(crate) fn take_view_step(&mut self, mut e: UndoEntry, undoing: bool) -> String {
        let Some(step) = e.view.take() else { return e.label };
        self.apply_view_state(&step.apply);
        let back = UndoEntry { view: Some(step.reversed()), ..e.clone() };
        if undoing {
            self.redo.push(back);
        } else {
            self.undo.push(back);
        }
        e.label
    }

    fn trim_view_steps(&mut self) {
        let mut total: usize = self.undo.iter().filter_map(|e| e.view.as_ref()).map(ViewStep::weight).fold(0, usize::saturating_add);
        if total <= VIEW_ID_BUDGET {
            return;
        }
        // the newest step always stays: it is what the person just did
        let mut i = 0;
        while total > VIEW_ID_BUDGET && i + 1 < self.undo.len() {
            match self.undo.get(i).and_then(|e| e.view.as_ref()).map(ViewStep::weight) {
                Some(w) => {
                    self.undo.remove(i);
                    self.undo_dropped = self.undo_dropped.wrapping_add(1);
                    total = total.saturating_sub(w);
                }
                None => i += 1,
            }
        }
    }
}

/// The step's name (Lightroom's: Change Source, Go to Next Photo, Selection…), and whether
/// only the filter's text changed (typing).
fn label_for(id: &str, before: &ViewState, after: &ViewState) -> (&'static str, bool) {
    let module_changed = before.module != after.module;
    let source_changed = before.source != after.source
        || before.browse != after.browse
        || before.library_folder != after.library_folder
        || before.folder_subfolders != after.folder_subfolders;
    let text_only = !module_changed && !source_changed && before.selection == after.selection && {
        // the word typed into the search box: its text, or its description for the semantic search
        let mut a = after.filter.clone();
        a.text = before.filter.text.clone();
        a.semantic = before.filter.semantic.clone();
        a == before.filter && (after.filter.text != before.filter.text || after.filter.semantic != before.filter.semantic)
    };
    let label = if module_changed {
        "Module Change"
    } else if source_changed {
        "Change Source"
    } else if before.filter != after.filter {
        "Change Filter"
    } else {
        match id {
            "library.next" | "library.down" => "Go to Next Photo",
            "library.previous" | "library.up" => "Go to Previous Photo",
            _ => "Selection",
        }
    };
    (label, text_only)
}
