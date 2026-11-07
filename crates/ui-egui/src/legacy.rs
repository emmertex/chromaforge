//! The names of this app's earlier identity in the UI (a `[legacy]` file of `xtask/src/brand.rs`): File ▸ More
//! Imports ▸ Import LightCraft Library… (test notes 2026-11-05, 19:33). A LightCraft library is a library in the
//! catalog format this app had under that name, which LightCraft itself still writes; `lcc_catalog::legacy` reads it,
//! and it goes through the same preview and merge as Import from Another Catalog. The library is only read.

use crate::menus::UiCommand;

/// The command's id.
pub const IMPORT_LIBRARY_ID: &str = "dialog.importLegacyLibrary";
/// File ▸ More Imports ▸ Import LightCraft Library….
pub const IMPORT_LIBRARY: UiCommand = (IMPORT_LIBRARY_ID, "Import LightCraft Library…", None, "File>More Imports");

/// Import LightCraft Library…: `{path?}`; without a path, a folder picker first. A folder that holds no LightCraft
/// library is refused (Import from Another Catalog takes this app's own).
#[cfg(not(target_arch = "wasm32"))]
pub fn import_library(app: &mut crate::LccApp, id: &str, p: &serde_json::Value) -> Result<serde_json::Value, String> {
    use serde_json::{Value, json};
    match p.get("path").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()) {
        Some(path) => {
            let found = lcc_catalog::library::resolve(std::path::Path::new(path)).is_ok_and(|d| lcc_catalog::legacy::is_legacy(&d));
            if !found {
                return Err(format!("{path} {}", crate::i18n::tr("holds no LightCraft library (its catalog.snap)")));
            }
            crate::catalog_ui::run(app, "dialog.importCatalog", &json!({"path": path})).unwrap_or(Ok(Value::Null))
        }
        None => match crate::catalog_ui::pick_folder(app, id, p, "path", "Import LightCraft Library") {
            Ok(Some(path)) => import_library(app, id, &json!({"path": path})),
            Ok(None) => Ok(Value::Null),
            Err(e) => Err(e),
        },
    }
}

#[cfg(test)]
#[cfg(not(target_arch = "wasm32"))]
mod tests {
    use serde_json::json;

    use crate::catalog_ui::CatalogDialog;
    use crate::{LccApp, Services};

    fn temp(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("lc-ui-legacy-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn app_in(dir: &std::path::Path) -> LccApp {
        let mut s = lcc_engine::Session::new().with_fs();
        s.open_library(dir, true).unwrap();
        LccApp::new(s, Services::default())
    }
    /// File ▸ More Imports ▸ Import LightCraft Library… (test notes 2026-11-05, 19:33): a LightCraft library opens the
    /// Import from Catalog preview and merges its photos with their pick and edits; the library itself is left as it
    /// was; a folder that holds no LightCraft library is refused.
    #[test]
    fn import_lightcraft_library_brings_its_photos_in_and_leaves_it_alone() {
        let root = temp("lightcraft");
        let mut app = app_in(&root.join("Lib"));
        assert!(crate::menus::UI_COMMANDS.iter().any(|c| c.0 == super::IMPORT_LIBRARY_ID && c.3 == "File>More Imports"), "in File ▸ More Imports");
        let lc = root.join("LightCraft Library");
        std::fs::create_dir_all(&lc).unwrap();
        let photo = json!({
            "id": 1, "source": {"type": "file", "path": root.join("shoot").join("a-0001.cr3").to_string_lossy()},
            "file_name": "a-0001.cr3", "format": "CR3", "width": 6000, "height": 4000, "imported": "2026-10-03T10:00:00Z",
            "meta": {"camera": "Canon EOS R5", "serial": "0123"}, "rating": 3, "flag": "pick", "label": null,
            "develop": {"light": {"exposure": 0.6}}, "versions": [], "history": [], "deleted": false
        });
        // as LightCraft writes it: the format first
        let catalog = json!({"photos": {"1": photo}, "albums": {}, "next_photo": 2, "next_album": 1});
        let snap = format!("{{\"format\":\"lightcraft-catalog\",\"version\":10,\"seq\":0,\"catalog\":{catalog}}}\n");
        std::fs::write(lc.join("catalog.snap"), snap).unwrap();
        let before = std::fs::read(lc.join("catalog.snap")).unwrap();

        app.run(super::IMPORT_LIBRARY_ID, json!({"path": lc.to_string_lossy()})).unwrap();
        let Some(CatalogDialog::ImportCatalog { plan: Ok(plan), path, .. }) = app.catalog_ui.dialog.clone() else {
            panic!("the import preview: {:?}", app.catalog_ui.dialog.is_some())
        };
        assert_eq!(plan["newPhotos"].as_array().map(Vec::len), Some(1), "{plan}");
        let photos = app.session.catalog.len();
        app.run("catalog.import", json!({"path": path, "rule": "keep"})).unwrap();
        assert_eq!(app.session.catalog.len(), photos + 1);
        let p = app.session.catalog.photos().find(|p| p.file_name == "a-0001.cr3").expect("the LightCraft photo");
        assert_eq!((p.flag, p.rating, p.develop.light.exposure), (lcc_catalog::Flag::Pick, 3, 0.6));
        assert_eq!(p.meta.camera_serial, "0123");
        assert_eq!(std::fs::read(lc.join("catalog.snap")).unwrap(), before, "the LightCraft library is unchanged");
        assert_eq!(std::fs::read_dir(&lc).unwrap().count(), 1, "nothing added to it");

        // this app's own catalog is not one
        app.catalog_ui.dialog = None;
        let e = app.run(super::IMPORT_LIBRARY_ID, json!({"path": root.join("Lib").to_string_lossy()})).unwrap_err();
        assert!(e.contains("LightCraft"), "{e}");
        assert!(app.catalog_ui.dialog.is_none());
        app.session.close_library().unwrap();
        let _ = std::fs::remove_dir_all(&root);
    }
}
