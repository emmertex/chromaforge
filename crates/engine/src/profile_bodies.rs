//! A camera profile per camera body (ported from OneSlips, LR-PROF-BODY): two bodies of one model
//! can each start with their own profile — a `.dcp` made from that body's chart, say. A body is
//! known by its serial number (Exif `BodySerialNumber`), else, for cameras that write none, by the
//! first two characters of its camera-style file names after any leading `_` (`_SA94454.ARW` →
//! `file:SA`, never a renamed file's `10-…`: [`file_body`]). The
//! default is applied to the body's raws at import while they are on the camera's default profile
//! ([`crate::import::apply_import_defaults`]; a Lightroom sidecar that names a profile still wins),
//! and to photos already in the library on request (`profile.applyBodyDefaults`).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak, mpsc};

use lcc_catalog::Photo;

/// Photo `p`'s body: its serial, else `file:XX` from its file name (`None` when neither says).
pub fn body(p: &Photo) -> Option<String> {
    body_from(&p.meta.camera_serial, &p.file_name)
}

/// A body from a serial number, else from a camera-style file name ([`file_body`]).
pub fn body_from(serial: &str, file_name: &str) -> Option<String> {
    let serial = serial.trim();
    if !serial.is_empty() {
        return Some(serial.to_string());
    }
    file_body(file_name).map(|k| format!("file:{k}"))
}

/// The body a file name names as cameras write them: its first two characters after any leading
/// `_`, letters or digits, followed by the camera's counter or `_` (`_SA94454.ARW` -> `SA`,
/// `IMG_0001.CR3` -> `IM`, `_74_0338.CR3` -> `74`). Two digits count only after a leading `_`, and
/// a dash or space after them (`10-B+B-0042.CR3`, `1-B+B-...`, `07 ceremony.CR3`) is a renamed
/// file's number, not a body: `None`, and the photo gets its camera's default.
pub fn file_body(file_name: &str) -> Option<String> {
    let underscore = file_name.starts_with('_');
    let mut chars = file_name.trim_start_matches('_').chars();
    let (a, b, next) = (chars.next()?, chars.next()?, chars.next()?);
    if !(a.is_ascii_alphanumeric() && b.is_ascii_alphanumeric() && (next.is_ascii_alphanumeric() || next == '_')) {
        return None;
    }
    if a.is_ascii_digit() && b.is_ascii_digit() && !underscore {
        return None;
    }
    Some(format!("{a}{b}").to_ascii_uppercase())
}

/// The key a body default is kept under: the camera model and the body, so a serial reused by
/// another maker's camera is another body.
pub fn key(model: &str, body: &str) -> String {
    format!("{}|{}", model.trim(), body.trim())
}

/// The key ([`key`]) of a raw with camera model `model` (else `camera`), serial `serial` and file
/// name `file_name` (`None` without a model or a body).
pub fn key_from(model: &str, camera: &str, serial: &str, file_name: &str) -> Option<String> {
    let model = if model.trim().is_empty() { camera.trim() } else { model.trim() };
    if model.is_empty() {
        return None;
    }
    body_from(serial, file_name).map(|b| key(model, &b))
}

/// How many imported raws got one camera profile, and from which bodies.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct ProfileCount {
    pub name: String,
    pub photos: usize,
    pub bodies: Vec<String>,
}

/// A camera profile's name for people (`""`: the camera's default).
pub fn profile_name(id: &str) -> String {
    match id {
        "" => "Camera default".into(),
        "lc" => "Built-in colour".into(),
        _ => lcc_raw::dcp::library_get(id)
            .map(|d| d.name.clone())
            .or_else(|| lcc_raw::dcp::calibrated().iter().find_map(|c| c.looks.iter().find(|(i, _)| i == id).map(|(_, d)| d.name.clone())))
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| id.trim_start_matches("dcp:").to_string()),
    }
}

/// One camera body among an import's raws, and the camera profile its raws will get.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanRow {
    /// `model|body`, as `cameraProfile.perBody` takes it.
    pub body: String,
    pub camera: String,
    /// The serial, or `file:XX`.
    pub serial: String,
    pub photos: usize,
    /// The profile they get (`""`: the camera's default).
    pub profile: String,
    pub profile_name: String,
    /// `auto` (the body's own default, else the camera's), `chosen` (perBody) or `all`.
    pub how: &'static str,
}

