//! The library's people store ([`crate::people_store`]) on the session: loaded when first
//! needed, saved on every change, and who a mask's `who` (a name, or a role in this shoot) is on
//! a photo. A damaged `people.json` is left as it is and never overwritten.
//!
//! Ported from OneSlips' fork (own design there).

use lcc_catalog::PhotoId;
use lcc_develop::recipe::Who;

use crate::Session;
use crate::people_store::{PeopleStore, PhotoContext};

#[derive(Default)]
pub(crate) struct PeopleState {
    /// The library directory the store was loaded for (`Some(None)`: no library).
    loaded_for: Option<Option<std::path::PathBuf>>,
    store: PeopleStore,
    /// The file couldn't be read: it isn't overwritten.
    blocked: bool,
    /// What went wrong loading it, for the user.
    pub(crate) messages: Vec<String>,
}

/// Who a `who` names on one photo.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resolved {
    /// This person (a name the photo's Face regions use).
    Person(String),
    /// Nobody is bound to the role here: the selection covers everyone, and why.
    Everyone(String),
}

impl Session {
    /// What a photo's scope (for roles) is judged by.
    pub fn photo_context(&self, id: PhotoId) -> PhotoContext {
        let Some(p) = self.catalog.photo(id) else { return PhotoContext { id: id.0, ..Default::default() } };
        let folder = match &p.source {
            lcc_catalog::Source::File { path } => {
                let n = crate::people_store::norm_folder(path);
                n.rsplit_once('/').map(|(dir, _)| dir.to_string())
            }
            _ => None,
        };
        PhotoContext { id: id.0, folder, albums: self.catalog.albums_of(id).into_iter().map(|a| a.0).collect(), date: p.date().to_string() }
    }

    fn people_load(&mut self) {
        let dir = self.library.as_ref().map(|l| l.dir.clone());
        if self.people.loaded_for.as_ref() == Some(&dir) {
            return;
        }
        self.people = PeopleState { loaded_for: Some(dir), ..Default::default() };
        let Some(lib) = self.library.as_mut() else { return };
        match lib.read_file(crate::people_store::FILE) {
            Ok(None) => {}
            Ok(Some(bytes)) => match PeopleStore::from_bytes(&bytes) {
                Ok(s) => self.people.store = s,
                Err(e) => {
                    log::error!("{e}");
                    self.people.messages.push(format!("{e}. It was left as it is; the list starts empty."));
                    self.people.blocked = true;
                }
            },
            Err(e) => {
                log::error!("{}: {e}", crate::people_store::FILE);
                self.people.blocked = true;
            }
        }
    }

    /// The people store (roles, appearance examples).
    pub fn people_store(&mut self) -> &PeopleStore {
        self.people_load();
        &self.people.store
    }

    /// Change the people store and save it.
    pub fn with_people<T>(&mut self, f: impl FnOnce(&mut PeopleStore) -> T) -> Result<T, String> {
        self.people_load();
        if self.people.blocked {
            return Err(format!("{} couldn't be read, so it isn't overwritten; move it aside to start a new one", crate::people_store::FILE));
        }
        let out = f(&mut self.people.store);
        let bytes = self.people.store.to_bytes();
        if let Some(lib) = self.library.as_mut() {
            lib.write_file(crate::people_store::FILE, &bytes).map_err(|e| format!("couldn't save {}: {e}", crate::people_store::FILE))?;
        }
        Ok(out)
    }

    /// Who `who` is on photo `id`.
    pub fn resolve_who(&mut self, id: PhotoId, who: &Who) -> Resolved {
        match who {
            Who::Name(n) => Resolved::Person(n.trim().to_string()),
            Who::Role(r) => {
                let ctx = self.photo_context(id);
                match self.people_store().role_for(r, &ctx) {
                    Some(n) => Resolved::Person(n.to_string()),
                    None => Resolved::Everyone(format!("No {} is set for this shoot, so the mask covers everyone (people.setRole)", r.trim())),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use lcc_develop::recipe::Who;
    use serde_json::json;

    use crate::Session;

    #[test]
    fn roles_resolve_per_photo_and_survive_hostile_arguments() {
        let mut s = Session::with_demo();
        let id = s.active().expect("demo photo");
        let set = s.execute("people.setRole", &json!({"role": "Bride", "person": "Sam", "scope": {"ids": [id.0]}})).expect("set");
        assert_eq!(set["person"], "Sam");
        assert_eq!(s.resolve_who(id, &Who::Role("bride".into())), super::Resolved::Person("Sam".into()));
        assert!(matches!(s.resolve_who(id, &Who::Role("Groom".into())), super::Resolved::Everyone(_)));
        let roles = s.execute("people.roles", &json!({})).expect("roles");
        assert_eq!(roles["here"][0]["person"], "Sam");
        for bad in [
            json!({}),
            json!({"role": "", "person": "x"}),
            json!({"role": "x", "person": "y", "scope": 7}),
            json!({"role": "x".repeat(300), "person": "y"}),
        ] {
            assert!(s.execute("people.setRole", &bad).is_err(), "{bad}");
        }
        assert_eq!(s.execute("people.clearRole", &json!({"role": "BRIDE"})).expect("clear")["removed"], 1);
        assert!(s.execute("people.forgetAll", &json!({})).is_ok());
        assert!(s.people_store().roles.is_empty());
    }
}
