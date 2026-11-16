//! The toast every rating, flag and colour label change shows, whichever way it was made (a key, a
//! menu row, the control channel): what the photos now are, with the count when several changed.
//! [`crate::LccApp::run`] shows it after the command succeeded, so a refused change says nothing.

use lcc_catalog::{Flag, PhotoId};
use serde_json::Value;

use crate::{LccApp, i18n};

/// Whether `id` is a flag change (its toast yields to the on-screen overlay that reports changes).
pub(crate) fn is_flag(id: &str) -> bool {
    matches!(id, "photo.flag" | "photo.pick" | "photo.reject" | "photo.unflag" | "photo.cycleFlag" | "photo.increaseFlag" | "photo.decreaseFlag")
}

/// Whether `id` changes ratings, flags or labels and so confirms with a toast.
pub(crate) fn confirms(id: &str) -> bool {
    is_flag(id) || matches!(id, "photo.rate" | "photo.increaseRating" | "photo.decreaseRating" | "photo.label" | "photo.clearLabel")
}

/// The photos the command acted on: its `ids`, else the selection, else the active photo; each once.
pub(crate) fn targets(app: &LccApp, params: &Value) -> Vec<PhotoId> {
    let mut ids: Vec<PhotoId> = match params.get("ids").and_then(Value::as_array) {
        Some(list) => list.iter().filter_map(Value::as_u64).map(PhotoId).collect(),
        None if app.session.selection.ids.is_empty() => app.session.active().into_iter().collect(),
        None => app.session.selection.ids.clone(),
    };
    let mut seen = std::collections::HashSet::new();
    ids.retain(|id| seen.insert(*id));
    ids
}

/// `text`, with how many photos changed when more than one did.
pub(crate) fn with_count(text: String, n: usize) -> String {
    if n > 1 { i18n::tr_format!("{toast} · {n} photos", toast = text, n = n) } else { text }
}

/// The value every target shares: `Some(None)` when they differ, `None` when there are no photos.
fn common<T: PartialEq>(app: &LccApp, targets: &[PhotoId], read: impl Fn(&std::sync::Arc<lcc_catalog::Photo>) -> T) -> Option<Option<T>> {
    let mut values = targets.iter().filter_map(|id| app.session.catalog.photo(*id)).map(read);
    let first = values.next()?;
    Some(values.all(|v| v == first).then_some(first))
}

/// The toast for rating or flag command `id` having just succeeded on `targets`.
pub(crate) fn feedback(app: &LccApp, id: &str, targets: &[PhotoId]) -> Option<String> {
    let text = if is_flag(id) {
        match common(app, targets, |p| p.flag)? {
            Some(Flag::Pick) => i18n::tr("Flagged as Pick").to_string(),
            Some(Flag::Reject) => i18n::tr("Flagged as Reject").to_string(),
            Some(Flag::None) => i18n::tr("Unflagged").to_string(),
            None => i18n::tr("Flag changed").to_string(),
        }
    } else {
        match common(app, targets, |p| p.rating)? {
            Some(0) => i18n::tr("Rating cleared").to_string(),
            Some(stars) => i18n::tr_format!("Rated {}", "★".repeat(usize::from(stars.min(5)))),
            None => i18n::tr("Rating changed").to_string(),
        }
    };
    Some(with_count(text, targets.len()))
}
