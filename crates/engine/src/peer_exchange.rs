//! The catalog exchange between paired devices (peer sync, after [`crate::peers`] has paired them).
//!
//! **What is shared.** A library takes part once it has a sync id (`<library>/sync.json`) and is
//! shared with a device. Turning sharing on offers the library to that device; accepting the offer
//! gives the other library the same sync id. Two libraries with the same sync id are one catalog.
//!
//! **What travels.** For every photo both libraries have (matched by the hash of the original
//! file's bytes; virtual copies and deleted photos stay out): the develop settings, rating, flag
//! and colour label ([`Record`]). Originals don't travel yet.
//!
//! **How.** Each side sends a digest per photo; each asks for the full records it lacks; then each
//! merges on its own UI thread. The merge is three-way against what each side last saw of itself
//! and of the other (`<library>/sync/<device>.json`): a change on one side is taken by the other;
//! when both changed a photo differently, the local edit stays and the other device's edit arrives
//! as a virtual copy in the same stack, so nothing is lost. A photo nobody touched yet never counts
//! as a change, so a fresh library simply takes the other's edits.
//!
//! The protocol is ordered (one side sends while the other reads), so neither can stall the other
//! with a large batch. Sources: own design (three-way merge after `crates/immich/src/sync.rs`).

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use lcc_catalog::{ColorLabel, Flag, Photo, PhotoId};
use lcc_develop::DevelopSettings;
use lcc_peer::Channel;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// One photo's shared state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    pub develop: Arc<DevelopSettings>,
    pub rating: u8,
    pub flag: Flag,
    pub label: Option<ColorLabel>,
    /// The flat-field map the settings reference, from this library's catalog: the other library stores it first
    /// (its catalog has its own table). Never a photo id: those are this catalog's only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub vignetting_maps: Vec<lcc_develop::EmbeddedVignettingMap>,
}

/// What this build's exchange can carry beyond the records of the first version: `vignettingCalibration` (records with their
/// flat-field maps). A device that doesn't say so is an older build, which would drop the maps it merges.
pub const CAPS: &[&str] = &["vignettingCalibration"];

impl Record {
    pub fn of(p: &Photo, cat: &lcc_catalog::Catalog) -> Record {
        let vignetting_maps =
            p.develop.optics.vignetting_calibration.as_ref().and_then(|id| cat.vignetting_map(id)).map(|r| r.map.clone()).into_iter().collect();
        Record { develop: p.develop.clone(), rating: p.rating, flag: p.flag, label: p.label, vignetting_maps }
    }

    /// Whether the settings reference a flat-field map.
    pub fn has_vignetting_calibration(&self) -> bool {
        self.develop.optics.vignetting_calibration.is_some()
    }

    /// A short, stable fingerprint (SHA-256 of the record's JSON, 128 bits in hex).
    pub fn digest(&self) -> String {
        let json = serde_json::to_vec(self).unwrap_or_default();
        Sha256::digest(&json).iter().take(16).map(|b| format!("{b:02x}")).collect()
    }

    /// Nobody has touched this photo: default settings, no rating, flag or label.
    pub fn untouched(&self) -> bool {
        // (new photos start with Constrain Crop on: that is not an edit)
        let unedited =
            DevelopSettings { geometry: lcc_develop::Geometry { constrain_crop: false, ..self.develop.geometry.clone() }, ..(*self.develop).clone() };
        unedited == DevelopSettings::default() && self.rating == 0 && self.flag == Flag::None && self.label.is_none()
    }
}

/// The key that names the same photo in both libraries.
pub fn key_of(p: &Photo) -> Option<&str> {
    if p.copy_of.is_some() || p.deleted {
        return None;
    }
    p.content_hash.as_deref().filter(|h| !h.is_empty() && !h.contains(":dup"))
}

/// The shared photos of a library, by key (the first photo with a key wins).
pub type Records = HashMap<String, (PhotoId, Record)>;

