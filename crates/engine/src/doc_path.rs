//! Paths stored in a file the app reads but the user did not choose path by path, such as the
//! originals a Lightroom catalog lists.
//!
//! Such a path reaches the file system only through [`resolve`]: a relative path is taken from the
//! document's folder, and local files and files on network shares are looked at (see
//! `craft_docpath`). Unlike upstream, which follows only the share the document itself is on, the
//! fork follows any share: a catalog on this computer with its originals on a NAS is the common
//! case (README → *Network drives and shares*). Device paths and URLs are not followed. A path
//! that is not followed is handled like a missing file.

use std::fmt;
use std::path::{Path, PathBuf};

pub use craft_docpath::Refused;

/// Why a stored path is not followed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotFollowed {
    /// The path names a device, is a URL, or is not a usable path.
    Refused(Refused),
    /// The path is not an absolute path on this computer, such as a Windows drive path read on
    /// macOS or Linux.
    NotHere,
}

impl fmt::Display for NotFollowed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused(r) => r.fmt(f),
            Self::NotHere => f.write_str("the path is not a path on this computer"),
        }
    }
}

impl std::error::Error for NotFollowed {}

/// The file to look at for `stored`, a path or `file:` URL stored in the document at `document`,
/// or why it is not followed. A relative `document` is taken from the working directory, where it
/// was opened from.
pub fn resolve(stored: &str, document: Option<&Path>) -> Result<PathBuf, NotFollowed> {
    let document = document.map(|d| if d.is_relative() { std::path::absolute(d).unwrap_or_else(|_| d.to_path_buf()) } else { d.to_path_buf() });
    match craft_docpath::resolve_absolute(stored, document.as_deref()) {
        Ok(path) => Ok(path),
        // craft_docpath refuses a share other than the document's own; the fork follows it
        Err(Refused::Network) => on_a_share(stored).and_then(|path| if path.is_relative() { Err(NotFollowed::NotHere) } else { Ok(path) }),
        // A usable path that is not absolute here: a Windows path on macOS or Linux, or a
        // relative path with no document folder to take it from.
        Err(Refused::Invalid) if craft_docpath::resolve(stored, document.as_deref()).is_ok() => Err(NotFollowed::NotHere),
        Err(r) => Err(NotFollowed::Refused(r)),
    }
}

/// `stored`, a network path craft_docpath refused only because the document is not on its share:
/// resolved on its own share (`.` and `..` folded; devices still refused). A `file://host/…` URL
/// is the share path `\\host\…`. On Linux and macOS a `//folder/…` path is the local folder the
/// system takes it for.
fn on_a_share(stored: &str) -> Result<PathBuf, NotFollowed> {
    if cfg!(not(windows)) && stored.starts_with("//") && Path::new(stored).is_absolute() {
        return Ok(PathBuf::from(stored));
    }
    let unc = match stored.get(..7).filter(|s| s.eq_ignore_ascii_case("file://")).and_then(|_| stored.get(7..)) {
        Some(rest) => format!(r"\\{}", rest.replace('/', r"\")),
        None => stored.to_string(),
    };
    // with the share path itself as the document, the share is the document's, which is followed
    craft_docpath::resolve(&unc, Some(Path::new(&unc))).map_err(NotFollowed::Refused)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths_are_taken_from_the_document_folder_and_devices_are_refused() {
        let doc = std::env::temp_dir().join("lrcat").join("catalog.lrcat");
        assert_eq!(resolve("Photos/a.jpg", Some(&doc)), Ok(std::env::temp_dir().join("lrcat").join("Photos").join("a.jpg")));
        assert_eq!(resolve("a.jpg", None), Err(NotFollowed::NotHere));
        assert_eq!(resolve(r"\\.\COM1", Some(&doc)), Err(NotFollowed::Refused(Refused::Device)));
    }

    /// A catalog on this computer with its originals on a NAS, or on another share than the
    /// catalog's: the originals are followed on every OS (upstream refuses them).
    #[test]
    fn originals_on_any_network_share_are_followed() {
        let doc = std::env::temp_dir().join("lrcat").join("catalog.lrcat");
        let share = resolve(r"\\nas\photos\2026\..\2025\x.jpg", Some(&doc));
        let url = resolve("file://nas/photos/2025/x.jpg", Some(&doc));
        let other_share = resolve(r"\\nas\photos\x.jpg", Some(Path::new(r"\\other\cats\c.lrcat")));
        if cfg!(windows) {
            assert_eq!(share, Ok(PathBuf::from(r"\\nas\photos\2025\x.jpg")));
            assert_eq!(url, Ok(PathBuf::from(r"\\nas\photos\2025\x.jpg")));
            assert_eq!(other_share, Ok(PathBuf::from(r"\\nas\photos\x.jpg")));
        } else {
            // a UNC path names no file on Linux or macOS (shares are mounted under a folder there):
            // not refused, just not a path on this computer
            for r in [&share, &url, &other_share] {
                assert_eq!(*r, Err(NotFollowed::NotHere));
            }
            assert_eq!(resolve("//mnt/nas/x.jpg", Some(&doc)), Ok(PathBuf::from("//mnt/nas/x.jpg")));
        }
        // a mounted share (mapped drive, /Volumes, /mnt) is an ordinary absolute path
        let mounted = if cfg!(windows) { r"Z:\photos\x.jpg" } else { "/Volumes/photos/x.jpg" };
        assert_eq!(resolve(mounted, Some(&doc)), Ok(PathBuf::from(mounted)));
        // still refused: devices
        assert_eq!(resolve(r"\\.\COM1", Some(&doc)), Err(NotFollowed::Refused(Refused::Device)));
        #[cfg(windows)]
        assert_eq!(resolve("Photos/2024:05/a.jpg", Some(&doc)), Err(NotFollowed::Refused(Refused::Invalid)));
        #[cfg(not(windows))]
        assert_eq!(resolve(r"C:\Photos\a.jpg", Some(&doc)), Err(NotFollowed::NotHere));
        let gvfs = Path::new("/run/user/1000/gvfs/smb-share:server=tokyo,share=photos/catalog.lrcat");
        #[cfg(not(windows))]
        assert_eq!(resolve("a.jpg", Some(gvfs)), Ok(gvfs.with_file_name("a.jpg")));
    }
}
