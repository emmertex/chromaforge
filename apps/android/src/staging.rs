//! The activity's staging folder (`<external data>/Incoming`): picked documents that could not be
//! read in place are copied there (`MainActivity`, `collect`). The activity reuses a copy when the
//! same document is picked again, so the folder only grows with new documents; [`clean`] deletes
//! the copies no library photo refers to any more (removed photos, or files copied again on import).
//!
//! It only ever deletes inside the staging folder, never follows symbolic links, leaves files
//! younger than [`MIN_AGE`] alone (a copy whose import is still being chosen) and keeps a file's
//! sidecars (`IMG_1.xmp` beside `IMG_1.CR3`) while the file is kept.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// The staging folder's name inside the external data folder (the activity's `incoming()`).
pub const DIR_NAME: &str = "Incoming";
/// Copies younger than this are kept: their import may not have happened yet.
pub const MIN_AGE: Duration = Duration::from_secs(24 * 60 * 60);
/// Folders deeper than this are left alone (the activity copies at most 8 levels).
const MAX_DEPTH: usize = 16;

/// What [`clean`] did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Cleaned {
    pub files: usize,
    pub bytes: u64,
}

/// Deletes the files under `dir` that are not in `keep` (absolute paths of library photos), are
/// not a kept file's sidecar and are older than `min_age` at `now`; then the folders left empty
/// (not `dir` itself). Errors are logged and skipped.
pub fn clean(dir: &Path, keep: &HashSet<PathBuf>, min_age: Duration, now: SystemTime) -> Cleaned {
    let mut out = Cleaned::default();
    // the stems of kept files, per folder, for their sidecars
    let kept_stems: HashSet<(PathBuf, String)> = keep.iter().filter_map(|p| Some((p.parent()?.to_path_buf(), stem_lower(p)?))).collect();
    walk(dir, 0, &mut |file| {
        if keep.contains(file) {
            return;
        }
        let sidecar_of_kept = file.parent().zip(stem_lower(file)).is_some_and(|(parent, stem)| kept_stems.contains(&(parent.to_path_buf(), stem)));
        if sidecar_of_kept {
            return;
        }
        let Ok(meta) = std::fs::symlink_metadata(file) else { return };
        if !meta.is_file() {
            return;
        }
        let young = meta.modified().ok().and_then(|m| now.duration_since(m).ok()).is_none_or(|age| age < min_age);
        if young {
            return;
        }
        match std::fs::remove_file(file) {
            Ok(()) => {
                out.files = out.files.saturating_add(1);
                out.bytes = out.bytes.saturating_add(meta.len());
            }
            Err(e) => log::warn!("staging: can't delete {}: {e}", file.display()),
        }
    });
    remove_empty(dir, 0, true);
    out
}

fn stem_lower(p: &Path) -> Option<String> {
    Some(p.file_stem()?.to_string_lossy().to_lowercase())
}

/// Calls `f` on every regular file under `dir` (no symbolic links followed).
fn walk(dir: &Path, depth: usize, f: &mut dyn FnMut(&Path)) {
    if depth > MAX_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else { continue };
        let path = entry.path();
        if kind.is_dir() {
            walk(&path, depth.saturating_add(1), f);
        } else if kind.is_file() {
            f(&path);
        }
    }
}

/// Removes the empty folders under `dir` (and `dir` itself unless `top`); returns whether `dir` is gone.
fn remove_empty(dir: &Path, depth: usize, top: bool) -> bool {
    if depth > MAX_DEPTH {
        return false;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return false };
    let mut empty = true;
    for entry in entries.flatten() {
        let is_dir = entry.file_type().is_ok_and(|k| k.is_dir());
        if !(is_dir && remove_empty(&entry.path(), depth.saturating_add(1), false)) {
            empty = false;
        }
    }
    // remove_dir only removes an empty folder, so a file that arrived meanwhile is safe
    empty && !top && std::fs::remove_dir(dir).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);
    impl TempDir {
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn tempdir(tag: &str) -> TempDir {
        let d = std::env::temp_dir().join(format!("lcc-android-staging-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        TempDir(d)
    }

    fn later() -> SystemTime {
        SystemTime::now() + Duration::from_secs(48 * 60 * 60)
    }

    #[test]
    fn deletes_only_unreferenced_old_copies_inside_the_folder() {
        let root = tempdir("t1");
        let staging = root.path().join(DIR_NAME);
        let outside = root.path().join("Library");
        std::fs::create_dir_all(staging.join("Trip/sub")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let kept = staging.join("Trip/IMG_1.CR3");
        for p in [&kept, &staging.join("Trip/IMG_1.xmp"), &staging.join("Trip/IMG_2.jpg"), &staging.join("Trip/sub/x.jpg"), &outside.join("y.jpg")] {
            std::fs::write(p, b"data").unwrap();
        }
        let keep: HashSet<PathBuf> = [kept.clone()].into();
        let cleaned = clean(&staging, &keep, MIN_AGE, later());
        assert_eq!(cleaned.files, 2);
        assert!(kept.exists() && staging.join("Trip/IMG_1.xmp").exists(), "a kept photo and its sidecar stay");
        assert!(!staging.join("Trip/IMG_2.jpg").exists());
        assert!(!staging.join("Trip/sub").exists(), "emptied folders go");
        assert!(staging.exists(), "the staging folder itself stays");
        assert!(outside.join("y.jpg").exists(), "nothing outside is touched");
    }

    #[test]
    fn young_copies_are_kept() {
        let root = tempdir("t2");
        let f = root.path().join("new.jpg");
        std::fs::write(&f, b"data").unwrap();
        let cleaned = clean(root.path(), &HashSet::new(), MIN_AGE, SystemTime::now());
        assert_eq!(cleaned, Cleaned::default());
        assert!(f.exists());
    }

    #[test]
    fn a_missing_folder_is_not_an_error() {
        let root = tempdir("t3");
        assert_eq!(clean(&root.path().join("nope"), &HashSet::new(), MIN_AGE, later()), Cleaned::default());
    }

    #[cfg(unix)]
    #[test]
    fn symbolic_links_are_not_followed() {
        let root = tempdir("t4");
        let staging = root.path().join(DIR_NAME);
        let elsewhere = root.path().join("elsewhere");
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::write(elsewhere.join("keep.jpg"), b"data").unwrap();
        std::os::unix::fs::symlink(&elsewhere, staging.join("link")).unwrap();
        clean(&staging, &HashSet::new(), MIN_AGE, later());
        assert!(elsewhere.join("keep.jpg").exists());
    }
}
