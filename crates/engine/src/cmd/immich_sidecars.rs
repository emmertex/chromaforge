//! IMM-SIDECARS: pull the XMP sidecars Immich stores for its assets, and push the catalog's
//! metadata back (plan/immich.md → Sidecars; docs/immich.md).
//!
//! Immich keeps an `.xmp` sidecar beside every uploaded asset and, for an external library, beside
//! the original on disk. `GET /asset-files` (v3.2.0) lists those files and
//! `GET /asset-files/{id}/download` serves one, so a second library can read back the XMP an
//! instance sent with its upload — including its `lc:settings` develop settings.
//!
//! - **Pull** (`immich.pullSidecars`): for every linked photo (or the selection / `ids`), download
//!   the asset's sidecar, apply it to the catalog as one undo step and write it to the photo's own
//!   sidecar. A photo whose catalog changed since its sidecar was in step, or which was edited
//!   after the asset's `updatedAt`, is left alone (and reported) unless `overwrite` is set.
//! - **Push** (`immich.pushSidecars`): photos in a mapped external-library folder get their whole
//!   XMP sidecar written beside the original (Immich then re-reads it); Immich-managed assets get
//!   the fields Immich keeps in a sidecar (rating, description, capture time, location, tags),
//!   which Immich merges into the sidecar it holds. The API has no way to replace a managed
//!   asset's sidecar, so its develop settings only travel on upload — [`lcc_immich::publish`]
//!   sends the whole XMP with an original. `externalOnly` keeps the old `immich.writeSidecars`
//!   behaviour.
//!
//! The network work runs on a worker (`background`) and its result comes back through
//! `remote.pump`, like the other Immich jobs; without `background` the same work runs inline.

use std::collections::BTreeSet;

use lcc_catalog::{Op, PhotoId, Source, SyncState as LinkState};
use lcc_immich::client::{NewAsset, UploadData};
use lcc_immich::link::SERVICE;
use lcc_immich::sync as merge;
use lcc_immich::types::AssetUpdate;
use lcc_immich::{ImmichError, extlib, sidecars};
use serde_json::{Value, json};

use super::{account_param, failure};
use crate::cmd::{CommandSpec, always, bad, bool_or, cmd};
use crate::remote::{FullSidecar, Msg, SidecarDirection, SidecarItem, SidecarPull, SidecarPush, SidecarsDone};
use crate::sidecar::StemOwners;
use crate::{Result, Session};

fn excluded(c: &lcc_catalog::Catalog) -> impl Fn(&str) -> bool + '_ {
    |k: &str| c.keyword_info(k.trim()).is_some_and(|i| !i.include_on_export)
}

/// A failed call that means the rest of the run can't work either (offline, bad key, TLS).
fn fatal(e: &ImmichError) -> bool {
    e.retryable() || matches!(e, ImmichError::BadKey | ImmichError::NoKey | ImmichError::Tls(_) | ImmichError::Untrusted { .. })
}

/// The confirmed links of `account` (probable links never take part).
fn linked(s: &Session, account: &str) -> BTreeSet<PhotoId> {
    s.catalog
        .remote_links()
        .iter()
        .filter(|r| r.service == SERVICE && r.account_id == account && r.sync_state != LinkState::Probable)
        .map(|r| r.photo_id)
        .collect()
}

/// The photos a sidecar run covers: `ids` / `id` when given, else `all`, else the selection, else
/// every photo linked to the account. Only linked photos are ever touched.
fn targets(s: &Session, p: &Value, account: &str) -> Result<Vec<PhotoId>> {
    let linked = linked(s, account);
    let explicit = matches!(p.get("ids"), Some(Value::Array(_))) || matches!(p.get("id"), Some(Value::Number(_)));
    if explicit {
        let ids = s.targets(p)?;
        return Ok(ids.into_iter().filter(|id| linked.contains(id)).collect());
    }
    if bool_or(p, "all", false) {
        return Ok(linked.iter().copied().collect());
    }
    let selection = s.selected();
    if selection.is_empty() { Ok(linked.iter().copied().collect()) } else { Ok(selection.into_iter().filter(|id| linked.contains(id)).collect()) }
}

