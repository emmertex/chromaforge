//! Updating a copy unpacked from the portable zip in place: unpack the new zip beside the program,
//! swap its files in, start the new version once the old one has quit, and keep the old files until
//! the new version has shown it works.
//!
//! Windows doesn't let a running program's file be overwritten or deleted, but it does let it be
//! renamed. So the running process swaps its own files, with no script or helper process:
//!
//! 1. The zip is unpacked into a fresh folder beside the program (`.<name>-update-<pid>`, on the same
//!    drive, so each later move is a rename). Refused before anything moves: an entry that would land
//!    outside it, a name Windows can't hold as given ([`safe_name`]: `:` streams, `\`, a trailing dot
//!    or space, device names such as `NUL`), a link, two entries for one file, a zip without the
//!    program, more than [`MAX_ENTRIES`] files or [`MAX_UNPACKED`] bytes. The zip's single top folder
//!    (the release zips have one) is dropped.
//! 2. A manifest beside the program (`.<name>-update.json`, [`manifest_path`]) records the files
//!    before the first one moves, and the swapping process holds an OS lock (on a file of its own,
//!    `.<name>-update.json.lock`) until it is done. Each write replaces the manifest whole (a
//!    temporary file renamed over it), so a write cut off half way leaves the previous contents.
//!    Each file is then renamed to `<file>.update-old` ([`OLD_SUFFIX`]) and the new one renamed in,
//!    the program last. Only the zip's files are touched: `portable.txt`, the portable data folder
//!    and anything else the person keeps there stay as they are. A rename refused for a moment
//!    (antivirus software holds new files briefly) is retried; if one still fails, every file moved
//!    so far is put back.
//! 3. The new program is started with `<PREFIX>_AFTER_UPDATE` set ([`AFTER_UPDATE`]) and a pipe as
//!    its standard input, which the old process holds until it ends (for any reason, a crash too).
//!    The new one waits for the pipe to close before it does anything ([`on_start`]): if quitting is
//!    cancelled, it starts when the old window does close.
//! 4. The new version counts its start in the manifest and holds the manifest's lock until it is up
//!    (its first frame, with the library open); then it confirms the update ([`confirm`]): only then
//!    are the `.update-old` files deleted. A start that finds an earlier, counted start of the new
//!    version that ended without getting that far (its lock gone with it) puts the previous files
//!    back, starts the previous version and ends ([`on_start`]). A start while the counted one is
//!    still coming up (the lock held) leaves the update alone.
//!
//! [`clean_up`] (each start, in the background) finishes what is left: `.update-old` files that
//! were still in use when the update was confirmed, the `.update-failed` files of an update that was
//! undone, and unpack folders and probes of a process that ended half way. It acts only on the
//! manifest's files and the program's own folder (never deeper), and not while another process
//! holds the manifest's lock. A manifest that can't be read is deleted, so it never stops later
//! updates.
//!
//! The steps use only renames, file copies and std's file locks, so they run (and are tested) on
//! every system; only Windows uses them.

