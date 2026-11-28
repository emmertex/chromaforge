//! Pruning scheduled backups (`Journal::backup` with `keep`) and the "restore from" hint (`library::latest_backup`)
//! touch only this library's backups: never another library's backups in a shared backup folder, a library or any
//! other folder kept there, or a copy kept before an upgrade.

use std::path::{Path, PathBuf};

use crate::db::DB_FILE;
use crate::library::{self, BACKUPS_DIR, CatalogSettings, SETTINGS_FILE, is_stamp};
use crate::tests_v4::{Scratch, fill, older_store};
use crate::*;

fn open(dir: &Path) -> (Journal, Catalog, LoadReport) {
    Journal::open(Box::new(FsStore::open(dir).unwrap())).unwrap()
}

/// A library of a few photos in `dir`.
fn library(dir: &Path) -> (Journal, Catalog) {
    std::fs::create_dir_all(dir).unwrap();
    let (mut j, mut c, _) = open(dir);
    fill(&mut j, &mut c, 5);
    (j, c)
}

/// The folders in `root`, sorted.
fn folders(root: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(root).unwrap().map(|e| e.unwrap().path()).filter(|p| p.is_dir()).collect();
    v.sort();
    v
}

/// A folder `root/name` that looks like a catalog (a store file) with a file of its own; `library_id` written into its
/// catalog settings when given.
fn foreign(root: &Path, name: &str, library_id: Option<&str>) -> PathBuf {
    let d = root.join(name);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join(DB_FILE), b"another library's store").unwrap();
    std::fs::write(d.join("keep-me.txt"), b"x").unwrap();
    if let Some(id) = library_id {
        std::fs::write(d.join(SETTINGS_FILE), format!("{{\"libraryId\":\"{id}\"}}")).unwrap();
    }
    d
}

fn id_of(dir: &Path) -> String {
    CatalogSettings::load(dir).library_id.expect("a backup gives the library an id")
}

/// The review's case: a library kept in the backup folder ("2000 Wedding" sorts before every backup's time) was
/// deleted whole by the first backup with `keep = 1`.
#[test]
fn a_folder_in_the_backup_root_that_is_not_a_backup_is_never_pruned() {
    let s = Scratch::new("prune-foreign");
    let (mut j, c) = library(&s.path().join("lib"));
    let root = s.path().join("backups");
    let wedding = foreign(&root, "2000 Wedding", None);
    // named like a backup, but of another library, or of none (in a folder that isn't this library's own)
    let theirs = foreign(&root, "1999-01-01 000000", Some("another-library"));
    let unnamed = foreign(&root, "1998-01-01 000000", None);
    let a = j.backup(&c, &root, 1).unwrap();
    // a copy of this very library kept there by hand: its settings name this library, but its name is no backup's
    let by_hand = foreign(&root, "0 copy of my library", Some(&id_of(&s.path().join("lib"))));
    let b = j.backup(&c, &root, 1).unwrap();
    for kept in [&wedding, &theirs, &unnamed, &by_hand] {
        assert!(kept.join("keep-me.txt").exists(), "{} was pruned", kept.display());
    }
    assert!(!a.exists() && b.exists(), "this library's older backup is pruned, its newest kept");
}

/// Two libraries backing up into one folder (a supported setting) each keep their own newest `keep` and never prune
/// the other's; two backups in the same second get folders of their own.
#[test]
fn two_libraries_sharing_a_backup_folder_keep_each_others_backups() {
    let s = Scratch::new("prune-shared");
    let root = s.path().join("shared");
    let (mut ja, ca) = library(&s.path().join("a"));
    let (mut jb, cb) = library(&s.path().join("b"));
    let mut made_a = Vec::new();
    let mut made_b = Vec::new();
    for _ in 0..3 {
        made_a.push(ja.backup(&ca, &root, 2).unwrap());
        made_b.push(jb.backup(&cb, &root, 2).unwrap());
    }
    assert_ne!(id_of(&s.path().join("a")), id_of(&s.path().join("b")));
    let mut want: Vec<PathBuf> = made_a[1..].iter().chain(&made_b[1..]).cloned().collect();
    want.sort();
    assert_eq!(folders(&root), want, "each library's newest 2, nothing of the other's pruned");
    let all: Vec<&PathBuf> = made_a.iter().chain(&made_b).collect();
    for (i, d) in all.iter().enumerate() {
        assert!(all.iter().skip(i + 1).all(|e| e != d), "every backup got a folder of its own");
    }
}