/// The asset id `photo` is linked to under `account`.
fn asset_of(s: &Session, photo: PhotoId, account: &str) -> Option<String> {
    s.catalog.remote_of(photo).find(|r| r.service == SERVICE && r.account_id == account).map(|r| r.remote_id.clone())
}

/// The version of `account`'s server (stored at the last check, else asked once).
fn server_version(s: &mut Session, account: &str, cmd: &str) -> Result<std::result::Result<lcc_immich::ServerVersion, Value>> {
    let Some(acc) = s.immich_accounts().map_err(|e| bad(cmd, e))?.get(account).cloned() else {
        return Err(bad(cmd, "no such account"));
    };
    if let Some(v) = acc.version {
        return Ok(Ok(v));
    }
    match s.immich_client(account) {
        Ok((_, c)) => Ok(c.version().map_err(|e| failure(&e))),
        Err(e) => Ok(Err(failure(&e))),
    }
}

// ---------------------------------------------------------------------- pull

fn pull(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "immich.pullSidecars";
    if s.remote.sidecars.active {
        return Ok(json!({"started": false, "reason": "a sidecar run is already running"}));
    }
    let account = account_param(s, p, C)?;
    let version = match server_version(s, &account, C)? {
        Ok(v) => v,
        Err(failed) => return Ok(failed),
    };
    if !sidecars::supports_asset_files(version) {
        return Ok(json!({
            "ok": false,
            "error": {
                "kind": "tooOld",
                "message": format!(
                    "pulling XMP sidecars needs Immich {} or later (this server runs {version}); pushing still works",
                    lcc_immich::MIN_ASSET_FILES_VERSION
                ),
                "retryable": false,
            }
        }));
    }
    let ids = targets(s, p, &account)?;
    let total = ids.len();
    let truncated = total > sidecars::MAX_PULL_ITEMS;
    let mut items = Vec::new();
    for photo in ids {
        if items.len() >= sidecars::MAX_PULL_ITEMS {
            break;
        }
        let Some(asset) = asset_of(s, photo, &account) else { continue };
        let Some(ph) = s.catalog.photo(photo) else { continue };
        items.push(SidecarPull { photo, asset, file_name: ph.file_name.clone() });
    }
    if items.is_empty() {
        return Ok(json!({
            "pulled": 0, "applied": 0, "written": 0, "skipped": [], "noSidecar": [], "failed": [], "dryRun": false
        }));
    }
    let (_, client) = match s.immich_client(&account) {
        Ok(x) => x,
        Err(e) => return Ok(failure(&e)),
    };
    let done = SidecarsDone {
        direction: SidecarDirection::Pull,
        items: Vec::new(),
        errors: Vec::new(),
        fatal: None,
        dry_run: bool_or(p, "dryRun", false),
        write_files: bool_or(p, "writeFiles", true),
        refresh: Vec::new(),
        overwrite: bool_or(p, "overwrite", false),
        truncated,
        local_written: Vec::new(),
        local_failed: Vec::new(),
        local_skipped: Vec::new(),
    };
    start(s, account, done, Work::Pull(items), client, bool_or(p, "background", false))
}

/// One asset's sidecar: its bytes, or why not. A fatal error is marked for the caller to stop.
fn pull_item(c: &lcc_immich::Client, work: SidecarPull, total: &mut u64) -> SidecarItem {
    let mut item = SidecarItem { photo: work.photo, asset: work.asset.clone(), file_name: work.file_name, ..SidecarItem::default() };
    let a = match c.asset(&work.asset) {
        Ok(a) => a,
        Err(ImmichError::NotFound(_)) => {
            item.skipped = Some("missing");
            return item;
        }
        Err(e) => {
            item.error = Some(e.to_string());
            item.fatal = fatal(&e).then(|| e.clone());
            return item;
        }
    };
    item.updated_at = a.updated_at.clone();
    if merge::untouchable(&a) {
        item.skipped = Some("locked");
        return item;
    }
    if a.is_trashed {
        item.skipped = Some("trashed");
        return item;
    }
    let file = match c.asset_sidecar(&work.asset) {
        Ok(Some(f)) => f,
        Ok(None) => {
            item.skipped = Some("noSidecar");
            return item;
        }
        Err(ImmichError::Forbidden(_)) => {
            item.error = Some("the API key lacks `assetFile.read`; add it in Immich".into());
            return item;
        }
        Err(e) => {
            item.fatal = fatal(&e).then(|| e.clone());
            item.error = Some(e.to_string());
            return item;
        }
    };
    let bytes = match c.download_asset_file(&file.id) {
        Ok(b) => b,
        Err(e) => {
            item.fatal = fatal(&e).then(|| e.clone());
            item.error = Some(e.to_string());
            return item;
        }
    };
    *total = total.saturating_add(bytes.len() as u64);
    if *total > sidecars::MAX_PULL_BYTES {
        item.error = Some("this run has read as many sidecar bytes as it keeps in memory; pull in smaller batches".into());
        return item;
    }
    match String::from_utf8(bytes) {
        Ok(xml) => item.xmp = Some(xml),
        Err(_) => item.error = Some("the sidecar is not UTF-8 text".into()),
    }
    item
}