use std::fs::{self, File, TryLockError};
use std::io::{self, BufReader, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::error::UpdateError;

/// What a replaced file is renamed to until the new version confirms the update.
pub const OLD_SUFFIX: &str = ".update-old";

/// What a file of an update that was undone is renamed to until the next start deletes it.
pub const FAILED_SUFFIX: &str = ".update-failed";

/// The environment variable (`<PREFIX>_AFTER_UPDATE`, through [`lcc_brand::env_var`]) the new
/// version is started with: it waits for the old one to quit. A variable, not an argument, so a
/// version that doesn't know it (an older release) ignores it instead of refusing to start.
pub const AFTER_UPDATE: &str = "AFTER_UPDATE";

/// The environment variable (`<PREFIX>_WINDOW_RETRY`) the app sets when it starts itself again
/// because its window's renderer failed to come up (the app's `window_gpu::RETRY_VAR`): that start
/// carries on the one that started it ([`on_start_again`]).
pub const WINDOW_RETRY: &str = "WINDOW_RETRY";

/// Appended to the manifest's name: the file its lock is held on.
const LOCK_SUFFIX: &str = ".lock";

/// Appended to the manifest's name: the new contents, before they are renamed over it.
const TMP_SUFFIX: &str = ".tmp";

/// The most files a zip may hold (the release zip has about ten).
pub const MAX_ENTRIES: usize = 4096;

/// The most bytes a zip may unpack to.
pub const MAX_UNPACKED: u64 = 4 << 30;

/// The largest manifest read back (it lists at most [`MAX_ENTRIES`] short paths).
const MAX_MANIFEST_BYTES: u64 = 1 << 20;

/// How long a rename or delete that Windows refuses for a moment is retried.
const RETRY_FOR: Duration = Duration::from_secs(3);

/// How long a leftover of the previous version is retried before it is left for the next start: a
/// program that just ended stays locked while Windows tears its process down.
const LEFTOVER_WAIT: Duration = Duration::from_secs(10);
const RETRY_PAUSE: Duration = Duration::from_millis(100);

/// `name` in lower case, for comparing names the way Windows does.
fn folded(name: &str) -> String {
    name.to_lowercase()
}

/// The program's stem (`app` for `app.exe`), which names the updater's own files beside it.
fn stem(exe_name: &str) -> String {
    Path::new(exe_name).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()
}

/// The unpack folder's name prefix for the program `exe_name` (`.app-update-`).
fn stage_prefix(exe_name: &str) -> String {
    format!(".{}-update-", stem(exe_name))
}

/// The manifest of the swap of the copy in `dir` whose program is `exe_name` (`.app-update.json`).
pub fn manifest_path(dir: &Path, exe_name: &str) -> PathBuf {
    dir.join(format!(".{}-update.json", stem(exe_name)))
}

/// `file` with `suffix` appended to its name (`app.exe.update-old`).
fn with_suffix(file: &Path, suffix: &str) -> PathBuf {
    let mut name = file.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(suffix);
    file.with_file_name(name)
}

fn old_path(file: &Path) -> PathBuf {
    with_suffix(file, OLD_SUFFIX)
}

fn failed_path(file: &Path) -> PathBuf {
    with_suffix(file, FAILED_SUFFIX)
}

fn install_error(what: impl std::fmt::Display) -> UpdateError {
    UpdateError::Install(what.to_string())
}

/// Whether files can be created in `dir` (a file created and deleted again).
pub fn writable(dir: &Path, exe_name: &str) -> bool {
    let probe = dir.join(format!("{}probe-{}", stage_prefix(exe_name), std::process::id()));
    match File::options().write(true).create_new(true).open(&probe) {
        Ok(f) => {
            drop(f);
            let _ = fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

// ---------- names

/// Whether `part` (one component of a path inside the zip) names a file Windows can hold as given:
/// no `\` or `/`, no `:` (a drive, `C:x`, or an alternate data stream, `a.exe:s`), no control
/// characters or the other characters Windows refuses, no trailing dot or space (Windows drops
/// them, so two names would land on one file), and not a device name (`CON`, `NUL`, `COM1`…, with
/// any extension). Checked on every system, so the tests cover it wherever they run.
fn safe_name(part: &str) -> bool {
    const RESERVED: [&str; 4] = ["con", "prn", "aux", "nul"];
    if part.is_empty() || part.ends_with('.') || part.ends_with(' ') {
        return false;
    }
    if part.chars().any(|c| c.is_control() || matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|')) {
        return false;
    }
    let base = folded(part.split('.').next().unwrap_or(part).trim_end());
    let numbered = |prefix: &str| base.strip_prefix(prefix).is_some_and(|n| n.len() == 1 && n.chars().all(|c| c.is_ascii_digit()));
    !(RESERVED.contains(&base.as_str()) || numbered("com") || numbered("lpt"))
}

/// `path` (relative, inside the copy) when every component is a plain, [`safe_name`] name.
fn safe_relative(path: &Path) -> bool {
    path.components().count() > 0 && path.components().all(|c| matches!(c, Component::Normal(n) if n.to_str().is_some_and(safe_name)))
}

/// Whether `rel` (a path inside the zip) is the program itself.
fn is_program(rel: &Path, exe_name: &str) -> bool {
    rel.components().count() == 1 && rel.to_string_lossy().eq_ignore_ascii_case(exe_name)
}

/// Whether `name` is one the updater gives its own files beside the program.
fn updater_name(name: &str, exe_name: &str) -> bool {
    let name = folded(name);
    name.ends_with(OLD_SUFFIX)
        || name.ends_with(FAILED_SUFFIX)
        || name.starts_with(&folded(&stage_prefix(exe_name)))
        // the manifest, its lock and its temporary file
        || name.starts_with(&folded(&format!(".{}-update.json", stem(exe_name))))
}

// ---------- the manifest

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    /// Files are being moved (by the process holding the lock, or one that stopped half way).
    Swapping,
    /// Every file is in place and the new version was started; it hasn't confirmed yet.
    Swapped,
    /// The new version came up: the `.update-old` files are no longer needed.
    Confirmed,
    /// The new version didn't come up and the previous files were put back: the `.update-failed`
    /// files are no longer needed.
    Undone,
}

impl State {
    fn text(self) -> &'static str {
        match self {
            State::Swapping => "swapping",
            State::Swapped => "swapped",
            State::Confirmed => "confirmed",
            State::Undone => "undone",
        }
    }

    fn parse(s: &str) -> Option<State> {
        [State::Swapping, State::Swapped, State::Confirmed, State::Undone].into_iter().find(|st| st.text() == s)
    }
}

/// What one swap moved, and how far it got.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Manifest {
    /// The version swapped from, and to.
    from: String,
    to: String,
    /// The zip's files, relative to the program's folder (the program last).
    files: Vec<PathBuf>,
    /// Those of `files` the copy didn't have before (nothing to put back for them).
    added: Vec<PathBuf>,
    state: State,
    /// How often the new version started without confirming.
    attempts: u32,
}

impl Manifest {
    fn to_json(&self) -> String {
        let text = |list: &[PathBuf]| -> Vec<String> {
            list.iter().map(|p| p.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/")).collect()
        };
        json!({"from": self.from, "to": self.to, "files": text(&self.files), "added": text(&self.added), "state": self.state.text(), "attempts": self.attempts})
            .to_string()
    }

    /// A list of relative paths, each segment a [`safe_name`] (no empty, `..` or rooted part).
    fn paths(v: &Value) -> Option<Vec<PathBuf>> {
        let list = v.as_array()?;
        if list.len() > MAX_ENTRIES {
            return None;
        }
        list.iter()
            .map(|f| {
                let s = f.as_str()?;
                s.split('/').all(safe_name).then(|| s.split('/').collect::<PathBuf>())
            })
            .collect()
    }

    /// The manifest in `text`. It sits in a folder the person can write, so it is read as hostile:
    /// every path must be a [`safe_relative`] one, and at most [`MAX_ENTRIES`] of them.
    fn parse(text: &str) -> Option<Manifest> {
        let v: Value = serde_json::from_str(text).ok()?;
        let files = Manifest::paths(v.get("files")?)?;
        let added = Manifest::paths(v.get("added")?)?;
        if !added.iter().all(|a| files.contains(a)) {
            return None;
        }
        Some(Manifest {
            from: v.get("from")?.as_str()?.to_string(),
            to: v.get("to")?.as_str()?.to_string(),
            files,
            added,
            state: State::parse(v.get("state")?.as_str()?)?,
            attempts: v.get("attempts")?.as_u64().and_then(|n| u32::try_from(n).ok())?,
        })
    }
}

/// The manifest at `path`, locked by this process for as long as it is held. The lock is on a file
/// of its own beside it (`<manifest>.lock`), which is never written: the manifest itself is
/// replaced whole by each write, which a lock on it would not survive.
struct Held {
    lock: File,
    path: PathBuf,
}

/// Lock the lock file of the manifest at `path`: `Ok(None)` when another handle holds it (another
/// process swapping or starting, or another start in this one). The file is created the first time
/// (hidden, empty) and never deleted: a process could still hold a lock on a deleted one while
/// another locks its replacement, and both would think they hold the manifest.
fn lock(path: &Path) -> io::Result<Option<File>> {
    let mut options = File::options();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_ATTRIBUTE_HIDDEN, for a file created now
        options.attributes(0x2);
    }
    let file = options.open(with_suffix(path, LOCK_SUFFIX))?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(e)) => Err(e),
    }
}