/// The rows of `import.profilePlan`: the new raws of `candidates` by body, with what `choice` gives them.
pub fn plan(s: &crate::Session, candidates: &[crate::import::ImportCandidate], choice: &ImportProfile) -> Vec<PlanRow> {
    let bodies = candidates
        .iter()
        .filter(|c| c.duplicate.is_none() && c.error.is_none() && c.existing.is_none())
        .filter_map(|c| Some((c.body.clone()?, c.camera.clone())));
    plan_bodies(s, bodies, choice)
}

/// [`plan`] for photos already read (the Import tab's browsed photos): a row per body.
pub fn plan_photos(s: &crate::Session, photos: &[&Photo], choice: &ImportProfile) -> Vec<PlanRow> {
    plan_bodies(s, photos.iter().filter_map(|p| Some((s.body_key(p, false)?, p.meta.camera.clone()))), choice)
}

fn plan_bodies(s: &crate::Session, bodies: impl Iterator<Item = (String, String)>, choice: &ImportProfile) -> Vec<PlanRow> {
    let mut rows: Vec<PlanRow> = Vec::new();
    for (body, camera) in bodies {
        if let Some(r) = rows.iter_mut().find(|r| r.body == body) {
            r.photos += 1;
            continue;
        }
        let auto = body_profile(&s.body_profiles, &body, &camera).filter(|id| known_profile(id)).cloned().unwrap_or_default();
        let (profile, how) = match choice {
            ImportProfile::Auto => (auto, "auto"),
            ImportProfile::All(id) => (id.clone(), "all"),
            ImportProfile::PerBody(m) => match m.get(&body) {
                Some(id) => (id.clone(), "chosen"),
                None => (auto, "auto"),
            },
        };
        let serial = body.rsplit_once('|').map_or(body.as_str(), |(_, b)| b).to_string();
        rows.push(PlanRow { serial, body, camera, photos: 1, profile_name: profile_name(&profile), profile, how });
    }
    rows
}

/// Photo `p`'s key ([`key`]), for raws with a camera model.
pub fn key_of(p: &Photo) -> Option<String> {
    let model = if p.meta.camera_model.trim().is_empty() { p.meta.camera.trim() } else { p.meta.camera_model.trim() };
    if model.is_empty() || !p.develops_raw() {
        return None;
    }
    body(p).map(|b| key(model, &b))
}

/// The camera profile an import gives its raws (`library.import cameraProfile`; ported from OneSlips,
/// LR-PROF-IMPORT-APPLY): each body's own default (automatic), one profile for every raw of its
/// camera, or one per body (keys as `profiles.bodies` lists them) for this import only.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ImportProfile {
    #[default]
    Auto,
    All(String),
    PerBody(std::collections::BTreeMap<String, String>),
}

impl ImportProfile {
    /// Parse `cameraProfile`: `"auto"` / absent, `{all: id}` or `{perBody: {body: id}}`; ids as
    /// `develop.cameraProfile` takes them (`""`: the camera's default).
    pub fn parse(v: Option<&serde_json::Value>) -> Result<Self, String> {
        use serde_json::Value;
        let check = |id: &Value| -> Result<String, String> {
            let id = id.as_str().ok_or("a camera profile id must be a string")?.trim();
            if !id.is_empty() && !known_profile(id) {
                return Err(format!("unknown camera profile `{id}`"));
            }
            Ok(id.to_string())
        };
        match v {
            None | Some(Value::Null) => Ok(Self::Auto),
            Some(Value::String(s)) if s == "auto" => Ok(Self::Auto),
            Some(Value::Object(o)) if o.len() == 1 && o.contains_key("all") => Ok(Self::All(check(&o["all"])?)),
            Some(Value::Object(o)) if o.len() == 1 && o.contains_key("perBody") => {
                let m = o["perBody"].as_object().ok_or("`perBody` must be {body: profile id}")?;
                if m.len() > 10_000 {
                    return Err("too many bodies".into());
                }
                let mut out = std::collections::BTreeMap::new();
                for (k, id) in m {
                    if k.trim().is_empty() || k.len() > 200 {
                        return Err("a body key must be as profiles.bodies lists it".into());
                    }
                    out.insert(k.trim().to_string(), check(id)?);
                }
                Ok(Self::PerBody(out))
            }
            Some(_) => Err("`cameraProfile` is \"auto\", {all: profile id} or {perBody: {body: profile id}}".into()),
        }
    }