// ---------------------------------------------------------------------- push

/// The photos a push covers: explicit `ids` / `id` or the selection when given, else every photo
/// that is linked to the account **or** sits in a mapped external-library folder. A photo in a
/// mapped folder is written even when the link pass has not matched it yet: the sidecar is a local
/// file and writing it is what lets Immich see the metadata.
fn push_targets(s: &Session, p: &Value, account: &str, maps: &[extlib::PathMap]) -> Result<Vec<PhotoId>> {
    let linked = linked(s, account);
    let mapped: BTreeSet<PhotoId> = s
        .catalog
        .photos()
        .filter(|ph| ph.in_library() && ph.copy_of.is_none())
        .filter(|ph| matches!(&ph.source, Source::File { path } if extlib::to_container(maps, path).is_some()))
        .map(|ph| ph.id)
        .collect();
    let all: BTreeSet<PhotoId> = linked.union(&mapped).copied().collect();
    let explicit = matches!(p.get("ids"), Some(Value::Array(_))) || matches!(p.get("id"), Some(Value::Number(_)));
    let selection = if explicit {
        Some(s.targets(p)?)
    } else if bool_or(p, "all", false) {
        None
    } else {
        let sel = s.selected();
        (!sel.is_empty()).then_some(sel)
    };
    Ok(match selection {
        Some(ids) => ids.into_iter().filter(|id| all.contains(id)).collect(),
        None => all.into_iter().collect(),
    })
}

