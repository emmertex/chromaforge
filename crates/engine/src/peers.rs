//! Peer sync between this app's copies on the local network (Settings ▸ Sync): the
//! [`lcc_peer::Service`] a session runs, started on demand.
//!
//! The service starts when Settings ▸ Sync looks for devices (`sync.find`), or at launch when
//! paired devices exist (`sync.start`), never by itself: a session that never asks opens no
//! sockets. Looking for devices shows in the activity stack; cancelling that row stops looking.
//! The paired devices and this device's identity live in `<settings>/sync/peers.json`.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{Value, json};

use crate::activity::{Activity, Cancel, TaskGuard};
use crate::peer_exchange::{self as px, Action, Base, CatalogInfo, LibrarySync, Record};
use crate::{EngineError, Result, Session};

/// How often the device with the lower id starts an exchange with each paired device online.
const EXCHANGE_EVERY: Duration = Duration::from_secs(15);
/// How often the shared records are refreshed for incoming exchanges.
const PUBLISH_EVERY: Duration = Duration::from_secs(1);

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// This library as the exchange threads see it (refreshed by `sync.tick`).
#[derive(Clone)]
struct Published {
    library: PathBuf,
    name: String,
    sync: LibrarySync,
    records: Arc<px::Records>,
}

/// How the last exchange with a device went.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct Exchange {
    running: bool,
    /// When it last finished (Unix seconds).
    last: u64,
    error: Option<String>,
    /// Photos that took the other device's edit, and copies made for edits on both sides.
    taken: usize,
    copies: usize,
}

/// Shared between the session and the exchange threads.
struct Hub {
    activity: Activity,
    published: Mutex<Option<Published>>,
    outcomes: Mutex<Vec<(lcc_peer::Peer, PathBuf, std::result::Result<px::Outcome, String>)>>,
    running: Mutex<HashSet<String>>,
    tried: Mutex<HashMap<String, Instant>>,
    offers: Mutex<Vec<(lcc_peer::Peer, CatalogInfo)>>,
    exchanges: Mutex<HashMap<String, Exchange>>,
}

impl Hub {
    /// Runs one exchange with `peer` on this thread and queues what came back.
    fn run(&self, peer: &lcc_peer::Peer, ch: &mut lcc_peer::Channel, initiator: bool) {
        let published = lock(&self.published).clone();
        let library = published.as_ref().map(|p| p.library.clone()).unwrap_or_default();
        let result = {
            let catalog = published
                .as_ref()
                .filter(|p| p.sync.shared_with(&peer.device_id))
                .and_then(|p| p.sync.sync_id.clone().map(|id| CatalogInfo { sync_id: id, name: p.name.clone() }));
            let (base_remote, conflicts) = match (&published, &catalog) {
                (Some(p), Some(c)) => {
                    let b = Base::load(&p.library, &peer.device_id, &c.sync_id);
                    (b.remote, b.conflicts.into_iter().collect())
                }
                _ => (HashMap::new(), Vec::new()),
            };
            let records = published.as_ref().map(|p| p.records.clone()).unwrap_or_default();
            let local = px::Local { catalog, records, base_remote, conflicts };
            // the activity row only while records actually travel
            let mut task: Option<TaskGuard> = None;
            let activity = self.activity.clone();
            px::exchange(ch, initiator, &local, &mut |n| {
                if n > 0 && task.is_none() {
                    task = Some(activity.start("sync", "Syncing library", Cancel::No));
                }
            })
        };
        // queued before it stops counting as running, so no new exchange with this peer starts
        // until its outcome is there to merge
        lock(&self.outcomes).push((peer.clone(), library, result));
        lock(&self.running).remove(&peer.device_id);
    }
}

/// The session's peer-sync state.
#[derive(Default)]
pub struct State {
    /// `<settings>/sync/peers.json`; `None` keeps the feature off (tests, `--memory`).
    pub store_path: Option<PathBuf>,
    /// The multicast DNS port; `None` = [`lcc_peer::beacon::PORT`] (5353; tests use their own).
    pub port: Option<u16>,
    /// How often exchanges start; `None` = [`EXCHANGE_EVERY`] (tests go faster).
    pub exchange_every: Option<Duration>,
    service: Option<Arc<lcc_peer::Service>>,
    /// The activity row while looking for devices, and its cancel flag.
    finding: Option<(TaskGuard, Arc<AtomicBool>)>,
    hub: Option<Arc<Hub>>,
    published_at: Option<Instant>,
}

impl std::fmt::Debug for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("State").field("store_path", &self.store_path).field("running", &self.service.is_some()).finish_non_exhaustive()
    }
}

/// What a new device is called: the computer's name where the system says it, else the platform.
pub fn default_device_name() -> String {
    let host = std::env::var("COMPUTERNAME")
        .ok()
        .or_else(|| std::env::var("HOSTNAME").ok())
        .or_else(|| std::fs::read_to_string("/etc/hostname").ok())
        .map(|h| h.trim().to_string())
        .filter(|h| !h.is_empty() && h != "localhost");
    host.unwrap_or_else(|| {
        if cfg!(target_os = "android") {
            "Android tablet".into()
        } else if cfg!(target_os = "macos") {
            "Mac".into()
        } else if cfg!(windows) {
            "Windows PC".into()
        } else {
            "Computer".into()
        }
    })
}

