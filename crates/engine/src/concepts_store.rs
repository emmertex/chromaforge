//! Remembered objects ("Wedding dress", "Groom's suit"): what the photographer taught, kept as
//! `concepts.json` in the library (like `people.json`). Each example is opaque JSON here
//! (`lcc_segment::concept::Example`: SAM 2.1 feature prototypes and colours of one taught
//! selection; the AI crate isn't in every build).

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const FILE: &str = "concepts.json";
const MAX_BYTES: usize = 64 << 20;
/// Examples a concept keeps (the oldest go first).
pub const MAX_EXAMPLES: usize = 12;
const MAX_CONCEPTS: usize = 500;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Concept {
    pub name: String,
    #[serde(default)]
    pub examples: Vec<Value>,
    /// The photos it was taught on (for the list; ids).
    #[serde(default)]
    pub photos: Vec<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ConceptStore {
    pub version: u32,
    #[serde(default)]
    pub concepts: Vec<Concept>,
}

impl Default for ConceptStore {
    fn default() -> Self {
        ConceptStore { version: 1, concepts: Vec::new() }
    }
}

fn same(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

impl ConceptStore {
    pub fn from_bytes(bytes: &[u8]) -> Result<ConceptStore, String> {
        if bytes.len() > MAX_BYTES {
            return Err(format!("{FILE} is unexpectedly large ({} MB)", bytes.len() >> 20));
        }
        let mut s: ConceptStore = serde_json::from_slice(bytes).map_err(|e| format!("{FILE} is damaged: {e}"))?;
        s.concepts.truncate(MAX_CONCEPTS);
        for c in &mut s.concepts {
            let n = c.examples.len();
            if n > MAX_EXAMPLES {
                c.examples.drain(..n - MAX_EXAMPLES);
            }
        }
        Ok(s)
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).unwrap_or_default()
    }

    pub fn get(&self, name: &str) -> Option<&Concept> {
        self.concepts.iter().find(|c| same(&c.name, name))
    }

    /// Add an example to `name` (created when new). → how many examples it has.
    pub fn teach(&mut self, name: &str, photo: u64, example: Value) -> Result<usize, String> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 120 {
            return Err("give the object a name (at most 120 characters)".into());
        }
        let i = match self.concepts.iter().position(|c| same(&c.name, name)) {
            Some(i) => i,
            None => {
                if self.concepts.len() >= MAX_CONCEPTS {
                    return Err("too many remembered objects".into());
                }
                self.concepts.push(Concept { name: name.into(), ..Default::default() });
                self.concepts.len() - 1
            }
        };
        let c = self.concepts.get_mut(i).ok_or("no such object")?;
        c.examples.push(example);
        if !c.photos.contains(&photo) {
            c.photos.push(photo);
        }
        let n = c.examples.len();
        if n > MAX_EXAMPLES {
            c.examples.drain(..n - MAX_EXAMPLES);
        }
        Ok(c.examples.len())
    }

    pub fn rename(&mut self, from: &str, to: &str) -> Result<(), String> {
        let to = to.trim();
        if to.is_empty() {
            return Err("missing name".into());
        }
        if !same(from, to) && self.get(to).is_some() {
            return Err(format!("an object called “{to}” exists already"));
        }
        let c = self.concepts.iter_mut().find(|c| same(&c.name, from)).ok_or_else(|| format!("no object called “{from}”"))?;
        c.name = to.into();
        Ok(())
    }

    /// Add `other`'s objects: the same name (any case) is one object, its examples added (the newest
    /// [`MAX_EXAMPLES`] kept, the same example once). → objects added or grown.
    pub fn merge(&mut self, other: ConceptStore) -> usize {
        let mut changed = 0;
        for theirs in other.concepts {
            let room = self.concepts.len() < MAX_CONCEPTS;
            match self.concepts.iter_mut().find(|c| same(&c.name, &theirs.name)) {
                Some(mine) => {
                    let before = mine.examples.len();
                    for ex in theirs.examples {
                        if !mine.examples.contains(&ex) {
                            mine.examples.push(ex);
                        }
                    }
                    for p in theirs.photos {
                        if !mine.photos.contains(&p) {
                            mine.photos.push(p);
                        }
                    }
                    let n = mine.examples.len();
                    if n > MAX_EXAMPLES {
                        mine.examples.drain(..n - MAX_EXAMPLES);
                    }
                    if n != before {
                        changed += 1;
                    }
                }
                None if room => {
                    self.concepts.push(theirs);
                    changed += 1;
                }
                None => {}
            }
        }
        changed
    }

    pub fn forget(&mut self, name: &str) -> bool {
        let n = self.concepts.len();
        self.concepts.retain(|c| !same(&c.name, name));
        n != self.concepts.len()
    }
}

