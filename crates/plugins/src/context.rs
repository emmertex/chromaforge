//! The host side of `host.call`: every request a plug-in makes, checked against its effective
//! capabilities before anything happens.
//!
//! | method                | capability                  | effect                                  |
//! |-----------------------|-----------------------------|-----------------------------------------|
//! | `catalog.query`, `photo.inspect`, `catalog.stats`, `keyword.list` | `catalog` | [`HostApi::catalog`] |
//! | `metadata.setStandard`| `metadataWrite`             | [`HostApi::set_metadata`]               |
//! | `library.add`         | `libraryAdd`                | [`HostApi::library_add`]                |
//! | `photo.pixels`        | `pixels`                    | [`HostApi::photo_pixels`]               |
//! | `develop.setCrop`     | `crop`                      | [`HostApi::set_crop`]                   |
//! | `photo.virtualCopy`   | `virtualCopy`               | [`HostApi::virtual_copy`]               |
//! | `ns.get`, `ns.set`, `ns.photos` | always (own namespace) | [`NamespaceStore`]               |
//! | `settings.get`, `settings.set` | always (own settings) | [`SettingsStore`], checked against the manifest's `settings` |
//! | `fs.readText`, `fs.list`, `fs.exists` | a readable root | std::fs                           |
//! | `fs.writeText`        | a writable root (or a file lent for this call) | std::fs              |
//! | `fs.readBytes`, `fs.writeBytes` | as the text calls; contents as base64 | std::fs              |
//! | `fs.remove`           | a file below a writable root (never a lent file) | std::fs            |
//! | `http.request`        | the URL's host in `network` | `lcc-net` (native only)                 |
//! | `program.list`        | none (declared programs only) | [`HostApi::find_program`]             |
//! | `program.start`       | the program in `programs`   | [`HostApi::start_program`] (native only; not in a job) |
//! | `job.progress`, `job.detail`, `job.itemDone`, `job.itemFailed`, `job.cancelled` | a job only | [`JobReporter`] |
//!
//! In a job, writes to the catalog (`metadata.setStandard`, `library.add`) are refused, and once
//! the job is cancelled every call but `job.cancelled` fails with "cancelled".

use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};

use serde_json::{Value, json};

use crate::manifest::Field;
use crate::permissions::{Group, Permissions, Request};
use crate::settings::SettingsStore;
use crate::store::NamespaceStore;

/// Largest file `fs.readText` returns / `fs.writeText` writes, and largest HTTP body.
pub const MAX_IO_BYTES: usize = 8 << 20;
/// Most entries `fs.list` returns.
pub const MAX_LIST: usize = 10_000;
/// Most lines one call adds to the plug-in's log about what the host did for it.
pub const MAX_NOTES: usize = 20;
/// Long edge `photo.pixels` gives when the plug-in names none.
pub const PIXELS_DEFAULT_EDGE: u32 = 1024;
/// Smallest and largest long edge `photo.pixels` gives: at the largest, a square photo's 8-bit RGB
/// as base64 stays well inside the 16 MiB message cap.
pub const PIXELS_MIN_EDGE: u32 = 64;
pub const PIXELS_MAX_EDGE: u32 = 1536;
/// Longest name `photo.virtualCopy` takes for a copy, in characters.
pub const MAX_COPY_NAME: usize = 256;