fn other(e: lcc_peer::Error) -> EngineError {
    EngineError::Other(e.to_string())
}

impl State {
    /// The service, started if needed (with the exchange hub answering paired devices).
    fn ensure(&mut self, activity: &Activity) -> Result<&Arc<lcc_peer::Service>> {
        if self.service.is_none() {
            let path = self.store_path.clone().ok_or_else(|| EngineError::Other("sync needs a settings folder".into()))?;
            let svc = Arc::new(lcc_peer::Service::start(path, &default_device_name(), self.port.unwrap_or(lcc_peer::beacon::PORT)).map_err(other)?);
            let hub = Arc::new(Hub {
                activity: activity.clone(),
                published: Mutex::new(None),
                outcomes: Mutex::new(Vec::new()),
                running: Mutex::new(HashSet::new()),
                tried: Mutex::new(HashMap::new()),
                offers: Mutex::new(Vec::new()),
                exchanges: Mutex::new(HashMap::new()),
            });
            let h = hub.clone();
            svc.set_hello_handler(Arc::new(move |peer, mut ch| {
                lock(&h.running).insert(peer.device_id.clone());
                h.run(&peer, &mut ch, false);
            }));
            self.hub = Some(hub);
            self.service = Some(svc);
        }
        self.service.as_ref().ok_or_else(|| EngineError::Other("sync isn't running".into()))
    }

    /// Stops looking for devices if the activity row was cancelled.
    fn reconcile(&mut self) {
        let cancelled = self.finding.as_ref().is_some_and(|(_, flag)| flag.load(Ordering::Relaxed));
        if cancelled {
            self.finding = None;
            if let Some(s) = &self.service {
                s.set_finding(false);
            }
        }
    }

    /// The stored identity and paired devices without starting anything.
    fn stored(&self) -> Option<lcc_peer::Store> {
        let path = self.store_path.as_ref()?;
        path.is_file().then(|| lcc_peer::Store::load_or_create(path, "").ok()).flatten()
    }
}

pub(crate) fn status(s: &mut Session) -> Result<Value> {
    let mut v = base_status(s)?;
    if let Some(o) = v.as_object_mut() {
        o.insert("library".into(), library_status(s));
        let (offers, exchanges) = match &s.peers.hub {
            Some(h) => (
                lock(&h.offers).iter().map(|(p, c)| json!({"deviceId": p.device_id, "name": p.name, "library": c.name})).collect::<Vec<_>>(),
                serde_json::to_value(&*lock(&h.exchanges)).unwrap_or(Value::Null),
            ),
            None => (Vec::new(), json!({})),
        };
        o.insert("offers".into(), json!(offers));
        o.insert("exchanges".into(), exchanges);
    }
    Ok(v)
}

fn base_status(s: &mut Session) -> Result<Value> {
    let st = &mut s.peers;
    st.reconcile();
    if let Some(svc) = &st.service {
        let mut v = serde_json::to_value(svc.status()).map_err(|e| EngineError::Other(e.to_string()))?;
        if let Some(o) = v.as_object_mut() {
            o.insert("running".into(), json!(true));
            o.insert("available".into(), json!(true));
        }
        return Ok(v);
    }
    let stored = st.stored();
    Ok(json!({
        "running": false,
        "available": st.store_path.is_some(),
        "deviceId": stored.as_ref().map(|s| s.identity.device_id.clone()),
        "name": stored.as_ref().map_or_else(default_device_name, |s| s.identity.name.clone()),
        "finding": false,
        "found": [],
        "paired": stored.map(|s| s.peers.iter().map(|p| json!({"deviceId": p.device_id, "name": p.name, "online": false, "pairedAt": p.paired_at})).collect::<Vec<_>>()).unwrap_or_default(),
        "pairing": null,
    }))
}

pub(crate) fn find(s: &mut Session, on: bool) -> Result<Value> {
    let activity = s.activity.clone();
    let st = &mut s.peers;
    if on {
        st.ensure(&activity)?.set_finding(true);
        if st.finding.is_none() {
            let flag = Arc::new(AtomicBool::new(false));
            let guard = activity.start("sync", "Looking for devices", Cancel::Flag(flag.clone()));
            st.finding = Some((guard, flag));
        }
    } else {
        st.finding = None;
        if let Some(svc) = &st.service {
            svc.set_finding(false);
        }
    }
    status(s)
}

/// Starts the service when paired devices exist (at launch), so they can find each other.
pub(crate) fn start(s: &mut Session) -> Result<Value> {
    let activity = s.activity.clone();
    let st = &mut s.peers;
    if st.service.is_none() && st.stored().is_some_and(|store| !store.peers.is_empty()) {
        st.ensure(&activity)?;
    }
    Ok(json!({"running": st.service.is_some()}))
}

