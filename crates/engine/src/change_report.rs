//! What the last command changed, for the on-screen overlay (OSD): every command that changes
//! photos (or the library) reports itself here, whatever ran it (a key, the mouse, a menu, a
//! slider, a control surface, an agent), so the overlay needs nothing from each command.
//!
//! [`Session::execute`] fills [`Session::last_change`] after a command that changed something:
//! the ops it logged (ratings, flags, labels, develop settings, metadata…) say which photos and
//! what kind of change, and a live edit inside an interaction (a slider being dragged, a dial
//! turning) counts its photos too. Background work (the `*.pump` commands, imports) never
//! reports: the overlay is for what the user just did.

use std::collections::BTreeSet;

use lcc_catalog::{ColorLabel, Flag, Op, PhotoId};
use serde_json::Value;

/// What kind of change a command made (the overlay words it from this).
#[derive(Clone, Debug, PartialEq)]
pub enum ChangeKind {
    /// Every photo got this rating.
    Rating(u8),
    /// Every photo got this flag.
    Flag(Flag),
    /// Every photo got this colour label (`None`: removed).
    Label(Option<ColorLabel>),
    /// Develop settings changed.
    Develop,
    /// Anything else (metadata, albums, versions, stacks, deletes…).
    Other,
}

/// The last change a command made.
#[derive(Clone, Debug, PartialEq)]
pub struct ChangeReport {
    /// Goes up with every report (the overlay shows a report once).
    pub seq: u64,
    /// The command's id.
    pub command: String,
    /// Its parameters.
    pub params: Value,
    /// The undo step's label when it made one (`Exposure`, `Flag as Pick`), else the command's
    /// label (English; the UI translates it).
    pub label: String,
    /// How many photos changed (0: none, e.g. an album renamed).
    pub photos: usize,
    pub kind: ChangeKind,
    /// The develop control the command set, when it set one (`light.exposure`): the overlay
    /// shows its value now.
    pub control: Option<String>,
    /// Undo / redo: the label of the step undone or redone.
    pub detail: Option<String>,
    /// A live change inside an interaction (a drag still going): the overlay updates in place.
    pub live: bool,
}

/// Commands that run by themselves (background workers, imports) and never report.
pub fn is_background(command: &str) -> bool {
    command.ends_with(".pump")
        || command.ends_with("Pump")
        || matches!(command, "library.import" | "library.autoImportScan" | "photo.reload" | "develop.beginInteraction" | "develop.cancelInteraction")
}

/// The photos an op changes in a way the user sees (not history bookkeeping or background
/// analysis), added to `into`; also what kind of change it is, merged into `kind`.
fn walk(op: &Op, into: &mut BTreeSet<PhotoId>, kind: &mut Option<ChangeKind>, depth: usize) {
    match op {
        Op::Batch { ops } if depth < 8 => {
            for o in ops {
                walk(o, into, kind, depth + 1);
            }
        }
        Op::SetRating { id, rating } => {
            into.insert(*id);
            merge(kind, ChangeKind::Rating(*rating));
        }
        Op::SetFlag { id, flag } => {
            into.insert(*id);
            merge(kind, ChangeKind::Flag(*flag));
        }
        Op::SetLabel { id, label } => {
            into.insert(*id);
            merge(kind, ChangeKind::Label(*label));
        }
        Op::SetDevelop { id, .. } => {
            into.insert(*id);
            merge(kind, ChangeKind::Develop);
        }
        Op::SetMeta { id, .. }
        | Op::SetDeleted { id, .. }
        | Op::SetVersions { id, .. }
        | Op::RemovePhoto { id }
        | Op::SetContent { id, .. }
        | Op::SetCaptured { id, .. }
        | Op::SetFile { id, .. }
        | Op::Relink { id, .. } => {
            into.insert(*id);
            merge(kind, ChangeKind::Other);
        }
        Op::AddPhoto { photo } => {
            into.insert(photo.id);
            merge(kind, ChangeKind::Other);
        }
        // bookkeeping that goes with another change, or the app's own background findings
        Op::SetHistory { .. }
        | Op::PushHistory { .. }
        | Op::SetHistoryCursor { .. }
        | Op::SetAnalysis { .. }
        | Op::SetEmbeddedLens { .. }
        | Op::SetAsShotWb { .. }
        | Op::SetBrowsed { .. }
        | Op::SetFolderRecord { .. } => {}
        _ => merge(kind, ChangeKind::Other),
    }
}