impl Held {
    /// Lock the manifest at `path` for a new swap. It must not exist yet.
    fn create(path: &Path) -> io::Result<Held> {
        let lock = lock(path)?.ok_or_else(|| io::Error::new(io::ErrorKind::WouldBlock, "the update's manifest is locked"))?;
        if path.exists() {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, "an update's manifest is already there"));
        }
        Ok(Held { lock, path: path.to_path_buf() })
    }

    /// Lock the manifest at `path`: `Ok(None)` when there is none, or another handle holds it (a
    /// swap running, or a start of the new version still coming up).
    fn open(path: &Path) -> io::Result<Option<Held>> {
        let tmp = with_suffix(path, TMP_SUFFIX);
        if !path.exists() && !tmp.exists() {
            return Ok(None);
        }
        let Some(lock) = lock(path)? else { return Ok(None) };
        // the holder may have finished, and deleted it, meanwhile; or a write was cut off between
        // deleting the manifest and renaming its new contents in (a file system that can't rename
        // over a file in one step, such as FAT): the new contents, written in full, are the manifest
        if !path.exists() {
            let whole = fs::read(&tmp).ok().and_then(|b| String::from_utf8(b).ok()).and_then(|t| Manifest::parse(&t)).is_some();
            if !whole || fs::rename(&tmp, path).is_err() {
                let _ = fs::remove_file(&tmp);
                return Ok(None);
            }
            log::warn!("updates: {} was restored from its last write", path.display());
        }
        Ok(Some(Held { lock, path: path.to_path_buf() }))
    }

    /// The manifest's contents: `Ok(None)` when they don't read as one.
    fn read(&self) -> io::Result<Option<Manifest>> {
        let mut bytes = Vec::new();
        File::open(&self.path)?.take(MAX_MANIFEST_BYTES).read_to_end(&mut bytes)?;
        Ok(String::from_utf8(bytes).ok().and_then(|text| Manifest::parse(&text)))
    }

    /// Replace the manifest with `m`: written in full to a temporary file, then renamed over it, so
    /// the manifest holds either the previous contents or the new ones, never a part.
    fn write(&self, m: &Manifest) -> io::Result<()> {
        let text = m.to_json();
        // one that couldn't be read back whole would be deleted, and the update with it
        if text.len() as u64 > MAX_MANIFEST_BYTES {
            return Err(io::Error::other(format!("the update lists more than {} KB of file names", MAX_MANIFEST_BYTES >> 10)));
        }
        let tmp = with_suffix(&self.path, TMP_SUFFIX);
        let mut file = File::create(&tmp)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        drop(file);
        retry(|| fs::rename(&tmp, &self.path))
    }

    /// Delete the manifest (and a temporary file of it), then let go of the lock.
    fn remove(self) {
        let Held { lock, path } = self;
        let _ = fs::remove_file(with_suffix(&path, TMP_SUFFIX));
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => log::warn!("updates: could not delete {}: {e}", path.display()),
        }
        drop(lock);
    }
}