pub fn records(catalog: &lcc_catalog::Catalog) -> Records {
    let mut out = Records::new();
    for p in catalog.photos() {
        if let Some(k) = key_of(p) {
            out.entry(k.to_string()).or_insert_with(|| (p.id, Record::of(p, catalog)));
        }
    }
    out
}

/// `<library>/sync.json`: whether the library takes part, and with whom.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LibrarySync {
    /// Shared by every library that is the same catalog; none until the library is first shared.
    pub sync_id: Option<String>,
    /// Paired devices this library is shared with.
    pub shared_with: Vec<String>,
    /// Offers turned down (device id, sync id): not asked again.
    pub declined: Vec<(String, String)>,
}

const MAX_FILE: u64 = 64 << 20;

fn read_json<T: for<'de> Deserialize<'de> + Default>(path: &Path) -> T {
    match std::fs::metadata(path) {
        Ok(m) if m.len() <= MAX_FILE => std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default(),
        _ => T::default(),
    }
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let json = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    lcc_catalog::safe_file::write_atomic(path, &json).map_err(|e| format!("{}: {e}", path.display()))
}

impl LibrarySync {
    pub fn path(library: &Path) -> PathBuf {
        library.join("sync.json")
    }
    pub fn load(library: &Path) -> LibrarySync {
        read_json(&Self::path(library))
    }
    pub fn save(&self, library: &Path) -> Result<(), String> {
        write_json(&Self::path(library), self)
    }
    pub fn shared_with(&self, device: &str) -> bool {
        self.sync_id.is_some() && self.shared_with.iter().any(|d| d == device)
    }
}

/// What each side last saw of a photo, per paired device: `<library>/sync/<device>.json`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Base {
    pub sync_id: String,
    /// key → digest of this library's record as last sent to the other device.
    pub local: HashMap<String, String>,
    /// key → digest of the other library's record at the last exchange.
    pub remote: HashMap<String, String>,
    /// When the last exchange finished (Unix seconds).
    pub last: u64,
    /// Photos this side kept both versions of (a copy) that the other side hasn't been told about
    /// yet: told, it keeps both too instead of taking this side's version.
    pub conflicts: std::collections::BTreeSet<String>,
    /// Photos the other side told us it kept both versions of, whose version hasn't arrived
    /// yet: when it does, this side keeps both too rather than taking it over its own.
    pub remote_conflicts: std::collections::BTreeSet<String>,
}

impl Base {
    pub fn path(library: &Path, device: &str) -> PathBuf {
        let safe: String = device.chars().filter(char::is_ascii_hexdigit).take(64).collect();
        library.join("sync").join(format!("{safe}.json"))
    }
    /// The base for `sync_id`; a base of another catalog starts afresh.
    pub fn load(library: &Path, device: &str, sync_id: &str) -> Base {
        let b: Base = read_json(&Self::path(library, device));
        if b.sync_id == sync_id { b } else { Base { sync_id: sync_id.to_string(), ..Default::default() } }
    }
    pub fn save(&self, library: &Path, device: &str) -> Result<(), String> {
        write_json(&Self::path(library, device), self)
    }
}

/// A library offered for sharing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogInfo {
    pub sync_id: String,
    pub name: String,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "camelCase")]
enum Msg {
    Catalogs {
        list: Vec<CatalogInfo>,
    },
    Digests {
        map: BTreeMap<String, String>,
        #[serde(default)]
        conflicts: Vec<String>,
        /// What this side's exchange can carry ([`CAPS`]); none from an older build.
        #[serde(default)]
        caps: Vec<String>,
    },
    Want {
        keys: Vec<String>,
    },
    Records {
        batch: Vec<(String, Record)>,
        last: bool,
    },
}

/// This side's part in an exchange.
pub struct Local {
    /// The library shared with the other device, if any.
    pub catalog: Option<CatalogInfo>,
    pub records: Arc<Records>,
    /// What this side last saw of the other's records (key → digest).
    pub base_remote: HashMap<String, String>,
    /// Photos this side kept both versions of, not yet told ([`Base::conflicts`]).
    pub conflicts: Vec<String>,
}

