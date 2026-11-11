//! Library ▸ Validate DNG Files: recompute the raw-image digest of the selected DNGs and compare it with the one the
//! file carries ([`lcc_raw::dngdigest`]), to find files damaged since they were written.
//!
//! The file work ([`validate`]) touches only the disk, never the session, so the app runs it on a worker thread with a
//! cancel flag and an activity row; `dng.validate` is the same thing synchronously for the CLI and MCP.

use std::sync::atomic::{AtomicBool, Ordering};

use lcc_catalog::PhotoId;
use lcc_raw::dngdigest::{DngCheck, check_dng};
use serde_json::{Value, json};

use crate::Session;

/// One DNG to check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DngTarget {
    pub id: PhotoId,
    pub name: String,
    pub path: String,
}

/// The files that are DNGs among `ids` (library photos with a file, DNG by format or extension), each once: virtual
/// copies share their master's file.
pub fn dng_targets(s: &Session, ids: &[PhotoId]) -> Vec<DngTarget> {
    let mut seen = std::collections::HashSet::new();
    ids.iter()
        .filter_map(|id| s.catalog.photo(*id))
        .filter_map(|p| {
            let path = crate::cmd::missing::checked_path(p)?;
            let dng = p.format.eq_ignore_ascii_case("dng") || path.to_ascii_lowercase().ends_with(".dng");
            (dng && seen.insert(path.to_string())).then(|| DngTarget { id: p.id, name: p.file_name.clone(), path: path.to_string() })
        })
        .collect()
}

/// What checking a batch found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DngReport {
    /// Files whose raw data matches their digest.
    pub intact: usize,
    /// `(name, why)`: files that do not match their digest, or whose raw data or file cannot be read.
    pub damaged: Vec<(String, String)>,
    /// `(name, why)`: files with no digest to compare, or a layout the check does not cover.
    pub unverifiable: Vec<(String, String)>,
    /// How many of the files were looked at before a cancel stopped the run.
    pub checked: usize,
    /// How many were asked for.
    pub total: usize,
    pub cancelled: bool,
}

impl DngReport {
    pub fn to_json(&self) -> Value {
        let list = |v: &[(String, String)]| v.iter().map(|(name, why)| json!({"file": name, "why": why})).collect::<Vec<_>>();
        json!({
            "total": self.total, "checked": self.checked, "intact": self.intact, "cancelled": self.cancelled,
            "damaged": list(&self.damaged), "unverifiable": list(&self.unverifiable),
        })
    }
}

/// Check one file on disk. A file that cannot be read is damaged (it is in the library and should be there).
pub fn check_file(path: &str) -> DngCheck {
    match std::fs::read(path) {
        Ok(bytes) => check_dng(&bytes),
        Err(e) => DngCheck::Damaged(format!("cannot be read: {e}")),
    }
}

/// Check every target, stopping between files once `cancel` is set; `progress(done)` after each.
pub fn validate(targets: &[DngTarget], cancel: &AtomicBool, progress: &dyn Fn(usize)) -> DngReport {
    let mut r = DngReport { total: targets.len(), ..DngReport::default() };
    for t in targets {
        if cancel.load(Ordering::Relaxed) {
            r.cancelled = true;
            break;
        }
        match check_file(&t.path) {
            DngCheck::Intact(_) => r.intact += 1,
            DngCheck::Damaged(why) => r.damaged.push((t.name.clone(), why)),
            DngCheck::Unverifiable(why) => r.unverifiable.push((t.name.clone(), why)),
        }
        r.checked += 1;
        progress(r.checked);
    }
    r
}