    /// The camera profile photo `p` gets from this choice (`None`: the automatic one stays): never
    /// one made for another camera model. `wait`: as [`crate::Session::body_key`].
    pub fn for_photo(&self, s: &crate::Session, p: &Photo, wait: bool) -> Option<String> {
        let id = match self {
            Self::Auto => return None,
            Self::All(id) => id.clone(),
            Self::PerBody(m) => m.get(&s.body_key(p, wait)?)?.clone(),
        };
        if !p.develops_raw() {
            return None;
        }
        let camera = if p.meta.camera.trim().is_empty() { p.meta.camera_model.trim() } else { p.meta.camera.trim() };
        let fits = id.is_empty()
            || id == "lc"
            || lcc_raw::dcp::library_get(&id).is_some_and(|d| lcc_raw::dcp::covers_model(&d.model, camera) || d.model == p.meta.camera_model)
            || lcc_raw::dcp::calibrated_for_camera(camera).is_some_and(|c| c.looks.iter().any(|(i, _)| *i == id));
        fits.then_some(id)
    }
}

/// The camera profile body `key` (`model|body`, [`key`]) of a camera written `camera` starts with
/// in `map`: its own entry, else one kept under the camera's whole name (`Sony ILCE-9|55` for an
/// `ILCE-9|55` body: a profile brought over from an older install names its camera by its
/// UniqueCameraModel when no photo of the body was in the library).
pub fn body_profile<'a>(map: &'a std::collections::BTreeMap<String, String>, key: &str, camera: &str) -> Option<&'a String> {
    if let Some(id) = map.get(key) {
        return Some(id);
    }
    let (_, body) = key.rsplit_once('|')?;
    let camera = camera.trim();
    if camera.is_empty() {
        return None;
    }
    map.iter()
        .find(|(k, _)| k.rsplit_once('|').is_some_and(|(m, b)| b == body && !m.trim().is_empty() && lcc_raw::dcp::covers_model(m, camera)))
        .map(|(_, id)| id)
}

/// Whether `id` names a camera profile `develop.cameraProfile` accepts (other than the default "").
pub fn known_profile(id: &str) -> bool {
    id == "lc" || lcc_raw::dcp::library_get(id).is_some() || lcc_raw::dcp::calibrated_look_exists(id)
}

impl crate::Session {
    /// The camera profile photo `p`'s body starts with, when one was chosen and still exists.
    pub fn body_default(&self, p: &Photo) -> Option<&str> {
        self.body_default_of(p, false)
    }

    /// [`Self::body_default`], `wait` as [`Self::body_key`].
    pub fn body_default_of(&self, p: &Photo, wait: bool) -> Option<&str> {
        let id = body_profile(&self.body_profiles, &self.body_key(p, wait)?, &p.meta.camera)?;
        known_profile(id).then_some(id.as_str())
    }

    /// Photo `p`'s body key ([`key_of`]). A raw catalogued without a serial number (browsed or
    /// imported before serials were kept) is known by the serial its file states
    /// ([`FileSerials`]): read now with `wait` (a command, an import), else in the background
    /// (a render: until it lands, the photo is known as [`key_of`] knows it).
    pub fn body_key(&self, p: &Photo, wait: bool) -> Option<String> {
        if let Some(serial) = self.missing_serial(p, wait) {
            let model = if p.meta.camera_model.trim().is_empty() { p.meta.camera.trim() } else { p.meta.camera_model.trim() };
            if !model.is_empty() {
                return Some(key(model, &serial));
            }
        }
        key_of(p)
    }

    /// The serial number raw `p` was catalogued without, as its file states it (`wait` as
    /// [`Self::body_key`]).
    pub fn missing_serial(&self, p: &Photo, wait: bool) -> Option<String> {
        if !p.meta.camera_serial.trim().is_empty() || !p.develops_raw() {
            return None;
        }
        let lcc_catalog::Source::File { path } = &p.source else { return None };
        self.file_serials.get(path, wait)
    }

    /// The camera profile an unedited raw gets at import with `choice` (the Import panel's), else
    /// its body's default, when that isn't the one it has (`None`: it keeps its own). A photo the
    /// user edited, or whose sidecar brought settings, keeps what it has.
    pub fn import_profile_for(&self, p: &Photo, choice: &ImportProfile, wait: bool) -> Option<String> {
        if p.edited.is_some() || !p.develops_raw() {
            return None;
        }
        choice.for_photo(self, p, wait).or_else(|| self.body_default_of(p, wait).map(str::to_string)).filter(|id| *id != p.develop.profile.camera)
    }

    /// The camera profile a browsed (Local) photo renders with in place of its own: what importing
    /// it now would give it ([`Self::import_profile_for`] with the Import panel's choice). Worked
    /// out at render time and never stored: the photo is not edited and no sidecar is written,
    /// and a body default set (or changed) after the folder was browsed shows at once.
    pub fn browse_profile(&self, p: &Photo) -> Option<String> {
        if !p.local || p.deleted {
            return None;
        }
        self.import_profile_for(p, &self.import_profile_choice, false)
    }