pub(crate) fn pair(s: &mut Session, device: &str) -> Result<Value> {
    let a = s.activity.clone();
    s.peers.ensure(&a)?.pair(device).map_err(other)?;
    status(s)
}

pub(crate) fn confirm(s: &mut Session, accept: bool) -> Result<Value> {
    let a = s.activity.clone();
    s.peers.ensure(&a)?.confirm(accept).map_err(other)?;
    status(s)
}

pub(crate) fn clear_pairing(s: &mut Session) -> Result<Value> {
    if let Some(svc) = &s.peers.service {
        svc.clear_pairing();
    }
    status(s)
}

pub(crate) fn unpair(s: &mut Session, device: &str) -> Result<Value> {
    let removed = match &s.peers.service {
        Some(svc) => svc.unpair(device).map_err(other)?,
        None => {
            let path = s.peers.store_path.clone().ok_or_else(|| EngineError::Other("sync needs a settings folder".into()))?;
            let mut store = s.peers.stored().ok_or_else(|| EngineError::Other("no device is paired".into()))?;
            let removed = store.remove_peer(device);
            if removed {
                store.save(&path).map_err(other)?;
            }
            removed
        }
    };
    if !removed {
        return Err(EngineError::Other(format!("no paired device `{device}`")));
    }
    status(s)
}

pub(crate) fn rename(s: &mut Session, name: &str) -> Result<Value> {
    let a = s.activity.clone();
    s.peers.ensure(&a)?.rename(name).map_err(other)?;
    status(s)
}

impl Session {
    /// Keep the peer-sync identity and paired devices in `<settings>/sync/peers.json`.
    pub fn with_default_peers(mut self) -> Self {
        self.peers.store_path = crate::config::config_dir().map(|d| d.join("sync").join("peers.json"));
        self
    }
}

/// The open library, if it is a folder on disk.
fn library_dir(s: &Session) -> Option<PathBuf> {
    s.library.as_ref().filter(|l| l.on_disk).map(|l| l.dir.clone())
}

fn library_name(dir: &std::path::Path) -> String {
    dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "Library".into())
}

fn library_status(s: &Session) -> Value {
    let Some(dir) = library_dir(s) else { return Value::Null };
    let sync = LibrarySync::load(&dir);
    json!({"name": library_name(&dir), "syncId": sync.sync_id, "sharedWith": sync.shared_with})
}

/// Shares (or stops sharing) the open library with a paired device.
pub(crate) fn share(s: &mut Session, device: &str, on: bool) -> Result<Value> {
    let dir = library_dir(s).ok_or_else(|| EngineError::Other("only a library on disk can be shared".into()))?;
    if on && !s.peers.stored().is_some_and(|st| st.peer(device).is_some()) {
        return Err(EngineError::Other(format!("not paired with `{device}`")));
    }
    let mut sync = LibrarySync::load(&dir);
    if on {
        if sync.sync_id.is_none() {
            sync.sync_id = Some(lcc_peer::identity::random_id().map_err(other)?);
        }
        if !sync.shared_with.iter().any(|d| d == device) {
            sync.shared_with.push(device.to_string());
        }
    } else {
        sync.shared_with.retain(|d| d != device);
    }
    sync.save(&dir).map_err(EngineError::Other)?;
    s.peers.published_at = None;
    status(s)
}

/// Takes up (or turns down) a library another device offered: this library becomes the same catalog.
pub(crate) fn answer_offer(s: &mut Session, device: &str, accept: bool) -> Result<Value> {
    let dir = library_dir(s).ok_or_else(|| EngineError::Other("only a library on disk can be shared".into()))?;
    let hub = s.peers.hub.clone().ok_or_else(|| EngineError::Other("no library was offered".into()))?;
    let offer = {
        let mut offers = lock(&hub.offers);
        let i = offers.iter().position(|(p, _)| p.device_id == device).ok_or_else(|| EngineError::Other("that device offered no library".into()))?;
        offers.remove(i).1
    };
    let mut sync = LibrarySync::load(&dir);
    if accept {
        sync.sync_id = Some(offer.sync_id);
        if !sync.shared_with.iter().any(|d| d == device) {
            sync.shared_with.push(device.to_string());
        }
    } else {
        sync.declined.push((device.to_string(), offer.sync_id));
    }
    sync.save(&dir).map_err(EngineError::Other)?;
    s.peers.published_at = None;
    status(s)
}

/// Writes one record onto a photo (only what differs), as one undoable step.
fn apply_record(s: &mut Session, id: lcc_catalog::PhotoId, r: &Record, label: &str) -> Result<()> {
    let Some(p) = s.catalog.photo(id).cloned() else { return Ok(()) };
    if p.develop != r.develop {
        s.set_develop(id, (*r.develop).clone(), label)?;
    }
    if p.rating != r.rating {
        s.commit(label, lcc_catalog::Op::SetRating { id, rating: r.rating.min(5) })?;
    }
    if p.flag != r.flag {
        s.commit(label, lcc_catalog::Op::SetFlag { id, flag: r.flag })?;
    }
    if p.label != r.label {
        s.commit(label, lcc_catalog::Op::SetLabel { id, label: r.label })?;
    }
    Ok(())
}

