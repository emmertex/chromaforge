//! Capabilities: what a plug-in asks for (in its manifest) and what the user granted.
//!
//! A plug-in gets nothing by default. The effective set for a call is the grant intersected with
//! the request ([`Permissions::intersect`]), so a grant can never widen what the manifest asked
//! for, and revoking a capability takes effect on the next call.

use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::programs::ProgramDecl;

/// Most network hosts / filesystem roots one plug-in may name.
pub const MAX_HOSTS: usize = 32;
pub const MAX_ROOTS: usize = 16;

/// A capability set.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Permissions {
    /// Read the catalog (`catalog.query`, `photo.inspect`, `catalog.stats`, `keyword.list`).
    #[serde(default)]
    pub catalog: bool,
    /// Write standard metadata fields (title, caption, keywords, …) of photos.
    #[serde(default)]
    pub metadata_write: bool,
    /// Add files to the library (`library.add`): a tool's output, from the folder of the photo it
    /// was made from, stacked on it or put in an album that holds it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub library_add: bool,
    /// Read a downsized copy of a photo's pixels (`photo.pixels`): the whole uncropped frame, as
    /// the crop tool shows it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pixels: bool,
    /// Set a photo's crop (`develop.setCrop`): one History step per photo, editable afterwards in
    /// the crop tool like any crop.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub crop: bool,
    /// Make a virtual copy of a photo (`photo.virtualCopy`): a new catalog entry sharing the
    /// photo's file, with its own settings, stacked with it. One History step.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub virtual_copy: bool,
    /// Hosts the plug-in may send HTTP(S) requests to: `api.example.com`, or `*.example.com` for
    /// any sub-domain (not the bare domain).
    #[serde(default)]
    pub network: Vec<String>,
    /// Folders the plug-in may read (and, with `write`, write) below.
    #[serde(default)]
    pub fs: Vec<FsRoot>,
    /// Programs the plug-in may start (`program.start`). Granted only by the user, in the Plugin
    /// Manager ([`crate::Manager::set_grant_by_user`]); never through a command.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub programs: Vec<ProgramDecl>,
}

/// A filesystem root.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FsRoot {
    pub path: String,
    #[serde(default)]
    pub write: bool,
}

fn valid_host_pattern(h: &str) -> bool {
    let h = h.strip_prefix("*.").unwrap_or(h);
    !h.is_empty()
        && h.len() <= 253
        && h.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.')
        && !h.starts_with('.')
        && !h.ends_with('.')
}

impl Permissions {
    /// Checks a requested set (from a manifest): well-formed hosts, absolute roots, within limits.
    pub fn validate(&self) -> std::result::Result<(), String> {
        if self.network.len() > MAX_HOSTS {
            return Err(format!("at most {MAX_HOSTS} network hosts"));
        }
        if let Some(h) = self.network.iter().find(|h| !valid_host_pattern(h)) {
            return Err(format!("bad network host {h:?} (lower-case host name, or *.domain)"));
        }
        if self.fs.len() > MAX_ROOTS {
            return Err(format!("at most {MAX_ROOTS} filesystem roots"));
        }
        if let Some(r) = self.fs.iter().find(|r| !Path::new(&r.path).is_absolute() || has_parent_refs(Path::new(&r.path))) {
            return Err(format!("filesystem root {:?} must be an absolute path without `..`", r.path));
        }
        crate::programs::validate_all(&self.programs)
    }

    /// The granted program `id`, if any.
    pub fn program(&self, id: &str) -> Option<&ProgramDecl> {
        self.programs.iter().find(|p| p.id == id)
    }

    /// What both sets allow: the effective capabilities of a grant for a request.
    pub fn intersect(&self, other: &Permissions) -> Permissions {
        Permissions {
            catalog: self.catalog && other.catalog,
            metadata_write: self.metadata_write && other.metadata_write,
            library_add: self.library_add && other.library_add,
            pixels: self.pixels && other.pixels,
            crop: self.crop && other.crop,
            virtual_copy: self.virtual_copy && other.virtual_copy,
            network: self.network.iter().filter(|h| other.network.contains(h)).cloned().collect(),
            fs: self
                .fs
                .iter()
                .filter_map(|r| other.fs.iter().find(|o| o.path == r.path).map(|o| FsRoot { path: r.path.clone(), write: r.write && o.write }))
                .collect(),
            // the requested entry, when it names no more than the granted one (`ProgramDecl::within`):
            // an update that widens where a program is found drops the grant
            programs: other.programs.iter().filter(|o| self.programs.iter().any(|p| o.within(p))).cloned().collect(),
        }
    }

    /// True when nothing is allowed.
    pub fn is_empty(&self) -> bool {
        !self.catalog
            && !self.metadata_write
            && !self.library_add
            && !self.pixels
            && !self.crop
            && !self.virtual_copy
            && self.network.is_empty()
            && self.fs.is_empty()
            && self.programs.is_empty()
    }

    /// May the plug-in talk to `host`? Matching ignores ASCII case.
    pub fn allows_host(&self, host: &str) -> bool {
        let host = host.to_ascii_lowercase();
        self.network.iter().any(|p| match p.strip_prefix("*.") {
            Some(domain) => host.strip_suffix(domain).is_some_and(|rest| rest.len() > 1 && rest.ends_with('.')),
            None => *p == host,
        })
    }

