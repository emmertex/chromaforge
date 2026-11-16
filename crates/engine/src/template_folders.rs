//! Template folders: the one model behind every template browser that groups its templates.
//!
//! A saved template is `{name, folder, settings}`; `folder` is `None` for [`DEFAULT_FOLDER`] and
//! otherwise the name of one of the owner's listed folders (empty ones included). A folder name
//! is a label stored in a preferences file, never a path: `/`, `\` and `..` are kept as typed and
//! no file or directory is made for a folder.
//!
//! The model is generic over the template type through [`Foldered`], so the engine's
//! `print.template.*` store, the Print module's own templates and the Slideshow module's templates
//! share the same rules (names, limits, case-insensitive lookup, rename and delete).
//!
//! Port of the engine part of an upstream change (upstream PR 3459, `print/template.rs` and
//! `cmd/print.rs`), generalised from print templates to any template type.

/// The folder a template is in until it is moved to another. It always exists and can't be
/// renamed or deleted.
pub const DEFAULT_FOLDER: &str = "User Templates";
/// Most user folders kept, besides [`DEFAULT_FOLDER`].
pub const MAX_FOLDERS: usize = 100;
/// Longest folder name, characters.
pub const MAX_FOLDER_NAME: usize = 80;

/// A template that lives in a folder.
pub trait Foldered {
    /// The template's name.
    fn name(&self) -> &str;
    /// The folder the template is listed in; `None` is [`DEFAULT_FOLDER`].
    fn folder(&self) -> Option<&str>;
    /// Moves the template to `folder` (`None`: [`DEFAULT_FOLDER`]).
    fn set_folder(&mut self, folder: Option<String>);
}

/// A folder name: trimmed, 1 to [`MAX_FOLDER_NAME`] characters, no control characters, and not
/// [`DEFAULT_FOLDER`] in any case.
pub fn clean_folder(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > MAX_FOLDER_NAME || name.chars().any(char::is_control) {
        return Err(format!("a folder name is 1 to {MAX_FOLDER_NAME} characters without control characters"));
    }
    if name.eq_ignore_ascii_case(DEFAULT_FOLDER) {
        return Err(format!("“{DEFAULT_FOLDER}” is always there; choose another name"));
    }
    Ok(name.to_string())
}

/// Sorts `folders` by name, ignoring case.
pub fn sort_folders(folders: &mut [String]) {
    folders.sort_by_cached_key(|f| f.to_lowercase());
}

/// Makes folders and templates read from a preferences file consistent: folders with a bad,
/// reserved or repeated name and those past [`MAX_FOLDERS`] are dropped; a template in a folder
/// that isn't listed gets the folder listed when its name is good and there is room, and goes to
/// [`DEFAULT_FOLDER`] otherwise; each template's folder takes the listed folder's spelling.
pub fn tidy_folders<T: Foldered>(folders: &mut Vec<String>, templates: &mut [T]) {
    let mut kept: Vec<String> = Vec::new();
    for f in folders.iter() {
        if let Ok(f) = clean_folder(f)
            && kept.len() < MAX_FOLDERS
            && !kept.iter().any(|k| k.eq_ignore_ascii_case(&f))
        {
            kept.push(f);
        }
    }
    for t in templates.iter_mut() {
        let Some(f) = t.folder().map(str::to_string) else { continue };
        if let Some(k) = kept.iter().find(|k| k.eq_ignore_ascii_case(f.trim())) {
            t.set_folder(Some(k.clone()));
        } else if let Ok(f) = clean_folder(&f)
            && kept.len() < MAX_FOLDERS
        {
            kept.push(f.clone());
            t.set_folder(Some(f));
        } else {
            t.set_folder(None);
        }
    }
    sort_folders(&mut kept);
    *folders = kept;
}

/// The folder `name` names: `Some(None)` for [`DEFAULT_FOLDER`], `Some(Some(spelling))` for a
/// listed folder (as listed), `None` when there is no such folder.
pub fn find_folder(folders: &[String], name: &str) -> Option<Option<String>> {
    let name = name.trim();
    if name.eq_ignore_ascii_case(DEFAULT_FOLDER) {
        return Some(None);
    }
    folders.iter().find(|f| f.eq_ignore_ascii_case(name)).cloned().map(Some)
}

/// How many templates are in `folder` (`None`: [`DEFAULT_FOLDER`]).
pub fn count_in<T: Foldered>(templates: &[T], folder: Option<&str>) -> usize {
    templates.iter().filter(|t| t.folder() == folder).count()
}

/// Adds the folder `name` (cleaned) to `folders`, kept sorted; returns the stored spelling.
pub fn create_folder(folders: &mut Vec<String>, name: &str) -> Result<String, String> {
    let name = clean_folder(name)?;
    if folders.iter().any(|f| f.eq_ignore_ascii_case(&name)) {
        return Err(format!("there is already a folder named “{name}”"));
    }
    if folders.len() >= MAX_FOLDERS {
        return Err(format!("at most {MAX_FOLDERS} folders; delete one first"));
    }
    folders.push(name.clone());
    sort_folders(folders);
    Ok(name)
}

/// Renames the listed folder `from` to `to` (cleaned) and keeps its templates in it. Returns the
/// new spelling and whether anything changed (`to` spelled as it already is changes nothing).
pub fn rename_folder<T: Foldered>(folders: &mut [String], templates: &mut [T], from: &str, to: &str) -> Result<(String, bool), String> {
    let to = clean_folder(to)?;
    if folders.iter().any(|f| f.eq_ignore_ascii_case(&to) && f != from) {
        return Err(format!("there is already a folder named “{to}”"));
    }
    if to == from {
        return Ok((to, false));
    }
    for f in folders.iter_mut() {
        if f == from {
            f.clone_from(&to);
        }
    }
    for t in templates.iter_mut() {
        if t.folder() == Some(from) {
            t.set_folder(Some(to.clone()));
        }
    }
    sort_folders(folders);
    Ok((to, true))
}

