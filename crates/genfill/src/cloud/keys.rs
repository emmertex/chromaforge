//! The user's API keys: kept on this machine only, never in the catalog, XMP, exports or logs.
//!
//! A key comes from `<PREFIX>_GENFILL_<PROVIDER>_KEY` (e.g. `<PREFIX>_GENFILL_OPENAI_KEY`) when
//! that is set, else from `genfill-keys.json` in the app's settings folder (the engine passes its
//! path). The file is written whole to a temporary name and renamed, and on Unix it is readable by
//! its owner only (0600). On Windows it lives in the user's own profile folder (`%APPDATA%`),
//! which only that user (and administrators) can read; Windows Credential Manager would need a
//! native call, which the app keeps out of everything but `crates/sysmem` (a proposed follow-up).
//!
//! Nothing here ever returns a key to a caller that shows things: [`KeyStore::status`] says where a
//! key comes from and [`KeyStore::hint`] its last four characters.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::Provider;

/// Where a provider's key comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeySource {
    None,
    /// The settings file.
    Settings,
    /// The environment variable.
    Env,
}

impl KeySource {
    pub fn id(self) -> &'static str {
        match self {
            KeySource::None => "none",
            KeySource::Settings => "settings",
            KeySource::Env => "env",
        }
    }
}

/// The keys file (`None`: environment variables only).
#[derive(Clone, Debug, Default)]
pub struct KeyStore {
    path: Option<PathBuf>,
}

/// Largest keys file read.
const MAX_FILE: u64 = 64 * 1024;

/// The environment variable for `p`'s key.
pub fn env_var(p: Provider) -> String {
    lcc_brand::env_var(&env_name(p))
}

/// [`env_var`] without the app's prefix (what [`lcc_brand::env`] takes).
fn env_name(p: Provider) -> String {
    format!("GENFILL_{}_KEY", p.info().id.to_ascii_uppercase())
}

/// Whether `key` looks like an API key at all (printable ASCII without spaces, 8…512 characters).
pub fn plausible(key: &str) -> bool {
    (8..=512).contains(&key.len()) && key.chars().all(|c| c.is_ascii_graphic())
}

impl KeyStore {
    pub fn new(path: Option<PathBuf>) -> KeyStore {
        KeyStore { path }
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    fn read(&self) -> BTreeMap<String, String> {
        let Some(p) = &self.path else { return BTreeMap::new() };
        if std::fs::metadata(p).map(|m| m.len() > MAX_FILE).unwrap_or(true) {
            return BTreeMap::new();
        }
        std::fs::read(p).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    /// The key for `p` and where it came from.
    pub fn get(&self, p: Provider) -> Option<(String, KeySource)> {
        if let Some(k) = lcc_brand::env(&env_name(p))
            && plausible(k.trim())
        {
            return Some((k.trim().to_string(), KeySource::Env));
        }
        self.read().get(p.info().id).map(|k| k.trim().to_string()).filter(|k| plausible(k)).map(|k| (k, KeySource::Settings))
    }

    pub fn status(&self, p: Provider) -> KeySource {
        self.get(p).map_or(KeySource::None, |(_, s)| s)
    }

    /// The key's last four characters (`…abcd`), to tell keys apart in Settings.
    pub fn hint(&self, p: Provider) -> Option<String> {
        let (k, _) = self.get(p)?;
        let tail: String = k.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
        Some(format!("…{tail}"))
    }

    /// Store (`Some`) or forget (`None`) the key for `p` in the settings file.
    pub fn set(&self, p: Provider, key: Option<&str>) -> Result<(), String> {
        let path = self.path.as_ref().ok_or("there is no settings folder to keep the key in; set the environment variable instead")?;
        let mut all = self.read();
        match key.map(str::trim) {
            Some(k) if !plausible(k) => return Err("that does not look like an API key (no spaces, 8 to 512 characters)".into()),
            Some(k) => {
                all.insert(p.info().id.to_string(), k.to_string());
            }
            None => {
                all.remove(p.info().id);
            }
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("could not create the settings folder: {e}"))?;
        }
        let tmp = path.with_extension("json.tmp");
        let body = serde_json::to_vec_pretty(&all).map_err(|e| e.to_string())?;
        write_private(&tmp, &body).map_err(|e| format!("could not save the key: {e}"))?;
        std::fs::rename(&tmp, path).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            format!("could not save the key: {e}")
        })
    }
}

#[cfg(unix)]
fn write_private(path: &Path, body: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path)?;
    f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    f.write_all(body)?;
    f.sync_all()
}

#[cfg(not(unix))]
fn write_private(path: &Path, body: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).open(path)?;
    f.write_all(body)?;
    f.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_stored_forgotten_and_never_shown() {
        let dir = std::env::temp_dir().join(format!("lc-genfill-keys-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = KeyStore::new(Some(dir.join("genfill-keys.json")));
        assert_eq!(store.status(Provider::Bfl), KeySource::None);
        store.set(Provider::Bfl, Some("  bfl-test-key-1234 ")).unwrap();
        assert_eq!(store.get(Provider::Bfl), Some(("bfl-test-key-1234".to_string(), KeySource::Settings)));
        assert_eq!(store.hint(Provider::Bfl).as_deref(), Some("…1234"));
        assert!(store.set(Provider::Bfl, Some("has space inside")).is_err());
        store.set(Provider::Bfl, None).unwrap();
        assert_eq!(store.status(Provider::Bfl), KeySource::None);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join("genfill-keys.json")).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        // a damaged file is no key, not a crash
        std::fs::write(dir.join("genfill-keys.json"), b"{not json").unwrap();
        assert_eq!(store.status(Provider::Bfl), KeySource::None);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(KeyStore::new(None).set(Provider::Bfl, Some("abcdefgh123")).is_err());
    }

    #[test]
    fn redaction_hides_the_key() {
        let k = "sk-proj-abcdefghijklmnop";
        assert_eq!(super::super::redact(&format!("bad key {k} given"), k), "bad key ••• given");
        assert_eq!(super::super::redact("Incorrect API key provided: sk-proj-abcd****mnop", k), "Incorrect API key provided: •••****mnop");
        assert_eq!(super::super::redact("short", "abc"), "short");
    }
}