/// The store on the session: loaded when first needed, saved on every change; a damaged file is
/// left as it is and never overwritten.
#[derive(Default)]
pub(crate) struct ConceptState {
    loaded_for: Option<Option<std::path::PathBuf>>,
    store: ConceptStore,
    blocked: bool,
}

impl crate::Session {
    fn concepts_load(&mut self) {
        let dir = self.library.as_ref().map(|l| l.dir.clone());
        if self.concepts.loaded_for.as_ref() == Some(&dir) {
            return;
        }
        self.concepts = ConceptState { loaded_for: Some(dir), ..Default::default() };
        let Some(lib) = self.library.as_mut() else { return };
        match lib.read_file(FILE) {
            Ok(None) => {}
            Ok(Some(bytes)) => match ConceptStore::from_bytes(&bytes) {
                Ok(s) => self.concepts.store = s,
                Err(e) => {
                    log::error!("{e}");
                    self.concepts.blocked = true;
                }
            },
            Err(e) => {
                log::error!("{FILE}: {e}");
                self.concepts.blocked = true;
            }
        }
    }

    /// The remembered objects.
    pub fn concept_store(&mut self) -> &ConceptStore {
        self.concepts_load();
        &self.concepts.store
    }

    /// Change the remembered objects and save them.
    pub fn with_concepts<T>(&mut self, f: impl FnOnce(&mut ConceptStore) -> T) -> Result<T, String> {
        self.concepts_load();
        if self.concepts.blocked {
            return Err(format!("{FILE} couldn't be read, so it isn't overwritten; move it aside to start a new one"));
        }
        let out = f(&mut self.concepts.store);
        let bytes = self.concepts.store.to_bytes();
        if let Some(lib) = self.library.as_mut() {
            lib.write_file(FILE, &bytes).map_err(|e| format!("couldn't save {FILE}: {e}"))?;
        }
        Ok(out)
    }

    /// Add a learned example to remembered object `name`. → how many examples it has.
    pub fn store_concept_example(&mut self, name: &str, photo: lcc_catalog::PhotoId, example: Value) -> Result<usize, String> {
        self.with_concepts(|st| st.teach(name, photo.0, example))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn teach_rename_forget() {
        let mut s = ConceptStore::default();
        assert_eq!(s.teach("Wedding dress", 1, Value::Null).unwrap(), 1);
        assert_eq!(s.teach("wedding DRESS", 2, Value::Null).unwrap(), 2);
        for i in 0..20 {
            s.teach("Wedding dress", 3 + i, Value::Null).unwrap();
        }
        assert_eq!(s.get("Wedding dress").unwrap().examples.len(), MAX_EXAMPLES);
        assert!(s.teach(" ", 1, Value::Null).is_err());
        s.teach("Suit", 1, Value::Null).unwrap();
        assert!(s.rename("Suit", "wedding dress").is_err());
        s.rename("Suit", "Groom's suit").unwrap();
        assert!(s.get("groom's suit").is_some());
        assert!(s.forget("GROOM'S SUIT"));
        assert!(ConceptStore::from_bytes(b"[1,2").is_err());
        // merging: one object per name, examples added once
        let mut other = ConceptStore::default();
        other.teach("WEDDING DRESS", 9, Value::from(1)).unwrap();
        other.teach("Veil", 9, Value::from(2)).unwrap();
        let mut mine = ConceptStore::default();
        mine.teach("Wedding dress", 1, Value::from(1)).unwrap();
        assert_eq!(mine.merge(other.clone()), 1);
        assert_eq!(mine.concepts.len(), 2);
        assert_eq!(mine.get("wedding dress").unwrap().examples.len(), 1);
        assert_eq!(mine.merge(other), 0, "twice is once");
        assert_eq!(ConceptStore::from_bytes(&s.to_bytes()).unwrap(), s);
    }
}