/// Catalog access the application provides. The default methods refuse, so a host without a
/// catalog (tests, the CLI before a library is open) only implements what it has.
pub trait HostApi {
    /// A read-only catalog request: `method` is one of `catalog.query`, `photo.inspect`,
    /// `catalog.stats`, `keyword.list`, with that engine command's parameters.
    fn catalog(&mut self, method: &str, params: &Value) -> Result<Value, String> {
        let _ = params;
        Err(format!("{method}: no catalog is available"))
    }
    /// Writes standard metadata: `{ids: [photo ids], title?, caption?, keywords?: [..], …}`.
    fn set_metadata(&mut self, params: &Value) -> Result<Value, String> {
        let _ = params;
        Err("metadata.setStandard: not available".into())
    }
    /// Adds a file a tool made to the library: `{path, from: photo id, stack?: bool, album?: id}`
    /// → `{photo, added, reloaded}`. The host checks everything (the file is in the folder of
    /// `from`'s original or below it, the album holds `from`) before it changes anything.
    fn library_add(&mut self, params: &Value) -> Result<Value, String> {
        let _ = params;
        Err("library.add: not available".into())
    }
    /// A downsized copy of photo `photo`'s pixels: the whole frame as the crop tool shows it
    /// (orientation, lens and perspective applied, no crop, no straighten), long edge at most
    /// `long_edge` (already clamped to [`PIXELS_MIN_EDGE`]..=[`PIXELS_MAX_EDGE`]), 8-bit sRGB.
    /// → `{width, height, channels: 3, encoding: "srgb8", data: base64 of interleaved RGB rows}`.
    /// Its coordinates are the ones [`HostApi::set_crop`] takes, normalized.
    fn photo_pixels(&mut self, photo: &str, long_edge: u32) -> Result<Value, String> {
        let _ = (photo, long_edge);
        Err("photo.pixels: not available".into())
    }
    /// Sets photo `photo`'s crop: `rect` `[x0, y0, x1, y1]` normalized in the frame
    /// [`HostApi::photo_pixels`] gives turned by `angle` (degrees, none: 0) about its centre. One History
    /// step; the user adjusts it afterwards in the crop tool like any crop. → the crop as set,
    /// `{rect: [x0, y0, x1, y1], angle}` (the host may fit it inside the photo).
    fn set_crop(&mut self, photo: &str, rect: [f64; 4], angle: Option<f64>) -> Result<Value, String> {
        let _ = (photo, rect, angle);
        Err("develop.setCrop: not available".into())
    }
    /// Makes a virtual copy of photo `photo`: a new catalog entry sharing its file, with a copy of
    /// its settings and metadata, stacked right after it and in the albums that hold it, named
    /// `name` or "Copy N". One History step; the selection stays as it was. → `{photo}`, the
    /// copy's id as a decimal string.
    fn virtual_copy(&mut self, photo: &str, name: Option<&str>) -> Result<Value, String> {
        let _ = (photo, name);
        Err("photo.virtualCopy: not available".into())
    }

    /// Where `program` is installed on this system (none: not installed). Read-only; never
    /// starts anything.
    fn find_program(&mut self, program: &crate::ProgramDecl) -> Vec<crate::Found> {
        let _ = program;
        Vec::new()
    }

    /// Starts a run ([`crate::StartRequest`]: the program's arguments and folder are ready) and
    /// watches it: when it exits, or the user stops waiting, the host calls the plug-in's
    /// `program.end` ([`crate::Manager::program_end_call`]), drops the reservation, and deletes
    /// the folder once the program has exited (by dropping the request). An error means nothing
    /// started, and the dropped request has deleted the folder.
    fn start_program(&mut self, _run: crate::StartRequest) -> Result<(), String> {
        Err("starting programs is not available here".into())
    }
}

/// A host with no catalog.
pub struct NoHost;
impl HostApi for NoHost {}

/// How a job words its progress: "3 of 25", "12 of 340 MB", "40 %".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum JobUnit {
    #[default]
    Count,
    Bytes,
    Percent,
}

/// Where a job's `job.*` host calls go: the app's activity row for it. Calls on the job's
/// thread; the row may be read and cancelled from others.
pub trait JobReporter {
    /// `done` of `total` (0: not known, the bar sweeps).
    fn progress(&self, done: u64, total: u64, unit: JobUnit);
    /// What the job is working on (a name, not a sentence).
    fn detail(&self, text: &str);
    /// One item (photo) finished: the row counts as a batch from now on.
    fn item_done(&self);
    /// One item failed, `why` naming it and the reason.
    fn item_failed(&self, why: &str);
    /// Whether the user (or an agent) asked the job to stop.
    fn cancelled(&self) -> bool;
}

/// A job nobody watches (a job run as a plain call: tests, [`crate::Manager::run_command`]): it
/// reports nowhere and is never cancelled.
pub struct Unwatched;
impl JobReporter for Unwatched {
    fn progress(&self, _: u64, _: u64, _: JobUnit) {}
    fn detail(&self, _: &str) {}
    fn item_done(&self) {}
    fn item_failed(&self, _: &str) {}
    fn cancelled(&self) -> bool {
        false
    }
}

