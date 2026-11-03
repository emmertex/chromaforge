//! A camera profile per camera body (ported from OneSlips, LR-PROF-BODY): two bodies of one model
//! can each start with their own profile — a `.dcp` made from that body's chart, say. A body is
//! known by its serial number (Exif `BodySerialNumber`), else, for cameras that write none, by the
//! first two characters of its file names after any leading `_` (`_SA94454.ARW` → `file:SA`). The
//! default is applied to the body's raws at import while they are on the camera's default profile
//! ([`crate::import::apply_import_defaults`]; a Lightroom sidecar that names a profile still wins),
//! and to photos already in the library on request (`profile.applyBodyDefaults`).

use lcc_catalog::Photo;

/// Photo `p`'s body: its serial, else `file:XX` from its file name (`None` when neither says).
pub fn body(p: &Photo) -> Option<String> {
    body_from(&p.meta.camera_serial, &p.file_name)
}

/// A body from a serial number, else from a file name's first two characters (as [`body`]).
pub fn body_from(serial: &str, file_name: &str) -> Option<String> {
    let serial = serial.trim();
    if !serial.is_empty() {
        return Some(serial.to_string());
    }
    let name = file_name.trim_start_matches('_');
    let key: String = name.chars().take(2).collect();
    (key.chars().count() == 2 && key.chars().all(|c| c.is_ascii_alphanumeric())).then(|| format!("file:{}", key.to_ascii_uppercase()))
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
    let mut rows: Vec<PlanRow> = Vec::new();
    for c in candidates.iter().filter(|c| c.duplicate.is_none() && c.error.is_none() && c.existing.is_none()) {
        let Some(body) = &c.body else { continue };
        if let Some(r) = rows.iter_mut().find(|r| &r.body == body) {
            r.photos += 1;
            continue;
        }
        let auto = s.body_profiles.get(body).filter(|id| known_profile(id)).cloned().unwrap_or_default();
        let (profile, how) = match choice {
            ImportProfile::Auto => (auto, "auto"),
            ImportProfile::All(id) => (id.clone(), "all"),
            ImportProfile::PerBody(m) => match m.get(body) {
                Some(id) => (id.clone(), "chosen"),
                None => (auto, "auto"),
            },
        };
        let serial = body.rsplit_once('|').map_or(body.as_str(), |(_, b)| b).to_string();
        rows.push(PlanRow { body: body.clone(), camera: c.camera.clone(), serial, photos: 1, profile_name: profile_name(&profile), profile, how });
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
    /// one made for another camera model.
    pub fn for_photo(&self, p: &Photo) -> Option<String> {
        let id = match self {
            Self::Auto => return None,
            Self::All(id) => id.clone(),
            Self::PerBody(m) => m.get(&key_of(p)?)?.clone(),
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

/// Whether `id` names a camera profile `develop.cameraProfile` accepts (other than the default "").
pub fn known_profile(id: &str) -> bool {
    id == "lc" || lcc_raw::dcp::library_get(id).is_some() || lcc_raw::dcp::calibrated_look_exists(id)
}

impl crate::Session {
    /// The camera profile photo `p`'s body starts with, when one was chosen and still exists.
    pub fn body_default(&self, p: &Photo) -> Option<&str> {
        let id = self.body_profiles.get(&key_of(p)?)?;
        known_profile(id).then_some(id.as_str())
    }
}
