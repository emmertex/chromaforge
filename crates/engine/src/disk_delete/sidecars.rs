//! The XMP sidecars that go with a file Delete from Disk moves to the trash, found from one
//! listing per folder.
//!
//! Issue #692: looking the sidecars up again for every file (a listing plus a `stat` of every entry,
//! then a second listing with two `stat`s per entry for every file of the run) made deleting
//! hundreds of photos take tens of minutes on Windows. Each folder of the run is now listed once,
//! before any of its files goes, and the listing answers every lookup. That gives the same answers
//! as a fresh listing: everything that disappears during the run is among the files leaving, which
//! the shared-stem test leaves out anyway.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// The folders of one Delete from Disk run, each listed once. `leaving` holds every original the
/// run moves to the trash: a stem they share with a sidecar doesn't keep it.
pub(crate) struct Folders {
    leaving: HashSet<PathBuf>,
    indexes: HashMap<PathBuf, FolderIndex>,
    reads: usize,
}

impl Folders {
    pub(crate) fn new(leaving: HashSet<PathBuf>) -> Self {
        Self { leaving, indexes: HashMap::new(), reads: 0 }
    }

    /// `original`'s sidecars that go with it: `<file>.xmp` always, `<stem>.xmp` only when no
    /// other file in the folder (besides those leaving) shares its stem. Only listed files are
    /// returned.
    pub(crate) fn sidecars_of(&mut self, original: &Path) -> Vec<PathBuf> {
        let (Some(name), Some(dir)) = (original.file_name(), original.parent()) else { return Vec::new() };
        if !self.indexes.contains_key(dir) {
            let index = FolderIndex::read(dir, &self.leaving);
            self.indexes.insert(dir.to_path_buf(), index);
            self.reads = self.reads.saturating_add(1);
        }
        let Some(index) = self.indexes.get(dir) else { return Vec::new() };
        index.sidecars_of(dir, &name.to_string_lossy())
    }

    /// How many folders were listed so far.
    #[cfg(test)]
    pub(crate) fn reads(&self) -> usize {
        self.reads
    }
}

/// One folder's entries, as spelled on disk.
#[derive(Debug, Default)]
struct FolderIndex {
    /// Every entry's name, files and folders alike.
    listed: HashSet<String>,
    /// The names of the regular files.
    files: Vec<String>,
    /// The entries that aren't `.xmp` files, by their stem compared without case.
    by_stem: HashMap<String, Vec<String>>,
    /// The entries that are among the run's originals (see [`Going`]).
    going: Going,
    /// The folder could be listed. One that can't counts every stem as shared: its sidecars stay.
    listable: bool,
}

impl FolderIndex {
    fn read(dir: &Path, leaving: &HashSet<PathBuf>) -> Self {
        let listing = if dir.as_os_str().is_empty() { Path::new(".") } else { dir };
        let Ok(entries) = std::fs::read_dir(listing) else { return Self::default() };
        let mut index = Self { listable: true, ..Self::default() };
        for e in entries.flatten() {
            // the listing says what each entry is; only a symlink needs a look at its target
            let is_file = match e.file_type() {
                Ok(t) if t.is_symlink() => e.path().is_file(),
                Ok(t) => t.is_file(),
                Err(_) => false,
            };
            index.add(e.file_name().to_string_lossy().into_owned(), is_file);
        }
        for name in leaving.iter().filter(|p| p.parent() == Some(dir)).filter_map(|p| p.file_name()) {
            index.going.add(&index.listed, &name.to_string_lossy());
        }
        index
    }

    fn add(&mut self, name: String, is_file: bool) {
        let p = Path::new(&name);
        let is_xmp = p.extension().is_some_and(|e| e.eq_ignore_ascii_case("xmp"));
        if let Some(stem) = p.file_stem().filter(|_| !is_xmp) {
            self.by_stem.entry(stem.to_string_lossy().to_lowercase()).or_default().push(name.clone());
        }
        if is_file {
            self.files.push(name.clone());
        }
        self.listed.insert(name);
    }

    fn sidecars_of(&self, dir: &Path, original: &str) -> Vec<PathBuf> {
        let (whole, stem) = sidecar_names(original, &self.files);
        let mut v: Vec<PathBuf> = whole.into_iter().map(|n| dir.join(n)).collect();
        if !stem.is_empty() && !self.stem_shared(original) {
            v.extend(stem.into_iter().map(|n| dir.join(n)));
        }
        v
    }

    /// Another entry that stays, not a sidecar, has `original`'s stem (compared without case).
    fn stem_shared(&self, original: &str) -> bool {
        if !self.listable {
            return true;
        }
        let Some(stem) = Path::new(original).file_stem().map(|s| s.to_string_lossy().to_lowercase()) else { return true };
        let mut itself = Going::default();
        itself.add(&self.listed, original);
        self.by_stem.get(&stem).is_some_and(|names| names.iter().any(|n| !self.going.matches(n) && !itself.matches(n)))
    }
}

/// The folder's entries that are files of the run, matched to the listing without touching the
/// disk: a path names the entry spelled the same, or, when the folder doesn't list its spelling
/// (a case-insensitive volume: `test.xmp` and `test.XMP` are one file), the entry that spells it
/// differently. That is [`crate::rename::RealFs::same_file`]'s rule, "they look alike and the
/// folder doesn't list both", answered from one listing.
#[derive(Debug, Default)]
struct Going {
    exact: HashSet<String>,
    respelled: HashSet<String>,
}