/// Deletes the listed folder `name`; its templates move to [`DEFAULT_FOLDER`]. Returns how many
/// templates moved.
pub fn delete_folder<T: Foldered>(folders: &mut Vec<String>, templates: &mut [T], name: &str) -> usize {
    folders.retain(|f| f != name);
    let mut moved = 0usize;
    for t in templates.iter_mut() {
        if t.folder() == Some(name) {
            t.set_folder(None);
            moved += 1;
        }
    }
    moved
}

/// Moves the template called `name` (any case) to `folder` (`None`: [`DEFAULT_FOLDER`]); `false`
/// when it was there already, an error when there is no such template.
pub fn move_template<T: Foldered>(templates: &mut [T], name: &str, folder: Option<String>) -> Result<bool, String> {
    let name = name.trim();
    let t = templates.iter_mut().find(|t| t.name().eq_ignore_ascii_case(name)).ok_or_else(|| format!("no template named `{name}`"))?;
    if t.folder() == folder.as_deref() {
        return Ok(false);
    }
    t.set_folder(folder);
    Ok(true)
}

/// Groups for a browser: [`DEFAULT_FOLDER`] first (`None`), then each listed folder, with the
/// indices (into `templates`) of the templates in it, in list order.
pub fn grouped<T: Foldered>(folders: &[String], templates: &[T]) -> Vec<(Option<String>, Vec<usize>)> {
    let idx = |f: Option<&str>| templates.iter().enumerate().filter(|(_, t)| t.folder() == f).map(|(i, _)| i).collect::<Vec<_>>();
    std::iter::once((None, idx(None))).chain(folders.iter().map(|f| (Some(f.clone()), idx(Some(f))))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug, PartialEq)]
    struct T(&'static str, Option<String>);
    impl Foldered for T {
        fn name(&self) -> &str {
            self.0
        }
        fn folder(&self) -> Option<&str> {
            self.1.as_deref()
        }
        fn set_folder(&mut self, folder: Option<String>) {
            self.1 = folder;
        }
    }
    fn t(n: &'static str, f: Option<&str>) -> T {
        T(n, f.map(str::to_string))
    }

    #[test]
    fn folder_names_are_checked_and_kept_as_typed() {
        assert_eq!(clean_folder("  Clients/2024 ").unwrap(), "Clients/2024");
        assert_eq!(clean_folder("..").unwrap(), "..");
        for bad in ["", "  ", "a\tb", &"x".repeat(MAX_FOLDER_NAME + 1), "user templates"] {
            assert!(clean_folder(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn create_rename_delete_keep_the_templates_with_their_folders() {
        let mut folders = Vec::new();
        create_folder(&mut folders, "Weddings").unwrap();
        create_folder(&mut folders, "proofs").unwrap();
        assert!(create_folder(&mut folders, "PROOFS").is_err());
        assert_eq!(folders, ["proofs", "Weddings"]);
        let mut ts = vec![t("a", Some("proofs")), t("b", None), t("c", Some("Weddings"))];
        assert_eq!(count_in(&ts, Some("proofs")), 1);
        assert_eq!(rename_folder(&mut folders, &mut ts, "proofs", "Sheets").unwrap(), ("Sheets".into(), true));
        assert_eq!(ts[0].1.as_deref(), Some("Sheets"));
        assert!(rename_folder(&mut folders, &mut ts, "Sheets", "weddings").is_err());
        assert_eq!(rename_folder(&mut folders, &mut ts, "Sheets", " Sheets ").unwrap(), ("Sheets".into(), false));
        assert_eq!(delete_folder(&mut folders, &mut ts, "Sheets"), 1);
        assert_eq!(ts[0].1, None);
        assert_eq!(folders, ["Weddings"]);
        let g = grouped(&folders, &ts);
        assert_eq!(g, [(None, vec![0, 1]), (Some("Weddings".into()), vec![2])]);
    }

    #[test]
    fn a_full_list_of_folders_takes_no_more() {
        let mut folders = Vec::new();
        for n in 0..MAX_FOLDERS {
            create_folder(&mut folders, &format!("F{n}")).unwrap();
        }
        assert!(create_folder(&mut folders, "One more").unwrap_err().contains("at most"));
    }

    #[test]
    fn hostile_stored_folders_are_tidied_not_trusted() {
        let mut folders = vec!["Empty".to_string(), "".into(), "empty".into(), "User Templates".into(), "Family".into()];
        let mut ts =
            vec![t("a", Some("Family")), t("b", Some(" family ")), t("c", Some("a\nb")), t("d", Some("USER TEMPLATES")), t("e", Some("Unlisted"))];
        tidy_folders(&mut folders, &mut ts);
        assert_eq!(folders, ["Empty", "Family", "Unlisted"]);
        let in_: Vec<_> = ts.iter().map(|t| t.1.clone()).collect();
        assert_eq!(in_, [Some("Family".into()), Some("Family".into()), None, None, Some("Unlisted".into())]);
        assert_eq!(find_folder(&folders, "user templates"), Some(None));
        assert_eq!(find_folder(&folders, "FAMILY"), Some(Some("Family".into())));
        assert_eq!(find_folder(&folders, "nope"), None);
    }
}