fn push(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "immich.pushSidecars";
    if s.remote.sidecars.active {
        return Ok(json!({"started": false, "reason": "a sidecar run is already running"}));
    }
    let account = account_param(s, p, C)?;
    let Some(acc) = s.immich_accounts().map_err(|e| bad(C, e))?.get(&account).cloned() else {
        return Err(bad(C, "no such account"));
    };
    let maps = acc.path_maps.clone();
    let ids = push_targets(s, p, &account, &maps)?;
    let dry_run = bool_or(p, "dryRun", false);
    let external_only = bool_or(p, "externalOnly", false);
    let full_mode = bool_or(p, "full", false);
    let refresh = bool_or(p, "refresh", true);
    let cfg = acc.sync.clone();
    let mut made = 0u64;
    let owners = StemOwners::among(&s.catalog, &ids);
    let mut work = Vec::new();
    let mut external = Vec::new();
    let mut written: Vec<String> = Vec::new();
    let mut failed: Vec<Value> = Vec::new();
    let mut skipped: Vec<Value> = Vec::new();
    for photo in ids {
        let Some(ph) = s.catalog.photo(photo) else { continue };
        let file_name = ph.file_name.clone();
        let deleted = ph.deleted;
        let copy = ph.copy_of.is_some();
        let source = ph.source.clone();
        if deleted || copy {
            skipped.push(json!({"id": photo.0, "file": file_name, "reason": "notALibraryPhoto"}));
            continue;
        }
        let Some(path) = (match &source {
            Source::File { path } => Some(path.clone()),
            Source::Demo { .. } => None,
        }) else {
            skipped.push(json!({"id": photo.0, "file": file_name, "reason": "notAFile"}));
            continue;
        };
        let asset = asset_of(s, photo, &account);
        if extlib::to_container(&maps, &path).is_some() {
            // the sidecar is the file on disk: Immich reads it on its next scan/refresh (a photo
            // not linked yet gets it too, so a later scan can match it)
            if dry_run {
                written.push(file_name);
                external.extend(asset);
                continue;
            }
            match s.save_sidecar_with(photo, &owners) {
                Ok(r) => {
                    written.push(r.path.display().to_string());
                    external.extend(asset);
                }
                Err(e) => failed.push(json!({"id": photo.0, "file": file_name, "error": e.to_string()})),
            }
        } else if external_only {
            skipped.push(json!({"id": photo.0, "file": file_name, "reason": "notInExternalLibrary"}));
        } else {
            let Some(asset) = asset else {
                skipped.push(json!({"id": photo.0, "file": file_name, "reason": "notLinked"}));
                continue;
            };
            // a managed asset: the fields Immich keeps in its sidecar, merged into the one it holds
            let Some(ph) = s.catalog.photo(photo) else { continue };
            let snap = merge::local(ph, &cfg, &excluded(&s.catalog));
            let fields: BTreeSet<merge::Field> = [
                merge::Field::Rating,
                merge::Field::Favorite,
                merge::Field::Description,
                merge::Field::Keywords,
                merge::Field::Location,
                merge::Field::Captured,
            ]
            .into_iter()
            .collect();
            let update: AssetUpdate = merge::remote_update(&snap, &fields, &cfg, None);
            let tags: Vec<String> = snap.keywords.iter().map(|k| merge::keyword_to_tag(k)).filter(|t| !t.is_empty()).collect();
            // full: the whole XMP, merged into the sidecar Immich holds, through a temporary asset
            let full = if full_mode {
                made = made.wrapping_add(1);
                let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0)
                    ^ photo.0.rotate_left(17)
                    ^ made.wrapping_mul(0x9E37_79B9_7F4A_7C15);
                match placeholder_png(seed) {
                    Some(placeholder) => Some(FullSidecar {
                        packet: crate::sidecar::Packets::of(ph, &s.catalog).with_gps.into_bytes(),
                        placeholder,
                        file_name: format!("cf-sidecar-{}-{made}.png", photo.0),
                        device_asset_id: format!("cf-sidecar-{}-{seed:016x}", photo.0),
                        created: created_iso(ph),
                    }),
                    None => {
                        failed.push(json!({"id": photo.0, "file": file_name, "error": "could not build the placeholder image"}));
                        continue;
                    }
                }
            } else {
                None
            };
            work.push(SidecarPush { photo, asset, update, tags, captured: snap.captured.clone(), full });
        }
    }
    let done = SidecarsDone {
        direction: SidecarDirection::Push,
        items: Vec::new(),
        errors: Vec::new(),
        fatal: None,
        dry_run,
        write_files: false,
        refresh: if refresh { external } else { Vec::new() },
        overwrite: true,
        truncated: false,
        local_written: written,
        local_failed: failed,
        local_skipped: skipped,
    };
    let (_, client) = match s.immich_client(&account) {
        Ok(x) => x,
        Err(e) => return Ok(failure(&e)),
    };
    start(s, account, done, Work::Push(work), client, bool_or(p, "background", false))
}

// ---------------------------------------------------------------------- running

/// The server work a run does (the pull's downloads, the push's metadata updates and refresh).
enum Work {
    Pull(Vec<SidecarPull>),
    Push(Vec<SidecarPush>),
}

/// Start a run: inline (`finish` here) or on a worker (through `remote.pump`).
fn start(s: &mut Session, account: String, done: SidecarsDone, work: Work, client: lcc_immich::Client, background: bool) -> Result<Value> {
    let label = match done.direction {
        SidecarDirection::Pull => "Pulling XMP sidecars from Immich",
        SidecarDirection::Push => "Pushing XMP sidecars to Immich",
    };
    s.remote.sidecars.active = true;
    s.remote.sidecars.direction = done.direction.name();
    let task = s.activity.start("immich", label, crate::activity::Cancel::No);
    let run = move || {
        let d = match work {
            Work::Pull(items) => pull_work(&client, items, done),
            Work::Push(items) => push_work(&client, items, done),
        };
        drop(task);
        d
    };
    if background {
        let tx = s.remote.sender();
        std::thread::spawn(move || {
            let _ = tx.send(Msg::Sidecars(Box::new(run())));
        });
        Ok(json!({"started": true, "account": account}))
    } else {
        let d = run();
        Ok(finish(s, d))
    }
}

