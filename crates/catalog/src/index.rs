//! Lookups derived from the catalog's own data, so questions asked once per photo of a big library
//! don't scan a list each time: which stack a photo is in, whether a regular album holds it.
//!
//! They are never saved (the on-disk format is unchanged). Each is built on first use from what
//! the catalog holds, so a catalog read from a snapshot, replayed from the journal or cloned starts
//! without them and builds them when asked. Once built, [`crate::Catalog::apply`] keeps them up to
//! date: the ops that change stacks or album photos (`AddStack`, `RemoveStack`, `SetStack`,
//! `AddAlbum`, `RemoveAlbum`, `SetAlbumPhotos`) update them as they apply, inverses and the
//! rollback of a failed batch included, as those are ops too. The keyword index is dropped by
//! every op that can change a keyword and rebuilt when next asked.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use crate::{AlbumId, Catalog, Op, PhotoId, StackId};

#[derive(Default)]
pub(crate) struct Indexes {
    /// Photo → the stack it is in; `None` when the catalog was read with a photo in two stacks
    /// (only a hand-edited file can hold that: ops refuse it), where a scan says which one counts.
    stacks: OnceLock<Option<HashMap<PhotoId, StackId>>>,
    /// Album → its photos (regular albums; a smart album's list is empty).
    albums: OnceLock<HashMap<AlbumId, HashSet<PhotoId>>>,
    /// The library's keywords (dropped by any op that can change them, see [`Indexes::op_applied`]).
    keywords: OnceLock<crate::keywords::KeywordIndex>,
    /// The keyword tree built from it (dropped with it).
    keyword_tree: OnceLock<std::sync::Arc<Vec<crate::keywords::KeywordNode>>>,
    /// Photo → its lower-cased free-text search fields ([`crate::query::search_fields_of`]), made by
    /// the first pass over the library that searches text. A photo an op changes is taken out (it is
    /// read from its own fields until the whole index is made again).
    search: OnceLock<HashMap<PhotoId, Box<[Box<str>]>>>,
    /// Photo → the lower-cased text a smart album's Text rule looks through ([`crate::rules::rule_text_of`]).
    rule_text: OnceLock<HashMap<PhotoId, Box<str>>>,
}

/// A copy builds its own when asked (copies are made to be saved, not queried).
impl Clone for Indexes {
    fn clone(&self) -> Self {
        Indexes::default()
    }
}

/// Derived from the rest of the catalog: two catalogs with the same data are equal whatever was
/// built.
impl PartialEq for Indexes {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl std::fmt::Debug for Indexes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let stacks = self.stacks.get().map(|m| m.as_ref().map(HashMap::len));
        f.debug_struct("Indexes")
            .field("stacks", &stacks)
            .field("albums", &self.albums.get().map(HashMap::len))
            .field("keywords", &self.keywords.get().is_some())
            .field("keyword_tree", &self.keyword_tree.get().is_some())
            .field("search", &self.search.get().map(HashMap::len))
            .field("rule_text", &self.rule_text.get().map(HashMap::len))
            .finish()
    }
}

impl Indexes {
    /// `op` was applied (on its own or in a batch): forget what it may have changed.
    pub(crate) fn op_applied(&mut self, op: &Op) {
        // (only the kinds that leave every photo's keywords, its presence and the keyword list as
        // they were keep the keyword index; a new kind drops it)
        let keeps_keywords = matches!(
            op,
            Op::SetRating { .. }
                | Op::SetFlag { .. }
                | Op::SetLabel { .. }
                | Op::SetDevelop { .. }
                | Op::SetVersions { .. }
                | Op::SetHistory { .. }
                | Op::PushHistory { .. }
                | Op::SetHistoryCursor { .. }
                | Op::SetPreview { .. }
                | Op::SetCaptured { .. }
                | Op::SetFile { .. }
                | Op::Relink { .. }
                | Op::SetSha1 { .. }
                | Op::SetAnalysis { .. }
                | Op::Batch { .. }
        );
        if !keeps_keywords {
            self.keywords = OnceLock::new();
            self.keyword_tree = OnceLock::new();
        }
    }