/// A copy kept before an upgrade (what an older version opens) is never pruned and never takes one of the scheduled
/// backups' places (review mutant M11: the guard against it was untested).
#[test]
fn a_copy_kept_before_an_upgrade_is_never_pruned() {
    let s = older_store("prune-upgrade", Some("11"));
    let (mut j, c, r) = open(s.path());
    let copy = r.upgrade_backup.clone().expect("the open kept a copy before upgrading");
    let root = s.path().join(BACKUPS_DIR);
    let made: Vec<PathBuf> = (0..3).map(|_| j.backup(&c, &root, 2).unwrap()).collect();
    let mut want = vec![copy, made[1].clone(), made[2].clone()];
    want.sort();
    assert_eq!(folders(&root), want, "the copy kept before the upgrade and the 2 newest scheduled backups");
}

/// Backups made before they recorded their library are this library's when they are in its own `backups` folder (no
/// other library writes there), and are left alone anywhere else.
#[test]
fn older_backups_without_a_library_id_are_pruned_only_in_the_librarys_own_folder() {
    let s = Scratch::new("prune-unnamed");
    let lib = s.path().join("lib");
    let (mut j, c) = library(&lib);
    let own = lib.join(BACKUPS_DIR);
    let old_own = foreign(&own, "2001-01-01 000000", None);
    let elsewhere = s.path().join("elsewhere");
    let old_elsewhere = foreign(&elsewhere, "2001-01-01 000000", None);
    let a = j.backup(&c, &own, 1).unwrap();
    let b = j.backup(&c, &elsewhere, 1).unwrap();
    assert!(!old_own.exists(), "an older backup in the library's own folder is pruned");
    assert_eq!(folders(&own), vec![a]);
    assert!(old_elsewhere.join("keep-me.txt").exists(), "in another folder it can't be told from another library's");
    assert_eq!(folders(&elsewhere), vec![old_elsewhere, b]);
}

/// The "restore from" hint names this library's newest backup, not another library's newer one in a shared folder or
/// a folder that only looks like a catalog.
#[test]
fn the_restore_hint_names_this_librarys_newest_backup() {
    let s = Scratch::new("hint-shared");
    let root = s.path().join("shared");
    let a_dir = s.path().join("a");
    let (mut ja, ca) = library(&a_dir);
    let mut st = CatalogSettings::load(&a_dir);
    st.backup_dir = Some(root.clone());
    st.save(&a_dir).unwrap();
    assert_eq!(library::latest_backup(&a_dir), None, "nothing of this library's yet");
    let mine = ja.backup(&ca, &root, 0).unwrap();
    // newer, but not this library's
    foreign(&root, "2999-01-01 000000", Some("another-library"));
    foreign(&root, "zzz", None);
    assert_eq!(library::latest_backup(&a_dir), Some(mine));
}

/// What counts as a backup's time in its folder name: [`library::stamp`]'s, with an optional `-<n>`.
#[test]
fn a_backup_name_is_a_stamp_with_an_optional_number() {
    assert!(is_stamp(&library::stamp(library::now_secs())), "what backups are named");
    assert!(is_stamp("2026-10-10 153005"));
    assert!(is_stamp("2026-10-10 153005-2") && is_stamp("2026-10-10 153005-12"));
    for not in [
        "",
        "2026-10-10 15300",
        "2026-10-10 153005-",
        "2026-10-10 153005-x",
        "2026-10-10 153005 copy",
        "2026-10-10 153005-2-3",
        "2026-13-10 153005",
        "2026-10-10 253005",
        "2026-10-10T153005",
        "before-v12 2026-10-10 153005",
        "2000 Wedding",
        "2026-10-10 15300é",
        "é2026-10-10 153005",
    ] {
        assert!(!is_stamp(not), "{not:?}");
    }
}