/// Everything one plug-in call may reach.
pub(crate) struct CallContext<'a> {
    /// The plug-in's id.
    pub plugin: &'a str,
    /// Its display name (the manifest's `name`).
    pub plugin_name: &'a str,
    /// What its manifest asks for (`program.list` answers for declared programs).
    pub requested: &'a Permissions,
    /// What it may do (granted ∩ requested).
    pub perms: &'a Permissions,
    /// Files lent for this call only (read and write), e.g. the exported file a hook post-processes.
    pub lent: &'a [PathBuf],
    /// Folders lent for this call only, for reading (a program run's folder in `program.end`).
    pub lent_dirs: &'a [PathBuf],
    /// Where `program.start` makes run folders (`None`: [`crate::programs::runs_root`]).
    pub runs_root: Option<&'a std::path::Path>,
    pub host: &'a mut dyn HostApi,
    pub store: &'a mut NamespaceStore,
    /// Lines for the plug-in's log about what the host did for it (a program that didn't start).
    pub notes: Vec<String>,
    /// The plug-in's settings (shared: the Plug-in Manager may read them while a call runs).
    pub settings: &'a Mutex<SettingsStore>,
    /// The settings the manifest declares.
    pub fields: &'a [Field],
    /// A prepare step (`command.prepare`, `settings.prepare`): it may read, never write.
    pub read_only: bool,
    /// The job this call is part of (`None`: not a job; the `job.*` calls are refused).
    pub job: Option<&'a dyn JobReporter>,
    /// What the user may be asked to allow: the manifest's request, less what no one may grant.
    pub askable: &'a Permissions,
    /// What the user said no to: refused quietly.
    pub denied: &'a [Request],
    /// The user can be asked: a request never asked about goes in `needs`.
    pub asking: bool,
    /// What calls were refused for that the user was never asked about.
    pub needs: &'a mut Vec<Request>,
}

/// Whether host call `method` with `p` changes something: refused in a read-only call.
fn writes(method: &str, p: &Value) -> bool {
    match method {
        "metadata.setStandard"
        | "library.add"
        | "develop.setCrop"
        | "photo.virtualCopy"
        | "ns.set"
        | "settings.set"
        | "fs.writeText"
        | "fs.writeBytes"
        | "fs.remove"
        | "program.start" => true,
        // a request that may change something on the server
        "http.request" => {
            let m = p.get("method").and_then(Value::as_str).unwrap_or("GET");
            !m.eq_ignore_ascii_case("GET") && !m.eq_ignore_ascii_case("HEAD")
        }
        _ => false,
    }
}

/// `rect` of `develop.setCrop`: four finite numbers in 0..=1, ordered `x0 < x1`, `y0 < y1`.
fn crop_rect(p: &Value) -> Result<[f64; 4], String> {
    const E: &str = "develop.setCrop: `rect` must be [x0, y0, x1, y1], normalized (0..1), with x0 < x1 and y0 < y1";
    let a = p.get("rect").and_then(Value::as_array).filter(|a| a.len() == 4).ok_or(E)?;
    let v: Vec<f64> = a.iter().filter_map(Value::as_f64).filter(|v| v.is_finite() && (0.0..=1.0).contains(v)).collect();
    match v[..] {
        [x0, y0, x1, y1] if x0 < x1 && y0 < y1 => Ok([x0, y0, x1, y1]),
        _ => Err(E.into()),
    }
}

fn str_arg<'v>(p: &'v Value, key: &str) -> Result<&'v str, String> {
    p.get(key).and_then(Value::as_str).ok_or_else(|| format!("missing string `{key}`"))
}

