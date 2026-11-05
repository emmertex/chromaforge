//! The Profile Manager's edits to the profile list (ported from OneSlips, LR-PROF-MANAGE): profiles
//! renamed, moved to another group (one of the user's own), reordered, and built-in looks removed
//! from this install ("Restore Built-in Looks" brings them back). Hidden profiles are
//! `Session::profile_hidden`. The menus and the profile browser list [`crate::Session::shown_groups`];
//! a photo that uses a hidden, renamed or removed profile keeps rendering with it.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Longest profile or group name the manager accepts.
pub const MAX_NAME: usize = 200;
/// Most entries kept in each list or map (a hostile prefs file can't grow them without bound).
pub const MAX_ENTRIES: usize = 10_000;

/// What the user changed about the profile list (saved in `presets.json` as `profileEdits`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ProfileEdits {
    /// Profile id → the user's name for it.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub names: BTreeMap<String, String>,
    /// Profile id → the group it was moved to.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub groups: BTreeMap<String, String>,
    /// Profile ids in the user's order (others follow in their own order).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub order: Vec<String>,
    /// Built-in looks removed from this install.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub removed: Vec<String>,
    /// Group names in the user's order, empty groups made with `profile.groupNew` too (others follow in their own
    /// order).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub group_order: Vec<String>,
}

impl ProfileEdits {
    /// `self` cut to the limits and stripped of empty names (for a prefs file read from disk).
    pub fn bounded(mut self) -> Self {
        let ok = |k: &String, v: &String| !k.is_empty() && !v.trim().is_empty() && v.chars().count() <= MAX_NAME;
        self.names = self.names.into_iter().filter(|(k, v)| ok(k, v)).take(MAX_ENTRIES).collect();
        self.groups = self.groups.into_iter().filter(|(k, v)| ok(k, v)).take(MAX_ENTRIES).collect();
        self.order.truncate(MAX_ENTRIES);
        self.removed.truncate(MAX_ENTRIES);
        self.group_order.retain(|g| !g.trim().is_empty() && g.chars().count() <= MAX_NAME);
        self.group_order.truncate(MAX_ENTRIES);
        self
    }

    pub fn is_empty(&self) -> bool {
        self == &ProfileEdits::default()
    }
}

/// One profile as the menus list it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShownProfile {
    pub id: String,
    /// The user's name when renamed, else its own.
    pub name: String,
    /// Its own name.
    pub original_name: String,
    pub group: String,
    /// Its own group.
    pub original_group: String,
    /// `look` (built in) or `lut` (imported).
    pub kind: &'static str,
    pub hidden: bool,
    /// The name and group are the app's own (translated for display).
    pub builtin_label: bool,
}

/// A group of profiles as the menus list it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ShownGroup {
    pub name: String,
    /// Not one of the app's own groups (an imported folder or the user's group): shown as named.
    pub imported: bool,
    pub profiles: Vec<ShownProfile>,
}

impl crate::Session {
    /// Every profile not removed from this install, hidden ones included, with the user's names and
    /// groups, in the user's order (then built-in looks, then LUT profiles).
    pub fn all_profiles(&self) -> Vec<ShownProfile> {
        let e = &self.profile_edits;
        let item = |id: &str, name: &str, group: &str, kind: &'static str| {
            let renamed = e.names.get(id).filter(|n| !n.trim().is_empty());
            let regrouped = e.groups.get(id).filter(|g| !g.trim().is_empty());
            ShownProfile {
                id: id.to_string(),
                name: renamed.cloned().unwrap_or_else(|| name.to_string()),
                original_name: name.to_string(),
                group: regrouped.cloned().unwrap_or_else(|| group.to_string()),
                original_group: group.to_string(),
                kind,
                hidden: self.profile_hidden.iter().any(|h| h == id),
                builtin_label: kind == "look" && renamed.is_none() && regrouped.is_none(),
            }
        };
        let mut out: Vec<ShownProfile> =
            crate::presets::PROFILES.iter().filter(|p| !e.removed.iter().any(|r| r == p.id)).map(|p| item(p.id, p.name, p.group, "look")).collect();
        out.extend(self.lut_profiles.iter().map(|p| item(&p.id, &p.name, &p.group, "lut")));
        let rank = |id: &str| e.order.iter().position(|o| o == id).unwrap_or(usize::MAX);
        // (a stable sort: profiles the user didn't place keep their own order, after the placed ones)
        out.sort_by_key(|p| rank(&p.id));
        // then the groups in the user's order (`profile.groupMove`); the others keep theirs, after them
        if !e.group_order.is_empty() {
            let group_rank = |g: &str| e.group_order.iter().position(|o| o == g).unwrap_or(usize::MAX);
            out.sort_by_key(|p| group_rank(&p.group));
        }
        out
    }

    /// Every group name in order: the groups of [`Self::all_profiles`] as they come, and the user's empty groups
    /// (`profile.groupNew`) where the user's order puts them.
    pub fn profile_group_names(&self) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        for p in self.all_profiles() {
            if !names.contains(&p.group) {
                names.push(p.group);
            }
        }
        for (i, g) in self.profile_edits.group_order.iter().enumerate() {
            if !names.contains(g) {
                names.insert(i.min(names.len()), g.clone());
            }
        }
        names
    }

    /// The groups the menus and the browser show: each with its shown profiles, in the order the
    /// groups first appear ([`Self::all_profiles`]); a group left empty is left out.
    pub fn shown_groups(&self) -> Vec<ShownGroup> {
        let builtin_groups = crate::presets::profile_groups();
        let mut groups: Vec<ShownGroup> = Vec::new();
        for p in self.all_profiles().into_iter().filter(|p| !p.hidden) {
            let imported = !(p.kind == "look" && builtin_groups.contains(&p.group.as_str()) && !self.profile_edits.groups.contains_key(&p.id));
            match groups.iter_mut().find(|g| g.name == p.group && g.imported == imported) {
                Some(g) => g.profiles.push(p),
                None => groups.push(ShownGroup { name: p.group.clone(), imported, profiles: vec![p] }),
            }
        }
        groups
    }
}