/// The pull's worker: download the sidecars (the catalog is touched in [`finish`]).
fn pull_work(c: &lcc_immich::Client, work: Vec<SidecarPull>, mut done: SidecarsDone) -> SidecarsDone {
    let mut total = 0u64;
    for it in work {
        let item = pull_item(c, it, &mut total);
        let stop = item.fatal.is_some();
        if let Some(e) = &item.fatal {
            done.fatal = Some(e.clone());
        }
        done.items.push(item);
        if stop || total > sidecars::MAX_PULL_BYTES {
            break;
        }
    }
    done
}

/// The push's worker: send the metadata updates and tags, then ask Immich to re-read the
/// external-library sidecars just written by the session.
fn push_work(c: &lcc_immich::Client, work: Vec<SidecarPush>, mut done: SidecarsDone) -> SidecarsDone {
    for it in work {
        let mut item = SidecarItem { photo: it.photo, asset: it.asset.clone(), ..SidecarItem::default() };
        let a = match c.asset(&it.asset) {
            Ok(a) => a,
            Err(ImmichError::NotFound(_)) => {
                item.skipped = Some("missing");
                done.items.push(item);
                continue;
            }
            Err(e) if fatal(&e) => {
                done.fatal = Some(e);
                break;
            }
            Err(e) => {
                item.error = Some(e.to_string());
                done.items.push(item);
                continue;
            }
        };
        item.file_name = a.original_file_name.clone();
        if merge::untouchable(&a) {
            item.skipped = Some("locked");
            done.items.push(item);
            continue;
        }
        if a.is_trashed {
            item.skipped = Some("trashed");
            done.items.push(item);
            continue;
        }
        if let Some(full) = it.full.clone() {
            // the whole XMP: Immich's sidecar as the base, ours merged in, copied over
            if done.dry_run {
                item.pushed = true;
            } else {
                match replace_sidecar(c, &it.asset, &full, &mut done.errors) {
                    Ok(()) => item.pushed = true,
                    Err(e) if fatal(&e) => {
                        done.fatal = Some(e);
                        break;
                    }
                    Err(e) => item.error = Some(e.to_string()),
                }
            }
            done.items.push(item);
            continue;
        }
        let mut update = it.update;
        if let Some(cap) = &it.captured
            && let Some(zone) = merge::zone_of(a.local_date_time.as_deref().unwrap_or_default())
        {
            update.date_time_original = Some(format!("{cap}{zone}"));
        }
        let mut pushed = false;
        if !update.is_empty() {
            pushed = true;
            if !done.dry_run
                && let Err(e) = c.update_asset(&it.asset, &update)
            {
                if fatal(&e) {
                    done.fatal = Some(e);
                    break;
                }
                item.error = Some(e.to_string());
                done.items.push(item);
                continue;
            }
        }
        // tags are only added: a tag given in Immich is not a change the catalog should undo
        let current: BTreeSet<&str> = a.tags.iter().map(|t| t.value.as_str()).collect();
        let add: Vec<String> = it.tags.iter().filter(|t| !current.contains(t.as_str())).cloned().collect();
        if !add.is_empty() {
            pushed = true;
            if !done.dry_run {
                match c.upsert_tags(&add) {
                    Ok(infos) => {
                        for info in infos {
                            if let Err(e) = c.tag_assets(&info.id, std::slice::from_ref(&it.asset)) {
                                if fatal(&e) {
                                    done.fatal = Some(e);
                                } else {
                                    done.errors.push(format!("{}: {e}", a.original_file_name));
                                }
                                break;
                            }
                        }
                    }
                    Err(e) => done.errors.push(format!("{}: tags: {e}", a.original_file_name)),
                }
            }
        }
        item.pushed = pushed;
        done.items.push(item);
    }
    if !done.dry_run && !done.refresh.is_empty() {
        if let Err(e) = c.discover_sidecars() {
            done.errors.push(format!("sidecar discovery: {e}"));
        }
        if let Err(e) = c.refresh_metadata(&done.refresh) {
            done.errors.push(format!("refresh-metadata: {e}"));
        }
    }
    done
}

