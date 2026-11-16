//! The saved UI state and app settings (`<settings>/ui.json`), as on the desktop: a damaged file
//! is set aside and reported, one that can't be read is never overwritten this run, and only
//! changes are written (atomically).
//!
//! Android may stop the process without warning once the app is in the background, so the
//! settings are written within seconds of a change rather than only when the app closes.

use std::path::{Path, PathBuf};

use lcc_ui_egui::{LccApp, UiState};

/// `<settings>/ui.json`; `None` when there is no settings folder.
pub fn path() -> Option<PathBuf> {
    lcc_engine::config::config_dir().map(|d| d.join("ui.json"))
}

/// What [`load`] found.
pub struct Loaded {
    pub ui: Option<UiState>,
    /// A message for the user when the file exists but can't be used.
    pub warning: Option<String>,
    /// The file couldn't be read: leave it alone this run.
    pub keep_file: bool,
}

pub fn load(path: Option<&Path>) -> Loaded {
    let none = Loaded { ui: None, warning: None, keep_file: false };
    let Some(path) = path else { return none };
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return none,
        Err(e) => {
            return Loaded {
                ui: None,
                warning: Some(format!(
                    "The app settings ({}) couldn't be read: {e}. Defaults are used, and the file isn't overwritten.",
                    path.display()
                )),
                keep_file: true,
            };
        }
    };
    match decode(&bytes) {
        Ok(ui) => Loaded { ui: Some(ui), warning: None, keep_file: false },
        Err(e) => {
            // keep the damaged file next to the new one, so nothing in it is lost
            let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
            let aside = path.with_extension(format!("json.corrupt-{stamp}"));
            let kept = std::fs::rename(path, &aside).is_ok();
            Loaded {
                ui: None,
                warning: Some(if kept {
                    format!("The app settings were damaged ({e}); defaults are used. The old file is kept as {}.", aside.display())
                } else {
                    format!("The app settings were damaged ({e}); defaults are used.")
                }),
                keep_file: !kept,
            }
        }
    }
}

/// A saved state; a stored panel layout that no longer validates is dropped, not the whole file.
fn decode(bytes: &[u8]) -> Result<UiState, String> {
    let mut saved: serde_json::Value = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    let fields = saved.as_object_mut().ok_or("settings must be an object")?;
    let bad_docking = fields.get("docking").is_some_and(|v| match serde_json::from_value::<Option<lcc_ui_egui::docking::Workspace>>(v.clone()) {
        Ok(Some(w)) => w.validate().is_err(),
        Ok(None) => false,
        Err(_) => true,
    });
    if bad_docking {
        fields.remove("docking");
    }
    serde_json::from_value::<UiState>(saved).map(UiState::sanitized).map_err(|e| e.to_string())
}

/// Writes the settings when they change.
pub struct Writer {
    path: Option<PathBuf>,
    written: Vec<u8>,
    keep_file: bool,
    checked: f64,
    failing: bool,
}

impl Writer {
    pub fn new(path: Option<PathBuf>, app: &LccApp, keep_file: bool) -> Self {
        Writer { path, written: serde_json::to_vec_pretty(&app.ui).unwrap_or_default(), keep_file, checked: 0.0, failing: false }
    }

    /// Writes `app.ui` if it changed (and the library opened: a temporary session saves nothing).
    pub fn save(&mut self, app: &LccApp) -> Result<(), String> {
        if self.keep_file || app.library_problem.is_some() {
            return Ok(());
        }
        let Some(path) = &self.path else { return Ok(()) };
        let ui = &app.ui;
        let bytes = match &ui.docking {
            Some(w) if w.validate().is_err() => serde_json::to_vec_pretty(&UiState { docking: None, ..ui.clone() }),
            _ => serde_json::to_vec_pretty(ui),
        }
        .map_err(|e| e.to_string())?;
        if bytes == self.written {
            return Ok(());
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("saving the app settings failed: {e}"))?;
        }
        lcc_engine::export::write_file_durable(&path.to_string_lossy(), &bytes).map_err(|e| format!("saving the app settings failed: {e}"))?;
        self.written = bytes;
        Ok(())
    }

    /// Called every frame: looks for changes every couple of seconds.
    pub fn tick(&mut self, app: &mut LccApp, now: f64) {
        if now - self.checked < 2.0 {
            return;
        }
        self.checked = now;
        match self.save(app) {
            Ok(()) => self.failing = false,
            Err(e) => {
                if !self.failing {
                    log::warn!("{e}");
                    app.notices.push(format!("{e}. {} keeps trying.", lcc_brand::DISPLAY_NAME));
                }
                self.failing = true;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_sanitizes_like_other_load_paths() {
        let raw = UiState { develop_classic: false, ..UiState::default() };
        let ui = decode(&serde_json::to_vec(&raw).unwrap()).unwrap();
        assert!(ui.develop_classic, "a loaded state goes through UiState::sanitized");
    }
}
