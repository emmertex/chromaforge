//! Synchronize Folder commands (see [`crate::sync`]).

use serde_json::{Value, json};

use super::{CommandSpec, always, bad, bool_or, cmd, str_param};
use crate::folder_watch::{self, NewFilePolicy};
use crate::sync::{SyncChoice, scan, synchronize};
use crate::{Result, Session};

fn path(p: &Value, c: &str) -> Result<String> {
    str_param(p, "path").filter(|d| !d.trim().is_empty()).map(str::to_string).ok_or_else(|| bad(c, "missing `path`"))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            query "folder.scanChanges",
            "Scan Folder for Changes",
            [],
            None,
            "{path, disk?: bool (a whole disk or share, or a folder holding disks)}: compare a folder of the library (and the folders inside it) with the disk; changes nothing → {path, new: [{path, name, format, …}] files the library doesn't have, duplicates: [{path, name, …, existing?: photoId}] (a list of candidates, no longer a count; library.importPreview's `duplicates` is still a count) files whose content a photo already has (the library's photo, or none for a copy of a file found earlier in the scan): imported only with importDuplicates, unreadable: [{path, error}], missing: [{id, path}] photos whose file is gone, moved: [{id, from, to}] photos whose file is elsewhere in the folder (same content), metadata: [{id, path}] photos whose XMP sidecar was saved after they came in or were last edited here and says something the library doesn't}",
            always,
            |s, p| {
                const C: &str = "folder.scanChanges";
                let path = path(p, C)?;
                let changes = scan(s, &path, bool_or(p, "disk", false)).map_err(|e| bad(C, e.to_string()))?;
                let v = serde_json::to_value(&changes).unwrap_or_default();
                s.folder_changes = Some(changes);
                Ok(v)
            }
        ),
        cmd!(
            "folder.synchronize",
            "Synchronize Folder",
            [],
            None,
            "{path, disk?: bool, scanned?: bool, importNew?: true, importDuplicates?: bool (with importNew, the files whose content a photo already has come in too, as photos of their own; default: the library's setting, library.preferences import.importDuplicates, off), relinkMoved?: true, removeMissing?: false, readMetadata?: false}: bring a folder of the library up to date with the disk (see folder.scanChanges): import its new files in place, relink photos whose file was renamed or moved within it, move photos whose file is gone to Recently Deleted, read XMP sidecars saved by other apps (the sidecar wins); acts on the last folder.scanChanges of the folder while it is current (the folder's photos unchanged), else scans again (or, with `scanned: true`, refuses); one undo step, no file is touched → {imported, relinked, removed, read, failed: [[path, error]]}",
            always,
            sync
        ),
        cmd!(
            "folder.watchPreferences",
            "Folder Polling Preferences",
            [],
            None,
            "{poll?: bool (off by default: costly on a VPN or slow link), intervalMinutes?: 1..1440 (default 5), newFiles?: ask (default: an \"N new\" badge on the folder) | add (added at once)} → current preferences (saved with the library)",
            always,
            watch_prefs
        ),
        cmd!(
            "folder.poll",
            "Check Folders for New Photos",
            [],
            None,
            "{}: list each of the library's folders once and scan again, as folder.scanChanges, only those whose listing (modification time or entries) changed since the last poll (the first poll only records the listings); new files are added (newFiles: add, imported in place) or counted for the folder's badge (ask; see folder.newFiles). Runs whether or not polling is on → {listed, offline, changed: [path], new: [{path, count}], imported, failed: [[path, error]]}",
            always,
            poll
        ),
        cmd!(
            query "folder.newFiles",
            "New Files in Folders",
            [],
            None,
            "{}: the folders in which a poll found files the library doesn't have yet (newFiles: ask), until they are added (folder.synchronize) → {folders: [{path, count}], total}",
            always,
            |s, _| {
                let folders: Vec<Value> = s.folder_new.iter().map(|(p, n)| json!({"path": p, "count": n})).collect();
                Ok(json!({"folders": folders, "total": s.folder_new.values().sum::<usize>()}))
            }
        ),
    ]
}

fn watch_prefs(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "folder.watchPreferences";
    let mut w = s.folder_watch;
    if let Some(on) = p.get("poll").and_then(Value::as_bool) {
        w.poll = on;
    }
    if let Some(m) = p.get("intervalMinutes") {
        let m = m.as_u64().ok_or_else(|| bad(C, "`intervalMinutes` is a whole number of minutes"))?;
        w.interval_minutes = u32::try_from(m).unwrap_or(u32::MAX);
    }
    if let Some(n) = str_param(p, "newFiles") {
        w.new_files = NewFilePolicy::parse(n).ok_or_else(|| bad(C, format!("unknown newFiles `{n}` (ask|add)")))?;
    }
    let w = w.sanitized();
    if w != s.folder_watch {
        s.folder_watch = w;
        s.save_prefs()?;
    }
    Ok(serde_json::to_value(s.folder_watch).unwrap_or_default())
}

fn poll(s: &mut Session, _: &Value) -> Result<Value> {
    let dirs = folder_watch::watched_dirs(&s.catalog);
    let polled = folder_watch::poll_dirs(&dirs, &s.folder_stamps, &std::sync::atomic::AtomicBool::new(false));
    Ok(folder_watch::apply(s, polled, |s, path| scan(s, path, false).map_err(|e| e.to_string())))
}

fn sync(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "folder.synchronize";
    let path = path(p, C)?;
    let d = SyncChoice::default();
    let choice = SyncChoice {
        import_new: bool_or(p, "importNew", d.import_new),
        import_duplicates: bool_or(p, "importDuplicates", s.import_defaults.import_duplicates),
        relink_moved: bool_or(p, "relinkMoved", d.relink_moved),
        remove_missing: bool_or(p, "removeMissing", d.remove_missing),
        read_metadata: bool_or(p, "readMetadata", d.read_metadata),
    };
    let changes = match s.take_folder_changes(&path) {
        Some(c) => c,
        // the person agreed to what a scan showed: never act on something else
        None if bool_or(p, "scanned", false) => return Err(bad(C, format!("{path} changed since it was scanned: scan again"))),
        None => scan(s, &path, bool_or(p, "disk", false)).map_err(|e| bad(C, e.to_string()))?,
    };
    let r = synchronize(s, changes, choice)?;
    Ok(json!({"imported": r.imported, "relinked": r.relinked, "removed": r.removed, "read": r.read, "failed": r.failed}))
}
