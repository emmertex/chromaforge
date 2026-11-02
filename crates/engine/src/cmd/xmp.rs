//! XMP sidecar commands: save/read metadata to/from files, sidecar preferences.

use lcc_catalog::Op;
use serde_json::{Value, json};

use super::{CommandSpec, always, bad, cmd, has_selection, str_param};
use crate::sidecar::SidecarNaming;
use crate::{Result, Session};

fn save(s: &mut Session, p: &Value) -> Result<Value> {
    // an automatic write still queued would land after this one
    s.settle_sidecar_writes()?;
    let mut written = Vec::new();
    let mut merged = Vec::new();
    let mut backups = Vec::new();
    let mut failed = Vec::new();
    let ids = s.targets(p)?;
    let owners = crate::sidecar::StemOwners::among(&s.catalog, &ids);
    let mut stamped = Vec::new();
    for id in ids {
        match s.save_sidecar_with(id, &owners) {
            Ok(r) => {
                stamped.push(id);
                let path = r.path.display().to_string();
                if r.merged {
                    merged.push(path.clone());
                }
                if let Some(b) = r.backup {
                    backups.push(json!({"path": path, "backup": b.display().to_string()}));
                }
                written.push(path);
            }
            Err(e) => failed.push(json!({"id": id.0, "error": e.to_string()})),
        }
    }
    s.record_xmp_stamps(&stamped, false);
    Ok(json!({"written": written, "merged": merged, "backups": backups, "failed": failed}))
}

fn read(s: &mut Session, p: &Value) -> Result<Value> {
    // read what the automatic writes queued so far wrote
    s.settle_sidecar_writes()?;
    let mut ops = Vec::new();
    let mut read = Vec::new();
    let mut failed = Vec::new();
    let mut stamped = Vec::new();
    let mut warnings = Vec::new();
    let mut fields = Vec::new();
    let ids = s.targets(p)?;
    let namings = s.sidecar_namings(&ids);
    for (id, naming) in ids.into_iter().zip(namings) {
        match s.read_sidecar_op_named(id, naming) {
            Ok(Some((op, from, mut photo_warnings, plugin_fields))) => {
                ops.push(op);
                fields.push((id, plugin_fields));
                stamped.push(id);
                read.push(json!({"id": id.0, "from": from.display().to_string()}));
                warnings.append(&mut photo_warnings);
            }
            Ok(None) => failed.push(json!({"id": id.0, "error": "no XMP sidecar or embedded XMP"})),
            Err(e) => failed.push(json!({"id": id.0, "error": e.to_string()})),
        }
    }
    if !ops.is_empty() {
        s.commit("Read Metadata from File", Op::Batch { ops })?;
        // what was just read is what the catalog holds: in step with the file
        s.record_xmp_stamps(&stamped, false);
        // Don't auto-write the sidecars just read: that would add our `lc:settings` to one that had
        // only another app's `crs:` settings, and later reads prefer `lc:settings`, so that app's
        // next edits would be ignored (issue #934). The next edit, or Save Metadata, writes them.
        s.skip_auto_write = true;
    }
    let restored = crate::cmd::plugins::restore_fields(s, &fields);
    let mut result = json!({"read": read, "failed": failed});
    if restored > 0 {
        result["pluginFields"] = json!(restored);
    }
    if !warnings.is_empty() {
        result["warnings"] = serde_json::to_value(warnings).unwrap_or_default();
    }
    Ok(result)
}

/// `photo.autoReadMetadata`: read the sidecars that changed on disk (another app edited them)
/// while the photo is unchanged in the catalog since the last sync, through Read Metadata from
/// File (one undo step, journaled). Photos changed on both sides keep their conflict.
fn auto_read(s: &mut Session, p: &Value) -> Result<Value> {
    use lcc_catalog::XmpStatus;
    let ids = s.targets(p)?;
    let mut changed = Vec::new();
    let mut conflicts = Vec::new();
    let mut incomplete = Vec::new();
    for id in ids {
        match s.xmp_status(id) {
            XmpStatus::ChangedOnDisk if !s.sidecar_complete(id) => {
                incomplete.push(json!({"id": id.0, "error": "the sidecar is empty or incomplete (still being written?)"}))
            }
            XmpStatus::ChangedOnDisk => changed.push(id.0),
            XmpStatus::Conflict => conflicts.push(id.0),
            _ => {}
        }
    }
    let mut out =
        if changed.is_empty() { json!({"read": [], "failed": []}) } else { s.execute("photo.readMetadataFromFile", &json!({"ids": changed}))? };
    if let Some(f) = out.get_mut("failed").and_then(Value::as_array_mut) {
        f.extend(incomplete);
    }
    out["conflicts"] = json!(conflicts);
    Ok(out)
}

