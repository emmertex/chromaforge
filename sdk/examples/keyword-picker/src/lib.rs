//! Example plug-in: settings, a dialog that remembers, and a `prepare` step. Build: `cargo build
//! --release --target wasm32-unknown-unknown`, then install
//! `target/wasm32-unknown-unknown/release/keyword_picker.wasm` from the Plug-in Manager and grant it
//! `catalog` (to list the keywords) and `metadataWrite` (to add or remove one).
//!
//! - **Apply Keyword…** (menu command): adds one of the catalog's keywords to the selected photos,
//!   or removes it. The keywords to choose from are only known when the command runs, so the
//!   command declares `prepare`: asked first (`command.prepare`), the plug-in explains why it can't
//!   run (no photo selected, no keyword in use), runs at once when only one keyword qualifies, or
//!   gives the dialog the keywords as options. The dialog opens with the keyword last applied
//!   (fields remember by default); *Remove it instead* always starts unticked (`remember: false`).
//! - **Settings** (the plug-in's page in the Plug-in Manager): which keywords to offer (used on at
//!   least so many photos) and in which order. The plug-in reads them with `host::settings`.

use lcc_plugin_sdk::serde_json::{Value, json};
use lcc_plugin_sdk::{Request, export_plugin, host};

const MANIFEST: &str = r#"{
  "id": "org.example.keyword-picker",
  "name": "Keyword Picker (example)",
  "requires": ["settings"],
  "description": "Adds one of the catalog's keywords to the selected photos, picked from the keywords in use",
  "author": "the app's authors",
  "permissions": {"catalog": true, "metadataWrite": true},
  "settings": [
    {"key": "minCount", "label": "Offer keywords used on at least", "type": "number", "min": 1, "max": 100000, "default": 1},
    {"key": "order", "label": "Order", "type": "choice", "options": ["Most used", "A to Z"], "default": "Most used"}
  ],
  "commands": [
    {"id": "apply", "label": "Apply Keyword…", "prepare": true,
     "dialog": [
       {"key": "keyword", "label": "Keyword", "type": "choice", "options": []},
       {"key": "remove", "label": "Remove it instead", "type": "bool", "remember": false}
     ]}
  ]
}"#;

/// Most keywords the dialog offers (a choice takes at most 64 options).
const MAX_OFFERED: usize = 64;

fn handle(req: &Request) -> Result<Value, String> {
    match req.hook() {
        "command.prepare" if req.command() == "apply" => prepare(req),
        "command" if req.command() == "apply" => apply(req),
        other => Err(format!("unsupported hook {other}")),
    }
}

/// What the command does on this selection: explain, run at once, or ask with these options.
fn prepare(req: &Request) -> Result<Value, String> {
    if req.selection().is_empty() {
        return Ok(json!({"message": "Select the photos to apply a keyword to first."}));
    }
    let settings = host::settings()?;
    let min = settings["minCount"].as_f64().unwrap_or(1.0);
    let keywords = offered(&host::call("keyword.list", json!({}))?, min, settings["order"].as_str().unwrap_or(""));
    Ok(match keywords.as_slice() {
        [] => json!({"message": format!("No keyword is used on at least {min} photos yet.")}),
        [only] => json!({"run": {"keyword": only, "remove": false}}),
        all => json!({"dialog": {"keyword": {"options": all, "default": all[0]}}}),
    })
}

fn apply(req: &Request) -> Result<Value, String> {
    let keyword = req.arg("keyword").as_str().filter(|k| !k.is_empty()).ok_or("choose a keyword")?;
    let ids = req.selection();
    if ids.is_empty() {
        return Err("no photos selected".into());
    }
    let remove = req.arg("remove").as_bool().unwrap_or(false);
    let key = if remove { "removeKeywords" } else { "addKeywords" };
    host::call("metadata.setStandard", json!({"ids": ids, key: [keyword]}))?;
    let done = if remove { "Removed" } else { "Added" };
    Ok(json!({"message": format!("{done} “{keyword}” on {} photos", ids.len())}))
}

/// The keywords (`keyword.list`'s tree, by full path) used on at least `min` photos, most used
/// first or A to Z, at most [`MAX_OFFERED`].
pub fn offered(tree: &Value, min: f64, order: &str) -> Vec<String> {
    let mut all: Vec<(String, f64)> = Vec::new();
    collect(tree, &mut all, 0);
    all.retain(|(_, n)| *n >= min);
    if order == "A to Z" {
        all.sort_by_key(|(k, _)| k.to_lowercase());
    } else {
        all.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    }
    all.into_iter().take(MAX_OFFERED).map(|(k, _)| k).collect()
}

fn collect(nodes: &Value, out: &mut Vec<(String, f64)>, depth: usize) {
    // keyword trees are shallow; a hostile reply can't recurse forever
    if depth > 32 {
        return;
    }
    for n in nodes.as_array().into_iter().flatten() {
        if let Some(path) = n["path"].as_str().filter(|p| !p.is_empty()) {
            out.push((path.to_string(), n["count"].as_f64().unwrap_or(0.0)));
        }
        collect(&n["children"], out, depth + 1);
    }
}

export_plugin!(package: MANIFEST, handle);

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> Value {
        json!([
            {"name": "Places", "path": "Places", "count": 0, "children": [
                {"name": "Oslo", "path": "Places|Oslo", "count": 4, "children": []},
                {"name": "Bergen", "path": "Places|Bergen", "count": 9, "children": []}]},
            {"name": "beach", "path": "beach", "count": 2, "children": []}
        ])
    }

    #[test]
    fn offers_what_the_settings_ask_for() {
        assert_eq!(offered(&tree(), 1.0, "Most used"), ["Places|Bergen", "Places|Oslo", "beach"]);
        assert_eq!(offered(&tree(), 3.0, "A to Z"), ["Places|Bergen", "Places|Oslo"]);
        assert!(offered(&tree(), 100.0, "Most used").is_empty());
        assert!(offered(&json!({"not": "a list"}), 1.0, "").is_empty());
    }

    #[test]
    fn the_manifest_parses() {
        let m: Value = lcc_plugin_sdk::serde_json::from_str(MANIFEST).unwrap();
        assert_eq!(m["commands"][0]["prepare"], true);
    }
}
