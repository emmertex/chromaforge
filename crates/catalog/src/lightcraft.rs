//! Reading a LightCraft library: File ▸ More Imports ▸ Import LightCraft Library… (test notes 2026-11-05, 19:33).
//!
//! LightCraft (the fork this app's photos may have been edited in first) keeps its library as a JSON snapshot plus
//! an op log, like this app's v3 catalogs, under its own format name (`lightcraft-catalog`, v10 and older). Its
//! records are this app's with a few differences, which are converted here:
//! - the camera's serial number is `serial` (here `camera_serial`);
//! - `setImportState` (the import state kept apart from the photo) is an op here only as part of the photo;
//! - a mask whose shape this app doesn't know is left out, the rest of the photo's edits are kept; a photo whose
//!   edits can't be read at all comes in unedited, with its rating, flag, label and metadata.
//!
//! The files are only read: nothing in the LightCraft library is changed, locked or upgraded. What comes out is a
//! [`Catalog`] in memory, which Import from Another Catalog then plans and merges like any other ([`crate::transfer`]).

use std::path::Path;
use std::sync::Arc;

use serde::Serialize;
use serde_json::Value;

use crate::journal::{LOG, SNAPSHOT};
use crate::{Catalog, CatalogError, Op, Photo, PhotoId, Result};

/// The format name LightCraft writes in `catalog.snap`.
pub const FORMAT: &str = "lightcraft-catalog";
/// The newest LightCraft catalog format read here.
pub const NEWEST: u32 = 10;
/// Nesting deeper than this in a record is left as it is (a LightCraft record is a few levels deep).
const MAX_DEPTH: usize = 64;
/// Edits kept with a photo (develop settings, History, Versions, Before and the import state); without them it still
/// comes in.
const EDIT_FIELDS: [&str; 7] = ["develop", "history", "history_cursor", "versions", "before", "import_state", "import_look"];

/// What reading a LightCraft library found.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LightcraftReport {
    /// The library's catalog format version (`None`: a log without a snapshot).
    pub version: Option<u32>,
    pub photos: usize,
    /// Log records applied after the snapshot.
    pub replayed: usize,
    /// Log records that couldn't be read or applied (damaged, or an op this app has no counterpart for).
    pub skipped_records: usize,
    /// Masks left out because this app doesn't know their shape.
    pub masks_left_out: usize,
    /// Photos whose edits couldn't be read: they come in unedited.
    pub edits_unreadable: usize,
    /// Photos that couldn't be read at all.
    pub photos_unreadable: usize,
}

/// Whether the library folder `dir` is a LightCraft library. Reads only the start of its snapshot.
pub fn is_lightcraft(dir: &Path) -> bool {
    crate::library::foreign_format(dir).as_deref() == Some(FORMAT)
}