fn status(s: &mut Session, p: &Value) -> Result<Value> {
    let mut counts = serde_json::Map::new();
    let photos: Vec<Value> = s
        .targets(p)?
        .into_iter()
        .map(|id| {
            let st = serde_json::to_value(s.xmp_status(id)).unwrap_or_default();
            let key = st.as_str().unwrap_or("unknown").to_string();
            let n = counts.get(&key).and_then(Value::as_u64).unwrap_or(0);
            counts.insert(key, json!(n + 1));
            json!({"id": id.0, "status": st})
        })
        .collect();
    Ok(json!({"photos": photos, "counts": counts}))
}

fn prefs(s: &mut Session, p: &Value) -> Result<Value> {
    // (a change of naming applies to writes made from now on)
    s.settle_sidecar_writes()?;
    if let Some(a) = p.get("autoWrite").and_then(Value::as_bool) {
        s.xmp.auto_write = a;
    }
    if let Some(a) = p.get("autoRead").and_then(Value::as_bool) {
        s.xmp.auto_read = a;
    }
    if let Some(a) = p.get("writeBrowsed").and_then(Value::as_bool) {
        s.xmp.write_browsed = a;
    }
    if let Some(n) = str_param(p, "naming") {
        s.xmp.naming = match n {
            "stem" => SidecarNaming::Stem,
            "full" => SidecarNaming::Full,
            other => return Err(bad("library.xmpPreferences", format!("unknown naming `{other}` (stem|full)"))),
        };
    }
    if p.as_object().is_some_and(|o| !o.is_empty()) {
        s.save_prefs()?;
    }
    Ok(serde_json::to_value(s.xmp).unwrap_or_default())
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "photo.saveMetadataToFile",
            "Save Metadata to File",
            ["Metadata"],
            Some("Cmd+S"),
            "{ids?} — writes each photo's XMP sidecar (metadata + develop settings) next to the original, merged into an existing one (other apps' data kept; an unreadable one is backed up first) → {written: [path], merged: [path], backups: [{path, backup}], failed}",
            has_selection,
            save
        ),
        cmd!(
            "photo.readMetadataFromFile",
            "Read Metadata from File",
            ["Metadata"],
            None,
            "{ids?} — reads each photo's XMP sidecar, or embedded XMP in raw/DNG and JPEG files when no sidecar exists: metadata and develop settings, one undo step; never writes the sidecar, even with auto-write on; JPEG uses the first complete standard XMP APP1 before SOS/EOI, within 16 MiB of headers and 4096 marker/fill steps; an unreadable sidecar fails without embedded fallback → {read, failed, warnings?: [{path, code, profile?, mode?, fields?}]}",
            has_selection,
            read
        ),
        cmd!(
            "photo.autoReadMetadata",
            "Read Changed Metadata Files",
            [],
            None,
            "{ids?} — reads the XMP sidecars that changed on disk (another app edited them) of photos unchanged in the catalog since the last sync, through Read Metadata from File (one undo step); photos changed on both sides are left as conflicts. The app runs it when a sidecar check finds such a change and library.xmpPreferences autoRead is on → {read, failed, conflicts: [id], warnings?}",
            always,
            auto_read
        ),
        cmd!(
            query "photo.xmpStatus",
            "Metadata File Status",
            [],
            None,
            "{ids?} — per photo: unknown (never read from / written to its XMP) | inSync | changedInCatalog | changedOnDisk | conflict → {photos: [{id, status}], counts}",
            always,
            status
        ),
        cmd!(
            "library.xmpPreferences",
            "XMP Sidecar Preferences",
            [],
            None,
            "{autoWrite?: bool, autoRead?: bool (default on: read sidecars other apps changed when the photo is unchanged in the catalog), naming?: stem (IMG_1.xmp, default) | full (IMG_1.CR3.xmp), writeBrowsed?: bool (auto-write also for photos only browsed in a folder, not added yet; default off: nothing is written into the folder until they are added)} → current preferences (saved with the library)",
            always,
            prefs
        ),
        cmd!(
            "library.retrySidecars",
            "Write Waiting XMP Sidecars",
            [],
            None,
            "{} — writes again the automatic XMP sidecars that failed (folder offline, read-only or full) whose originals are reachable now → {written, waiting}",
            always,
            |s, _| {
                s.flush_sidecar_writes();
                let written = s.retry_unsaved_sidecars();
                Ok(json!({"written": written, "waiting": s.unsaved_sidecars()}))
            }
        ),
        cmd!("library.toggleAutoWriteXmp", "Automatically Write Changes into XMP", ["Metadata"], None, "{} → {autoWrite}", always, |s, _| {
            let on = !s.xmp.auto_write;
            prefs(s, &json!({"autoWrite": on}))
        }),
    ]
}
