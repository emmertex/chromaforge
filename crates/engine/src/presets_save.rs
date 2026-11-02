//! `presets.json` saved off the UI thread.
//!
//! A Lightroom collection imported whole is 10 000+ presets and ~40 MB of JSON. Every change to
//! the preset list, the favourite profiles or the recently applied ones (each profile applied
//! from the console or the browser) serialized, hashed and wrote all of it on the UI thread
//! before the next frame: the app froze for the length of a 40 MB save (TEST-NOTES 2026-10-16,
//! the 10 s frame). Now the session hands a snapshot (the list is shared copy-on-write: no copy)
//! to one background thread, which writes the newest one it has (older ones still waiting are
//! dropped) and skips a write whose bytes are those already on disk.
//!
//! Only for stores that can be written from another thread ([`Store::background_writer`]) and
//! sessions that ask for it ([`crate::Session::background_saves`], the desktop app); everything
//! else saves as before, at once. Closing the library and dropping the session wait for the
//! save in progress. A failed background save is reported to the next persist, which marks the
//! presets unsaved (warning, backups refuse) and saves them again.
//!
//! Ported from OneSlips' fork (PERF-UI-STALL).

use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Duration;

use lcc_catalog::Store;
use lcc_develop::Preset;
use serde::Serialize;

/// What `presets.json` holds, as of a moment (cheap to take: the list is shared).
pub(crate) struct Snapshot {
    items: Arc<Vec<Preset>>,
    folders: Vec<String>,
    profile_favorites: Vec<String>,
    profile_recent: Vec<String>,
    at_import: Vec<String>,
    hidden: Vec<String>,
    builtin_names: std::collections::BTreeMap<String, String>,
    builtin_groups: std::collections::BTreeMap<String, String>,
    profile_hidden: Vec<String>,
    body_profiles: std::collections::BTreeMap<String, String>,
}

/// `presets.json`'s shape, borrowed (the same JSON as `library::PresetsFile`, field for field).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FileRef<'a> {
    user: Vec<&'a Preset>,
    folders: &'a [String],
    favorites: Vec<&'a str>,
    #[serde(skip_serializing_if = "<[String]>::is_empty")]
    at_import: &'a [String],
    #[serde(skip_serializing_if = "<[String]>::is_empty")]
    hidden: &'a [String],
    #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    builtin_names: &'a std::collections::BTreeMap<String, String>,
    #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    builtin_groups: &'a std::collections::BTreeMap<String, String>,
    profile_favorites: &'a [String],
    profile_recent: &'a [String],
    #[serde(skip_serializing_if = "<[String]>::is_empty")]
    profile_hidden: &'a [String],
    #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    body_profiles: &'a std::collections::BTreeMap<String, String>,
}

impl Snapshot {
    pub(crate) fn of(s: &crate::Session) -> Snapshot {
        Snapshot {
            items: s.presets.snapshot(),
            folders: s.preset_folders.clone(),
            profile_favorites: s.profile_favorites.clone(),
            profile_recent: s.profile_recent.clone(),
            at_import: s.preset_at_import.clone(),
            hidden: s.preset_hidden.clone(),
            builtin_names: s.preset_builtin_names.clone(),
            builtin_groups: s.preset_builtin_groups.clone(),
            profile_hidden: s.profile_hidden.clone(),
            body_profiles: s.body_profiles.clone(),
        }
    }

    /// The file's text.
    pub(crate) fn json(&self) -> String {
        let f = FileRef {
            user: self.items.iter().filter(|p| !p.builtin).collect(),
            folders: &self.folders,
            favorites: self.items.iter().filter(|p| p.builtin && p.favorite).map(|p| p.id.as_str()).collect(),
            at_import: &self.at_import,
            hidden: &self.hidden,
            builtin_names: &self.builtin_names,
            builtin_groups: &self.builtin_groups,
            profile_favorites: &self.profile_favorites,
            profile_recent: &self.profile_recent,
            profile_hidden: &self.profile_hidden,
            body_profiles: &self.body_profiles,
        };
        serde_json::to_string_pretty(&f).unwrap_or_default()
    }
}

struct Job {
    snapshot: Snapshot,
    store: Box<dyn Store>,
    name: &'static str,
    /// Where it goes (the store's place and the file's name): the last hash is kept per target.
    target: String,
}

#[derive(Default)]
struct State {
    next: Option<Job>,
    busy: bool,
    stop: bool,
    /// Hash of what each target holds (written by this saver, or known when it started).
    hashes: HashMap<String, u64>,
    /// The last save that failed (taken by [`Saver::take_error`]).
    error: Option<String>,
    /// Files written so far (tests).
    writes: u64,
}

struct Shared {
    state: Mutex<State>,
    cv: Condvar,
}

/// The background saver (one per session, started on its first save).
pub(crate) struct Saver {
    shared: Arc<Shared>,
    worker: Option<std::thread::JoinHandle<()>>,
}

/// How long closing waits for a save in progress.
const FLUSH_WAIT: Duration = Duration::from_secs(20);