/// `k` merged into what was seen so far: the same kind stays, a develop change with something
/// else (its metadata, a rotation) is a develop change, anything else mixed is `Other`.
fn merge(kind: &mut Option<ChangeKind>, k: ChangeKind) {
    *kind = Some(match kind.take() {
        None => k,
        Some(prev) if prev == k => prev,
        Some(ChangeKind::Develop | ChangeKind::Other) if k == ChangeKind::Develop => ChangeKind::Develop,
        Some(ChangeKind::Develop) if k == ChangeKind::Other => ChangeKind::Develop,
        _ => ChangeKind::Other,
    });
}

/// The photos and kind of change in `ops` (`None`: nothing the user would see).
pub fn summarize(ops: &[Op]) -> Option<(usize, ChangeKind)> {
    let mut photos = BTreeSet::new();
    let mut kind = None;
    for op in ops {
        walk(op, &mut photos, &mut kind, 0);
    }
    kind.map(|k| (photos.len(), k))
}

/// The develop control a command's parameters name, when it names one.
pub fn control_of(command: &str, params: &Value) -> Option<String> {
    if !matches!(command, "develop.set" | "develop.adjust" | "develop.reset1" | "develop.resetControl") {
        return None;
    }
    let c = params.get("control").and_then(Value::as_str).map(str::to_string).or_else(|| {
        let o = params.get("values")?.as_object()?;
        (o.len() == 1).then(|| o.keys().next().cloned()).flatten()
    })?;
    lcc_develop::controls::find(&c).map(|_| c)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn ratings_flags_labels_and_develop() {
        let ids = [PhotoId(1), PhotoId(2), PhotoId(3)];
        let ops = vec![Op::Batch { ops: ids.iter().map(|id| Op::SetRating { id: *id, rating: 3 }).collect() }];
        assert_eq!(summarize(&ops), Some((3, ChangeKind::Rating(3))));
        let ops = vec![Op::SetFlag { id: PhotoId(1), flag: Flag::Pick }];
        assert_eq!(summarize(&ops), Some((1, ChangeKind::Flag(Flag::Pick))));
        let ops = vec![Op::SetLabel { id: PhotoId(1), label: Some(ColorLabel::Red) }, Op::SetLabel { id: PhotoId(1), label: Some(ColorLabel::Red) }];
        assert_eq!(summarize(&ops), Some((1, ChangeKind::Label(Some(ColorLabel::Red)))), "one photo counted once");
        let d = Arc::new(lcc_develop::DevelopSettings::default());
        let ops = vec![
            Op::SetDevelop { id: PhotoId(1), settings: d.clone(), label: "Exposure".into(), edited: None },
            Op::PushHistory { id: PhotoId(1), step: lcc_catalog::HistoryStep { label: "Exposure".into(), settings: d.clone() } },
            Op::SetDevelop { id: PhotoId(2), settings: d, label: "Exposure".into(), edited: None },
        ];
        assert_eq!(summarize(&ops), Some((2, ChangeKind::Develop)), "history bookkeeping isn't a change of its own");
        let mixed = vec![Op::SetRating { id: PhotoId(1), rating: 2 }, Op::SetFlag { id: PhotoId(1), flag: Flag::Reject }];
        assert_eq!(summarize(&mixed), Some((1, ChangeKind::Other)));
        assert_eq!(summarize(&[Op::SetHistoryCursor { id: PhotoId(1), cursor: None }]), None, "nothing the user sees");
    }

    #[test]
    fn control_named_by_the_parameters() {
        assert_eq!(control_of("develop.set", &serde_json::json!({"control": "light.exposure", "value": 1})).as_deref(), Some("light.exposure"));
        assert_eq!(control_of("develop.set", &serde_json::json!({"values": {"light.contrast": 10}})).as_deref(), Some("light.contrast"));
        assert_eq!(control_of("develop.set", &serde_json::json!({"values": {"light.contrast": 10, "light.exposure": 1}})), None);
        assert_eq!(control_of("develop.set", &serde_json::json!({"control": "nonsense"})), None);
        assert_eq!(control_of("photo.rate", &serde_json::json!({"control": "light.exposure"})), None);
    }

    #[test]
    fn background_commands_never_report() {
        assert!(is_background("faces.pump") && is_background("photo.rejectEyesPump") && is_background("library.import"));
        assert!(!is_background("photo.rate") && !is_background("develop.set"));
    }
}
