//! Delete from Disk (`photo.deleteFromDisk`): the photos' files go to the system trash
//! (the macOS Trash, the Windows Recycle Bin, the freedesktop trash on Linux and the BSDs) and the
//! photos leave the library.
//!
//! **Never a permanent delete.** A file is only ever *moved to the trash*, through
//! [`Session::trash`] (installed by [`Session::with_fs`]); without it (the web build, a session
//! without a file system) the command refuses, and nothing changes.
//!
//! What goes with a photo:
//! - its virtual copies (they share its file and would be left without one);
//! - its XMP sidecar: `<file>.xmp`, and `<stem>.xmp` unless another file in the folder shares the
//!   stem (`IMG_1.CR3` beside `IMG_1.JPG`: the stem sidecar may be the JPEG's too, so it stays).
//!
//! What stays:
//! - a file another photo still uses (a virtual copy deleted on its own): the photo only leaves
//!   the library, like Lightroom Classic's Remove;
//! - a photo whose file isn't there (a disk that isn't connected) or couldn't be moved to the
//!   trash: it stays in the library and the report says why, so no photo leaves the library
//!   while its file is still somewhere on disk.
//!
//! Callers outside the UI (CLI, MCP, the control channel) must pass `confirm: true`: the command
//! moves files, so it never runs on an id alone. The desktop app asks in its Delete dialog first.
//!
//! The trashing holds an activity row ("Deleting from disk", with progress; cancellable between
//! files: what was moved by then leaves the library, the rest stays).
//!
//! One undo step brings the catalog records back, pointing at their old paths: put the files back
//! from the trash and the photos find them again, with their edits.
//!
//! Sources: own design; the behaviour (Delete from Disk next to Remove) follows Lightroom
//! Classic's delete dialog as observed in the running application.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use lcc_catalog::{Catalog, PhotoId, Source};
use serde_json::{Value, json};

use crate::{EngineError, Result, Session};

/// Moves one file to the system trash (never deletes it outright). `Err` carries a message for
/// people.
pub type TrashFn = Arc<dyn Fn(&Path) -> std::result::Result<(), String> + Send + Sync>;

/// The system trash (the `trash` crate: Trash, Recycle Bin, freedesktop trash).
#[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
pub fn system_trash() -> TrashFn {
    Arc::new(|path: &Path| trash::delete(path).map_err(|e| e.to_string()))
}

/// One file to move to the trash and the photos that leave the library with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Group {
    pub photos: Vec<PhotoId>,
    pub original: PathBuf,
}

/// What Delete from Disk would do with a set of photos, from the catalog alone (no file is
/// touched: the sidecars are looked up while trashing, see [`sidecars_of`]).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Plan {
    pub groups: Vec<Group>,
    /// Photos that leave the library while their file stays (other photos use it).
    pub keep_file: Vec<(PhotoId, String)>,
    /// Photos with no file on disk (demo scenes): they stay.
    pub no_file: Vec<PhotoId>,
}

/// The photos `targets` (unknown ids are ignored) plus their virtual copies, grouped by file.
pub(crate) fn plan(cat: &Catalog, targets: &[PhotoId]) -> Plan {
    let mut leaving: BTreeSet<PhotoId> = targets.iter().copied().filter(|id| cat.photo(*id).is_some()).collect();
    // virtual copies go with their master: they share its file
    let copies: Vec<PhotoId> = cat.photos().filter(|p| p.copy_of.is_some_and(|m| leaving.contains(&m))).map(|p| p.id).collect();
    leaving.extend(copies);

    let mut plan = Plan::default();
    let mut by_path: BTreeMap<String, Vec<PhotoId>> = BTreeMap::new();
    for id in &leaving {
        match cat.photo(*id).map(|p| &p.source) {
            Some(Source::File { path }) if !path.is_empty() => by_path.entry(path.clone()).or_default().push(*id),
            _ => plan.no_file.push(*id),
        }
    }
    // files a photo that stays (in the library, in Recently Deleted or seen in Local) still uses
    let shared: HashSet<&str> = cat
        .photos()
        .filter(|p| !leaving.contains(&p.id))
        .filter_map(|p| match &p.source {
            Source::File { path } if by_path.contains_key(path) => Some(path.as_str()),
            _ => None,
        })
        .collect();
    for (path, photos) in &by_path {
        if shared.contains(path.as_str()) {
            plan.keep_file.extend(photos.iter().map(|id| (*id, path.clone())));
            continue;
        }
        plan.groups.push(Group { photos: photos.clone(), original: PathBuf::from(path) });
    }
    plan
}