/// A tiny unique PNG for the temporary asset a full sidecar push creates. It never holds a user
/// photo; it is deleted as soon as its sidecar has been copied onto the real asset.
fn placeholder_png(seed: u64) -> Option<Vec<u8>> {
    let (w, h) = (8usize, 8usize);
    let data: Vec<[u8; 4]> = (0..w * h)
        .map(|i| {
            let v = seed.wrapping_mul(6364136223846793005).wrapping_add((i as u64).wrapping_mul(1442695040888963407));
            [(v >> 24) as u8, (v >> 40) as u8, (v >> 56) as u8, 255]
        })
        .collect();
    let img = lcc_raster::Rgba8 { width: w, height: h, data };
    lcc_codecs::encode_png(&lcc_codecs::EncodeImage::rgba8(&img), &lcc_codecs::EncodeMeta::default()).ok()
}

/// The photo's capture time as `fileCreatedAt` (`YYYY-MM-DDTHH:MM:SS.000Z` when it has no zone).
fn created_iso(p: &lcc_catalog::Photo) -> String {
    let c = p.captured.clone().unwrap_or_else(|| p.imported.clone());
    if c.len() == 19 { format!("{c}.000Z") } else { c }
}

/// Replace a managed asset's whole sidecar: read the one Immich holds, merge the app's packet into
/// it (Immich's/other writers' properties are kept), upload the merged XMP on a temporary
/// placeholder, copy just the sidecar onto the target, then delete the placeholder. Immich's API
/// has no endpoint that writes an existing asset's sidecar; this uses its `copyAsset` for that.
fn replace_sidecar(c: &lcc_immich::Client, target: &str, full: &FullSidecar, errors: &mut Vec<String>) -> std::result::Result<(), ImmichError> {
    let existing = match c.asset_sidecar(target)? {
        Some(f) => {
            let bytes = c.download_asset_file(&f.id)?;
            String::from_utf8(bytes).map_err(|_| ImmichError::Protocol("Immich's sidecar is not UTF-8 text".into()))?
        }
        None => String::new(),
    };
    let fresh = String::from_utf8(full.packet.clone()).map_err(|_| ImmichError::Protocol("the app's packet is not UTF-8 text".into()))?;
    let merged = crate::sidecar::merge_packet(&existing, &fresh).map_err(ImmichError::Protocol)?;
    let (temp, duplicate) = c.upload(&NewAsset {
        data: UploadData::Bytes(&full.placeholder),
        file_name: &full.file_name,
        mime: "image/png",
        device_asset_id: &full.device_asset_id,
        device_id: "sidecar-push",
        created: &full.created,
        favorite: false,
        sidecar: Some(merged.as_bytes()),
    })?;
    if duplicate {
        return Err(ImmichError::Protocol("the placeholder asset already existed; no sidecar was uploaded".into()));
    }
    let copied = c.copy_sidecar(&temp, target);
    // the placeholder is ours: remove it at once, whether the copy worked or not
    if let Err(e) = c.delete_assets(std::slice::from_ref(&temp), true) {
        errors.push(format!("temporary asset {temp}: {e}"));
    }
    copied?;
    // Immich may already have followed the copied sidecar; ask it to re-read it anyway
    if let Err(e) = c.refresh_metadata(std::slice::from_ref(&target.to_string())) {
        errors.push(format!("refresh-metadata: {e}"));
    }
    Ok(())
}

// ---------------------------------------------------------------------- taking the result in

/// Take a finished run in on the session thread: the pull's catalog changes and local sidecars,
/// or the push's summary. Also `remote.pump`'s part.
pub(crate) fn finish(s: &mut Session, d: SidecarsDone) -> Value {
    s.remote.sidecars.active = false;
    s.remote.sidecars.direction = d.direction.name();
    let direction = d.direction.name();
    let now = (s.clock)();
    let mut out = match d.direction {
        SidecarDirection::Pull => finish_pull(s, d),
        SidecarDirection::Push => summarize_push(d),
    };
    out["direction"] = json!(direction);
    s.remote.sidecars.last = Some(summary(&out, &now));
    out
}