/// The manifest `held` holds, with its contents. One that can't be read as a manifest (edited by
/// hand, or cut off on a file system that can't rename over a file) is deleted: nothing can be done
/// with it, and left in place it would refuse every later update.
fn contents(held: Held) -> Option<(Held, Manifest)> {
    match held.read() {
        Ok(Some(m)) => Some((held, m)),
        Ok(None) => {
            log::warn!("updates: {} is not an update manifest that can be read; deleted", held.path.display());
            held.remove();
            None
        }
        Err(e) => {
            log::warn!("updates: could not read {}: {e}", held.path.display());
            None
        }
    }
}

/// The manifests of the updates whose start this process counted, held (so locked) until it
/// confirms: while one is held, another start of the copy (one by hand, or the renderer's relaunch)
/// sees a start still coming up, not one that failed. When the process ends, however it ends, the
/// lock goes with it. A list keyed by the manifest's path, so tests in other folders don't share it.
static RUNNING: Mutex<Vec<Held>> = Mutex::new(Vec::new());

fn keep_running(held: Held) {
    RUNNING.lock().unwrap_or_else(PoisonError::into_inner).push(held);
}

fn take_running(path: &Path) -> Option<Held> {
    let mut list = RUNNING.lock().unwrap_or_else(PoisonError::into_inner);
    let i = list.iter().position(|h| h.path == path)?;
    Some(list.swap_remove(i))
}

// ---------- the swap

/// Replace the files of the copy in `dir` (whose program is `exe_name`, at version `from`) with
/// those in `zip` (version `to`), then start the new program through `start`. `Ok(n)`: `n` files
/// were replaced or added and the new version is starting; the caller quits. `Err`: nothing in
/// `dir` changed.
pub fn swap_in(
    zip: &Path,
    dir: &Path,
    exe_name: &str,
    from: &str,
    to: &str,
    start: &mut dyn FnMut(&Path) -> Result<(), String>,
) -> Result<usize, UpdateError> {
    // an earlier update still being finished: its leftovers first, and nothing new while it isn't done
    let manifest = manifest_path(dir, exe_name);
    if manifest.exists() {
        clean_up(dir, exe_name);
        if manifest.exists() {
            return Err(install_error(
                "the last update of this copy isn't finished (a file of it is still in use); quit every copy of the app, start it again, then try again",
            ));
        }
    }
    let stage = dir.join(format!("{}{}", stage_prefix(exe_name), std::process::id()));
    if stage.exists() {
        fs::remove_dir_all(&stage).map_err(|e| install_error(format!("could not clear {}: {e}", stage.display())))?;
    }
    fs::create_dir(&stage).map_err(|e| install_error(format!("could not unpack the update beside the program ({}): {e}", stage.display())))?;
    let result = unpack(zip, &stage, exe_name).and_then(|files| swap_and_start(&stage, dir, exe_name, files, from, to, start));
    if let Err(e) = fs::remove_dir_all(&stage) {
        log::warn!("updates: could not delete {}: {e}", stage.display());
    }
    result
}

/// The swap itself, under the manifest's lock; everything is put back on any failure.
fn swap_and_start(
    stage: &Path,
    dir: &Path,
    exe_name: &str,
    files: Vec<PathBuf>,
    from: &str,
    to: &str,
    start: &mut dyn FnMut(&Path) -> Result<(), String>,
) -> Result<usize, UpdateError> {
    let path = manifest_path(dir, exe_name);
    let held = Held::create(&path).map_err(|e| install_error(format!("could not write {}: {e}", path.display())))?;
    let added = files.iter().filter(|f| !dir.join(f).exists()).cloned().collect();
    let mut m = Manifest { from: from.to_string(), to: to.to_string(), files, added, state: State::Swapping, attempts: 0 };
    if let Err(e) = held.write(&m) {
        held.remove();
        return Err(install_error(format!("could not write {}: {e}", path.display())));
    }
    let mut moved = Vec::new();
    let swapped = swap(stage, dir, &m.files, &mut moved)
        .and_then(|()| {
            m.state = State::Swapped;
            held.write(&m).map_err(|e| install_error(format!("could not write {}: {e}", path.display())))
        })
        .and_then(|()| {
            let exe =
                m.files.iter().find(|f| is_program(f, exe_name)).map(|f| dir.join(f)).ok_or_else(|| install_error("the update has no program"))?;
            start(&exe).map_err(|e| install_error(format!("the new version could not be started ({e})")))
        });
    match swapped {
        Ok(()) => {
            // the lock goes; the manifest stays for the new version
            drop(held);
            Ok(m.files.len())
        }
        Err(e) => {
            let problems = roll_back(&moved);
            held.remove();
            let e = match e {
                UpdateError::Install(why) => why,
                other => other.to_string(),
            };
            if problems.is_empty() {
                Err(install_error(format!("{e}; this copy was left as it was")))
            } else {
                Err(install_error(format!("{e}; putting the previous files back also failed: {}", problems.join("; "))))
            }
        }
    }
}

