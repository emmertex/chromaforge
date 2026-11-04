//! Who is who, for masks and presets: per-shoot roles ("Bride" is Sam in this folder) and the
//! appearance examples that recognise a named person on other photos. Kept in the library as
//! `people.json` — local only: never written to sidecars, XMP, exports or the network. The faces'
//! names themselves stay where they already are (the photos' Face regions).
//!
//! Face signatures are biometric data of the photographer's clients: `people.forget` /
//! `people.forgetAll` delete them (see `docs/people-and-roles.md`).
//!
//! The appearance signatures are stored as opaque JSON here (their type lives in the AI crate,
//! which not every build has).

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The store's file in the library.
pub const FILE: &str = "people.json";
/// Largest store read (a damaged or hostile file isn't parsed).
const MAX_BYTES: usize = 32 << 20;
/// Appearance examples kept per person (the newest replace the oldest).
pub const MAX_EXAMPLES: usize = 64;
/// Role bindings kept.
const MAX_ROLES: usize = 5000;

/// Where a role applies.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Scope {
    /// A folder and its sub-folders (path as stored in the catalog).
    Folder(String),
    /// An album.
    Album(u64),
    /// Photos taken in a date range (inclusive, `YYYY-MM-DD`).
    Dates { from: String, to: String },
    /// These photos.
    Photos(Vec<u64>),
}

/// "In this scope, `role` is `person`."
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleBinding {
    pub role: String,
    pub person: String,
    pub scope: Scope,
}