impl Saver {
    /// A saver with its thread, or `None` when no thread can start (the caller saves at once).
    pub(crate) fn start() -> Option<Saver> {
        let shared = Arc::new(Shared { state: Mutex::new(State::default()), cv: Condvar::new() });
        let worker = {
            let shared = Arc::clone(&shared);
            std::thread::Builder::new().name("presets-save".into()).spawn(move || run(&shared)).ok()?
        };
        Some(Saver { shared, worker: Some(worker) })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.shared.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Save `snapshot` as `name` through `store` (a handle a worker may write through); `known` is
    /// the hash of what the file holds, when the caller knows it. Replaces a save still waiting.
    pub(crate) fn save(&self, snapshot: Snapshot, store: Box<dyn Store>, name: &'static str, known: u64) {
        let target = format!("{}/{name}", store.describe());
        let mut st = self.lock();
        if known != 0 {
            st.hashes.entry(target.clone()).or_insert(known);
        }
        st.next = Some(Job { snapshot, store, name, target });
        drop(st);
        self.shared.cv.notify_all();
    }

    /// Why the last background save failed (once): the caller saves again.
    pub(crate) fn take_error(&self) -> Option<String> {
        self.lock().error.take()
    }

    /// Wait (up to `timeout`) until nothing is waiting or being written. Whether that happened.
    pub(crate) fn flush(&self, timeout: Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        let mut st = self.lock();
        while st.busy || st.next.is_some() {
            let Some(left) = deadline.checked_duration_since(std::time::Instant::now()).filter(|d| !d.is_zero()) else { return false };
            st = self.shared.cv.wait_timeout(st, left).unwrap_or_else(PoisonError::into_inner).0;
        }
        true
    }

    /// Files written so far (tests).
    #[cfg(test)]
    pub(crate) fn writes(&self) -> u64 {
        self.lock().writes
    }
}

fn run(shared: &Shared) {
    loop {
        let job = {
            let mut st = shared.state.lock().unwrap_or_else(PoisonError::into_inner);
            loop {
                if let Some(j) = st.next.take() {
                    st.busy = true;
                    break j;
                }
                if st.stop {
                    return;
                }
                st = shared.cv.wait(st).unwrap_or_else(PoisonError::into_inner);
            }
        };
        let Job { snapshot, mut store, name, target } = job;
        let json = snapshot.json();
        drop(snapshot);
        let hash = crate::library::bytes_hash(json.as_bytes());
        let same = shared.state.lock().unwrap_or_else(PoisonError::into_inner).hashes.get(&target) == Some(&hash);
        let result = if same { Ok(false) } else { store.write_atomic(name, json.as_bytes()).map(|()| true) };
        let mut st = shared.state.lock().unwrap_or_else(PoisonError::into_inner);
        match result {
            Ok(wrote) => {
                st.hashes.insert(target, hash);
                st.writes += u64::from(wrote);
            }
            Err(e) => {
                log::error!("presets: {name} not saved: {e}");
                st.hashes.remove(&target);
                st.error = Some(format!("{name}: {e}"));
            }
        }
        st.busy = false;
        drop(st);
        shared.cv.notify_all();
    }
}

impl Drop for Saver {
    fn drop(&mut self) {
        let done = self.flush(FLUSH_WAIT);
        self.lock().stop = true;
        self.shared.cv.notify_all();
        if done && let Some(w) = self.worker.take() {
            let _ = w.join();
        } else if !done {
            log::warn!("presets: a save was still running when the session closed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preset(i: usize) -> Preset {
        Preset {
            id: format!("user.p{i}"),
            name: format!("P{i}"),
            group: "G".into(),
            settings: serde_json::json!({"light": {"exposure": i as f64 / 10.0}}),
            favorite: i.is_multiple_of(3),
            builtin: false,
        }
    }

    fn snapshot(n: usize, recent: &[&str]) -> Snapshot {
        Snapshot {
            items: Arc::new((0..n).map(preset).collect()),
            folders: vec!["F".into()],
            profile_favorites: vec!["a".into()],
            profile_recent: recent.iter().map(|s| s.to_string()).collect(),
            at_import: Vec::new(),
            hidden: Vec::new(),
            builtin_names: Default::default(),
            builtin_groups: Default::default(),
            profile_hidden: Vec::new(),
            body_profiles: Default::default(),
        }
    }

    /// The borrowed form writes what the owned file type reads back.
    #[test]
    fn a_snapshot_writes_the_presets_file_format() {
        let s = snapshot(3, &["x", "y"]);
        let back: serde_json::Value = serde_json::from_str(&s.json()).unwrap();
        let user = back["user"].as_array().unwrap();
        assert_eq!(user.len(), 3);
        assert_eq!(back["folders"], serde_json::json!(["F"]));
        assert_eq!(back["profileRecent"], serde_json::json!(["x", "y"]));
        assert_eq!(back["profileFavorites"], serde_json::json!(["a"]));
        let _: lcc_develop::Preset = serde_json::from_value(user[0].clone()).unwrap();
    }

    /// Saves run on the worker, the newest wins, a save of the same bytes writes nothing, and a
    /// failed one is reported for the caller to retry.
    #[test]
    fn saves_happen_in_the_background_newest_first_and_skip_unchanged_bytes() {
        let mem = lcc_catalog::MemStore { background: true, ..Default::default() };
        let saver = Saver::start().unwrap();
        saver.save(snapshot(5, &["one"]), Box::new(mem.clone()), "presets.json", 0);
        saver.save(snapshot(6, &["two"]), Box::new(mem.clone()), "presets.json", 0);
        assert!(saver.flush(Duration::from_secs(10)));
        let text = String::from_utf8(mem.get("presets.json").unwrap()).unwrap();
        assert!(text.contains("\"two\""), "the newest snapshot is on disk");
        let written = saver.writes();
        saver.save(snapshot(6, &["two"]), Box::new(mem.clone()), "presets.json", 0);
        assert!(saver.flush(Duration::from_secs(10)));
        assert_eq!(saver.writes(), written, "the same bytes again are not written");
        assert!(saver.take_error().is_none());
    }
}