/// The originals of `plan` that go to the trash (the set [`Folders`] takes).
fn originals(plan: &Plan) -> HashSet<PathBuf> {
    plan.groups.iter().map(|g| g.original.clone()).collect()
}

/// One folder's listing, read once for a whole Delete from Disk. The sidecar and shared-stem
/// questions for every file of the folder are answered from it, with no further file-system call.
///
/// Issue #692: listing the folder again for each file, and asking the file system about every
/// entry each time, took half an hour for 800 photos in one folder on Windows. A listing read
/// before the first file goes gives the same answers as a fresh one: everything that disappears
/// meanwhile is among `leaving` or a sidecar, and neither counts as sharing a stem.
#[derive(Debug, Default)]
pub(crate) struct FolderIndex {
    /// The folder could be listed; when it couldn't, every stem counts as shared, so no
    /// `<stem>.xmp` goes (the `<file>.xmp` form isn't found either).
    listed: bool,
    /// The files, spelled as listed.
    files: Vec<String>,
    /// Every entry's spelling, files and folders alike.
    spellings: HashSet<String>,
    /// The entries that aren't sidecars (files and folders), by their stem compared without case.
    by_stem: HashMap<String, Vec<String>>,
    /// The names of the originals leaving from this folder.
    leaving: Vec<String>,
}

impl FolderIndex {
    /// Lists `dir` (`""` means the current folder) once. The entries' types come with the listing
    /// on most platforms; only a symbolic link needs a look at its target.
    fn read(dir: &Path, leaving: &HashSet<PathBuf>) -> Self {
        let listing = if dir.as_os_str().is_empty() { Path::new(".") } else { dir };
        let mut index = Self {
            leaving: leaving
                .iter()
                .filter(|l| l.parent() == Some(dir))
                .filter_map(|l| l.file_name())
                .map(|n| n.to_string_lossy().into_owned())
                .collect(),
            ..Self::default()
        };
        let Ok(entries) = std::fs::read_dir(listing) else { return index };
        index.listed = true;
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let is_file = match e.file_type() {
                Ok(t) if !t.is_symlink() => t.is_file(),
                _ => e.path().is_file(),
            };
            if is_file {
                index.files.push(name.clone());
            }
            let as_path = Path::new(&name);
            let is_sidecar = as_path.extension().is_some_and(|x| x.eq_ignore_ascii_case("xmp"));
            if let Some(stem) = as_path.file_stem().filter(|_| !is_sidecar) {
                index.by_stem.entry(stem.to_string_lossy().to_lowercase()).or_default().push(name.clone());
            }
            index.spellings.insert(name);
        }
        index
    }

    /// Whether the listed entry `name` is the file `other` names: the same spelling, or the same
    /// letters in another case while the folder doesn't list `other`'s spelling (one file under
    /// two spellings on a case-insensitive volume; a case-sensitive folder holding `IMG.JPG` and
    /// `img.jpg` lists both, and they are two files).
    fn is_entry_of(&self, name: &str, other: &str) -> bool {
        name == other || (name.to_lowercase() == other.to_lowercase() && !self.spellings.contains(other))
    }

    /// Another entry of the folder, not a sidecar and not among the originals leaving, has
    /// `original`'s stem (compared without case). A folder that couldn't be listed counts as
    /// shared: the sidecar stays.
    fn stem_shared(&self, original: &str) -> bool {
        if !self.listed {
            return true;
        }
        let Some(stem) = Path::new(original).file_stem() else { return true };
        self.by_stem
            .get(&stem.to_string_lossy().to_lowercase())
            .is_some_and(|names| names.iter().any(|n| !self.is_entry_of(n, original) && !self.leaving.iter().any(|l| self.is_entry_of(n, l))))
    }

    /// The XMP sidecars of `original` (a file of this folder) that go with it: `<file>.xmp`
    /// always, `<stem>.xmp` only when no other file in the folder (besides the originals leaving)
    /// shares its stem. Only listed files are named.
    fn sidecars_of(&self, original: &Path) -> Vec<PathBuf> {
        let (Some(name), Some(dir)) = (original.file_name(), original.parent()) else { return Vec::new() };
        let name = name.to_string_lossy();
        let (whole, stem) = sidecar_names(&name, &self.files);
        let mut v: Vec<PathBuf> = whole.into_iter().map(|n| dir.join(n)).collect();
        if !stem.is_empty() && !self.stem_shared(&name) {
            v.extend(stem.into_iter().map(|n| dir.join(n)));
        }
        v
    }
}