fn finish_pull(s: &mut Session, d: SidecarsDone) -> Value {
    let mut pulled = 0u64;
    let mut skipped: Vec<Value> = Vec::new();
    let mut no_sidecar: Vec<u64> = Vec::new();
    let mut failed: Vec<Value> = Vec::new();
    let mut report: Vec<Value> = Vec::new();
    let mut planned: Vec<Op> = Vec::new();
    let mut applied: BTreeSet<PhotoId> = BTreeSet::new();
    for item in d.items {
        if let Some(why) = item.skipped {
            if why == "noSidecar" {
                no_sidecar.push(item.photo.0);
            } else {
                skipped.push(json!({"id": item.photo.0, "file": item.file_name, "reason": why}));
            }
            continue;
        }
        if let Some(e) = item.error {
            failed.push(json!({"id": item.photo.0, "file": item.file_name, "error": e}));
            continue;
        }
        let Some(xml) = item.xmp else { continue };
        pulled += 1;
        let Some(p) = s.catalog.photo(item.photo) else { continue };
        if !d.overwrite
            && let Some(why) = sidecars::pull_skip_reason(s.xmp_status(item.photo), p.edited.as_deref(), item.updated_at.as_deref())
        {
            skipped.push(json!({"id": item.photo.0, "file": item.file_name, "reason": why}));
            continue;
        }
        let parsed = match crate::sidecar::parse_sidecar(&xml, crate::crs::Target::for_photo(p)) {
            Ok(sc) => sc,
            Err(e) => {
                failed.push(json!({"id": item.photo.0, "file": item.file_name, "error": format!("not XMP: {e}")}));
                continue;
            }
        };
        let detail = json!({
            "id": item.photo.0,
            "file": item.file_name,
            "assetId": item.asset,
            "rating": parsed.rating,
            "caption": parsed.caption,
            "keywords": parsed.keywords.as_ref().map(Vec::len).unwrap_or(0),
            "develop": parsed.develop.is_some(),
            "gps": parsed.gps.is_some(),
            "captured": parsed.captured,
        });
        match s.sidecar_op(item.photo, parsed) {
            Ok(Op::Batch { ops }) => {
                planned.extend(ops);
                applied.insert(item.photo);
                report.push(detail);
            }
            Ok(op) => {
                planned.push(op);
                applied.insert(item.photo);
                report.push(detail);
            }
            Err(e) => failed.push(json!({"id": item.photo.0, "file": item.file_name, "error": e.to_string()})),
        }
    }
    let mut committed = d.dry_run;
    if !d.dry_run && !planned.is_empty() {
        // one undo step for the whole run, like Read Metadata from File
        match s.commit("Read XMP Sidecars from Immich", Op::Batch { ops: planned }) {
            Ok(()) => committed = true,
            Err(e) => failed.push(json!({"error": e.to_string()})),
        }
    }
    let mut written = 0u64;
    let mut write_failed: Vec<Value> = Vec::new();
    if committed && !d.dry_run && d.write_files && !applied.is_empty() {
        let ids: Vec<PhotoId> = applied.iter().copied().collect();
        let owners = StemOwners::among(&s.catalog, &ids);
        let mut stamped = Vec::new();
        for id in ids {
            match s.save_sidecar_with(id, &owners) {
                Ok(_) => {
                    written += 1;
                    stamped.push(id);
                }
                Err(e) => write_failed.push(json!({"id": id.0, "error": e.to_string()})),
            }
        }
        s.record_xmp_stamps(&stamped, false);
    }
    if let Some(e) = &d.fatal {
        failed.push(json!({"error": e.to_string()}));
    }
    json!({
        "pulled": pulled,
        "applied": if committed { applied.len() } else { 0 },
        "written": written,
        "skipped": skipped,
        "noSidecar": no_sidecar,
        "failed": failed,
        "writeFailed": write_failed,
        "items": report,
        "dryRun": d.dry_run,
        "overwrite": d.overwrite,
        "truncated": d.truncated,
    })
}