/// Merges what an exchange brought back into the open library (it must still be the same one).
fn merge(s: &mut Session, peer: &lcc_peer::Peer, library: &std::path::Path, o: px::Outcome) -> Result<(usize, usize)> {
    merge_with(s, peer, library, o, &mut apply_record)
}

type ApplyFn<'a> = dyn FnMut(&mut Session, lcc_catalog::PhotoId, &Record, &str) -> Result<()> + 'a;

/// [`merge`] with the record writer passed in (tests make it fail). A photo whose merge fails
/// leaves its base untouched; every other photo's merge still lands and the base is saved, so
/// a failure never leaves the base behind edits that were applied (which would show up as
/// spurious collisions, and copies, on the next exchange). The first error is returned.
fn merge_with(s: &mut Session, peer: &lcc_peer::Peer, library: &std::path::Path, o: px::Outcome, apply: &mut ApplyFn) -> Result<(usize, usize)> {
    let Some(sync_id) = o.sync_id else { return Ok((0, 0)) };
    if library_dir(s).as_deref() != Some(library) || LibrarySync::load(library).sync_id.as_deref() != Some(&sync_id) {
        return Ok((0, 0));
    }
    let mut base = Base::load(library, &peer.device_id, &sync_id);
    let current = px::records(&s.catalog);
    let label = format!("Sync from {}", peer.name);
    let (mut taken, mut copies) = (0, 0);
    let mut new_conflicts = Vec::new();
    let mut failed = None;
    // the other side kept both versions of these: remembered until its version arrives, as the
    // telling can come in an exchange that doesn't carry the record yet
    base.remote_conflicts.extend(o.remote_conflicts.iter().cloned());
    for (key, remote_digest) in &o.remote {
        let Some((id, local)) = current.get(key) else { continue };
        let step: Result<()> = match px::decide(local, remote_digest, o.records.get(key), &base, key, base.remote_conflicts.contains(key)) {
            Action::Take(r) => apply(s, *id, &r, &label).map(|()| {
                base.remote_conflicts.remove(key);
                taken += 1;
            }),
            Action::Copy(r) => {
                let name = format!("{} edit", peer.name);
                crate::cmd::organize::virtual_copy(s, *id, &name)
                    .and_then(|copy| match copy {
                        Some(copy) => apply(s, copy, &r, &label).map(|()| copies += 1),
                        None => Ok(()),
                    })
                    .map(|()| {
                        // tell the other side next time, unless it kept both already
                        if !base.remote_conflicts.remove(key) {
                            new_conflicts.push(key.clone());
                        }
                    })
            }
            Action::Keep if local.digest() == *remote_digest => {
                base.remote_conflicts.remove(key);
                Ok(())
            }
            Action::Keep => Ok(()),
        };
        if let Err(e) = step {
            // this photo's base stays as it was: the next exchange tries it again
            failed.get_or_insert(e);
            continue;
        }
        // what the other side now believes about this photo here: the digest it was sent (the
        // record may have changed since; that change counts next time)
        if let Some(sent) = o.sent.get(key) {
            base.local.insert(key.clone(), sent.clone());
        }
        base.remote.insert(key.clone(), remote_digest.clone());
    }
    for told in &o.told {
        base.conflicts.remove(told);
    }
    base.conflicts.extend(new_conflicts);
    base.last = now_secs();
    base.save(library, &peer.device_id).map_err(EngineError::Other)?;
    match failed {
        Some(e) => Err(e),
        None => Ok((taken, copies)),
    }
}

