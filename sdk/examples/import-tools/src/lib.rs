//! Example plug-in: the import hooks and `library.remove`. Build: `cargo build --release --target
//! wasm32-unknown-unknown`, then install
//! `target/wasm32-unknown-unknown/release/import_tools.wasm` from the Plug-in Manager and grant it
//! `libraryRemove` (for *Remove from Library…*) and `metadataWrite` (for the afterImport tag).
//! Needs `beforeImport`, `afterImport` and `libraryRemove` hosts.
//!
//! - **Before import** (`hooks.beforeImport`): asked about each file as an import prepares it.
//!   With the *Transcode rule* setting naming an extension (say `jxl`), a file is replaced by a
//!   twin of the same stem in that format beside it (a transcode a tool such as `cjxl` has made in
//!   advance; the host checks the twin exists before it uses it); with *Skip rule* naming an
//!   extension, those files are left out of the import altogether. Any other file imports as it is. (A hook that transcodes itself would
//!   start a granted program here — `host::program_start` works in this hook on an import the
//!   user started — and answer `{"replace": …}` with what it made.)
//! - **After import** (`hooks.afterImport`): tags each photo an import added with the *Imported*
//!   custom metadata field's date, as one `metadata.setStandard` per batch (cap: 1000 photos).
//! - **Remove from Library…** (menu command): moves the selected photos to Recently Deleted with
//!   `host::library_remove` — the files on disk are never touched.

use lcc_plugin_sdk::serde_json::{Value, json};
use lcc_plugin_sdk::{Request, export_plugin, host};

const MANIFEST: &str = r#"{
  "id": "org.example.import-tools",
  "name": "Import Tools (example)",
  "version": "1.0.0",
  "requires": ["beforeImport", "afterImport", "libraryRemove", "settings"],
  "description": "Replaces or skips files on import, tags imported photos, and moves photos to Recently Deleted",
  "author": "the app's authors",
  "license": "MIT",
  "permissions": {
    "libraryRemove": true,
    "metadataWrite": true
  },
  "settings": [
    {"key": "transcodeExt", "label": "Transcode rule: prefer a twin with this extension", "type": "text", "default": ""},
    {"key": "skipExt", "label": "Skip rule: leave files with this extension out", "type": "text", "default": ""}
  ],
  "hooks": {"beforeImport": true, "afterImport": true},
  "metadataFields": [{"key": "imported", "label": "Imported", "type": "text", "readOnly": true}],
  "commands": [{"id": "remove", "label": "Remove from Library…"}]
}"#;

/// The extension of `path`, lower case.
fn ext_of(path: &str) -> String {
    path.rsplit('.').next().unwrap_or("").to_ascii_lowercase()
}

/// `name` without its extension (`IMG_0001.CR2` → `IMG_0001`).
fn stem_of(path: &str) -> &str {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    match name.rfind('.') {
        Some(i) => &name[..i],
        None => name,
    }
}

fn handle(req: &Request) -> Result<Value, String> {
    match req.hook() {
        "beforeImport" => before_import(req),
        "afterImport" => after_import(req),
        "command" if req.command() == "remove" => remove(req),
        other => Err(format!("unsupported hook {other}")),
    }
}

/// The twin to import instead, or the skip verdict, from the settings' rules.
fn before_import(req: &Request) -> Result<Value, String> {
    let settings = host::settings()?;
    let path = req.path();
    let stem = stem_of(path);
    if let Some(skip) = ext(&settings, "skipExt")
        && ext_of(path) == skip
    {
        host::log(&format!("beforeImport: skipping {path}"));
        return Ok(json!({"skip": true}));
    }
    if let Some(want) = ext(&settings, "transcodeExt") {
        let folder = path.rsplit_once(['/', '\\']).map_or(".", |(d, _)| d);
        let twin = format!("{folder}/{stem}.{want}");
        // the host checks the path before it uses it: no such twin (or a format the library
        // cannot import) is logged there, and the original file imports as is
        if twin != path {
            host::log(&format!("beforeImport: {path} → {twin}"));
            return Ok(json!({"replace": twin}));
        }
    }
    Ok(json!({}))
}

/// A setting as a lower-case extension (one word of letters).
fn ext(settings: &Value, key: &str) -> Option<String> {
    let v = settings[key].as_str()?.trim().trim_start_matches('.').to_ascii_lowercase();
    (!v.is_empty() && v.chars().all(|c| c.is_ascii_alphanumeric())).then_some(v)
}

/// Tags each imported photo with the date it was imported (`metadataFields` values live in the
/// plug-in's namespace; a read-only field only the plug-in sets).
fn after_import(req: &Request) -> Result<Value, String> {
    let mut n = 0usize;
    for photo in req.imported() {
        let Some(id) = photo.get("id").and_then(Value::as_str) else { continue };
        let when = photo.get("imported").and_then(Value::as_str).unwrap_or("");
        host::ns_set(id, "imported", json!(when))?;
        n += 1;
    }
    Ok(json!({"message": format!("Tagged {n} photo(s)")}))
}

/// The selected photos go to Recently Deleted (the files stay on disk).
fn remove(req: &Request) -> Result<Value, String> {
    let ids = req.selection();
    if ids.is_empty() {
        return Ok(json!({"message": "Select the photos to remove first."}));
    }
    let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
    let r = host::library_remove(&refs)?;
    Ok(json!({"message": format!("Moved {} photo(s) to Recently Deleted; the files stay on disk.", r["removed"].as_u64().unwrap_or(0))}))
}

export_plugin!(MANIFEST, handle);