    /// An op changes photo `id` (or adds or removes it), of a library of `photos`: its search text
    /// is made again from the photo when next asked. Once an eighth of the library is not in a
    /// text index, the index is dropped, to be made whole again by the next pass.
    pub(crate) fn photo_touched(&mut self, id: PhotoId, photos: usize) {
        let gone = |len: usize| len.saturating_mul(8) < photos.saturating_mul(7);
        if let Some(m) = self.search.get_mut() {
            m.remove(&id);
            if gone(m.len()) {
                self.search = OnceLock::new();
            }
        }
        if let Some(m) = self.rule_text.get_mut() {
            m.remove(&id);
            if gone(m.len()) {
                self.rule_text = OnceLock::new();
            }
        }
    }

    /// Stack `id` held `old` and holds `new` now (empty: it was added, or is gone).
    pub(crate) fn stack_changed(&mut self, id: StackId, old: &[PhotoId], new: &[PhotoId]) {
        let Some(Some(m)) = self.stacks.get_mut() else { return };
        for p in old {
            if m.get(p) == Some(&id) {
                m.remove(p);
            }
        }
        for p in new {
            m.insert(*p, id);
        }
    }

    /// Album `id` holds `photos` now (`None`: it is gone).
    pub(crate) fn album_changed(&mut self, id: AlbumId, photos: Option<&[PhotoId]>) {
        let Some(m) = self.albums.get_mut() else { return };
        match photos {
            Some(photos) => {
                m.insert(id, photos.iter().copied().collect());
            }
            None => {
                m.remove(&id);
            }
        }
    }
}

impl Catalog {
    /// Whether `p` is this catalog's own photo (not a copy with other values tried against it).
    fn is_own(&self, p: &crate::Photo) -> bool {
        self.photos.get(&p.id).is_some_and(|own| std::ptr::eq(&**own, p))
    }

    /// `p`'s lower-cased free-text search fields, when `p` is this catalog's own photo and they are
    /// kept. `build`: make them for every photo first when they weren't (a pass over the whole
    /// library does; testing one photo shouldn't make them for 300 000).
    pub(crate) fn search_fields(&self, p: &crate::Photo, build: bool) -> Option<&[Box<str>]> {
        if !self.is_own(p) {
            return None;
        }
        let m = if build {
            self.index.search.get_or_init(|| self.photos.values().map(|p| (p.id, crate::query::search_fields_of(p))).collect())
        } else {
            self.index.search.get()?
        };
        m.get(&p.id).map(|f| &**f)
    }

    /// `p`'s lower-cased, trimmed Text-rule text when `p` is this catalog's own photo, kept for
    /// every photo from the first ask (a smart album's count or view asks photo by photo).
    pub(crate) fn rule_text(&self, p: &crate::Photo) -> Option<&str> {
        if !self.is_own(p) {
            return None;
        }
        let m = self.index.rule_text.get_or_init(|| self.photos.values().map(|p| (p.id, crate::rules::rule_text_of(p).into())).collect());
        m.get(&p.id).map(|t| &**t)
    }

    /// The stack photo `photo` is in, by the index (built on first use).
    pub(crate) fn stack_id_of(&self, photo: PhotoId) -> Option<StackId> {
        let m = self.index.stacks.get_or_init(|| {
            let mut m = HashMap::new();
            for s in self.stacks.values() {
                for p in &s.photos {
                    if m.insert(*p, s.id).is_some_and(|other| other != s.id) {
                        return None;
                    }
                }
            }
            Some(m)
        });
        match m {
            Some(m) => m.get(&photo).copied(),
            // in id order, the first stack listing the photo has it
            None => self.stacks.values().find(|s| s.photos.contains(&photo)).map(|s| s.id),
        }
    }

    /// Whether album `album`'s own list holds `photo`, by the index (built on first use).
    pub(crate) fn album_lists(&self, album: AlbumId, photo: PhotoId) -> bool {
        let m = self.index.albums.get_or_init(|| self.albums.values().map(|a| (a.id, a.photos.iter().copied().collect())).collect());
        m.get(&album).is_some_and(|s| s.contains(&photo))
    }

    /// The library's keywords, by the index (built on first use).
    pub(crate) fn keyword_index(&self) -> &crate::keywords::KeywordIndex {
        self.index.keywords.get_or_init(|| crate::keywords::KeywordIndex::build(self))
    }

    /// The keyword tree, by the index (built by `build` on first use).
    pub(crate) fn cached_keyword_tree(
        &self,
        build: impl FnOnce() -> std::sync::Arc<Vec<crate::keywords::KeywordNode>>,
    ) -> std::sync::Arc<Vec<crate::keywords::KeywordNode>> {
        self.index.keyword_tree.get_or_init(build).clone()
    }
}