impl CallContext<'_> {
    /// Adds a line for the plug-in's log, up to [`MAX_NOTES`] per call (a plug-in that fails in a
    /// loop can't flood it).
    fn note(&mut self, line: String) {
        match self.notes.len() {
            n if n < MAX_NOTES => self.notes.push(line),
            n if n == MAX_NOTES => self.notes.push("more of the same in this call are left out".into()),
            _ => {}
        }
    }

    /// The error for a call that needs `r`, which isn't granted. Denied: refused quietly. Never
    /// asked, with the user there to ask: noted in `needs`, for the host to ask once the call
    /// ends. Never asked, with nobody to ask (a headless run, a job, an agent's request): the
    /// error says where to allow it.
    fn refuse(&mut self, r: Request) -> String {
        if self.denied.contains(&r) {
            return denied(&r);
        }
        if self.asking {
            if !self.needs.contains(&r) {
                self.needs.push(r.clone());
            }
            return denied(&r);
        }
        format!("needs `{r}`, not granted to this plug-in; allow it in the Plug-in Manager (File ▸ Plug-ins ▸ Plug-in Manager…)")
    }

    /// `path` resolved for reading (or writing) under a granted folder, or a file (`lent`: and a
    /// folder) lent for this call. Outside them, a folder the manifest asks for that holds it is
    /// what the call needs ([`CallContext::refuse`]).
    fn path(&mut self, path: &str, write: bool, lent: bool) -> Result<PathBuf, String> {
        let (files, dirs): (&[PathBuf], &[PathBuf]) = match (lent, write) {
            (false, _) => (&[], &[]),
            (true, true) => (self.lent, &[]),
            (true, false) => (self.lent, self.lent_dirs),
        };
        let e = match self.perms.check_path_in(path, write, files, dirs) {
            Ok(p) => return Ok(p),
            Err(e) => e,
        };
        let root = self
            .askable
            .fs
            .iter()
            .find(|r| Permissions { fs: vec![(*r).clone()], ..Default::default() }.check_path(path, write, &[]).is_ok())
            .map(|r| r.path.clone());
        Err(match root {
            Some(root) => format!("{path}: {}", self.refuse(Request::value(Group::Fs, root))),
            None => e,
        })
    }

    pub fn dispatch(&mut self, method: &str, p: &Value) -> Result<Value, String> {
        if self.read_only && writes(method, p) {
            return Err(format!("{method}: a prepare step can only read"));
        }
        if method.starts_with("job.") {
            return self.job_call(method, p);
        }
        if let Some(job) = self.job {
            if job.cancelled() {
                return Err("cancelled".into());
            }
            if matches!(method, "metadata.setStandard" | "library.add" | "develop.setCrop" | "photo.virtualCopy") {
                return Err(format!("{method}: writes to the catalog aren't available in a job yet"));
            }
            if method == "program.start" {
                return Err("program.start is not available in a job; start the program from a command that isn't a job".into());
            }
        }
        match method {
            "catalog.query" | "photo.inspect" | "catalog.stats" | "keyword.list" => {
                if !self.perms.catalog {
                    return Err(self.refuse(Request::group(Group::Catalog)));
                }
                self.host.catalog(method, p)
            }
            "metadata.setStandard" => {
                if !self.perms.metadata_write {
                    return Err(self.refuse(Request::group(Group::MetadataWrite)));
                }
                self.host.set_metadata(p)
            }
            "library.add" => {
                if !self.perms.library_add {
                    return Err(self.refuse(Request::group(Group::LibraryAdd)));
                }
                if !p.is_object() {
                    return Err("library.add: params must be {path, from, stack?, album?}".into());
                }
                self.host.library_add(p)
            }
            "photo.pixels" => {
                if !self.perms.pixels {
                    return Err(self.refuse(Request::group(Group::Pixels)));
                }
                let photo = str_arg(p, "photo")?;
                let edge = match p.get("longEdge") {
                    None | Some(Value::Null) => PIXELS_DEFAULT_EDGE,
                    Some(v) => v
                        .as_f64()
                        .filter(|e| e.is_finite())
                        .ok_or("photo.pixels: `longEdge` must be a number")?
                        .round()
                        .clamp(f64::from(PIXELS_MIN_EDGE), f64::from(PIXELS_MAX_EDGE)) as u32,
                };
                self.host.photo_pixels(photo, edge)
            }
            "develop.setCrop" => {
                if !self.perms.crop {
                    return Err(self.refuse(Request::group(Group::Crop)));
                }
                let photo = str_arg(p, "photo")?;
                let rect = crop_rect(p)?;
                let angle = match p.get("angle") {
                    None | Some(Value::Null) => None,
                    Some(v) => Some(v.as_f64().filter(|a| a.is_finite()).ok_or("develop.setCrop: `angle` must be a number of degrees")?),
                };
                self.host.set_crop(photo, rect, angle)
            }
            "photo.virtualCopy" => {
                if !self.perms.virtual_copy {
                    return Err(self.refuse(Request::group(Group::VirtualCopy)));
                }
                let photo = str_arg(p, "photo")?;
                let name = match p.get("name") {
                    None | Some(Value::Null) => None,
                    Some(Value::String(n)) if n.chars().count() <= MAX_COPY_NAME => Some(n.trim()).filter(|n| !n.is_empty()),
                    Some(_) => return Err(format!("photo.virtualCopy: `name` must be text of at most {MAX_COPY_NAME} characters")),
                };
                self.host.virtual_copy(photo, name)
            }
            "ns.get" => Ok(self.store.get(str_arg(p, "photo")?)),
            "ns.set" => {
                self.store.set(str_arg(p, "photo")?, str_arg(p, "key")?, p.get("value").cloned().unwrap_or(Value::Null))?;
                Ok(Value::Null)
            }
            "ns.photos" => Ok(json!(self.store.photos())),
            "settings.get" => Ok(Value::Object(self.settings.lock().unwrap_or_else(PoisonError::into_inner).values(self.fields))),
            "settings.set" => {
                let key = str_arg(p, "key")?;
                let f = self.fields.iter().find(|f| f.key == key).ok_or_else(|| format!("no setting {key:?} in the manifest"))?;
                let v = match p.get("value") {
                    None | Some(Value::Null) => None,
                    Some(v) => Some(f.coerce(v)?),
                };
                let mut s = self.settings.lock().unwrap_or_else(PoisonError::into_inner);
                s.set(key, v.clone()).map_err(|e| e.to_string())?;
                s.save().map_err(|e| e.to_string())?;
                Ok(v.unwrap_or_else(|| f.default_value()))
            }
            "fs.readText" => {
                let path = self.path(str_arg(p, "path")?, false, true)?;
                let bytes = crate::safe_io::read_checked(&path, MAX_IO_BYTES as u64).map_err(|e| format!("{}: {e}", path.display()))?;
                String::from_utf8(bytes).map(Value::from).map_err(|_| format!("{}: not UTF-8 text", path.display()))
            }
            "fs.writeText" => {
                let path = self.path(str_arg(p, "path")?, true, true)?;
                let text = str_arg(p, "text")?;
                if text.len() > MAX_IO_BYTES {
                    return Err(format!("larger than {MAX_IO_BYTES} bytes"));
                }
                crate::safe_io::write_checked(&path, text.as_bytes()).map(|_| Value::Null).map_err(|e| format!("{}: {e}", path.display()))
            }
            "fs.readBytes" => {
                let path = self.path(str_arg(p, "path")?, false, true)?;
                crate::safe_io::read_checked(&path, MAX_IO_BYTES as u64)
                    .map(|b| Value::from(crate::b64::encode(&b)))
                    .map_err(|e| format!("{}: {e}", path.display()))
            }
            "fs.writeBytes" => {
                let path = self.path(str_arg(p, "path")?, true, true)?;
                let text = str_arg(p, "base64")?;
                if text.len() > MAX_IO_BYTES / 3 * 4 + 4 {
                    return Err(format!("larger than {MAX_IO_BYTES} bytes"));
                }
                let bytes = crate::b64::decode(text)?;
                crate::safe_io::write_checked(&path, &bytes).map(|_| Value::Null).map_err(|e| format!("{}: {e}", path.display()))
            }
            "fs.remove" => {
                let path = self.path(str_arg(p, "path")?, true, false)?;
                if !std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_file()) {
                    return Err(format!("{}: not a file", path.display()));
                }
                std::fs::remove_file(&path).map(|_| Value::Null).map_err(|e| format!("{}: {e}", path.display()))
            }
            "fs.exists" => {
                let path = self.path(str_arg(p, "path")?, false, true)?;
                Ok(Value::from(path.exists()))
            }
            "fs.list" => {
                let path = self.path(str_arg(p, "path")?, false, true)?;
                let rd = std::fs::read_dir(&path).map_err(|e| format!("{}: {e}", path.display()))?;
                let mut names: Vec<Value> = rd
                    .filter_map(|e| e.ok())
                    .take(MAX_LIST)
                    .map(|e| json!({"name": e.file_name().to_string_lossy(), "dir": e.file_type().is_ok_and(|t| t.is_dir())}))
                    .collect();
                names.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
                Ok(Value::Array(names))
            }
            "http.request" => self.http(p),
            "program.list" => Ok(self.program_list()),
            "program.start" => {
                let r = self.program_start(p);
                if let Err(e) = &r {
                    self.note(format!("program.start failed: {e}"));
                }
                r
            }
            _ => Err(format!("unknown host method {method:?}")),
        }
    }

    /// A `job.*` call: reported to the job's row; refused outside a job.
    fn job_call(&mut self, method: &str, p: &Value) -> Result<Value, String> {
        let Some(job) = self.job else {
            return Err(format!("{method}: only a command declared as a job (\"job\": true) can report progress"));
        };
        if method == "job.cancelled" {
            return Ok(Value::from(job.cancelled()));
        }
        if job.cancelled() {
            return Err("cancelled".into());
        }
        let count = |key: &str| p.get(key).and_then(Value::as_u64).ok_or_else(|| format!("{method} needs `{key}`, a whole number 0 or more"));
        match method {
            "job.progress" => {
                let (done, total) = (count("done")?, count("total")?);
                let unit = match p.get("unit") {
                    None | Some(Value::Null) => JobUnit::Count,
                    Some(u) => match u.as_str() {
                        Some("count") => JobUnit::Count,
                        Some("bytes") => JobUnit::Bytes,
                        Some("percent") => JobUnit::Percent,
                        _ => return Err(format!("job.progress: `unit` is {u}; it takes \"count\", \"bytes\" or \"percent\"")),
                    },
                };
                job.progress(done, total, unit);
            }
            "job.detail" => job.detail(str_arg(p, "text")?),
            "job.itemDone" => job.item_done(),
            "job.itemFailed" => job.item_failed(str_arg(p, "why")?),
            _ => return Err(format!("unknown host method {method:?}")),
        }
        Ok(Value::Null)
    }

    /// Each declared program: `{program, name, granted, installed: [{bundleId?, path?, version?}]}`,
    /// `path` only once the program is granted (before, the plug-in learns what is installed, not
    /// where).
    fn program_list(&mut self) -> Value {
        let list: Vec<Value> = self
            .requested
            .programs
            .iter()
            .map(|d| {
                let granted = self.perms.program(&d.id).is_some();
                let installed: Vec<Value> = installs(d, self.host.find_program(d))
                    .into_iter()
                    .map(|f| json!({"bundleId": f.bundle_id, "path": granted.then_some(f.path), "version": f.version}))
                    .map(|mut f| {
                        if let Some(o) = f.as_object_mut() {
                            o.retain(|_, v| !v.is_null());
                        }
                        f
                    })
                    .collect();
                json!({"program": d.id, "name": d.name, "granted": granted, "installed": installed})
            })
            .collect();
        Value::Array(list)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn program_start(&mut self, p: &Value) -> Result<Value, String> {
        use crate::programs::{Found, Reservation, RunDir, StartRequest, expand_args, runs_root};
        let id = str_arg(p, "program")?;
        if self.requested.program(id).is_none() {
            return Err(format!("{id:?} is not one of this plug-in's programs"));
        }
        let Some(decl) = self.perms.program(id).cloned() else { return Err(self.refuse(Request::value(Group::Programs, id))) };
        let files = match p.get("files") {
            None | Some(Value::Null) => serde_json::Map::new(),
            Some(Value::Object(m)) => m.clone(),
            Some(_) => return Err("`files` must be an object {name: text}".into()),
        };
        // everything the plug-in sent is checked before a folder is made or a program looked up
        crate::programs::checked_files(&files)?;
        expand_args(p.get("args").unwrap_or(&Value::Null), std::path::Path::new(""))?;
        let wanted = match p.get("bundleId") {
            None | Some(Value::Null) => None,
            Some(Value::String(b)) if !cfg!(target_os = "macos") => return Err(format!("bundleId {b:?}: bundle ids are for macOS only")),
            Some(Value::String(b)) if !decl.mac_bundle_ids.contains(b) => return Err(format!("bundleId {b:?} is not one of {id}'s")),
            Some(Value::String(b)) => Some(b.clone()),
            Some(_) => return Err("`bundleId` must be a string".into()),
        };
        let found: Vec<Found> = installs(&decl, self.host.find_program(&decl));
        let found = match &wanted {
            Some(b) => found.into_iter().find(|f| f.bundle_id.as_deref() == Some(b)),
            None => found.into_iter().next(),
        }
        .ok_or_else(|| format!("{} is not installed{}", decl.name, wanted.as_deref().map(|b| format!(" ({b})")).unwrap_or_default()))?;
        let reservation = Reservation::take(self.plugin, id)?;
        let dir = RunDir::create(&self.runs_root.map_or_else(runs_root, std::path::Path::to_path_buf))?;
        // on an error from here on, dropping `dir` deletes the folder
        dir.write_files(&files)?;
        let args = expand_args(p.get("args").unwrap_or(&Value::Null), dir.path())?;
        let tmp = dir.path().to_path_buf();
        let run_id = reservation.run().to_string();
        let req =
            StartRequest { plugin: self.plugin.to_string(), plugin_name: self.plugin_name.to_string(), program: decl, found, args, dir, reservation };
        self.host.start_program(req)?;
        Ok(json!({"run": run_id, "tmpDir": tmp}))
    }

    #[cfg(target_arch = "wasm32")]
    fn program_start(&mut self, _: &Value) -> Result<Value, String> {
        Err("program.start is not available in the web build".into())
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn http(&mut self, p: &Value) -> Result<Value, String> {
        use lcc_net::{Client, ClientConfig, Method, Url};
        let url = str_arg(p, "url")?;
        let parsed = Url::parse(url).map_err(|e| e.to_string())?;
        if !self.perms.allows_host(&parsed.host) {
            let asked =
                self.askable.network.iter().find(|h| Permissions { network: vec![(*h).clone()], ..Default::default() }.allows_host(&parsed.host));
            return Err(match asked.cloned() {
                Some(h) => format!("{}: {}", parsed.host, self.refuse(Request::value(Group::Network, h))),
                None => format!("{}: not one of the `network` hosts this plug-in asks for", parsed.host),
            });
        }
        let method = match p.get("method").and_then(Value::as_str).unwrap_or("GET").to_ascii_uppercase().as_str() {
            "GET" => Method::Get,
            "HEAD" => Method::Head,
            "POST" => Method::Post,
            "PUT" => Method::Put,
            "PATCH" => Method::Patch,
            "DELETE" => Method::Delete,
            m => return Err(format!("unsupported HTTP method {m}")),
        };
        // No automatic redirects: a redirect to another host must be a new, checked request.
        let config = ClientConfig { max_redirects: 0, max_body: MAX_IO_BYTES as u64, ..ClientConfig::default() };
        let client = Client::new(config).map_err(|e| e.to_string())?;
        let mut req = client.request(method, url);
        if let Some(h) = p.get("headers").and_then(Value::as_object) {
            for (k, v) in h.iter().take(64) {
                if let Some(v) = v.as_str() {
                    req = req.secret_header(k, v);
                }
            }
        }
        if let Some(body) = p.get("body").and_then(Value::as_str) {
            if body.len() > MAX_IO_BYTES {
                return Err(format!("request body larger than {MAX_IO_BYTES} bytes"));
            }
            let ct = p.get("contentType").and_then(Value::as_str).unwrap_or("application/json");
            req = req.body(ct, body.as_bytes().to_vec());
        } else if let Some(b64) = p.get("bodyBase64").and_then(Value::as_str) {
            if b64.len() > MAX_IO_BYTES / 3 * 4 + 4 {
                return Err(format!("request body larger than {MAX_IO_BYTES} bytes"));
            }
            let ct = p.get("contentType").and_then(Value::as_str).unwrap_or("application/octet-stream");
            req = req.body(ct, crate::b64::decode(b64)?);
        }
        let resp = req.send().map_err(|e| e.to_string())?;
        let status = resp.status;
        let location = resp.header("location").map(str::to_string);
        let ctype = resp.header("content-type").map(str::to_string);
        let body = resp.bytes().map_err(|e| e.to_string())?;
        Ok(json!({"status": status, "contentType": ctype, "location": location, "body": String::from_utf8_lossy(&body)}))
    }

    #[cfg(target_arch = "wasm32")]
    fn http(&mut self, _: &Value) -> Result<Value, String> {
        Err("http.request is not available in the web build".into())
    }
}

/// What the app found of `decl`, in the entry's order: on macOS by bundle id, and only the
/// entry's bundle ids.
fn installs(decl: &crate::ProgramDecl, mut found: Vec<crate::Found>) -> Vec<crate::Found> {
    if !decl.mac_bundle_ids.is_empty() && found.iter().any(|f| f.bundle_id.is_some()) {
        found.retain(|f| f.bundle_id.as_ref().is_some_and(|b| decl.mac_bundle_ids.contains(b)));
        found.sort_by_key(|f| decl.mac_bundle_ids.iter().position(|b| Some(b) == f.bundle_id.as_ref()));
    }
    found
}

fn denied(r: &Request) -> String {
    format!("permission `{r}` not granted to this plug-in")
}