    /// Photo `p` as it renders: a browsed photo with [`Self::browse_profile`] in its settings, else
    /// `p` itself. The same `Arc` comes back while nothing changed, so render caches keep working.
    pub fn rendered_photo(&self, p: &Arc<Photo>) -> Arc<Photo> {
        let Some(camera) = self.browse_profile(p) else { return p.clone() };
        let mut held = lock(&self.browse_renders.0);
        if let Some((of, cam, r)) = held.get(&p.id)
            && std::ptr::eq(of.as_ptr(), Arc::as_ptr(p))
            && *cam == camera
        {
            return r.clone();
        }
        let mut shown = (**p).clone();
        let mut d = (*shown.develop).clone();
        d.profile.camera.clone_from(&camera);
        shown.develop = Arc::new(d);
        let shown = Arc::new(shown);
        if held.len() >= BROWSE_RENDERS {
            held.clear();
        }
        held.insert(p.id, (Arc::downgrade(p), camera, shown.clone()));
        shown
    }

    /// [`Self::rendered_photo`] of photo `id`.
    pub fn rendered_photo_of(&self, id: lcc_catalog::PhotoId) -> Option<Arc<Photo>> {
        self.catalog.photo(id).map(|p| self.rendered_photo(p))
    }
}

/// At most this many browsed photos' render settings are held ([`crate::Session::rendered_photo`]).
const BROWSE_RENDERS: usize = 50_000;

/// Browsed photos as they render: photo → (the record it was made from, the profile, the photo as rendered).
#[derive(Default)]
pub struct BrowseRenders(Mutex<HashMap<lcc_catalog::PhotoId, (Weak<Photo>, String, Arc<Photo>)>>);

/// How much of a raw is read for its serial number: a CR3 keeps its metadata (`moov`) at the start,
/// a TIFF-based raw its Exif near the start.
const SERIAL_HEAD: u64 = 1 << 20;

/// At most this many files' serial numbers are held.
const SERIAL_ENTRIES: usize = 200_000;

/// Serial numbers read from the files of raws catalogued without one (browsed before the app kept
/// serials, say: their body would otherwise be guessed from the file name). Read once per file, in
/// the background for renders.
#[derive(Default)]
pub struct FileSerials {
    /// File → its serial (`None`: being read, or the file states none).
    read: Arc<Mutex<HashMap<String, Option<String>>>>,
    queue: Mutex<Option<mpsc::Sender<String>>>,
}

impl FileSerials {
    /// The serial number file `path` states. `wait`: read it now when it isn't known yet; else a
    /// read is queued and `None` comes back until it lands.
    pub fn get(&self, path: &str, wait: bool) -> Option<String> {
        let known = lock(&self.read).get(path).cloned();
        match known {
            Some(Some(serial)) => return Some(serial),
            Some(None) if !wait => return None,
            _ => {}
        }
        if wait {
            let serial = read_serial(path);
            store(&self.read, path, serial.clone());
            return serial;
        }
        store(&self.read, path, None);
        let mut queue = lock(&self.queue);
        if queue.as_ref().is_none_or(|q| q.send(path.to_string()).is_err()) {
            let (tx, rx) = mpsc::channel::<String>();
            let read = self.read.clone();
            let started = std::thread::Builder::new().name("body-serials".into()).spawn(move || {
                while let Ok(path) = rx.recv() {
                    let serial = read_serial(&path);
                    store(&read, &path, serial);
                }
            });
            match started {
                Ok(_) if tx.send(path.to_string()).is_ok() => *queue = Some(tx),
                Ok(_) => {}
                Err(e) => log::warn!("could not start the serial number reader: {e}"),
            }
        }
        None
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn store(read: &Mutex<HashMap<String, Option<String>>>, path: &str, serial: Option<String>) {
    let mut m = lock(read);
    if m.len() >= SERIAL_ENTRIES && !m.contains_key(path) {
        m.clear();
    }
    m.insert(path.to_string(), serial);
}

/// The body serial number in the first [`SERIAL_HEAD`] bytes of file `path` (`None`: unreadable, or none).
pub fn read_serial(path: &str) -> Option<String> {
    use std::io::Read;
    let file = std::fs::File::open(path).ok()?;
    let mut head = Vec::new();
    file.take(SERIAL_HEAD).read_to_end(&mut head).ok()?;
    let serial: String = lcc_meta::extract(&head).serial_number?.trim().chars().take(64).collect();
    (!serial.is_empty()).then_some(serial)
}