/// The folders of one Delete from Disk, each listed at most once (see [`FolderIndex`]).
pub(crate) struct Folders {
    leaving: HashSet<PathBuf>,
    indexes: HashMap<PathBuf, FolderIndex>,
}

impl Folders {
    /// `leaving`: the originals going to the trash (their stems don't count as shared).
    pub(crate) fn new(leaving: HashSet<PathBuf>) -> Self {
        Self { leaving, indexes: HashMap::new() }
    }

    /// The XMP sidecars that go with `original`, from its folder's listing (read on first use).
    pub(crate) fn sidecars_of(&mut self, original: &Path) -> Vec<PathBuf> {
        let Some(dir) = original.parent() else { return Vec::new() };
        if !self.indexes.contains_key(dir) {
            self.indexes.insert(dir.to_path_buf(), FolderIndex::read(dir, &self.leaving));
        }
        self.indexes.get(dir).map(|i| i.sidecars_of(original)).unwrap_or_default()
    }

    /// How many folders were listed so far.
    #[cfg(test)]
    pub(crate) fn listings(&self) -> usize {
        self.indexes.len()
    }
}

/// The entries of `names` (one folder's file names, as listed) that are `original`'s sidecars,
/// compared without case: `<original>.xmp` and `<stem>.xmp`. Each listed file appears at most once.
fn sidecar_names<'a>(original: &str, names: &'a [String]) -> (Vec<&'a str>, Vec<&'a str>) {
    let lower = original.to_lowercase();
    let whole = format!("{lower}.xmp");
    let stem = Path::new(original).file_stem().map(|s| format!("{}.xmp", s.to_string_lossy().to_lowercase()));
    let (mut w, mut st) = (Vec::new(), Vec::new());
    for n in names {
        let l = n.to_lowercase();
        if l == whole {
            w.push(n.as_str());
        } else if stem.as_deref() == Some(l.as_str()) && l != lower {
            st.push(n.as_str());
        }
    }
    (w, st)
}

/// `photo.deleteFromDisk`: move the targets' files to the trash and take the photos out of the
/// library (one undo step). See the module docs.
pub(crate) fn delete_from_disk(s: &mut Session, p: &Value) -> Result<Value> {
    if p.get("confirm").and_then(Value::as_bool) != Some(true) {
        return Err(crate::cmd::bad("photo.deleteFromDisk", "moves the photos' files to the system trash: pass confirm: true to go ahead"));
    }
    let Some(trash) = s.trash.clone() else {
        return Err(EngineError::Other("Delete from Disk isn't available here: this build can't move files to the system trash".into()));
    };
    let targets = s.targets(p)?;
    if targets.is_empty() {
        return Err(crate::cmd::bad("photo.deleteFromDisk", "no photos to delete"));
    }
    let plan = plan(&s.catalog, &targets);
    let mut folders = Folders::new(originals(&plan));
    let shown = |path: &Path| path.to_string_lossy().to_string();
    let (mut remove, mut trashed, mut kept, mut failed) = (Vec::new(), Vec::new(), Vec::new(), Vec::<(String, String)>::new());
    let task = s.activity.start("deleteFromDisk", "Deleting from disk", crate::activity::Cancel::Yes);
    let total = u64::try_from(plan.groups.len()).unwrap_or(u64::MAX);
    let mut cancelled = 0usize;
    for (i, g) in plan.groups.iter().enumerate() {
        if task.is_cancelled() {
            cancelled = cancelled.saturating_add(1);
            continue;
        }
        task.progress(u64::try_from(i).unwrap_or(u64::MAX), total);
        if let Some(name) = g.original.file_name() {
            task.detail(&name.to_string_lossy());
        }
        // never remove a photo whose file is still somewhere: an unplugged disk, a folder
        match std::fs::symlink_metadata(&g.original) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                failed.push((shown(&g.original), "the file isn't there (is its disk connected?)".into()));
                continue;
            }
            Err(e) => {
                failed.push((shown(&g.original), e.to_string()));
                continue;
            }
            Ok(m) if m.is_dir() => {
                failed.push((shown(&g.original), "this is a folder, not a photo's file".into()));
                continue;
            }
            Ok(_) => {}
        }
        // from the folder's listing, read before its first file goes (see `FolderIndex`)
        let sidecars = folders.sidecars_of(&g.original);
        if let Err(e) = trash(&g.original) {
            failed.push((shown(&g.original), e));
            continue;
        }
        trashed.push(shown(&g.original));
        remove.extend(g.photos.iter().copied());
        for sc in &sidecars {
            // (on a case-insensitive disk `a.xmp` and `a.XMP` are one file, gone with the first)
            if !sc.is_file() {
                continue;
            }
            match trash(sc) {
                Ok(()) => trashed.push(shown(sc)),
                Err(e) => failed.push((shown(sc), e)),
            }
        }
    }
    drop(task);
    for (id, path) in &plan.keep_file {
        remove.push(*id);
        kept.push(path.clone());
    }
    for id in &plan.no_file {
        let name = s.catalog.photo(*id).map(|p| p.file_name.clone()).unwrap_or_default();
        failed.push((name, "not a file on disk".into()));
    }
    if remove.is_empty() {
        let why = if cancelled > 0 {
            "cancelled".to_string()
        } else {
            failed.first().map(|(path, e)| format!("{path}: {e}")).unwrap_or_else(|| "no photos to delete".into())
        };
        return Err(EngineError::Other(format!("Nothing was moved to the system trash. {why}")));
    }
    let op = s.catalog.delete_photos_permanently_ops(&remove);
    s.commit("Delete from Disk", op)?;
    kept.dedup();
    Ok(json!({"deleted": remove.len(), "trashed": trashed, "kept": kept, "failed": failed, "cancelled": cancelled}))
}

