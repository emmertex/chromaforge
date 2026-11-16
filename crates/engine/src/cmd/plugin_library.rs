//! What plug-ins learn of the Library and add to it: the `source` a command was started from
//! (the Library source the session shows), and `library.add`, which adds a file a tool made from
//! a photo to the library, stacked on that photo or in an album that holds it.

use std::path::Path;

use lcc_catalog::{AlbumId, Op, PhotoId, Source};
use serde_json::{Value, json};

use crate::{LibrarySource, Session};

/// The kinds of `source` a plug-in can be given.
const KINDS: &[&str] = &["album", "smartAlbum", "folder", "allPhotos", "other"];

/// The Library source `s` shows, as a plug-in command gets it: `{kind: album | smartAlbum, id,
/// name, quick?}`, `{kind: folder, path}`, `{kind: allPhotos}` or `{kind: other, name}` (the
/// built-in view: `picks`, `recentlyAdded`, …). A search or filter on top doesn't change it.
pub(crate) fn source(s: &Session) -> Value {
    let album = |id: AlbumId| {
        s.catalog.album(id).map(|al| {
            if al.folder {
                // holds albums, not photos: nothing a plug-in could file into
                return json!({"kind": "other", "name": "albumFolder"});
            }
            let kind = if al.is_smart() { "smartAlbum" } else { "album" };
            let mut v = json!({"kind": kind, "id": id.0.to_string(), "name": al.name});
            if al.quick {
                v["quick"] = json!(true);
            }
            v
        })
    };
    let folder = |path: Option<&str>| path.filter(|p| !p.is_empty()).map(|p| json!({"kind": "folder", "path": p}));
    let found = match s.source {
        LibrarySource::All => Some(json!({"kind": "allPhotos"})),
        LibrarySource::Album(id) => album(id),
        LibrarySource::QuickCollection => s.catalog.quick_collection().and_then(album),
        LibrarySource::Folder => folder(s.browse.as_ref().map(|b| b.path.as_str())),
        LibrarySource::LibraryFolder => folder(s.library_folder.as_deref()),
        _ => None,
    };
    found.unwrap_or_else(|| json!({"kind": "other", "name": other_name(s.source)}))
}

/// The name a built-in view goes by in `{kind: other, name}`: the source's own id.
fn other_name(src: LibrarySource) -> String {
    serde_json::to_value(src).ok().and_then(|v| v.get("kind").and_then(Value::as_str).map(str::to_string)).unwrap_or_default()
}

/// A `source` someone passed to `plugin.run` / `plugin.prepare`, checked: an object whose `kind`
/// is one of [`KINDS`], with only short strings and `quick` in it.
pub(super) fn checked_source(v: &Value) -> std::result::Result<Value, String> {
    let o = v.as_object().ok_or("`source` must be an object {kind, id?, name?, path?}")?;
    let kind = o.get("kind").and_then(Value::as_str).ok_or("`source` needs a `kind`")?;
    if !KINDS.contains(&kind) {
        return Err(format!("unknown source kind {kind:?} ({})", KINDS.join(", ")));
    }
    for (k, x) in o {
        match (k.as_str(), x) {
            ("kind", _) => {}
            ("id" | "name" | "path", Value::String(t)) if t.len() <= 4096 => {}
            ("quick", Value::Bool(_)) => {}
            _ => return Err(format!("`source.{k}` is not allowed here (id, name, path: strings; quick: bool)")),
        }
    }
    Ok(v.clone())
}

/// One photo id: a number, or the decimal string the SDK hands out.
fn id_of(v: Option<&Value>, key: &str) -> std::result::Result<u64, String> {
    let v = v.ok_or_else(|| format!("missing `{key}`"))?;
    v.as_u64().or_else(|| v.as_str().and_then(|t| t.trim().parse().ok())).ok_or_else(|| format!("`{key}`: {v} is not an id"))
}