/// Unpack `zip` into `stage`, its single top folder dropped; the files' paths, relative to `stage`,
/// the program last.
fn unpack(zip: &Path, stage: &Path, exe_name: &str) -> Result<Vec<PathBuf>, UpdateError> {
    let file = File::open(zip).map_err(|e| install_error(format!("could not open {}: {e}", zip.display())))?;
    let mut archive =
        zip::ZipArchive::new(BufReader::new(file)).map_err(|e| install_error(format!("{} is not a zip that can be read: {e}", zip.display())))?;
    if archive.len() > MAX_ENTRIES {
        return Err(install_error(format!("the update zip holds {} entries, more than an update has", archive.len())));
    }
    // the files, by index, and where each goes; folders are made as the files need them
    let mut entries: Vec<(usize, PathBuf)> = Vec::new();
    for i in 0..archive.len() {
        let entry = archive.by_index(i).map_err(|e| install_error(format!("the update zip is damaged: {e}")))?;
        let name = entry.name().map_err(|e| install_error(format!("the update zip is damaged: {e}")))?;
        let Some(path) = entry.enclosed_name() else {
            return Err(install_error(format!("the update zip names a file outside its folder ({name:?})")));
        };
        // the name as stored too: `enclosed_name` reads it with this system's rules, under which
        // `C:x` or `a\b` can look harmless on one system and not on another
        if !name.split('/').filter(|part| !part.is_empty()).all(safe_name) {
            return Err(install_error(format!("the update zip names a file Windows can't hold as given ({name:?})")));
        }
        if entry.is_symlink() {
            return Err(install_error(format!("the update zip holds a link ({name:?})")));
        }
        if entry.is_dir() || path.components().count() == 0 {
            continue;
        }
        if !safe_relative(&path) {
            return Err(install_error(format!("the update zip names a file Windows can't hold as given ({:?})", entry.name())));
        }
        entries.push((i, path));
    }
    let top = single_top_folder(entries.iter().map(|(_, p)| p.as_path()));
    let files: Vec<(usize, PathBuf)> =
        entries.into_iter().map(|(i, p)| (i, top.as_ref().and_then(|t| p.strip_prefix(t).ok()).map(Path::to_path_buf).unwrap_or(p))).collect();
    if !files.iter().any(|(_, p)| is_program(p, exe_name)) {
        return Err(install_error(format!("the update zip has no {exe_name} at its top")));
    }
    if let Some((_, p)) = files.iter().find(|(_, p)| p.components().any(|c| updater_name(&c.as_os_str().to_string_lossy(), exe_name))) {
        return Err(install_error(format!("the update zip holds {} (a name the updater uses itself)", p.display())));
    }
    // one file twice (or two names Windows takes for one, differing only in case)
    let mut seen = std::collections::HashSet::new();
    if let Some((_, p)) = files.iter().find(|(_, p)| !seen.insert(folded(&p.to_string_lossy()))) {
        return Err(install_error(format!("the update zip holds {} twice", p.display())));
    }
    let mut left = MAX_UNPACKED;
    for (i, rel) in &files {
        let mut entry = archive.by_index(*i).map_err(|e| install_error(format!("the update zip is damaged: {e}")))?;
        let out = stage.join(rel);
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent).map_err(|e| install_error(format!("could not create {}: {e}", parent.display())))?;
        }
        let mut file =
            File::options().write(true).create_new(true).open(&out).map_err(|e| install_error(format!("could not write {}: {e}", out.display())))?;
        // one byte past the budget shows that the zip is larger than allowed
        let n = io::copy(&mut (&mut entry).take(left.saturating_add(1)), &mut file)
            .map_err(|e| install_error(format!("could not unpack {}: {e}", rel.display())))?;
        if n > left {
            return Err(install_error(format!("the update zip unpacks to more than {} MB", MAX_UNPACKED >> 20)));
        }
        left -= n;
        file.sync_all().map_err(|e| install_error(format!("could not write {}: {e}", out.display())))?;
    }
    // the program last: an update stopped half way still has a program to start
    let (program, mut rest): (Vec<_>, Vec<_>) = files.into_iter().map(|(_, p)| p).partition(|p| is_program(p, exe_name));
    rest.extend(program);
    Ok(rest)
}