/// `sync.tick`, every second or so while sync runs: refreshes what incoming exchanges see,
/// merges what finished exchanges brought back, and starts exchanges with paired devices online
/// (the device with the lower id starts).
pub(crate) fn tick(s: &mut Session) -> Result<Value> {
    let (Some(svc), Some(hub)) = (s.peers.service.clone(), s.peers.hub.clone()) else { return Ok(json!({"running": false})) };
    if s.peers.published_at.is_none_or(|t| t.elapsed() >= PUBLISH_EVERY) {
        s.peers.published_at = Some(Instant::now());
        let published = library_dir(s).map(|dir| {
            let sync = LibrarySync::load(&dir);
            let records = if sync.sync_id.is_some() { px::records(&s.catalog) } else { px::Records::new() };
            Published { name: library_name(&dir), library: dir, sync, records: Arc::new(records) }
        });
        *lock(&hub.published) = published;
    }
    // merge first: a new exchange loads this library's sync base, which must already hold what
    // the last one learnt (above all the collisions to tell the other side about)
    let outcomes = std::mem::take(&mut *lock(&hub.outcomes));
    let (mut taken, mut copies) = (0, 0);
    for (peer, library, result) in outcomes {
        let mut record = Exchange { last: now_secs(), ..Default::default() };
        match result {
            Err(e) => record.error = Some(e),
            Ok(mut o) => {
                if let Some(offer) = o.offer.take() {
                    let declined = library_dir(s)
                        .is_some_and(|d| LibrarySync::load(&d).declined.iter().any(|(dev, id)| *dev == peer.device_id && *id == offer.sync_id));
                    let mut offers = lock(&hub.offers);
                    if !declined && !offers.iter().any(|(p, _)| p.device_id == peer.device_id) {
                        offers.push((peer.clone(), offer));
                    }
                }
                match merge(s, &peer, &library, o) {
                    Ok((t, c)) => {
                        record.taken = t;
                        record.copies = c;
                        taken += t;
                        copies += c;
                    }
                    Err(e) => record.error = Some(e.to_string()),
                }
            }
        }
        record.running = lock(&hub.running).contains(&peer.device_id);
        lock(&hub.exchanges).insert(peer.device_id.clone(), record);
    }
    let me = svc.device_id();
    for peer in svc.online_peers() {
        if me >= peer.device_id || lock(&hub.running).contains(&peer.device_id) {
            continue;
        }
        let every = s.peers.exchange_every.unwrap_or(EXCHANGE_EVERY);
        let due = lock(&hub.tried).get(&peer.device_id).is_none_or(|t| t.elapsed() >= every);
        if !due {
            continue;
        }
        lock(&hub.tried).insert(peer.device_id.clone(), Instant::now());
        lock(&hub.running).insert(peer.device_id.clone());
        lock(&hub.exchanges).entry(peer.device_id.clone()).or_default().running = true;
        let (svc, hub) = (svc.clone(), hub.clone());
        let spawned = std::thread::Builder::new().name("peer-exchange".into()).spawn(move || match svc.connect(&peer.device_id) {
            Ok(mut ch) => hub.run(&peer, &mut ch, true),
            Err(e) => {
                let id = peer.device_id.clone();
                lock(&hub.outcomes).push((peer, PathBuf::new(), Err(e.to_string())));
                lock(&hub.running).remove(&id);
            }
        });
        if let Err(e) = spawned {
            log::warn!("sync: no thread for an exchange: {e}");
        }
    }
    Ok(json!({"running": true, "taken": taken, "copies": copies}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn session(dir: &std::path::Path, name: &str, port: u16) -> Session {
        let mut s = Session::new();
        s.peers.store_path = Some(dir.join(format!("{name}.json")));
        s.peers.port = Some(port);
        s.peers.exchange_every = Some(Duration::from_millis(200));
        s
    }

    fn until(s: &mut Session, what: &str, f: impl Fn(&Value) -> bool) -> Value {
        let t = Instant::now();
        loop {
            let v = s.execute("sync.status", &json!({})).unwrap();
            if f(&v) {
                return v;
            }
            assert!(t.elapsed() < Duration::from_secs(20), "timed out waiting for {what}: {v}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Settings ▸ Sync's whole flow through commands: both look, one pairs, both confirm the same
    /// code, both list the other as paired; looking shows in the activity stack until stopped.
    #[test]
    fn two_sessions_pair_through_the_commands() {
        // macOS LAN pairing check: discovery times out on the macOS CI runner; checked in person on
        // real Macs instead (see "Checked in person, not on CI" in docs/ci.md)
        if cfg!(target_os = "macos") && std::env::var_os("CI").is_some() {
            lcc_testkit::skip!(System, "LAN discovery on the macOS CI runner (macOS LAN pairing check, tested in person)");
            return;
        }
        let dir = std::env::temp_dir().join(format!("lcc-engine-peers-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let port = 30000 + (std::process::id() % 9000) as u16;
        let (mut a, mut b) = (session(&dir, "a", port), session(&dir, "b", port));
        // nothing runs before it is asked for
        assert_eq!(a.execute("sync.status", &json!({})).unwrap()["running"], json!(false));
        assert_eq!(a.execute("sync.start", &json!({})).unwrap()["running"], json!(false));
        a.execute("sync.find", &json!({"on": true})).unwrap();
        b.execute("sync.find", &json!({"on": true})).unwrap();
        assert!(a.activity.list().iter().any(|r| r.kind == "sync"));
        let b_id = b.execute("sync.status", &json!({})).unwrap()["deviceId"].as_str().unwrap().to_string();
        until(&mut a, "a to find b", |v| v["found"].as_array().is_some_and(|f| f.iter().any(|d| d["deviceId"] == b_id.as_str())));
        a.execute("sync.pair", &json!({"device": b_id})).unwrap();
        let sa = until(&mut a, "a's code", |v| v["pairing"]["stage"] == "confirm");
        let sb = until(&mut b, "b's code", |v| v["pairing"]["stage"] == "confirm");
        assert_eq!(sa["pairing"]["code"], sb["pairing"]["code"]);
        a.execute("sync.confirm", &json!({"accept": true})).unwrap();
        b.execute("sync.confirm", &json!({"accept": true})).unwrap();
        until(&mut a, "a paired", |v| v["paired"].as_array().is_some_and(|p| p.len() == 1));
        until(&mut b, "b paired", |v| v["paired"].as_array().is_some_and(|p| p.len() == 1));
        a.execute("sync.find", &json!({"on": false})).unwrap();
        assert!(!a.activity.list().iter().any(|r| r.kind == "sync"));
        // bad parameters are errors, not panics
        for (cmd, p) in [
            ("sync.find", json!({})),
            ("sync.pair", json!({"device": ""})),
            ("sync.confirm", json!({"accept": "yes"})),
            ("sync.unpair", json!({"device": "nobody"})),
        ] {
            assert!(a.execute(cmd, &p).is_err(), "{cmd}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn write_png(path: &std::path::Path) {
        let (w, h) = (16usize, 12usize);
        let data: Vec<[u8; 4]> = (0..w * h).map(|i| [(i % w * 9) as u8, (i / w * 13) as u8, 7, 255]).collect();
        let img = lcc_raster::Rgba8 { width: w, height: h, data };
        let bytes = lcc_codecs::encode_png(&lcc_codecs::EncodeImage::rgba8(&img), &lcc_codecs::EncodeMeta::default()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    /// Ticks both sessions (as their UIs would) until `done`.
    fn tick_until(a: &mut Session, b: &mut Session, what: &str, done: impl Fn(&Session, &Session) -> bool) {
        let t = Instant::now();
        while !done(a, b) {
            a.execute("sync.tick", &json!({})).unwrap();
            b.execute("sync.tick", &json!({})).unwrap();
            assert!(t.elapsed() < Duration::from_secs(60), "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn only_master(s: &Session) -> lcc_catalog::PhotoId {
        s.catalog.photos().find(|p| p.copy_of.is_none()).map(|p| p.id).unwrap()
    }

    /// The whole flow on two libraries holding the same photo: share, accept the offer, an edit
    /// on one side arrives on the other, and edits on both sides end as a copy in a stack.
    #[test]
    fn shared_libraries_exchange_edits_and_keep_both_sides_of_a_collision() {
        // macOS LAN pairing check: discovery times out on the macOS CI runner; checked in person on
        // real Macs instead (see "Checked in person, not on CI" in docs/ci.md)
        if cfg!(target_os = "macos") && std::env::var_os("CI").is_some() {
            lcc_testkit::skip!(System, "LAN discovery on the macOS CI runner (macOS LAN pairing check, tested in person)");
            return;
        }
        let dir = std::env::temp_dir().join(format!("lcc-engine-exchange-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        let photo = dir.join("src/p.png");
        write_png(&photo);
        let port = 31000 + (std::process::id() % 9000) as u16;
        let open = |name: &str| {
            let mut s = session(&dir, name, port);
            s = s.with_fs();
            s.open_library(dir.join(format!("{name}-lib")), false).unwrap();
            s.execute("library.import", &json!({"paths": [photo.to_string_lossy()]})).unwrap();
            s
        };
        let (mut a, mut b) = (open("a"), open("b"));
        // pair
        a.execute("sync.find", &json!({"on": true})).unwrap();
        b.execute("sync.find", &json!({"on": true})).unwrap();
        let b_id = b.execute("sync.status", &json!({})).unwrap()["deviceId"].as_str().unwrap().to_string();
        let a_id = a.execute("sync.status", &json!({})).unwrap()["deviceId"].as_str().unwrap().to_string();
        until(&mut a, "a to find b", |v| v["found"].as_array().is_some_and(|f| f.iter().any(|d| d["deviceId"] == b_id.as_str())));
        a.execute("sync.pair", &json!({"device": b_id})).unwrap();
        until(&mut a, "a's code", |v| v["pairing"]["stage"] == "confirm");
        until(&mut b, "b's code", |v| v["pairing"]["stage"] == "confirm");
        a.execute("sync.confirm", &json!({"accept": true})).unwrap();
        b.execute("sync.confirm", &json!({"accept": true})).unwrap();
        until(&mut a, "paired", |v| v["paired"].as_array().is_some_and(|p| p.len() == 1));
        until(&mut b, "paired", |v| v["paired"].as_array().is_some_and(|p| p.len() == 1));

        // a shares; b sees the offer and takes it up
        a.execute("sync.share", &json!({"device": b_id, "on": true})).unwrap();
        tick_until(&mut a, &mut b, "the offer", |_, b| b.peers.hub.as_ref().is_some_and(|h| !lock(&h.offers).is_empty()));
        b.execute("sync.answerOffer", &json!({"device": a_id, "accept": true})).unwrap();

        // an edit on a reaches b
        let pa = only_master(&a);
        a.execute("photo.rate", &json!({"ids": [pa.0], "rating": 4})).unwrap();
        let mut d = (*a.catalog.photo(pa).unwrap().develop).clone();
        d.light.exposure = 1.0;
        a.set_develop(pa, d, "test").unwrap();
        tick_until(&mut a, &mut b, "b to take a's edit", |_, b| {
            let p = b.catalog.photo(only_master(b)).unwrap();
            p.rating == 4 && p.develop.light.exposure == 1.0
        });
        assert_eq!(b.catalog.photos().count(), 1, "no copy for a one-sided edit");

        // both edit: each keeps its own and gains the other's as a stacked copy
        let pb = only_master(&b);
        let mut da = (*a.catalog.photo(pa).unwrap().develop).clone();
        da.light.exposure = 2.0;
        a.set_develop(pa, da, "test").unwrap();
        let mut db = (*b.catalog.photo(pb).unwrap().develop).clone();
        db.light.exposure = -2.0;
        b.set_develop(pb, db, "test").unwrap();
        tick_until(&mut a, &mut b, "copies on both sides", |a, b| a.catalog.photos().count() == 2 && b.catalog.photos().count() == 2);
        assert_eq!(a.catalog.photo(pa).unwrap().develop.light.exposure, 2.0);
        assert_eq!(b.catalog.photo(pb).unwrap().develop.light.exposure, -2.0);
        let copy = a.catalog.photos().find(|p| p.copy_of == Some(pa)).unwrap();
        assert_eq!(copy.develop.light.exposure, -2.0);
        assert!(a.catalog.stack_of(pa).is_some(), "the copy is stacked with its original");
        // and it settles: more rounds make no more copies
        for _ in 0..3 {
            for s in [&a, &b] {
                if let Some(h) = &s.peers.hub {
                    lock(&h.tried).clear();
                }
            }
            std::thread::sleep(Duration::from_millis(300));
            a.execute("sync.tick", &json!({})).unwrap();
            b.execute("sync.tick", &json!({})).unwrap();
        }
        std::thread::sleep(Duration::from_millis(500));
        a.execute("sync.tick", &json!({})).unwrap();
        b.execute("sync.tick", &json!({})).unwrap();
        assert_eq!((a.catalog.photos().count(), b.catalog.photos().count()), (2, 2));
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn device_id(s: &mut Session) -> String {
        s.execute("sync.status", &json!({})).unwrap()["deviceId"].as_str().unwrap().to_string()
    }

    /// Pairs two looking sessions through the commands.
    fn pair_up(a: &mut Session, b: &mut Session) {
        let b_id = device_id(b);
        until(a, "a to find b", |v| v["found"].as_array().is_some_and(|f| f.iter().any(|d| d["deviceId"] == b_id.as_str())));
        a.execute("sync.pair", &json!({"device": b_id})).unwrap();
        until(a, "a's code", |v| v["pairing"]["stage"] == "confirm");
        until(b, "b's code", |v| v["pairing"]["stage"] == "confirm");
        a.execute("sync.confirm", &json!({"accept": true})).unwrap();
        b.execute("sync.confirm", &json!({"accept": true})).unwrap();
        until(a, "a paired", |v| v["paired"].as_array().is_some_and(|p| p.iter().any(|d| d["deviceId"] == b_id.as_str())));
        a.execute("sync.clearPairing", &json!({})).unwrap();
        b.execute("sync.clearPairing", &json!({})).unwrap();
    }

    /// Edits travel along a chain: A is paired with B, B with C (never A with C); an edit made
    /// on A reaches C through B, and one made on C reaches A the same way.
    #[test]
    fn edits_travel_across_a_chain_of_devices() {
        // macOS LAN pairing check: discovery times out on the macOS CI runner; checked in person on
        // real Macs instead (see "Checked in person, not on CI" in docs/ci.md)
        if cfg!(target_os = "macos") && std::env::var_os("CI").is_some() {
            lcc_testkit::skip!(System, "LAN discovery on the macOS CI runner (macOS LAN pairing check, tested in person)");
            return;
        }
        let dir = std::env::temp_dir().join(format!("lcc-engine-chain-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        let photo = dir.join("src/p.png");
        write_png(&photo);
        let port = 22000 + (std::process::id() % 8000) as u16;
        let open = |name: &str| {
            let mut s = session(&dir, name, port).with_fs();
            s.open_library(dir.join(format!("{name}-lib")), false).unwrap();
            s.execute("library.import", &json!({"paths": [photo.to_string_lossy()]})).unwrap();
            s.execute("sync.find", &json!({"on": true})).unwrap();
            s
        };
        let (mut a, mut b, mut c) = (open("a"), open("b"), open("c"));
        pair_up(&mut a, &mut b);
        pair_up(&mut b, &mut c);
        let (a_id, b_id, c_id) = (device_id(&mut a), device_id(&mut b), device_id(&mut c));
        let tick_all = |a: &mut Session, b: &mut Session, c: &mut Session| {
            for s in [a, b, c] {
                s.execute("sync.tick", &json!({})).unwrap();
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        let wait = |what: &str, t: Instant| assert!(t.elapsed() < Duration::from_secs(60), "timed out waiting for {what}");
        // A shares with B; B takes it up and shares the same catalog with C, who takes it up
        a.execute("sync.share", &json!({"device": b_id, "on": true})).unwrap();
        let t = Instant::now();
        while b.peers.hub.as_ref().is_none_or(|h| lock(&h.offers).is_empty()) {
            tick_all(&mut a, &mut b, &mut c);
            wait("b's offer", t);
        }
        b.execute("sync.answerOffer", &json!({"device": a_id, "accept": true})).unwrap();
        b.execute("sync.share", &json!({"device": c_id, "on": true})).unwrap();
        let t = Instant::now();
        while c.peers.hub.as_ref().is_none_or(|h| lock(&h.offers).is_empty()) {
            tick_all(&mut a, &mut b, &mut c);
            wait("c's offer", t);
        }
        c.execute("sync.answerOffer", &json!({"device": b_id, "accept": true})).unwrap();

        let pa = only_master(&a);
        a.execute("photo.rate", &json!({"ids": [pa.0], "rating": 5})).unwrap();
        let t = Instant::now();
        while c.catalog.photo(only_master(&c)).unwrap().rating != 5 {
            tick_all(&mut a, &mut b, &mut c);
            wait("A's rating at C", t);
        }
        let pc = only_master(&c);
        let mut d = (*c.catalog.photo(pc).unwrap().develop).clone();
        d.light.exposure = 0.7;
        c.set_develop(pc, d, "test").unwrap();
        let t = Instant::now();
        while a.catalog.photo(pa).unwrap().develop.light.exposure != 0.7 {
            tick_all(&mut a, &mut b, &mut c);
            wait("C's edit at A", t);
        }
        assert_eq!([a.catalog.photos().count(), b.catalog.photos().count(), c.catalog.photos().count()], [1, 1, 1], "no copies for one-sided edits");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A merge that fails part-way still saves the base for the photos it did merge, so the next
    /// exchange doesn't see their applied edits as fresh collisions (a second, spurious copy).
    #[test]
    fn a_failed_merge_keeps_the_base_of_what_it_merged() {
        let dir = std::env::temp_dir().join(format!("lcc-engine-merge-fail-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        let (p1, p2) = (dir.join("src/a.png"), dir.join("src/b.png"));
        write_png(&p1);
        // a second original with other bytes (keys are content hashes)
        let mut bytes = std::fs::read(&p1).unwrap();
        bytes.extend_from_slice(b"different");
        std::fs::write(&p2, bytes).unwrap();
        let lib = dir.join("lib");
        let mut s = Session::new().with_fs();
        s.open_library(lib.clone(), false).unwrap();
        s.execute("library.import", &json!({"paths": [p1.to_string_lossy(), p2.to_string_lossy()]})).unwrap();
        let recs = px::records(&s.catalog);
        assert_eq!(recs.len(), 2);
        let mut keys: Vec<String> = recs.keys().cloned().collect();
        keys.sort();
        let (k1, k2) = (keys[0].clone(), keys[1].clone());
        let (id1, id2) = (recs[&k1].0, recs[&k2].0);
        // k1 edited here too (both sides changed: a copy); k2 untouched here (taken)
        s.execute("photo.rate", &json!({"ids": [id1.0], "rating": 2})).unwrap();
        LibrarySync { sync_id: Some("cat".into()), ..Default::default() }.save(&lib).unwrap();
        let peer = lcc_peer::Peer { device_id: "ab12".into(), name: "Other".into(), public_key: String::new(), paired_at: 0 };
        let theirs = Record { rating: 3, ..recs[&k2].1.clone() };
        let outcome = |s: &Session| px::Outcome {
            sync_id: Some("cat".into()),
            remote: [(k1.clone(), theirs.digest()), (k2.clone(), theirs.digest())].into(),
            records: [(k1.clone(), theirs.clone()), (k2.clone(), theirs.clone())].into(),
            sent: px::records(&s.catalog).iter().map(|(k, (_, r))| (k.clone(), r.digest())).collect(),
            ..Default::default()
        };
        let o = outcome(&s);
        let r = merge_with(&mut s, &peer, &lib, o, &mut |s, id, r, l| {
            if id == id2 { Err(EngineError::Other("disk full".into())) } else { apply_record(s, id, r, l) }
        });
        assert!(r.is_err(), "the failure is reported");
        assert_eq!(s.catalog.photos().count(), 3, "k1 got its copy");
        let base = Base::load(&lib, &peer.device_id, "cat");
        assert_eq!(base.remote.get(&k1), Some(&theirs.digest()), "k1's merge is in the base");
        assert!(!base.remote.contains_key(&k2), "k2's failed merge is not");
        assert!(base.conflicts.contains(&k1), "the collision will be told");
        // the same exchange again: k1 settles (no second copy), k2 is taken now
        let o = outcome(&s);
        assert_eq!(merge(&mut s, &peer, &lib, o).unwrap(), (1, 0));
        assert_eq!(s.catalog.photos().count(), 3, "no spurious second copy");
        assert_eq!(s.catalog.photo(id2).unwrap().rating, 3);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