    /// Resolves `path` for reading (or writing) under a granted root, or `extra` (exact files the
    /// host lends for one call, like the file an export hook post-processes). Rejects relative
    /// paths and `..`, and resolves symbolic links of the existing part so a link cannot lead
    /// out of a root. Returns the resolved path, which is the one to open.
    pub fn check_path(&self, path: &str, write: bool, extra: &[PathBuf]) -> std::result::Result<PathBuf, String> {
        self.check_path_in(path, write, extra, &[])
    }

    /// [`Permissions::check_path`], also allowing reads below `lent_dirs` (folders lent to one
    /// call for reading, like a program run's folder in `program.end`).
    pub fn check_path_in(&self, path: &str, write: bool, extra: &[PathBuf], lent_dirs: &[PathBuf]) -> std::result::Result<PathBuf, String> {
        let p = Path::new(path);
        if !p.is_absolute() || has_parent_refs(p) {
            return Err(format!("{path}: must be an absolute path without `..`"));
        }
        // the resolved path is what the caller opens: a link swapped into the original path after this check
        // can't redirect it (and `safe_io` checks again at open time)
        let real = resolve(p);
        if has_parent_refs(&real) {
            return Err(format!("{path}: must be an absolute path without `..`"));
        }
        if extra.iter().any(|e| resolve(e) == real) {
            return Ok(real);
        }
        if !write && lent_dirs.iter().any(|d| real.starts_with(resolve(d))) {
            return Ok(real);
        }
        for root in &self.fs {
            if write && !root.write {
                continue;
            }
            if real.starts_with(resolve(Path::new(&root.path))) {
                return Ok(real);
            }
        }
        Err(format!("{path}: not inside a folder this plug-in may {}", if write { "write" } else { "read" }))
    }
}

fn has_parent_refs(p: &Path) -> bool {
    p.components().any(|c| matches!(c, Component::ParentDir))
}

/// `p` with its longest existing prefix canonicalised (symbolic links resolved) and the missing
/// rest appended.
fn resolve(p: &Path) -> PathBuf {
    let mut existing = p.to_path_buf();
    let mut rest = Vec::new();
    // bounded: one step per component
    for _ in 0..4096 {
        if let Ok(c) = existing.canonicalize() {
            let mut out = c;
            for r in rest.iter().rev() {
                out.push(r);
            }
            return out;
        }
        match (existing.file_name().map(|n| n.to_os_string()), existing.parent().map(Path::to_path_buf)) {
            (Some(name), Some(parent)) => {
                rest.push(name);
                existing = parent;
            }
            _ => break,
        }
    }
    p.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_match_exactly_or_by_subdomain() {
        let p = Permissions { network: vec!["api.example.com".into(), "*.cdn.org".into()], ..Default::default() };
        assert!(p.allows_host("api.example.com"));
        assert!(p.allows_host("API.example.com"));
        assert!(!p.allows_host("evil.api.example.com"));
        assert!(!p.allows_host("example.com"));
        assert!(p.allows_host("a.cdn.org"));
        assert!(!p.allows_host("cdn.org"));
        assert!(!p.allows_host("evilcdn.org"));
    }

    #[test]
    fn intersection_never_widens() {
        let req =
            Permissions { catalog: true, network: vec!["a.com".into()], fs: vec![FsRoot { path: "/x".into(), write: true }], ..Default::default() };
        let grant = Permissions {
            catalog: true,
            metadata_write: true,
            library_add: true,
            pixels: true,
            crop: true,
            virtual_copy: true,
            network: vec!["a.com".into(), "b.com".into()],
            fs: vec![FsRoot { path: "/x".into(), write: false }],
            programs: vec![],
        };
        let e = grant.intersect(&req);
        assert!(e.catalog && !e.metadata_write && !e.library_add && !e.pixels && !e.crop && !e.virtual_copy);
        assert_eq!(e.network, vec!["a.com".to_string()]);
        assert_eq!(e.fs, vec![FsRoot { path: "/x".into(), write: false }]);
    }

    #[test]
    fn paths_stay_inside_roots() {
        let dir = std::env::temp_dir().join(format!("lcc-plugins-perm-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("in")).unwrap();
        let root = dir.join("in");
        let p = Permissions { fs: vec![FsRoot { path: root.display().to_string(), write: false }], ..Default::default() };
        let inside = root.join("a/b.txt");
        assert!(p.check_path(&inside.display().to_string(), false, &[]).is_ok());
        assert!(p.check_path(&inside.display().to_string(), true, &[]).is_err(), "read-only root");
        assert!(p.check_path(&dir.join("out.txt").display().to_string(), false, &[]).is_err());
        assert!(p.check_path(&format!("{}/../out.txt", root.display()), false, &[]).is_err());
        assert!(p.check_path("relative.txt", false, &[]).is_err());
        let lent = dir.join("export.jpg");
        assert!(p.check_path(&lent.display().to_string(), true, std::slice::from_ref(&lent)).is_ok());
        #[cfg(unix)]
        {
            let link = root.join("escape");
            let _ = std::fs::remove_file(&link);
            std::os::unix::fs::symlink(&dir, &link).unwrap();
            assert!(p.check_path(&link.join("out.txt").display().to_string(), false, &[]).is_err(), "a link can't lead out");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn validate_rejects_bad_requests() {
        assert!(Permissions { network: vec!["Bad Host".into()], ..Default::default() }.validate().is_err());
        assert!(Permissions { fs: vec![FsRoot { path: "rel".into(), write: false }], ..Default::default() }.validate().is_err());
        assert!(Permissions { network: vec!["*.ok.com".into()], ..Default::default() }.validate().is_ok());
    }
}