/// The one folder every path starts in, when they all sit inside the same one.
fn single_top_folder<'a>(paths: impl Iterator<Item = &'a Path>) -> Option<PathBuf> {
    let mut top: Option<&std::ffi::OsStr> = None;
    let mut any = false;
    for p in paths {
        any = true;
        let mut parts = p.components();
        let (Some(Component::Normal(first)), Some(_)) = (parts.next(), parts.next()) else { return None };
        match top {
            None => top = Some(first),
            Some(t) if t == first => {}
            Some(_) => return None,
        }
    }
    if any { top.map(PathBuf::from) } else { None }
}

/// One file the swap moved, so it can be put back.
#[derive(Debug)]
struct Moved {
    /// The file's place in the copy.
    target: PathBuf,
    /// Where the previous file went (`None`: the update added the file).
    old: Option<PathBuf>,
    /// Whether the new file is in place.
    placed: bool,
}

/// Move each unpacked file from `stage` into `dir`, keeping the file it replaces as `.update-old`;
/// `moved` records each step for [`roll_back`].
fn swap(stage: &Path, dir: &Path, files: &[PathBuf], moved: &mut Vec<Moved>) -> Result<(), UpdateError> {
    for rel in files {
        let target = dir.join(rel);
        let staged = stage.join(rel);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|e| install_error(format!("could not create {}: {e}", parent.display())))?;
        }
        if target.is_dir() {
            return Err(install_error(format!("{} is a folder; the update has a file there", target.display())));
        }
        let old = if target.exists() {
            let old = old_path(&target);
            if old.exists() {
                // a leftover nothing tracks any more (no manifest names it): the file in place is
                // the one to keep
                retry(|| fs::remove_file(&old)).map_err(|e| {
                    install_error(format!(
                        "{} from an earlier update can't be deleted ({e}); quit every copy of the app and try again",
                        old.display()
                    ))
                })?;
            }
            retry(|| fs::rename(&target, &old)).map_err(|e| install_error(format!("could not move {} aside: {e}", target.display())))?;
            // a read-only file can't be deleted later, once the update is confirmed
            clear_read_only(&old);
            Some(old)
        } else {
            None
        };
        moved.push(Moved { target: target.clone(), old, placed: false });
        retry(|| fs::rename(&staged, &target)).map_err(|e| install_error(format!("could not put the new {} in place: {e}", rel.display())))?;
        if let Some(m) = moved.last_mut() {
            m.placed = true;
        }
    }
    Ok(())
}

/// Clear `file`'s read-only attribute (Windows refuses to delete a read-only file); a file that
/// isn't read-only, or can't be changed, is left as it is.
fn clear_read_only(file: &Path) {
    let Ok(meta) = fs::metadata(file) else { return };
    let mut perms = meta.permissions();
    if perms.readonly() {
        // on Windows this clears FILE_ATTRIBUTE_READONLY only; on Unix (where the copy is never
        // swapped outside the tests) it makes the file writable by its owner's group and others too,
        // which doesn't matter for a file about to be deleted
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        if let Err(e) = fs::set_permissions(file, perms) {
            log::debug!("updates: {} stays read-only: {e}", file.display());
        }
    }
}

/// Undo `moved`, newest first: delete each new file and rename the previous one back. What could
/// not be put back, one sentence each.
fn roll_back(moved: &[Moved]) -> Vec<String> {
    let mut problems = Vec::new();
    for m in moved.iter().rev() {
        if m.placed
            && let Err(e) = retry(|| fs::remove_file(&m.target))
        {
            problems.push(format!("{} could not be deleted ({e})", m.target.display()));
            continue;
        }
        if let Some(old) = &m.old
            && let Err(e) = retry(|| fs::rename(old, &m.target))
        {
            problems.push(format!("{} could not be renamed back to {} ({e})", old.display(), m.target.display()));
        }
    }
    problems
}

/// `op`, retried for [`RETRY_FOR`] while Windows refuses it for a moment (a file another process,
/// usually a virus scanner, has open).
fn retry<T>(op: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    retry_for(RETRY_FOR, op)
}

/// [`retry`] for up to `wait`.
fn retry_for<T>(wait: Duration, mut op: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    let start = Instant::now();
    loop {
        match op() {
            Ok(v) => return Ok(v),
            // 32: sharing violation, 33: lock violation
            Err(e) if (e.kind() == io::ErrorKind::PermissionDenied || matches!(e.raw_os_error(), Some(32 | 33))) && start.elapsed() < wait => {
                std::thread::sleep(RETRY_PAUSE)
            }
            Err(e) => return Err(e),
        }
    }
}

// ---------- the new version's start

/// What a starting copy does after [`on_start`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Start {
    /// Go on starting.
    Go,
    /// Go on starting: a zip update started this process, and the version it replaced has ended.
    AfterUpdate,
    /// The update was undone and `0`, the previous version, started in this one's place: end now,
    /// before anything is opened.
    Undone(PathBuf),
}