/// Read the LightCraft library in folder `dir` (or its `catalog.snap`) without changing it.
pub fn load(path: &Path) -> Result<(Catalog, LightcraftReport)> {
    let dir = if path.is_file() { path.parent().unwrap_or(path) } else { path };
    let read = |name: &str| match std::fs::read(dir.join(name)) {
        Ok(b) => Ok(Some(b)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(CatalogError::Io(format!("{}: {e}", dir.join(name).display()))),
    };
    let snap = read(SNAPSHOT)?;
    let log = read(LOG)?;
    if snap.is_none() && log.is_none() {
        return Err(CatalogError::Invalid(format!("{} holds no LightCraft library", dir.display())));
    }
    let mut report = LightcraftReport::default();
    let (mut cat, seq) = match snap {
        Some(bytes) => {
            let v: Value = serde_json::from_slice(&bytes).map_err(|e| CatalogError::Corrupt(format!("{SNAPSHOT}: {e}")))?;
            let format = v.get("format").and_then(Value::as_str).unwrap_or("");
            if format != FORMAT {
                return Err(CatalogError::Foreign(format!("{SNAPSHOT}: format {format:?}, not a LightCraft library")));
            }
            let version = v.get("version").and_then(Value::as_u64).and_then(|n| u32::try_from(n).ok()).unwrap_or(0);
            if version > NEWEST {
                return Err(CatalogError::Newer(format!(
                    "this LightCraft library is catalog format v{version}; LightCraft libraries up to v{NEWEST} can be imported"
                )));
            }
            report.version = Some(version);
            let seq = v.get("seq").and_then(Value::as_u64).unwrap_or(0);
            let body = v.get("catalog").cloned().unwrap_or(Value::Null);
            (catalog_of(body, &mut report)?, seq)
        }
        None => (Catalog::new(), 0),
    };
    if let Some(log) = log {
        replay(&mut cat, &log, seq, &mut report);
    }
    cat.repair();
    cat.revision = 0;
    report.photos = cat.photos().count();
    Ok((cat, report))
}

/// The snapshot's catalog, converted: each photo on its own, so one this app can't read costs only itself.
fn catalog_of(mut body: Value, report: &mut LightcraftReport) -> Result<Catalog> {
    let Some(obj) = body.as_object_mut() else {
        return Err(CatalogError::Corrupt(format!("{SNAPSHOT}: no catalog")));
    };
    if let Some(Value::Object(photos)) = obj.get_mut("photos") {
        photos.retain(|_, p| match photo_value(p, report) {
            Some(fixed) => {
                *p = fixed;
                true
            }
            None => {
                report.photos_unreadable += 1;
                false
            }
        });
    }
    match serde_json::from_value::<Catalog>(body.clone()) {
        Ok(c) => Ok(c),
        Err(e) => {
            // the photos alone (an album, stack or list this app reads differently is left behind)
            log::warn!("LightCraft library: {e}; importing its photos only");
            let photos = body.get("photos").cloned().unwrap_or_else(|| Value::Object(Default::default()));
            let next = body.get("next_photo").cloned().unwrap_or(Value::from(0));
            let only = serde_json::json!({"photos": photos, "albums": {}, "next_photo": next, "next_album": 0});
            serde_json::from_value::<Catalog>(only).map_err(|e| CatalogError::Corrupt(format!("{SNAPSHOT}: {e}")))
        }
    }
}

/// One photo record converted, or `None` when even its identity can't be read.
fn photo_value(p: &Value, report: &mut LightcraftReport) -> Option<Value> {
    let mut fixed = p.clone();
    convert(&mut fixed, 0, report);
    if serde_json::from_value::<Photo>(fixed.clone()).is_ok() {
        return Some(fixed);
    }
    // its edits are what this app can't read: it comes in unedited
    let obj = fixed.as_object_mut()?;
    for f in EDIT_FIELDS {
        obj.remove(f);
    }
    obj.insert("develop".into(), serde_json::to_value(lcc_develop::DevelopSettings::default()).ok()?);
    serde_json::from_value::<Photo>(fixed.clone()).ok()?;
    report.edits_unreadable += 1;
    Some(fixed)
}

/// Turn LightCraft's record shapes into this app's, in place: the serial number's name, and masks this app can't
/// read left out.
fn convert(v: &mut Value, depth: usize, report: &mut LightcraftReport) {
    if depth > MAX_DEPTH {
        return;
    }
    match v {
        Value::Object(o) => {
            // a photo's metadata: `serial` beside `camera`
            if o.contains_key("camera")
                && !o.contains_key("camera_serial")
                && let Some(serial) = o.remove("serial")
            {
                o.insert("camera_serial".into(), serial);
            }
            if let Some(Value::Array(masks)) = o.get_mut("masks") {
                let before = masks.len();
                masks.retain(|m| m.get("components").is_none() || serde_json::from_value::<lcc_develop::Mask>(m.clone()).is_ok());
                report.masks_left_out += before - masks.len();
            }
            for x in o.values_mut() {
                convert(x, depth + 1, report);
            }
        }
        Value::Array(a) => {
            for x in a {
                convert(x, depth + 1, report);
            }
        }
        _ => {}
    }
}

/// Apply the log's records after the snapshot (`seq`), as LightCraft would on opening; a damaged record ends the
/// replay there, as it does in LightCraft.
fn replay(cat: &mut Catalog, log: &[u8], seq: u64, report: &mut LightcraftReport) {
    let lines: Vec<&[u8]> = log.split(|b| *b == b'\n').filter(|l| !l.iter().all(u8::is_ascii_whitespace)).collect();
    let last = lines.len().saturating_sub(1);
    for (i, line) in lines.into_iter().enumerate() {
        let Some((s, op)) = std::str::from_utf8(line).ok().and_then(|l| record(l.trim_end_matches('\r'))) else {
            // a torn last record is dropped; damage earlier stops the replay
            report.skipped_records += 1;
            if i < last {
                break;
            }
            continue;
        };
        if s <= seq {
            continue;
        }
        let mut op = op;
        convert(&mut op, 0, report);
        if apply(cat, op, 0, report) {
            report.replayed += 1;
        } else {
            report.skipped_records += 1;
        }
    }
}

/// One log line `{"seq":N,"crc":C,"op":{…}}` whose CRC matches: (seq, op).
fn record(line: &str) -> Option<(u64, Value)> {
    let rest = line.strip_prefix("{\"seq\":")?;
    let (seq, rest) = rest.split_once(",\"crc\":")?;
    let (crc, rest) = rest.split_once(",\"op\":")?;
    let body = rest.strip_suffix('}')?;
    let (seq, crc) = (seq.parse::<u64>().ok()?, crc.parse::<u32>().ok()?);
    if crc32fast::hash(body.as_bytes()) != crc {
        return None;
    }
    Some((seq, serde_json::from_str(body).ok()?))
}

/// Apply one converted LightCraft op. → whether it took.
fn apply(cat: &mut Catalog, op: Value, depth: usize, report: &mut LightcraftReport) -> bool {
    match op.get("op").and_then(Value::as_str) {
        Some("setImportState") => {
            let Some(id) = op.get("id").and_then(Value::as_u64) else { return false };
            let state = match op.get("state") {
                None | Some(Value::Null) => None,
                Some(s) => match serde_json::from_value::<lcc_develop::DevelopSettings>(s.clone()) {
                    Ok(s) => Some(Arc::new(s)),
                    Err(_) => return false,
                },
            };
            cat.photo_mut(PhotoId(id)).map(|p| p.import_state = state).is_ok()
        }
        Some("batch") if depth < MAX_DEPTH => {
            let ops = op.get("ops").and_then(Value::as_array).cloned().unwrap_or_default();
            let mut any = false;
            for o in ops {
                any |= apply(cat, o, depth + 1, report);
            }
            any
        }
        Some("addPhoto") => {
            let Some(p) = op.get("photo") else { return false };
            let Some(photo) = photo_value(p, report).and_then(|v| serde_json::from_value::<Photo>(v).ok()) else { return false };
            cat.apply(Op::AddPhoto { photo: Box::new(photo) }).is_ok()
        }
        _ => serde_json::from_value::<Op>(op).ok().is_some_and(|op| cat.apply(op).is_ok()),
    }
}
