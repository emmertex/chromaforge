//! File access for plug-ins and the plug-in store that a symbolic link swapped in after the path
//! check can't redirect, and writes that are durable (synced file and folder) and atomic.
//!
//! - [`write_atomic`]: a new temp file (exclusive create, so it never follows a link planted at its
//!   name) in the target's folder, synced, renamed over the target (a rename replaces a link
//!   itself, never what it points to), then the folder synced.
//! - [`write_checked`]: the same, after checking that the temp file really landed in the folder
//!   the permission check resolved (a folder swapped for a link in between is refused).
//! - [`read_checked`]: opens, then checks that the opened file is the regular file at the checked
//!   path (not a link swapped in) before reading.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

fn pid() -> u32 {
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::process::id()
    }
    #[cfg(target_arch = "wasm32")]
    {
        0
    }
}

/// Sync a folder so a rename in it is durable (POSIX; opening a folder fails on Windows, where
/// NTFS journals the rename, so errors are ignored).
fn sync_dir(dir: &Path) {
    if let Ok(d) = File::open(dir) {
        let _ = d.sync_all();
    }
}

/// A new temp file next to `path`.
fn create_temp(path: &Path) -> std::io::Result<(PathBuf, File)> {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    for _ in 0..1000 {
        let tmp = dir.join(format!(".{name}.{}.{}.tmp", pid(), N.fetch_add(1, Ordering::Relaxed)));
        match OpenOptions::new().write(true).create_new(true).open(&tmp) {
            Ok(f) => return Ok((tmp, f)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, format!("no free temp file name in {}", dir.display())))
}

/// Temp file + sync + `verify(temp path)` + rename + folder sync. Any failure removes the temp
/// file and leaves `path` as it was.
fn replace(path: &Path, bytes: &[u8], verify: &dyn Fn(&Path) -> std::io::Result<()>) -> std::io::Result<()> {
    let (tmp, mut f) = create_temp(path)?;
    let r = (|| {
        verify(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    drop(f);
    if let Err(e) = r {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        sync_dir(dir);
    }
    Ok(())
}

/// Replace (or create) `path` with `bytes` atomically and durably; see the module docs.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    replace(path, bytes, &|_| Ok(()))
}

/// [`write_atomic`] to a path a permission check resolved (its folder canonical): refused when
/// the temp file did not land in that very folder.
pub(crate) fn write_checked(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let want = path.parent().map(Path::to_path_buf);
    replace(path, bytes, &|tmp| {
        let got = tmp.canonicalize()?;
        if got.parent().map(Path::to_path_buf) != want {
            return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "the folder changed while writing (a symbolic link?)"));
        }
        Ok(())
    })
}

/// Read at most `max` bytes of the app's own file at `path` (no link check: the path is the app's,
/// not one a plug-in named, and needn't be canonical, e.g. `/var` vs `/private/var` on macOS).
pub(crate) fn read_capped(path: &Path, max: u64) -> std::io::Result<Vec<u8>> {
    let f = File::open(path)?;
    let mut out = Vec::new();
    f.take(max.saturating_add(1)).read_to_end(&mut out)?;
    if out.len() as u64 > max {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, format!("larger than {max} bytes")));
    }
    Ok(out)
}

/// Read at most `max` bytes of the regular file at the resolved `path`, refusing a link swapped
/// in after the check (the opened file must be the very file at `path`, not reached through a
/// link).
pub(crate) fn read_checked(path: &Path, max: u64) -> std::io::Result<Vec<u8>> {
    let f = File::open(path)?;
    let opened = f.metadata()?;
    let here = std::fs::symlink_metadata(path)?;
    let moved = || std::io::Error::new(std::io::ErrorKind::PermissionDenied, "the file changed while opening it (a symbolic link?)");
    if !here.is_file() || !opened.is_file() || path.canonicalize()? != path {
        return Err(moved());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if (opened.dev(), opened.ino()) != (here.dev(), here.ino()) {
            return Err(moved());
        }
    }
    if opened.len() > max {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, format!("larger than {max} bytes")));
    }
    let mut out = Vec::new();
    f.take(max.saturating_add(1)).read_to_end(&mut out)?;
    if out.len() as u64 > max {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, format!("larger than {max} bytes")));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lcc-plugins-safe-io-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.canonicalize().unwrap()
    }

    #[test]
    fn atomic_writes_replace_and_leave_no_temp() {
        let d = tmp("atomic");
        let p = d.join("plugins.json");
        write_atomic(&p, b"one").unwrap();
        write_checked(&p, b"two").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"two");
        assert_eq!(std::fs::read_dir(&d).unwrap().count(), 1);
        assert_eq!(read_checked(&p, 10).unwrap(), b"two");
        assert!(read_checked(&p, 2).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    #[test]
    fn links_never_redirect_reads_or_writes() {
        let d = tmp("links");
        let inside = d.join("root");
        let outside = d.join("outside");
        std::fs::create_dir_all(&inside).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let secret = outside.join("secret.txt");
        std::fs::write(&secret, b"secret").unwrap();
        // a file swapped for a link after the check: not read through
        let p = inside.join("a.txt");
        std::os::unix::fs::symlink(&secret, &p).unwrap();
        assert!(read_checked(&p, 100).is_err());
        // a write replaces the link itself, never the file it points to
        write_checked(&p, b"mine").unwrap();
        assert_eq!(std::fs::read(&secret).unwrap(), b"secret");
        assert!(std::fs::symlink_metadata(&p).unwrap().is_file());
        // a folder swapped for a link after the check: the write is refused
        let sub = inside.join("sub");
        std::os::unix::fs::symlink(&outside, &sub).unwrap();
        assert!(write_checked(&sub.join("x.txt"), b"x").is_err());
        assert!(!outside.join("x.txt").exists());
        assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 1, "no temp file left outside");
        let _ = std::fs::remove_dir_all(&d);
    }
}