impl Going {
    fn add(&mut self, listed: &HashSet<String>, name: &str) {
        if listed.contains(name) {
            self.exact.insert(name.to_string());
        } else {
            self.respelled.insert(name.to_lowercase());
        }
    }

    fn matches(&self, name: &str) -> bool {
        self.exact.contains(name) || (!self.respelled.is_empty() && self.respelled.contains(&name.to_lowercase()))
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

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir {
        path: PathBuf,
    }
    impl TempDir {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!("lcc-test-sidecars-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Self { path }
        }
        fn file(&self, name: &str) -> PathBuf {
            let p = self.path.join(name);
            std::fs::write(&p, b"x").unwrap();
            p
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    fn sorted(mut v: Vec<PathBuf>) -> Vec<PathBuf> {
        v.sort();
        v
    }

    /// Issue #692: a run over many files in one folder lists it once; before, every file listed the
    /// folder twice and compared every entry with every file of the run.
    #[test]
    fn a_folder_is_listed_once_for_the_whole_run() {
        let tmp = TempDir::new("once");
        for i in 0..40 {
            tmp.file(&format!("keep_{i}.jpg"));
        }
        let originals: Vec<PathBuf> = (0..60).map(|i| tmp.file(&format!("del_{i}.cr3"))).collect();
        for i in 0..60 {
            tmp.file(&format!("del_{i}.xmp"));
        }
        let mut folders = Folders::new(originals.iter().cloned().collect());
        for (i, o) in originals.iter().enumerate() {
            assert_eq!(folders.sidecars_of(o), vec![tmp.path.join(format!("del_{i}.xmp"))]);
        }
        assert_eq!(folders.reads(), 1);
    }

    /// The listing's answers for the cases the per-file lookups covered.
    #[test]
    fn sidecars_from_the_listing() {
        let tmp = TempDir::new("cases");
        let cr3 = tmp.file("test.cr3");
        let cr3_xmp = tmp.file("test.cr3.xmp");
        let stem_xmp = tmp.file("test.xmp");
        // alone: both sidecars go
        assert_eq!(sorted(Folders::new(HashSet::from([cr3.clone()])).sidecars_of(&cr3)), sorted(vec![cr3_xmp.clone(), stem_xmp.clone()]));

        // a JPEG shares the stem: test.xmp may be its sidecar too, so it stays
        let jpg = tmp.file("test.jpg");
        assert_eq!(Folders::new(HashSet::from([cr3.clone()])).sidecars_of(&cr3), vec![cr3_xmp.clone()]);
        // unless the JPEG leaves in the same run
        let mut both = Folders::new(HashSet::from([cr3.clone(), jpg.clone()]));
        assert!(both.sidecars_of(&cr3).contains(&stem_xmp));
        assert!(both.sidecars_of(&jpg).contains(&stem_xmp));
        assert_eq!(both.reads(), 1);
        // a same-named file leaving from another folder doesn't count
        let elsewhere = tmp.path.join("other").join("test.jpg");
        assert_eq!(Folders::new(HashSet::from([cr3.clone(), elsewhere])).sidecars_of(&cr3), vec![cr3_xmp.clone()]);
        // the original spelled differently from the listing (a case-insensitive volume) is itself
        let respelled = tmp.path.join("TEST.CR3");
        assert_eq!(Folders::new(HashSet::from([respelled.clone()])).sidecars_of(&respelled), vec![cr3_xmp.clone()]);
        std::fs::remove_file(&jpg).unwrap();
        assert_eq!(sorted(Folders::new(HashSet::from([respelled.clone()])).sidecars_of(&respelled)), sorted(vec![cr3_xmp.clone(), stem_xmp.clone()]));

        // a folder with the stem counts as sharing it, as it did
        std::fs::create_dir(tmp.path.join("test")).unwrap();
        assert_eq!(Folders::new(HashSet::from([cr3.clone()])).sidecars_of(&cr3), vec![cr3_xmp.clone()]);

        // a folder that can't be listed, and paths without a file name, give nothing and no panic
        let gone = tmp.path.join("missing").join("x.jpg");
        assert!(Folders::new(HashSet::from([gone.clone()])).sidecars_of(&gone).is_empty());
        for p in ["", "/", ".", "..", "/x.jpg/"] {
            let p = PathBuf::from(p);
            let _ = Folders::new(HashSet::from([p.clone()])).sidecars_of(&p);
        }
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

    /// A photo whose listing names both spellings (a case-sensitive volume) keeps them apart.
    #[test]
    fn two_spellings_listed_are_two_files() {
        let mut index = FolderIndex { listable: true, ..FolderIndex::default() };
        for n in ["IMG.CR3", "img.cr3", "img.xmp"] {
            index.add(n.to_string(), true);
        }
        index.going.add(&index.listed, "IMG.CR3");
        // img.cr3 stays, so img.xmp stays with it
        assert!(index.stem_shared("IMG.CR3"));
        // with img.cr3 leaving too, the stem is free
        index.going.add(&index.listed, "img.cr3");
        assert!(!index.stem_shared("IMG.CR3"));
    }
}