/// What an exchange brought back, for the UI thread to merge.
#[derive(Debug, Default)]
pub struct Outcome {
    pub sync_id: Option<String>,
    /// The other side's digests (key → digest).
    pub remote: HashMap<String, String>,
    /// The full records this side asked for.
    pub records: HashMap<String, Record>,
    /// This side's digests as sent (what the other side now believes about this library).
    pub sent: HashMap<String, String>,
    /// The collisions this side told the other about (now delivered).
    pub told: Vec<String>,
    /// Photos the other side kept both versions of.
    pub remote_conflicts: std::collections::HashSet<String>,
    /// The other side offered a library this one doesn't share yet.
    pub offer: Option<CatalogInfo>,
    /// Photos left out of this exchange because they use a flat-field map and the other device's build can't carry
    /// one (it would drop the map when it merged them).
    pub left_out: usize,
}

/// The most records sent in one frame, and roughly the most bytes.
const BATCH: usize = 64;
const BATCH_BYTES: usize = 16 << 20;
/// The most keys accepted from the other side (a hostile peer can't exhaust memory).
const MAX_KEYS: usize = 1_000_000;

fn send(ch: &mut Channel, m: &Msg) -> Result<(), String> {
    ch.send_json(m).map_err(|e| e.to_string())
}
fn recv(ch: &mut Channel) -> Result<Msg, String> {
    ch.recv_json::<Msg>().map_err(|e| e.to_string())
}

/// The photos that stay out of an exchange with a device whose build says it can carry `remote_caps`: with an older
/// one (no `vignettingCalibration`), every photo whose settings here use a flat-field map.
pub fn held_back(records: &Records, remote_caps: &[String]) -> Vec<String> {
    if remote_caps.iter().any(|c| c == "vignettingCalibration") {
        return Vec::new();
    }
    records.iter().filter(|(_, (_, r))| r.has_vignetting_calibration()).map(|(k, _)| k.clone()).collect()
}

/// The other side's turn, then this side's, or the other way round.
fn swap(ch: &mut Channel, first: bool, mine: &Msg) -> Result<Msg, String> {
    if first {
        send(ch, mine)?;
        recv(ch)
    } else {
        let theirs = recv(ch)?;
        send(ch, mine)?;
        Ok(theirs)
    }
}

fn send_records(ch: &mut Channel, records: &Records, keys: &[String]) -> Result<(), String> {
    let mut batch = Vec::new();
    let mut bytes = 0usize;
    for k in keys {
        let Some((_, r)) = records.get(k) else { continue };
        bytes = bytes.saturating_add(serde_json::to_vec(r).map(|v| v.len()).unwrap_or(0));
        batch.push((k.clone(), r.clone()));
        if batch.len() >= BATCH || bytes >= BATCH_BYTES {
            send(ch, &Msg::Records { batch: std::mem::take(&mut batch), last: false })?;
            bytes = 0;
        }
    }
    send(ch, &Msg::Records { batch, last: true })
}

fn recv_records(ch: &mut Channel, wanted: &[String]) -> Result<HashMap<String, Record>, String> {
    let wanted: std::collections::HashSet<&String> = wanted.iter().collect();
    let mut out = HashMap::new();
    loop {
        let Msg::Records { batch, last } = recv(ch)? else { return Err("unexpected message".into()) };
        for (k, r) in batch {
            if wanted.contains(&k) {
                out.insert(k, r);
            }
        }
        if last {
            return Ok(out);
        }
    }
}

