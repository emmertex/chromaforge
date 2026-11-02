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
    let serial = p.meta.camera_serial.trim();
    if !serial.is_empty() {
        return Some(serial.to_string());
    }
    let name = p.file_name.trim_start_matches('_');
    let key: String = name.chars().take(2).collect();
    (key.chars().count() == 2 && key.chars().all(|c| c.is_ascii_alphanumeric())).then(|| format!("file:{}", key.to_ascii_uppercase()))
}

/// The key a body default is kept under: the camera model and the body, so a serial reused by
/// another maker's camera is another body.
pub fn key(model: &str, body: &str) -> String {
    format!("{}|{}", model.trim(), body.trim())
}

/// Photo `p`'s key ([`key`]), for raws with a camera model.
pub fn key_of(p: &Photo) -> Option<String> {
    let model = if p.meta.camera_model.trim().is_empty() { p.meta.camera.trim() } else { p.meta.camera_model.trim() };
    if model.is_empty() || !p.develops_raw() {
        return None;
    }
    body(p).map(|b| key(model, &b))
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
