//! `store.*`: the app-wide store of presets, profiles and habits ([`crate::app_store`]).

use std::path::PathBuf;

use serde_json::{Value, json};

use super::{CommandSpec, always, bad, cmd};
use crate::{Result, Session};

/// `paths` as folders (at most 1000; blanks dropped).
fn paths(p: &Value) -> Vec<PathBuf> {
    p.get("paths")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).filter(|x| !x.trim().is_empty()).take(1000).map(PathBuf::from).collect())
        .unwrap_or_default()
}

fn force(p: &Value) -> bool {
    p.get("force").and_then(Value::as_bool) == Some(true)
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(query "store.info", "Shared Presets and Profiles", [], None, "{} → {available, dir, inUse, libraryKeepsOwn, presets, exportPresets, lutProfiles, effects, merged: [{library, at, presets, profiles, effects, remap}]}", always, |s, _| Ok(s.store_info())),
        cmd!(
            "store.mergeLibraries",
            "Merge Catalogs into Shared Presets",
            [],
            None,
            "{paths: [library folders], force?: bool}: the presets, profiles, effects, export presets and templates of these libraries join the shared store, once per library unless `force` (read only: nothing in them is written; a profile id the store uses for another file gets a new one, and the library's photos follow it when it opens) → {merged: [{library, presets, profiles, effects, problems, skipped}]}",
            always,
            |s: &mut Session, p: &Value| -> Result<Value> {
                let reports = s.merge_libraries(&paths(p), force(p)).map_err(|e| bad("store.mergeLibraries", e.to_string()))?;
                Ok(json!({"merged": reports}))
            }
        ),
        cmd!(
            "store.gatherPresets",
            "Import Presets from Older Catalogs",
            ["File", "More Imports"],
            None,
            "{paths?: [library folders], force?: bool}: export presets, print templates and the other named presets of these libraries, the open one and every library merged before, read from each library's prefs.json and its backups' (once per library unless `force`), join the shared store: the same preset once, the same name with other content as \"name 2\"; nothing in the libraries is written → {libraries, files, count, skipped, problems}",
            always,
            |s: &mut Session, p: &Value| -> Result<Value> {
                s.gather_presets(&paths(p), force(p)).map_err(|e| bad("store.gatherPresets", e.to_string()))
            }
        ),
        cmd!(
            "store.importOlderInstalls",
            "Import Presets from Older Installs",
            ["File", "More Imports"],
            None,
            "{paths?: [store folders], force?: bool}: the presets an older install of this app kept under one of its earlier names (each earlier settings folder's Library Store; `paths`: those store folders instead) join the shared store, each folder once unless `force`: develop presets with their favourites and At Import / hidden marks, local adjustment presets, LUT profiles, import defaults and export presets, and the camera profiles (.dcp) of its Profiles folder, installed as Import Profiles & Presets… installs them, with the camera bodies the older install started with them (a body with a profile of its own keeps it); the same preset twice is kept once, the same name with other content as \"name 2\"; a camera profile there already (the same file, or the same camera and ProfileName) is named in cameraAlready, a damaged one in problems; nothing in those folders is written → {merged: [{library, presets, profiles, effects, cameraProfiles?, cameraAlready?, cameraBodies?, problems, skipped}]}",
            always,
            |s: &mut Session, p: &Value| -> Result<Value> {
                let reports = s.merge_older_installs(&paths(p), force(p)).map_err(|e| bad("store.importOlderInstalls", e.to_string()))?;
                Ok(json!({"merged": reports}))
            }
        ),
        cmd!(
            "store.useLibrary",
            "Store Presets and Profiles with This Catalog",
            [],
            None,
            "{keep: bool}: true: the open library keeps its own copy of the shared presets, profiles (files included) and habits from now on; false: it uses the shared store again (its own merged in first) → store.info",
            always,
            |s: &mut Session, p: &Value| -> Result<Value> {
                let keep = p.get("keep").and_then(Value::as_bool).ok_or_else(|| bad("store.useLibrary", "missing keep (true or false)"))?;
                s.store_use_library(keep).map_err(|e| bad("store.useLibrary", e.to_string()))
            }
        ),
    ]
}