fn summarize_push(d: SidecarsDone) -> Value {
    let mut pushed = 0u64;
    let mut unchanged = 0u64;
    let mut skipped: Vec<Value> = d.local_skipped;
    let mut failed: Vec<Value> = d.local_failed;
    for item in d.items {
        if let Some(why) = item.skipped {
            skipped.push(json!({"id": item.photo.0, "file": item.file_name, "reason": why}));
        } else if let Some(e) = item.error {
            failed.push(json!({"id": item.photo.0, "file": item.file_name, "error": e}));
        } else if item.pushed {
            pushed += 1;
        } else {
            unchanged += 1;
        }
    }
    let errors: Vec<Value> = d.fatal.as_ref().map(|e| vec![json!(e.to_string())]).unwrap_or_default();
    json!({
        "pushed": pushed,
        "unchanged": unchanged,
        "written": d.local_written.len(),
        "paths": d.local_written,
        "skipped": skipped,
        "failed": failed,
        "refreshErrors": d.errors,
        "errors": errors,
        "dryRun": d.dry_run,
    })
}

/// What `remote.pump` and the panel keep: counts and the first errors, never every item (a run
/// can cover thousands of photos).
fn summary(out: &Value, now: &str) -> Value {
    let len = |k: &str| out.get(k).and_then(Value::as_array).map(|a| a.len() as u64).unwrap_or(0);
    let errors: Vec<Value> = out.get("failed").and_then(Value::as_array).map(|a| a.iter().take(5).cloned().collect()).unwrap_or_default();
    json!({
        "direction": out.get("direction").and_then(Value::as_str).unwrap_or_default(),
        "pulled": out.get("pulled").and_then(Value::as_u64).unwrap_or(0),
        "applied": out.get("applied").and_then(Value::as_u64).unwrap_or(0),
        "written": out.get("written").and_then(Value::as_u64).unwrap_or(0),
        "pushed": out.get("pushed").and_then(Value::as_u64).unwrap_or(0),
        "unchanged": out.get("unchanged").and_then(Value::as_u64).unwrap_or(0),
        "skipped": len("skipped"),
        "noSidecar": len("noSidecar"),
        "failed": len("failed") + len("writeFailed"),
        "errors": errors,
        "dryRun": out.get("dryRun").and_then(Value::as_bool).unwrap_or(false),
        "truncated": out.get("truncated").and_then(Value::as_bool).unwrap_or(false),
        "finishedAt": now,
    })
}

// ---------------------------------------------------------------------- commands

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "immich.pushSidecars",
            "Push XMP Sidecars to Immich",
            [],
            None,
            "{account?, ids?, externalOnly?: bool, full?: bool = false, refresh?: bool = true, dryRun?: bool, background?: bool} — \
write the whole XMP sidecar beside each linked photo in a mapped external-library folder (Immich re-reads it), \
and send the fields Immich keeps in its own sidecar (rating, description, capture time, location, tags) for \
managed assets, which Immich merges into the sidecar it holds; `full` replaces a managed asset's whole sidecar \
instead: Immich's sidecar is read and the app's packet merged into it, uploaded on a temporary placeholder, copied \
onto the asset (`PUT /assets/copy`) and the placeholder deleted (needs assetFile.read, asset.upload, asset.copy, \
asset.delete; creates and removes one tiny asset per photo). `externalOnly` writes only the external-library files \
→ {written, paths, pushed, unchanged, skipped, failed, refreshErrors} or {started}",
            always,
            push
        ),
        cmd!(
            "immich.pullSidecars",
            "Pull XMP Sidecars from Immich",
            ["Library", "Immich"],
            None,
            "{account?, ids?, all?, overwrite?: bool, writeFiles?: bool = true, dryRun?: bool, background?: bool} — \
download each linked asset's XMP sidecar (v3.2.0+; the one Immich stored, with the app's `lc:settings` when it was \
sent), apply it to the catalog as one undo step and write it beside the original; a photo edited here after the \
asset changed is left alone unless `overwrite` (needs `assetFile.read`) → {pulled, applied, written, skipped, \
noSidecar, failed, items, dryRun} or {started}",
            always,
            pull
        ),
    ]
}