/// At the start of the copy whose program is `exe` (before the library is opened): when a zip
/// update started it (`after_update`), wait for the old version to end (`wait_for_old`, which
/// returns once the pipe from it closes); then, when the copy has an unconfirmed update, count this
/// start and hold the manifest until [`confirm`], or undo the update when an earlier, counted start
/// of the new version ended without confirming it. A start while the counted one is still coming
/// up (it holds the manifest) leaves the update alone. `start` starts the previous program after an
/// undo.
pub fn on_start(exe: &Path, after_update: bool, wait_for_old: &mut dyn FnMut(), start: &mut dyn FnMut(&Path) -> Result<(), String>) -> Start {
    begin(exe, after_update, false, wait_for_old, start)
}

/// [`on_start`] for a start the app made itself because its window's renderer failed to come up
/// (`<PREFIX>_WINDOW_RETRY`, [`WINDOW_RETRY`]): it carries on the start that made it, which is
/// ending right now. It waits for that one to let go of the manifest, then holds it in its place:
/// it neither counts as a start of its own nor undoes the update.
pub fn on_start_again(exe: &Path, after_update: bool, wait_for_old: &mut dyn FnMut(), start: &mut dyn FnMut(&Path) -> Result<(), String>) -> Start {
    begin(exe, after_update, true, wait_for_old, start)
}

fn begin(exe: &Path, after_update: bool, again: bool, wait_for_old: &mut dyn FnMut(), start: &mut dyn FnMut(&Path) -> Result<(), String>) -> Start {
    let (Some(dir), Some(exe_name)) = (exe.parent(), exe.file_name().map(|n| n.to_string_lossy().to_string())) else { return Start::Go };
    // only a version an update just put in place waits: a process that merely inherited the
    // variable (a child of the new version) finds no previous program beside it
    let waited = after_update && old_path(exe).exists();
    if waited {
        wait_for_old();
    }
    let go = if waited { Start::AfterUpdate } else { Start::Go };
    let path = manifest_path(dir, &exe_name);
    let opened = if again { open_when_free(&path) } else { Held::open(&path) };
    let held = match opened {
        Ok(Some(h)) => h,
        // none, or held: a swap running, or a start of the new version still coming up
        Ok(None) => return go,
        Err(e) => {
            log::warn!("updates: could not open this copy's update manifest: {e}");
            return go;
        }
    };
    let Some((held, mut m)) = contents(held) else { return go };
    match m.state {
        // a swap that stopped half way (its process holds no lock any more): back to the previous version
        State::Swapping => {
            log::warn!("updates: the update from {} to {} stopped half way; the previous version is put back", m.from, m.to);
            undo(dir, &exe_name, held, &mut m, start)
        }
        // the renderer's relaunch: the start that made it was counted already (when it wasn't, as it
        // found another start coming up, this one counts below)
        State::Swapped if again && m.attempts > 0 => {
            keep_running(held);
            go
        }
        // a counted start that ended (its lock is gone) without coming up
        State::Swapped if m.attempts > 0 => {
            log::warn!("updates: {} started before without coming up; the previous version, {}, is put back", m.to, m.from);
            undo(dir, &exe_name, held, &mut m, start)
        }
        State::Swapped => {
            m.attempts += 1;
            if let Err(e) = held.write(&m) {
                log::warn!("updates: could not count this start in the update manifest: {e}");
            }
            keep_running(held);
            go
        }
        State::Confirmed | State::Undone => go,
    }
}

/// [`Held::open`], retried for up to [`LEFTOVER_WAIT`] while another handle holds the manifest (the
/// start that relaunched this one, ending).
fn open_when_free(path: &Path) -> io::Result<Option<Held>> {
    let since = Instant::now();
    loop {
        match Held::open(path)? {
            Some(h) => return Ok(Some(h)),
            None if !path.exists() || since.elapsed() >= LEFTOVER_WAIT => return Ok(None),
            None => std::thread::sleep(RETRY_PAUSE),
        }
    }
}

/// This process counted its start of the new version but ends on purpose before coming up (a
/// relaunch that found its library open in another copy): the start is taken back and the manifest
/// let go, so the next start isn't taken for a failed one.
pub fn release_start(exe: &Path) {
    let (Some(dir), Some(exe_name)) = (exe.parent(), exe.file_name().map(|n| n.to_string_lossy().to_string())) else { return };
    let Some(held) = take_running(&manifest_path(dir, &exe_name)) else { return };
    let Some((held, mut m)) = contents(held) else { return };
    if m.state == State::Swapped && m.attempts > 0 {
        m.attempts -= 1;
        if let Err(e) = held.write(&m) {
            log::warn!("updates: could not take this start back in the update manifest: {e}");
        }
    }
}