/// Runs one exchange over an established channel. `initiator`: this side connected.
/// `on_records` hears how many records are about to travel (both ways), once, before they do.
pub fn exchange(ch: &mut Channel, initiator: bool, local: &Local, on_records: &mut dyn FnMut(usize)) -> Result<Outcome, String> {
    let mine = Msg::Catalogs { list: local.catalog.clone().into_iter().collect() };
    let Msg::Catalogs { list: theirs } = swap(ch, initiator, &mine)? else { return Err("unexpected message".into()) };
    let common = local.catalog.as_ref().filter(|c| theirs.iter().any(|t| t.sync_id == c.sync_id)).map(|c| c.sync_id.clone());
    let Some(sync_id) = common else {
        let offer = theirs.into_iter().find(|t| local.catalog.as_ref().is_none_or(|c| c.sync_id != t.sync_id));
        return Ok(Outcome { offer, ..Default::default() });
    };

    let my_digests: BTreeMap<String, String> = local.records.iter().map(|(k, (_, r))| (k.clone(), r.digest())).collect();
    let caps = CAPS.iter().map(|c| c.to_string()).collect();
    let Msg::Digests { map: mut remote, conflicts: remote_conflicts, caps: remote_caps } =
        swap(ch, initiator, &Msg::Digests { map: my_digests.clone(), conflicts: local.conflicts.clone(), caps })?
    else {
        return Err("unexpected message".into());
    };
    if remote.len() > MAX_KEYS {
        return Err("the other catalog is too large".into());
    }
    // an older build on the other side can't carry flat-field maps: photos that use one here stay out of this
    // exchange both ways (it would take them without the map, or overwrite them without it)
    let mut my_digests = my_digests;
    let held = held_back(&local.records, &remote_caps);
    for k in &held {
        my_digests.remove(k);
        remote.remove(k);
    }
    let left_out = held.len();
    if left_out > 0 {
        log::warn!("sync: {left_out} photo(s) with a vignetting calibration left out: the other device's build can't carry vignetting maps");
    }
    // the records this side lacks: changed over there since last time, present here, different
    let want: Vec<String> = remote
        .iter()
        .filter(|(k, d)| local.base_remote.get(*k) != Some(*d) && my_digests.get(*k).is_some_and(|mine| mine != *d))
        .map(|(k, _)| k.clone())
        .collect();
    let Msg::Want { keys: their_want } = swap(ch, initiator, &Msg::Want { keys: want.clone() })? else { return Err("unexpected message".into()) };
    if their_want.len() > MAX_KEYS {
        return Err("too many records asked for".into());
    }
    // (only what this side offered: a photo left out stays out, whatever the other side asks)
    let their_want: Vec<String> = their_want.into_iter().filter(|k| my_digests.contains_key(k)).collect();
    on_records(want.len().saturating_add(their_want.len()));
    let records = if initiator {
        send_records(ch, &local.records, &their_want)?;
        recv_records(ch, &want)?
    } else {
        let r = recv_records(ch, &want)?;
        send_records(ch, &local.records, &their_want)?;
        r
    };
    Ok(Outcome {
        sync_id: Some(sync_id),
        remote: remote.into_iter().collect(),
        records,
        sent: my_digests.into_iter().collect(),
        told: local.conflicts.clone(),
        remote_conflicts: remote_conflicts.into_iter().take(MAX_KEYS).collect(),
        offer: None,
        left_out,
    })
}

/// What the merge does with one photo.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// Take the other side's record.
    Take(Record),
    /// Both changed it: keep this side's, add the other's as a virtual copy.
    Copy(Record),
    Keep,
}