/// The photos among `targets` (with their virtual copies) whose file Delete from Disk would move
/// to the trash, and how many files that is (originals only): what the confirmation shows. Works
/// from the catalog alone, so it stays quick for thousands of photos.
pub fn preview(cat: &Catalog, targets: &[PhotoId]) -> (usize, usize) {
    let plan = plan(cat, targets);
    let photos = plan.groups.iter().map(|g| g.photos.len()).sum::<usize>() + plan.keep_file.len();
    (photos, plan.groups.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn add_test_photo(s: &mut Session, path: &Path) -> PhotoId {
        let id = s.catalog.alloc_photo_id();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let op = lcc_catalog::Op::AddPhoto {
            photo: Box::new(lcc_catalog::Photo::new(
                id,
                lcc_catalog::Source::File { path: path.to_string_lossy().to_string() },
                &name,
                "JPEG",
                100,
                100,
                "2026-01-01",
            )),
        };
        s.commit("Add test photo", op).unwrap();
        id
    }

    fn add_virtual_copy(s: &mut Session, master_id: PhotoId) -> PhotoId {
        let copy_id = s.catalog.alloc_photo_id();
        let master = s.catalog.photo(master_id).unwrap();
        let mut copy = (**master).clone();
        copy.id = copy_id;
        copy.copy_of = Some(master_id);
        copy.copy_name = Some("Copy 1".into());
        let op = lcc_catalog::Op::AddPhoto { photo: Box::new(copy) };
        s.commit("Add copy", op).unwrap();
        copy_id
    }

    struct TempDir {
        path: PathBuf,
    }
    impl TempDir {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!("lcc-test-delete-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Self { path }
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn plan_single_file_and_preview() {
        let tmp = TempDir::new("single");
        let file = tmp.path.join("img.jpg");
        std::fs::write(&file, b"test").unwrap();

        let mut s = Session::new();
        let id = add_test_photo(&mut s, &file);

        let p = plan(&s.catalog, &[id]);
        assert_eq!(p.groups.len(), 1);
        assert_eq!(p.groups[0].photos, vec![id]);
        assert_eq!(p.groups[0].original, file);
        assert!(Folders::new(originals(&p)).sidecars_of(&file).is_empty());
        assert!(p.keep_file.is_empty());
        assert!(p.no_file.is_empty());

        assert_eq!(preview(&s.catalog, &[id]), (1, 1));
    }

    #[test]
    fn plan_sidecars_shared_stem() {
        let tmp = TempDir::new("sidecars");
        let cr3 = tmp.path.join("test.cr3");
        let cr3_xmp = tmp.path.join("test.cr3.xmp");
        let stem_xmp = tmp.path.join("test.xmp");
        let jpg = tmp.path.join("test.jpg");

        std::fs::write(&cr3, b"raw").unwrap();
        std::fs::write(&cr3_xmp, b"xmp").unwrap();
        std::fs::write(&stem_xmp, b"xmp").unwrap();

        let mut s = Session::new();
        let id_cr3 = add_test_photo(&mut s, &cr3);

        // Alone without test.jpg: both test.cr3.xmp and test.xmp are sidecars
        let p = plan(&s.catalog, &[id_cr3]);
        assert_eq!(Folders::new(originals(&p)).sidecars_of(&cr3).len(), 2);

        // Now add test.jpg on disk and in catalog
        std::fs::write(&jpg, b"jpeg").unwrap();
        let id_jpg = add_test_photo(&mut s, &jpg);

        // Only deleting CR3: test.xmp is shared with test.jpg, so only test.cr3.xmp goes
        let p_cr3_only = plan(&s.catalog, &[id_cr3]);
        assert_eq!(Folders::new(originals(&p_cr3_only)).sidecars_of(&cr3), vec![cr3_xmp.clone()]);

        // Deleting both CR3 and JPG: test.xmp is included because all owners leave
        let p_both = plan(&s.catalog, &[id_cr3, id_jpg]);
        let mut folders = Folders::new(originals(&p_both));
        let all_sidecars: Vec<PathBuf> = p_both.groups.iter().flat_map(|g| folders.sidecars_of(&g.original)).collect();
        assert_eq!(folders.listings(), 1);
        assert!(all_sidecars.contains(&cr3_xmp));
        assert!(all_sidecars.contains(&stem_xmp));
    }

    /// Issue #159: a case-insensitive volume lists one sidecar once (probing `.xmp`, `.XMP`,
    /// `.Xmp` found it three times); a case-sensitive folder holding two spellings lists both.
    #[test]
    fn sidecar_names_follow_the_listing() {
        let one = vec!["test.cr3".to_string(), "test.cr3.XMP".to_string(), "Test.xmp".to_string(), "other.xmp".to_string()];
        assert_eq!(sidecar_names("test.cr3", &one), (vec!["test.cr3.XMP"], vec!["Test.xmp"]));
        let two = vec!["a.jpg".to_string(), "a.jpg.xmp".to_string(), "a.jpg.XMP".to_string()];
        assert_eq!(sidecar_names("a.jpg", &two).0.len(), 2);
        // an original that is itself an .xmp is never its own sidecar
        assert_eq!(sidecar_names("x.xmp", &["x.xmp".to_string()]), (vec![], vec![]));
    }

    #[test]
    fn sidecars_listed_once_on_disk() {
        let tmp = TempDir::new("once");
        let raw = tmp.path.join("img.cr3");
        std::fs::write(&raw, b"raw").unwrap();
        std::fs::write(tmp.path.join("img.cr3.xmp"), b"x").unwrap();
        assert_eq!(Folders::new(HashSet::new()).sidecars_of(&raw), vec![tmp.path.join("img.cr3.xmp")]);
    }

    #[test]
    fn plan_virtual_copies() {
        let tmp = TempDir::new("vc");
        let file = tmp.path.join("img.jpg");
        std::fs::write(&file, b"test").unwrap();

        let mut s = Session::new();
        let master = add_test_photo(&mut s, &file);
        let copy = add_virtual_copy(&mut s, master);

        // Deleting master deletes both, groups 1 file
        let p_master = plan(&s.catalog, &[master]);
        assert_eq!(p_master.groups.len(), 1);
        assert_eq!(p_master.groups[0].photos.len(), 2);
        assert_eq!(preview(&s.catalog, &[master]), (2, 1));

        // Deleting copy alone leaves file on disk
        let p_copy = plan(&s.catalog, &[copy]);
        assert_eq!(p_copy.groups.len(), 0);
        assert_eq!(p_copy.keep_file, vec![(copy, file.to_string_lossy().to_string())]);
        assert_eq!(preview(&s.catalog, &[copy]), (1, 0));
    }

    #[test]
    fn delete_from_disk_command_trashes_and_undo_restores() {
        let tmp = TempDir::new("cmd");
        let file = tmp.path.join("photo.jpg");
        let xmp = tmp.path.join("photo.jpg.xmp");
        std::fs::write(&file, b"photo").unwrap();
        std::fs::write(&xmp, b"xmp").unwrap();

        let mut s = Session::new();
        let trashed = Arc::new(Mutex::new(Vec::new()));
        let t_clone = trashed.clone();
        s.trash = Some(Arc::new(move |p: &Path| {
            t_clone.lock().unwrap().push(p.to_path_buf());
            Ok(())
        }));

        let id = add_test_photo(&mut s, &file);
        s.selection = crate::Selection::single(id);

        let r = s.execute("photo.deleteFromDisk", &json!({"ids": [id.0], "confirm": true})).unwrap();
        assert_eq!(r["deleted"], 1);

        let trashed_paths = trashed.lock().unwrap().clone();
        assert!(trashed_paths.contains(&file));
        assert!(trashed_paths.contains(&xmp));
        assert!(s.catalog.photo(id).is_none(), "photo permanently removed from catalog");

        // Undo brings catalog record back
        s.execute("edit.undo", &json!({})).unwrap();
        assert!(s.catalog.photo(id).is_some(), "undo restored catalog record");
    }

    #[test]
    fn delete_from_disk_virtual_copy_keeps_file() {
        let tmp = TempDir::new("vc-del");
        let file = tmp.path.join("photo.jpg");
        std::fs::write(&file, b"photo").unwrap();

        let mut s = Session::new();
        let trashed = Arc::new(Mutex::new(Vec::new()));
        let t_clone = trashed.clone();
        s.trash = Some(Arc::new(move |p: &Path| {
            t_clone.lock().unwrap().push(p.to_path_buf());
            Ok(())
        }));

        let master = add_test_photo(&mut s, &file);
        let copy = add_virtual_copy(&mut s, master);
        s.selection = crate::Selection::single(copy);

        let r = s.execute("photo.deleteFromDisk", &json!({"ids": [copy.0], "confirm": true})).unwrap();
        assert_eq!(r["deleted"], 1);
        assert!(trashed.lock().unwrap().is_empty(), "file was NOT trashed");
        assert!(s.catalog.photo(copy).is_none(), "copy was removed");
        assert!(s.catalog.photo(master).is_some(), "master remains");
    }

    #[test]
    fn delete_from_disk_missing_file_or_no_trash_fails_safely() {
        let mut s = Session::new();
        let id = s.catalog.alloc_photo_id();
        let op = lcc_catalog::Op::AddPhoto {
            photo: Box::new(lcc_catalog::Photo::new(
                id,
                lcc_catalog::Source::File { path: "/nonexistent/photo.jpg".into() },
                "photo.jpg",
                "JPEG",
                100,
                100,
                "2026-01-01",
            )),
        };
        s.commit("Add photo", op).unwrap();
        s.selection = crate::Selection::single(id);

        // Without trash
        let err = s.execute("photo.deleteFromDisk", &json!({"ids": [id.0], "confirm": true})).unwrap_err();
        assert!(err.to_string().contains("Delete from Disk isn't available here"));

        // With trash, but file doesn't exist
        s.trash = Some(Arc::new(|_| Ok(())));
        let err = s.execute("photo.deleteFromDisk", &json!({"ids": [id.0], "confirm": true})).unwrap_err();
        assert!(err.to_string().contains("Nothing was moved"));
        assert!(s.catalog.photo(id).is_some(), "photo stayed in library");
    }

    /// CLI, MCP and the control channel can't move files to the trash without saying so.
    #[test]
    fn delete_from_disk_requires_confirm() {
        let tmp = TempDir::new("confirm");
        let file = tmp.path.join("photo.jpg");
        std::fs::write(&file, b"photo").unwrap();
        let mut s = Session::new();
        let trashed = Arc::new(Mutex::new(Vec::new()));
        let t_clone = trashed.clone();
        s.trash = Some(Arc::new(move |p: &Path| {
            t_clone.lock().unwrap().push(p.to_path_buf());
            Ok(())
        }));
        let id = add_test_photo(&mut s, &file);
        s.selection = crate::Selection::single(id);
        for p in [json!({"ids": [id.0]}), json!({"ids": [id.0], "confirm": false}), json!({"ids": [id.0], "confirm": "yes"})] {
            assert!(s.execute("photo.deleteFromDisk", &p).is_err(), "{p}");
        }
        assert!(trashed.lock().unwrap().is_empty(), "nothing trashed without confirm");
        assert!(s.catalog.photo(id).is_some());
    }

    /// The trashing shows in the activity stack while it runs and leaves no row behind.
    #[test]
    fn delete_from_disk_shows_in_the_activity_stack() {
        let tmp = TempDir::new("activity");
        let file = tmp.path.join("photo.jpg");
        std::fs::write(&file, b"photo").unwrap();
        let mut s = Session::new();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (activity, seen2) = (s.activity.clone(), seen.clone());
        s.trash = Some(Arc::new(move |_: &Path| {
            seen2.lock().unwrap().extend(activity.list().into_iter().map(|t| t.kind));
            Ok(())
        }));
        let id = add_test_photo(&mut s, &file);
        s.selection = crate::Selection::single(id);
        s.execute("photo.deleteFromDisk", &json!({"ids": [id.0], "confirm": true})).unwrap();
        assert!(seen.lock().unwrap().contains(&"deleteFromDisk"), "a row while trashing");
        assert!(s.activity.list().is_empty(), "no row afterwards");
    }

    /// Cancelling from the stack stops between files: what was moved leaves the library, the rest stays.
    #[test]
    fn delete_from_disk_cancel_keeps_the_rest() {
        let tmp = TempDir::new("cancel");
        let (a, b) = (tmp.path.join("a.jpg"), tmp.path.join("b.jpg"));
        std::fs::write(&a, b"a").unwrap();
        std::fs::write(&b, b"b").unwrap();
        let mut s = Session::new();
        let activity = s.activity.clone();
        s.trash = Some(Arc::new(move |_: &Path| {
            activity.cancel_all();
            Ok(())
        }));
        let (ia, ib) = (add_test_photo(&mut s, &a), add_test_photo(&mut s, &b));
        s.selection = crate::Selection::single(ia);
        let r = s.execute("photo.deleteFromDisk", &json!({"ids": [ia.0, ib.0], "confirm": true})).unwrap();
        assert_eq!(r["deleted"], 1);
        assert_eq!(r["cancelled"], 1);
        assert_eq!([ia, ib].iter().filter(|id| s.catalog.photo(**id).is_some()).count(), 1);
    }

    /// Issue #692: 800 photos in one folder took half an hour, the folder being listed (and every
    /// entry stat-ed) again for each file. One Delete from Disk lists each folder once.
    #[test]
    fn delete_from_disk_lists_each_folder_once() {
        let tmp = TempDir::new("once-per-folder");
        let sub = tmp.path.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        let mut s = Session::new();
        let mut ids = Vec::new();
        for i in 0..40 {
            let dir = if i % 2 == 0 { &tmp.path } else { &sub };
            let f = dir.join(format!("img_{i:03}.cr3"));
            std::fs::write(&f, b"raw").unwrap();
            std::fs::write(dir.join(format!("img_{i:03}.cr3.xmp")), b"x").unwrap();
            std::fs::write(dir.join(format!("img_{i:03}.xmp")), b"x").unwrap();
            ids.push(add_test_photo(&mut s, &f));
        }
        let plan = plan(&s.catalog, &ids);
        let mut folders = Folders::new(originals(&plan));
        let sidecars: Vec<PathBuf> = plan.groups.iter().flat_map(|g| folders.sidecars_of(&g.original)).collect();
        assert_eq!(folders.listings(), 2, "one listing per folder");
        assert_eq!(sidecars.len(), 80, "both sidecars of every file");

        let trashed = Arc::new(Mutex::new(0usize));
        let t = trashed.clone();
        s.trash = Some(Arc::new(move |p: &Path| {
            std::fs::remove_file(p).map_err(|e| e.to_string())?;
            *t.lock().unwrap() += 1;
            Ok(())
        }));
        s.selection = crate::Selection { ids: ids.clone(), active: ids.first().copied() };
        let r = s.execute("photo.deleteFromDisk", &json!({"ids": ids.iter().map(|i| i.0).collect::<Vec<_>>(), "confirm": true})).unwrap();
        assert_eq!(r["deleted"], 40);
        assert_eq!(*trashed.lock().unwrap(), 120, "40 originals and 80 sidecars");
        assert!(std::fs::read_dir(&sub).unwrap().next().is_none());
    }

    /// The listing answers the same way the file system did: one sidecar under two spellings
    /// on a case-insensitive volume, a stem shared with a file that stays or leaves, a listed
    /// original whose catalog path is spelled in another case, and a folder that can't be listed.
    #[test]
    fn folder_index_answers_like_the_file_system() {
        let tmp = TempDir::new("index");
        for n in ["IMG.CR3", "IMG.JPG", "IMG.xmp", "IMG.CR3.xmp", "alone.cr3", "alone.xmp", "pair.cr3", "pair.xmp"] {
            std::fs::write(tmp.path.join(n), b"x").unwrap();
        }
        std::fs::create_dir_all(tmp.path.join("pair")).unwrap();
        let cr3 = tmp.path.join("IMG.CR3");
        let jpg = tmp.path.join("IMG.JPG");

        // IMG.xmp may be the JPEG's too: it stays while the JPEG stays
        let mut only_cr3 = Folders::new(HashSet::from([cr3.clone()]));
        assert_eq!(only_cr3.sidecars_of(&cr3), vec![tmp.path.join("IMG.CR3.xmp")]);
        // and goes when both leave, named once
        let mut both = Folders::new(HashSet::from([cr3.clone(), jpg.clone()]));
        assert_eq!(both.sidecars_of(&cr3), vec![tmp.path.join("IMG.CR3.xmp"), tmp.path.join("IMG.xmp")]);
        assert_eq!(both.sidecars_of(&jpg), vec![tmp.path.join("IMG.xmp")]);
        assert_eq!(both.listings(), 1);
        // a catalog path spelled in another case still names the listed file, so it isn't
        // "another file with the stem", and the sidecar goes; this folder lists one spelling
        let respelled = tmp.path.join("img.cr3");
        let mut other_case = Folders::new(HashSet::from([respelled.clone(), tmp.path.join("img.jpg")]));
        let index = FolderIndex::read(&tmp.path, &other_case.leaving);
        assert!(!index.stem_shared("img.cr3"));
        assert_eq!(other_case.sidecars_of(&respelled).len(), 2);
        // a folder named like the stem counts as sharing it, as it always did
        let pair = tmp.path.join("pair.cr3");
        assert!(Folders::new(HashSet::from([pair.clone()])).sidecars_of(&pair).is_empty());
        // nothing else shares "alone"
        let alone = tmp.path.join("alone.cr3");
        assert_eq!(Folders::new(HashSet::from([alone.clone()])).sidecars_of(&alone), vec![tmp.path.join("alone.xmp")]);
        // an unlistable folder: no sidecar is named, nothing panics
        let missing = tmp.path.join("missing").join("x.cr3");
        assert!(Folders::new(HashSet::from([missing.clone()])).sidecars_of(&missing).is_empty());
        assert!(Folders::new(HashSet::new()).sidecars_of(Path::new("")).is_empty());
    }

    /// A case-sensitive folder listing both `IMG.JPG` and `img.jpg` holds two files: the one
    /// leaving doesn't free the stem sidecar the other may use.
    #[test]
    fn folder_index_tells_two_spellings_apart() {
        let names = |v: &[&str]| v.iter().map(|n| n.to_string()).collect::<Vec<String>>();
        let index = FolderIndex {
            listed: true,
            files: names(&["IMG.JPG", "img.jpg", "img.xmp"]),
            spellings: names(&["IMG.JPG", "img.jpg", "img.xmp"]).into_iter().collect(),
            by_stem: HashMap::from([("img".to_string(), names(&["IMG.JPG", "img.jpg"]))]),
            leaving: names(&["IMG.JPG"]),
        };
        assert!(index.stem_shared("IMG.JPG"));
        // a case-insensitive volume lists one spelling: the catalog's other spelling is that file
        let one = FolderIndex {
            listed: true,
            files: names(&["IMG.JPG", "img.xmp"]),
            spellings: names(&["IMG.JPG", "img.xmp"]).into_iter().collect(),
            by_stem: HashMap::from([("img".to_string(), names(&["IMG.JPG"]))]),
            leaving: Vec::new(),
        };
        assert!(!one.stem_shared("img.jpg"));
        // a folder that couldn't be listed keeps every stem sidecar
        assert!(FolderIndex::default().stem_shared("img.jpg"));
    }
}