/// One face of a named person, remembered for recognising them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Example {
    pub photo: u64,
    /// The face box (normalized, oriented frame).
    pub rect: [f64; 4],
    /// The appearance signature (`an appearance signature from the AI crate`).
    pub signature: Value,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PersonData {
    pub name: String,
    #[serde(default)]
    pub examples: Vec<Example>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PeopleStore {
    pub version: u32,
    #[serde(default)]
    pub roles: Vec<RoleBinding>,
    #[serde(default)]
    pub people: Vec<PersonData>,
}

impl Default for PeopleStore {
    fn default() -> Self {
        PeopleStore { version: 1, roles: Vec::new(), people: Vec::new() }
    }
}

/// What a photo's scope is judged by.
#[derive(Clone, Debug, Default)]
pub struct PhotoContext {
    pub id: u64,
    /// The folder holding it ('/'-separated).
    pub folder: Option<String>,
    pub albums: Vec<u64>,
    /// Its date (`YYYY-MM-DD…`).
    pub date: String,
}

/// `path` with '/' separators and no trailing one, for comparing folders.
pub fn norm_folder(path: &str) -> String {
    let p = path.replace('\\', "/");
    p.trim_end_matches('/').to_string()
}

fn same(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

impl Scope {
    /// How specifically it covers the photo (higher wins), or `None` when it doesn't.
    fn rank(&self, p: &PhotoContext) -> Option<usize> {
        match self {
            Scope::Photos(ids) => ids.contains(&p.id).then_some(1_000_000),
            Scope::Album(a) => p.albums.contains(a).then_some(100_000),
            Scope::Folder(f) => {
                let f = norm_folder(f);
                let photo = p.folder.as_deref().map(norm_folder)?;
                let inside = photo.eq_ignore_ascii_case(&f)
                    || (photo.len() > f.len()
                        && photo.get(..f.len()).is_some_and(|h| h.eq_ignore_ascii_case(&f))
                        && photo.get(f.len()..).is_some_and(|t| t.starts_with('/')));
                // a deeper folder is more specific
                inside.then_some(1000 + f.len().min(9000))
            }
            Scope::Dates { from, to } => {
                let d = p.date.get(..10).unwrap_or(&p.date);
                (!d.is_empty() && from.as_str() <= d && d <= to.as_str()).then_some(10)
            }
        }
    }
}

impl PeopleStore {
    /// Parse a stored file; a damaged one is an error (the caller starts empty and says so).
    pub fn from_bytes(bytes: &[u8]) -> Result<PeopleStore, String> {
        if bytes.len() > MAX_BYTES {
            return Err(format!("{FILE} is unexpectedly large ({} MB)", bytes.len() >> 20));
        }
        let mut s: PeopleStore = serde_json::from_slice(bytes).map_err(|e| format!("{FILE} is damaged: {e}"))?;
        s.roles.truncate(MAX_ROLES);
        for p in &mut s.people {
            let n = p.examples.len();
            if n > MAX_EXAMPLES {
                p.examples.drain(..n - MAX_EXAMPLES);
            }
        }
        Ok(s)
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).unwrap_or_default()
    }

    /// Who has `role` for this photo (the most specific binding).
    pub fn role_for(&self, role: &str, p: &PhotoContext) -> Option<&str> {
        self.roles
            .iter()
            .filter(|b| same(&b.role, role))
            .filter_map(|b| b.scope.rank(p).map(|r| (r, b)))
            .max_by_key(|(r, _)| *r)
            .map(|(_, b)| b.person.as_str())
    }

    /// The roles that apply to this photo: `(role, person)`, one per role.
    pub fn roles_for(&self, p: &PhotoContext) -> Vec<(String, String)> {
        let mut roles: Vec<String> = self.roles.iter().map(|b| b.role.clone()).collect();
        roles.sort_by_key(|r| r.to_lowercase());
        roles.dedup_by(|a, b| same(a, b));
        roles.into_iter().filter_map(|r| self.role_for(&r, p).map(|n| (r.clone(), n.to_string()))).collect()
    }

    /// The roles `person` has for this photo.
    pub fn person_roles(&self, person: &str, p: &PhotoContext) -> Vec<String> {
        self.roles_for(p).into_iter().filter(|(_, n)| same(n, person)).map(|(r, _)| r).collect()
    }

    /// Bind `role` to `person` in `scope` (replacing the role's binding in the same scope).
    pub fn set_role(&mut self, role: &str, person: &str, scope: Scope) -> Result<(), String> {
        let (role, person) = (role.trim(), person.trim());
        if role.is_empty() || person.is_empty() {
            return Err("a role and a person are needed".into());
        }
        if role.chars().count() > 80 || person.chars().count() > 200 {
            return Err("that name is too long".into());
        }
        self.roles.retain(|b| !(same(&b.role, role) && b.scope == scope));
        if self.roles.len() >= MAX_ROLES {
            return Err("too many roles".into());
        }
        self.roles.push(RoleBinding { role: role.into(), person: person.into(), scope });
        Ok(())
    }

    /// Remove `role`'s bindings (in `scope`, or everywhere); how many went.
    pub fn clear_role(&mut self, role: &str, scope: Option<&Scope>) -> usize {
        let n = self.roles.len();
        self.roles.retain(|b| !(same(&b.role, role) && scope.is_none_or(|s| *s == b.scope)));
        n - self.roles.len()
    }

    pub fn person(&self, name: &str) -> Option<&PersonData> {
        self.people.iter().find(|p| same(&p.name, name))
    }

    /// Remember a face of `name` (a face already remembered for that photo and box is replaced).
    pub fn add_example(&mut self, name: &str, ex: Example) {
        let name = name.trim();
        if name.is_empty() {
            return;
        }
        // a face belongs to one person: drop it from whoever had it
        for p in &mut self.people {
            p.examples.retain(|e| !(e.photo == ex.photo && iou(e.rect, ex.rect) > 0.5));
        }
        let i = match self.people.iter().position(|p| same(&p.name, name)) {
            Some(i) => i,
            None => {
                self.people.push(PersonData { name: name.into(), examples: Vec::new() });
                self.people.len() - 1
            }
        };
        if let Some(p) = self.people.get_mut(i) {
            p.examples.push(ex);
            let n = p.examples.len();
            if n > MAX_EXAMPLES {
                p.examples.drain(..n - MAX_EXAMPLES);
            }
        }
        self.people.retain(|p| !p.examples.is_empty());
    }

    /// Rename `from` to `to` everywhere (merging into `to` when that person exists).
    pub fn rename(&mut self, from: &str, to: &str) {
        let to = to.trim();
        for b in &mut self.roles {
            if same(&b.person, from) {
                b.person = to.into();
            }
        }
        let moved: Vec<Example> = self.people.iter_mut().filter(|p| same(&p.name, from)).flat_map(|p| std::mem::take(&mut p.examples)).collect();
        self.people.retain(|p| !same(&p.name, from) || !p.examples.is_empty());
        for e in moved {
            self.add_example(to, e);
        }
    }

    /// Delete `name`'s examples and roles; whether there was anything.
    pub fn forget(&mut self, name: &str) -> bool {
        let n = (self.people.len(), self.roles.len());
        self.people.retain(|p| !same(&p.name, name));
        self.roles.retain(|b| !same(&b.person, name));
        n != (self.people.len(), self.roles.len())
    }

    /// Forget the examples remembered on these photos (they were removed).
    pub fn forget_photos(&mut self, photos: &[u64]) {
        for p in &mut self.people {
            p.examples.retain(|e| !photos.contains(&e.photo));
        }
        self.people.retain(|p| !p.examples.is_empty());
    }
}

fn iou(a: [f64; 4], b: [f64; 4]) -> f64 {
    let ix = (a[2].min(b[2]) - a[0].max(b[0])).max(0.0);
    let iy = (a[3].min(b[3]) - a[1].max(b[1])).max(0.0);
    let inter = ix * iy;
    let union = (a[2] - a[0]).max(0.0) * (a[3] - a[1]).max(0.0) + (b[2] - b[0]).max(0.0) * (b[3] - b[1]).max(0.0) - inter;
    if union > 0.0 { inter / union } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(folder: &str, date: &str) -> PhotoContext {
        PhotoContext { id: 7, folder: Some(folder.into()), albums: vec![3], date: date.into() }
    }

    #[test]
    fn roles_resolve_per_shoot_most_specific_first() {
        let mut s = PeopleStore::default();
        s.set_role("Bride", "Sam", Scope::Folder("W:/Weddings/2026-09-27 Alex and Sam".into())).unwrap();
        s.set_role("Bride", "Chloe", Scope::Folder("W:\\Weddings\\2025-09-04 Chloe".into())).unwrap();
        s.set_role("Groom", "Jack", Scope::Dates { from: "2025-09-04".into(), to: "2025-09-04".into() }).unwrap();
        let sam = ctx("W:/Weddings/2026-09-27 Alex and Sam/RAWs", "2026-09-27T10:00");
        assert_eq!(s.role_for("bride", &sam), Some("Sam"));
        let chloe = ctx("W:/Weddings/2025-09-04 Chloe/RAWs", "2025-09-04T12:00:00");
        assert_eq!(s.role_for("Bride", &chloe), Some("Chloe"));
        assert_eq!(s.role_for("Groom", &chloe), Some("Jack"));
        assert_eq!(s.role_for("Groom", &sam), None);
        // a folder whose name merely starts the same isn't inside
        assert_eq!(s.role_for("Bride", &ctx("W:/Weddings/2025-09-04 Chloe and Johnathon", "")), None);
        // an explicit photo binding beats the folder
        s.set_role("Bride", "Alex", Scope::Photos(vec![7])).unwrap();
        assert_eq!(s.role_for("Bride", &sam), Some("Alex"));
        assert_eq!(s.roles_for(&chloe), vec![("Bride".into(), "Alex".into()), ("Groom".into(), "Jack".into())]);
        assert_eq!(s.clear_role("bride", None), 3);
        assert!(s.set_role(" ", "x", Scope::Album(1)).is_err());
    }

    #[test]
    fn examples_rename_merge_and_forget() {
        let mut s = PeopleStore::default();
        let ex = |photo, x: f64| Example { photo, rect: [x, 0.1, x + 0.1, 0.2], signature: Value::Null };
        s.add_example("Jack", ex(1, 0.1));
        s.add_example("Jack", ex(2, 0.1));
        s.add_example("Sam", ex(1, 0.5));
        // the same face named again moves to the new name
        s.add_example("Robin", ex(1, 0.5));
        assert!(s.person("Sam").is_none());
        s.rename("Robin", "Jack");
        assert_eq!(s.person("jack").map(|p| p.examples.len()), Some(3));
        s.set_role("Groom", "Jack", Scope::Album(1)).unwrap();
        s.forget_photos(&[2]);
        assert_eq!(s.person("Jack").map(|p| p.examples.len()), Some(2));
        assert!(s.forget("JACK"));
        assert!(s.people.is_empty() && s.roles.is_empty());
        for _ in 0..(MAX_EXAMPLES + 5) {
            s.add_example("Many", ex(9, 0.0).clone());
            s.people.iter_mut().for_each(|p| p.examples.iter_mut().for_each(|e| e.photo += 100));
        }
        assert!(s.person("Many").is_some_and(|p| p.examples.len() <= MAX_EXAMPLES));
    }

    #[test]
    fn damaged_files_are_errors_not_crashes() {
        assert!(PeopleStore::from_bytes(b"{not json").is_err());
        assert!(PeopleStore::from_bytes(br#"{"version":1,"roles":[{"role":1}]}"#).is_err());
        let s = PeopleStore::default();
        assert_eq!(PeopleStore::from_bytes(&s.to_bytes()).unwrap(), s);
    }
}