/// `library.add {path, from, stack?, album?}` (see `lcc_plugins::HostApi::library_add`):
/// everything is checked before the library changes, and what changes is one undo step. The
/// user's selection is left as it was: the call comes when a tool finishes, often while the user
/// works on something else. → `{photo, added, reloaded}`
pub(super) fn add(s: &mut Session, p: &Value) -> std::result::Result<Value, String> {
    let path = p.get("path").and_then(Value::as_str).filter(|t| !t.is_empty()).ok_or("missing `path`")?;
    let from = PhotoId(id_of(p.get("from"), "from")?);
    let stack = match p.get("stack") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err("`stack` must be true or false".into()),
    };
    let album = match p.get("album") {
        None | Some(Value::Null) => None,
        a => Some(AlbumId(id_of(a, "album")?)),
    };

    // `from`: a photo of the library with a file
    let ph = s.catalog.photo(from).filter(|ph| ph.in_library()).ok_or_else(|| format!("photo {} is not in the catalog", from.0))?;
    let Source::File { path: original } = &ph.source else {
        return Err(format!("photo {} has no file, so nothing can be made from it", from.0));
    };
    // `path`: a file in the folder of `from`'s original or below it, links resolved
    if !Path::new(path).is_absolute() {
        return Err(format!("{path}: not an absolute path"));
    }
    let target = std::fs::canonicalize(path).map_err(|e| format!("{path} can't be found ({e})"))?;
    if !target.is_file() {
        return Err(format!("{path}: not a file"));
    }
    let folder = Path::new(original)
        .parent()
        .and_then(|d| std::fs::canonicalize(d).ok())
        .ok_or_else(|| format!("the folder of photo {}'s original ({original}) can't be found", from.0))?;
    if !target.starts_with(&folder) {
        return Err(format!("{path} is not in the folder of photo {}'s original ({}) or below it", from.0, folder.display()));
    }
    if std::fs::canonicalize(original).is_ok_and(|o| o == target) {
        return Err(format!("{path} is photo {}'s own original", from.0));
    }
    // imported as the library spells the folder (not through a link), so a file is never in it
    // under two paths, and only what was checked is read
    let within = target.strip_prefix(&folder).map_err(|_| format!("{path} is not in the folder of photo {}'s original", from.0))?;
    let Some(spelled) = Path::new(original).parent().map(|d| d.join(within)) else {
        return Err(format!("the folder of photo {}'s original ({original}) can't be found", from.0));
    };
    let spelled = spelled.to_string_lossy();
    // `album`: a regular album that holds `from`
    if let Some(a) = album {
        let al = s.catalog.album(a).ok_or_else(|| format!("no album {}", a.0))?;
        if al.folder || al.is_smart() {
            return Err(format!("album {:?} can't hold photos (a smart album or a folder)", al.name));
        }
        if !al.photos.contains(&from) {
            return Err(format!("album {:?} doesn't hold photo {}", al.name, from.0));
        }
    }

    let selection = s.selection.clone();
    let mark = s.undo_mark();
    let r = match add_checked(s, &spelled, from, stack, album) {
        Ok(v) => {
            match steps_since(s, mark) {
                0 => {}
                // one step already (an import that stacked the file by its own link): just named
                1 => {
                    if let Some(e) = s.undo.last_mut() {
                        e.label = "Add from Plug-in".into();
                    }
                }
                n => s.merge_undo(n, "Add from Plug-in"),
            }
            Ok(v)
        }
        // what a call that failed half-way did is taken back, so a failure adds nothing
        Err(e) => Err(match take_back(s, steps_since(s, mark)) {
            Ok(()) => e,
            Err(u) => format!("{e} (and what was done so far could not be taken back: {u})"),
        }),
    };
    s.selection = selection;
    r
}

/// How many undo steps were added since [`Session::undo_mark`] was `mark` (as many as the undo
/// history still holds).
fn steps_since(s: &Session, mark: u64) -> usize {
    usize::try_from(s.undo_mark().wrapping_sub(mark)).unwrap_or(usize::MAX).min(s.undo.len())
}

/// Undo the last `steps` undo steps, leaving no redo behind.
pub(crate) fn take_back(s: &mut Session, steps: usize) -> std::result::Result<(), String> {
    for _ in 0..steps {
        s.undo_step().map_err(|e| e.to_string())?;
        s.redo.pop();
    }
    Ok(())
}

fn add_checked(s: &mut Session, path: &str, from: PhotoId, stack: bool, album: Option<AlbumId>) -> std::result::Result<Value, String> {
    // a copy of a file the library has at another path is still the tool's own result
    let report = s.execute("library.import", &json!({"paths": [path], "importDuplicates": true})).map_err(|e| e.to_string())?;
    let first = |k: &str| report[k].get(0).and_then(Value::as_u64);
    // (photo, new to the library, read before now: a duplicate or one back from Recently Deleted)
    let (photo, added, known) = if let Some(id) = first("imported") {
        (PhotoId(id), true, false)
    } else if let Some(id) = first("restored") {
        (PhotoId(id), true, true)
    } else if let Some(id) = report["duplicates"].as_array().into_iter().flatten().find_map(|d| d["existing"].as_u64()) {
        (PhotoId(id), false, true)
    } else {
        let why = report["failed"].get(0).and_then(|f| f.get(1)).and_then(Value::as_str).unwrap_or("the library can't read it");
        return Err(format!("{path} could not be added: {why}"));
    };
    if photo == from {
        return Err(format!("{path} is photo {}'s own original", from.0));
    }
    // read before: read it again when the tool wrote over it
    let reloaded = known && {
        use crate::cmd::reload as r;
        let items = r::plan(s, &json!({"ids": [photo.0]})).map_err(|e| e.to_string())?;
        let results = r::probe(r::file_probe(s).as_ref(), &items, &std::sync::atomic::AtomicBool::new(false), &|_| {});
        r::apply(s, items, results).map_err(|e| e.to_string())?.contains(&photo)
    };
    if stack && s.catalog.stack_of(photo).is_none_or(|st| s.catalog.stack_of(from).is_none_or(|f| f.id != st.id)) {
        super::edit_in::stack_edit(s, photo, from).map_err(|e| e.to_string())?;
    }
    if let Some(a) = album
        && let Some(al) = s.catalog.album(a)
        && !al.photos.contains(&photo)
    {
        let mut photos = al.photos.clone();
        photos.push(photo);
        let cover = al.cover.or(photos.first().copied());
        s.commit("Add to Album", Op::Batch { ops: vec![Op::SetAlbumPhotos { id: a, photos }, Op::SetAlbumCover { id: a, cover }] })
            .map_err(|e| e.to_string())?;
    }
    Ok(json!({"photo": photo.0.to_string(), "added": added, "reloaded": reloaded}))
}