/// The three-way decision for one photo present on both sides.
/// `remote_conflict`: the other side kept both versions of this photo, so this side does too.
pub fn decide(local: &Record, remote_digest: &str, remote: Option<&Record>, base: &Base, key: &str, remote_conflict: bool) -> Action {
    let ld = local.digest();
    if ld == remote_digest {
        return Action::Keep;
    }
    let local_changed = base.local.get(key).is_none_or(|b| *b != ld) && !(!base.local.contains_key(key) && local.untouched());
    let remote_changed =
        base.remote.get(key).is_none_or(|b| b != remote_digest) && !(!base.remote.contains_key(key) && remote.is_some_and(Record::untouched));
    match (local_changed, remote_changed, remote) {
        (_, false, _) | (_, true, None) => Action::Keep,
        (false, true, Some(r)) if remote_conflict => Action::Copy(r.clone()),
        (false, true, Some(r)) => Action::Take(r.clone()),
        (true, true, Some(r)) => Action::Copy(r.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(exposure: f64, rating: u8) -> Record {
        let mut d = DevelopSettings::default();
        d.light.exposure = exposure;
        Record { develop: Arc::new(d), rating, flag: Flag::None, label: None, vignetting_maps: Vec::new() }
    }

    fn base(local: &Record, remote: &Record) -> Base {
        Base {
            sync_id: "s".into(),
            local: [("k".to_string(), local.digest())].into(),
            remote: [("k".to_string(), remote.digest())].into(),
            last: 0,
            conflicts: Default::default(),
            remote_conflicts: Default::default(),
        }
    }

    #[test]
    fn a_change_on_one_side_is_taken_by_the_other() {
        let old = rec(0.5, 1);
        let new = rec(1.0, 1);
        assert_eq!(decide(&old, &new.digest(), Some(&new), &base(&old, &old), "k", false), Action::Take(new.clone()));
        // the side that made the change keeps it
        assert_eq!(decide(&new, &old.digest(), Some(&old), &base(&old, &old), "k", false), Action::Keep);
    }

    #[test]
    fn both_changed_makes_a_copy_and_then_settles() {
        let old = rec(0.5, 1);
        let mine = rec(1.0, 1);
        let theirs = rec(-1.0, 3);
        assert_eq!(decide(&mine, &theirs.digest(), Some(&theirs), &base(&old, &old), "k", false), Action::Copy(theirs.clone()));
        // after the exchange each side's base holds what it saw: no second copy
        assert_eq!(decide(&mine, &theirs.digest(), Some(&theirs), &base(&mine, &theirs), "k", false), Action::Keep);
    }

    #[test]
    fn a_fresh_library_takes_the_edits_without_copies() {
        let fresh = Record { develop: Arc::new(DevelopSettings::default()), rating: 0, flag: Flag::None, label: None, vignetting_maps: Vec::new() };
        let edited = rec(1.0, 4);
        let empty = Base::default();
        assert_eq!(decide(&fresh, &edited.digest(), Some(&edited), &empty, "k", false), Action::Take(edited.clone()));
        assert_eq!(decide(&edited, &fresh.digest(), Some(&fresh), &empty, "k", false), Action::Keep);
        // both edited before they first met: a copy
        let other = rec(-0.5, 2);
        assert_eq!(decide(&edited, &other.digest(), Some(&other), &empty, "k", false), Action::Copy(other));
    }

    #[test]
    fn a_collision_the_other_side_resolved_is_kept_on_both() {
        let old = rec(0.5, 1);
        let mine = rec(1.0, 1);
        let theirs = rec(-1.0, 3);
        // the other side saw "mine" and kept both: here too, both stay
        assert_eq!(decide(&mine, &theirs.digest(), Some(&theirs), &base(&mine, &old), "k", true), Action::Copy(theirs.clone()));
        assert_eq!(decide(&mine, &theirs.digest(), Some(&theirs), &base(&mine, &old), "k", false), Action::Take(theirs));
    }

    #[test]
    fn digests_are_stable_and_tell_records_apart() {
        assert_eq!(rec(1.0, 1).digest(), rec(1.0, 1).digest());
        assert_ne!(rec(1.0, 1).digest(), rec(1.0, 2).digest());
        assert_eq!(rec(1.0, 1).digest().len(), 32);
    }

    #[test]
    fn damaged_sync_files_load_as_empty() {
        let dir = std::env::temp_dir().join(format!("lcc-exchange-files-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sync")).unwrap();
        std::fs::write(LibrarySync::path(&dir), b"{not json").unwrap();
        assert_eq!(LibrarySync::load(&dir), LibrarySync::default());
        std::fs::write(Base::path(&dir, "ab"), b"\xff").unwrap();
        assert!(Base::load(&dir, "ab", "s").remote.is_empty());
        // a device id can't climb out of the folder
        assert!(Base::path(&dir, "../../x").starts_with(dir.join("sync")));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