/// Put the previous files back (the new ones become `.update-failed`), record it, and start the
/// previous program when this one's file was among them.
fn undo(dir: &Path, exe_name: &str, held: Held, m: &mut Manifest, start: &mut dyn FnMut(&Path) -> Result<(), String>) -> Start {
    // the program is put back only when it was swapped: a swap stopped before reaching it left it
    // as it was, and that is the program running now
    let program_swapped = m.files.iter().any(|f| is_program(f, exe_name) && (old_path(&dir.join(f)).exists() || m.added.contains(f)));
    for rel in &m.files {
        let target = dir.join(rel);
        let (old, failed) = (old_path(&target), failed_path(&target));
        // a file the swap never reached: as it was
        if !old.exists() && !m.added.contains(rel) {
            continue;
        }
        if target.exists() {
            let _ = fs::remove_file(&failed);
            if let Err(e) = retry(|| fs::rename(&target, &failed)) {
                log::warn!("updates: could not move {} aside: {e}", target.display());
                continue;
            }
        }
        if old.exists()
            && let Err(e) = retry(|| fs::rename(&old, &target))
        {
            log::warn!("updates: could not put {} back: {e}", target.display());
        }
    }
    m.state = State::Undone;
    if let Err(e) = held.write(m) {
        log::warn!("updates: could not record the undone update: {e}");
    }
    // let go before the previous version starts, so its clean-up finds the manifest free
    drop(held);
    let exe = dir.join(exe_name);
    if !program_swapped || !exe.is_file() {
        return Start::Go;
    }
    match start(&exe) {
        Ok(()) => Start::Undone(exe),
        Err(e) => {
            log::warn!("updates: the previous version could not be started ({e}); start it by hand");
            Start::Undone(exe)
        }
    }
}

/// In the new version, once it is up (its first frame, the library open): the update worked, so
/// the previous files go. Nothing to do without an unconfirmed update.
pub fn confirm(exe: &Path) {
    let (Some(dir), Some(exe_name)) = (exe.parent(), exe.file_name().map(|n| n.to_string_lossy().to_string())) else { return };
    let path = manifest_path(dir, &exe_name);
    // the manifest this start holds; a start that found it held by another (since gone) opens it
    let held = match take_running(&path) {
        Some(h) => h,
        None => match Held::open(&path) {
            Ok(Some(h)) => h,
            _ => return,
        },
    };
    let Some((held, mut m)) = contents(held) else { return };
    if m.state != State::Swapped {
        return;
    }
    m.state = State::Confirmed;
    if let Err(e) = held.write(&m) {
        log::warn!("updates: could not confirm the update in its manifest: {e}");
        return;
    }
    log::info!("updates: {} is up; the files of {} are deleted", m.to, m.from);
    finish(dir, held, &m);
}

/// Delete the leftovers a confirmed or undone update lists, and the manifest once none is left.
/// Runs off the UI thread: a file still in use is retried for [`LEFTOVER_WAIT`] (the process that
/// ran it may be ending right now) and otherwise left for the next start.
fn finish(dir: &Path, held: Held, m: &Manifest) {
    let leftover = |f: &Path| match m.state {
        State::Confirmed => old_path(f),
        _ => failed_path(f),
    };
    let mut left = 0usize;
    for rel in &m.files {
        let file = leftover(&dir.join(rel));
        match retry_for(LEFTOVER_WAIT, || fs::remove_file(&file)) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => {
                left += 1;
                log::debug!("updates: {} is still in use ({e}); deleted next time", file.display());
            }
        }
    }
    if m.state == State::Undone {
        log::warn!("updates: the update from {} to {} was undone; {} runs", m.from, m.to, m.from);
    }
    if left == 0 {
        held.remove();
    }
}

/// Finish what an update left in the copy in `dir` (whose program is `exe_name`), each start, in the
/// background: the leftovers of a confirmed or undone update ([`confirm`]), and unpack folders and
/// probes of a process that ended half way. Only the manifest's files and the program's own folder
/// are looked at, and nothing while another process holds the manifest (it is swapping). Never
/// fails: what can't be deleted now (a file still in use) is left for the next start.
pub fn clean_up(dir: &Path, exe_name: &str) {
    let path = manifest_path(dir, exe_name);
    let tmp = with_suffix(&path, TMP_SUFFIX);
    match Held::open(&path) {
        Ok(Some(held)) => {
            // a write that stopped before its rename; nobody else writes while this lock is held
            let _ = fs::remove_file(&tmp);
            if let Some((held, m)) = contents(held)
                && matches!(m.state, State::Confirmed | State::Undone)
            {
                finish(dir, held, &m);
            }
        }
        // another process holds it: a swap is running, its unpack folder is in use
        Ok(None) if path.exists() => return,
        // no update under way ([`Held::open`] deleted a temporary file that wasn't a whole manifest)
        Ok(None) => {}
        Err(e) => {
            log::debug!("updates: could not open {}: {e}", path.display());
            return;
        }
    }
    let prefix = folded(&stage_prefix(exe_name));
    let ours = format!("{prefix}{}", std::process::id());
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let name = folded(&entry.file_name().to_string_lossy());
        let Ok(kind) = entry.file_type() else { continue };
        if !name.starts_with(&prefix) || name == ours {
            continue;
        }
        let gone = if kind.is_dir() { fs::remove_dir_all(entry.path()) } else { fs::remove_file(entry.path()) };
        if let Err(e) = gone {
            log::debug!("updates: could not delete {}: {e}", entry.path().display());
        }
    }
}

#[cfg(test)]
mod tests;
