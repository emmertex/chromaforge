//! The built-in Hard Drive service: each published collection is a folder inside the service's
//! folder, kept in sync with the collection (new and modified photos are written, removed ones
//! deleted). Remote ids are file names inside the collection's folder.

use std::path::{Path, PathBuf};

use crate::config::CollectionConfig;
use crate::service::{PublishError, PublishService, Published, Upload};
use crate::{KIND_HARD_DRIVE, ServiceConfig};

/// A name usable as one path segment: separators and control characters become `_`; empty, `.`
/// and `..` are refused.
pub fn safe_name(s: &str) -> Option<String> {
    let n: String = s.trim().chars().map(|c| if matches!(c, '/' | '\\' | ':' | '\0') || c.is_control() { '_' } else { c }).collect();
    let n = n.trim().to_string();
    if n.is_empty() || n == "." || n == ".." { None } else { Some(n) }
}

/// The service's folder (`settings.dir`).
pub fn service_dir(svc: &ServiceConfig) -> Result<PathBuf, PublishError> {
    let dir = svc.settings.get("dir").and_then(|d| d.as_str()).map(str::trim).filter(|d| !d.is_empty());
    dir.map(PathBuf::from).ok_or_else(|| PublishError::Config(format!("the publish service `{}` has no folder: set one", svc.name)))
}

pub struct HardDrive {
    folder: PathBuf,
}

impl HardDrive {
    pub fn open(svc: &ServiceConfig, coll: &CollectionConfig) -> Result<HardDrive, PublishError> {
        let folder = safe_name(&coll.folder).ok_or_else(|| PublishError::Config(format!("`{}` can't be a folder name", coll.folder)))?;
        Ok(HardDrive { folder: service_dir(svc)?.join(folder) })
    }

    /// The collection's folder.
    pub fn folder(&self) -> &Path {
        &self.folder
    }

    fn path_of(&self, remote_id: &str) -> Result<PathBuf, PublishError> {
        // remote ids come from the catalog: never let one point outside the folder
        match safe_name(remote_id) {
            Some(n) if n == remote_id => Ok(self.folder.join(n)),
            _ => Err(PublishError::Config(format!("`{remote_id}` is not a file name in the collection's folder"))),
        }
    }

    /// `wanted`, or the first of `a-2.jpg`, `a-3.jpg`, ... (the app's naming for clashes) whose file
    /// and sidecars don't belong to another photo. Files of this photo's previous publication
    /// don't count: a re-publish replaces them.
    fn free_name(&self, wanted: &str, up: &Upload<'_>) -> String {
        let own = |n: &str| up.previous.is_some_and(|p| n == p || up.sidecars.iter().any(|(ext, _)| with_ext(p, ext) == n));
        let taken = |n: &str| !own(n) && self.folder.join(n).exists();
        let free = |n: &str| !taken(n) && !up.sidecars.iter().any(|(ext, _)| taken(&with_ext(n, ext)));
        if free(wanted) {
            return wanted.to_string();
        }
        let (stem, ext) = match wanted.rsplit_once('.') {
            Some((s, e)) if !s.is_empty() => (s, Some(e)),
            _ => (wanted, None),
        };
        (2u32..)
            .map(|i| match ext {
                Some(e) => format!("{stem}-{i}.{e}"),
                None => format!("{stem}-{i}"),
            })
            .find(|n| free(n))
            .unwrap_or_else(|| wanted.to_string())
    }
}

fn remove_file(path: &Path) -> Result<(), PublishError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(PublishError::Io(format!("{}: {e}", path.display()))),
    }
}

/// `name` with its extension replaced by `ext` (a sidecar's name).
pub fn with_ext(name: &str, ext: &str) -> String {
    let stem = name.rsplit_once('.').map(|(s, _)| s).unwrap_or(name);
    format!("{stem}.{ext}")
}

impl PublishService for HardDrive {
    fn kind(&self) -> &'static str {
        KIND_HARD_DRIVE
    }

    fn publish(&mut self, up: &Upload<'_>) -> Result<Published, PublishError> {
        let wanted = safe_name(up.file_name).ok_or_else(|| PublishError::Config(format!("`{}` can't be a file name", up.file_name)))?;
        std::fs::create_dir_all(&self.folder).map_err(|e| PublishError::Io(format!("{}: {e}", self.folder.display())))?;
        let name = self.free_name(&wanted, up);
        let path = self.folder.join(&name);
        lcc_catalog::safe_file::write_atomic(&path, up.bytes).map_err(|e| PublishError::Io(format!("{}: {e}", path.display())))?;
        let mut written = vec![name.clone()];
        for (ext, bytes) in up.sidecars {
            let sidecar = with_ext(&name, ext);
            let p = self.folder.join(&sidecar);
            lcc_catalog::safe_file::write_atomic(&p, bytes).map_err(|e| PublishError::Io(format!("{}: {e}", p.display())))?;
            written.push(sidecar);
        }
        // a re-publish under another name (the naming template changed): the old file goes
        if let Some(prev) = up.previous.filter(|p| *p != name) {
            self.remove_keeping(prev, &written)?;
        }
        Ok(Published { remote_id: name })
    }

    fn remove(&mut self, remote_id: &str) -> Result<(), PublishError> {
        self.remove_keeping(remote_id, &[])
    }
}

impl HardDrive {
    /// Delete `remote_id` and its XMP sidecar, except files named in `keep` (just written by the
    /// publish that replaces it: `a.jpg` -> `a.png` shares the sidecar `a.xmp`).
    fn remove_keeping(&mut self, remote_id: &str, keep: &[String]) -> Result<(), PublishError> {
        let path = self.path_of(remote_id)?;
        remove_file(&path)?;
        let xmp_name = with_ext(remote_id, "xmp");
        let xmp = self.folder.join(&xmp_name);
        if xmp != path && !keep.contains(&xmp_name) {
            remove_file(&xmp)?;
        }
        Ok(())
    }
}
