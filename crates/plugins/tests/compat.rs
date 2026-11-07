//! Compatibility: the host features a plug-in requires (`requires`), the `version` rule and
//! downgrades. A plug-in this version can't run is never installed, and one that stops loading
//! (the app was downgraded) loses nothing.

use std::path::{Path, PathBuf};

use lcc_plugins::{Error, Manager, Manifest, Permissions};
use serde_json::{Value, json};

/// A module whose manifest is `manifest`; it answers every call with `{}`.
fn module(manifest: &Value) -> Vec<u8> {
    let manifest = manifest.to_string();
    let esc = |s: &str| s.bytes().map(|b| format!("\\{b:02x}")).collect::<String>();
    let src = format!(
        r#"(module
  (memory (export "memory") 2)
  (data (i32.const 16) "{m}")
  (data (i32.const 32768) "{{}}")
  (func (export "lcc_abi_version") (result i32) (i32.const 1))
  (func (export "lcc_manifest") (result i64) (i64.or (i64.shl (i64.const {ml}) (i64.const 32)) (i64.const 16)))
  (func (export "lcc_alloc") (param i32) (result i32) (i32.const 65536))
  (func (export "lcc_handle") (param i32 i32) (result i64) (i64.or (i64.shl (i64.const 2) (i64.const 32)) (i64.const 32768))))"#,
        m = esc(&manifest),
        ml = manifest.len()
    );
    wat::parse_str(&src).unwrap()
}

/// A plug-in `id` at `version` asking for the catalog (and metadata writes when `write`), with a
/// custom metadata field `job` (its data lives in the plug-in's own file).
fn plugin(id: &str, version: &str, requires: &[&str], write: bool) -> Vec<u8> {
    module(&json!({
        "id": id, "name": "Test", "version": version, "requires": requires,
        "permissions": {"catalog": true, "metadataWrite": write},
        "metadataFields": [{"key": "job", "label": "Job", "type": "text"}],
    }))
}

fn catalog_and_write() -> Permissions {
    Permissions { catalog: true, metadata_write: true, ..Default::default() }
}

/// A fresh plug-in folder.
fn folder(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lcc-plugins-compat-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn files(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = walk(dir).into_iter().map(|p| p.strip_prefix(dir).unwrap().display().to_string()).collect();
    v.sort();
    v
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() { out.extend(walk(&p)) } else { out.push(p) }
    }
    out
}

fn plugins_json(dir: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(dir.join("plugins.json")).unwrap()).unwrap()
}

#[test]
fn a_plugin_needing_features_the_host_lacks_is_not_installed() {
    let dir = folder("requires");
    let mut m = Manager::open_with_features(&dir, &["jobs"]).unwrap();
    let e = m.install_bytes(&plugin("t.needs", "1.0.0", &["settings", "jobs", "programs"], false), Some(catalog_and_write()), false).unwrap_err();
    assert_eq!(e, Error::Unsupported(vec!["settings".into(), "programs".into()]), "every missing feature, and only those");
    let msg = e.to_string();
    assert!(msg.contains("`settings`") && msg.contains("`programs`") && !msg.contains("`jobs`"), "{msg}");
    assert!(m.get("t.needs").is_none(), "not installed");
    assert!(files(&dir).is_empty(), "no module, grant or data written: {:?}", files(&dir));

    // no `requires`, an empty one, or only features the host has: installs as before
    m.install_bytes(&module(&json!({"id": "t.plain", "name": "P", "version": "1.0.0"})), None, false).unwrap();
    m.install_bytes(&plugin("t.empty", "1.0.0", &[], false), None, false).unwrap();
    let ok = m.install_bytes(&plugin("t.known", "1.0.0", &["jobs"], false), None, false).unwrap();
    assert_eq!(ok.info()["requires"], json!(["jobs"]));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_malformed_requires_is_a_manifest_error() {
    let many: Vec<String> = (0..65).map(|i| format!("f{i}")).collect();
    for bad in [
        json!("jobs"),
        json!([1]),
        json!([null]),
        json!(["bad name"]),
        json!([""]),
        json!(["x".repeat(65)]),
        json!(["a", "a"]),
        json!(many),
        json!({"jobs": true}),
    ] {
        let m = json!({"id": "t.bad", "name": "B", "version": "1.0.0", "requires": bad});
        assert!(matches!(Manifest::parse(m.to_string().as_bytes()), Err(Error::Manifest(_))), "{bad}");
        let mut mgr = Manager::in_memory();
        mgr.set_features(&["jobs", "a"]);
        assert!(mgr.install_bytes(&module(&m), None, false).is_err(), "{bad}");
        assert_eq!(mgr.list().count(), 0, "nothing installed for {bad}");
    }
    let max: Vec<String> = (0..64).map(|i| format!("f{i}")).collect();
    assert!(Manifest::parse(json!({"id": "t", "name": "T", "requires": max}).to_string().as_bytes()).is_ok(), "64 names are fine");
}

#[test]
fn a_plugin_that_stops_loading_keeps_its_entry_and_data() {
    let dir = folder("kept");
    let mut m = Manager::open_with_features(&dir, &["jobs"]).unwrap();
    m.install_bytes(&plugin("t.jobs", "1.0.0", &["jobs"], true), Some(catalog_and_write()), false).unwrap();
    m.set_metadata_field("t.jobs", "7", "job", &json!("J-1")).unwrap();
    m.install_bytes(&plugin("t.other", "1.0.0", &[], false), None, false).unwrap();
    m.set_enabled("t.other", false).unwrap();
    let entry = plugins_json(&dir)["plugins"]["t.jobs"].clone();
    assert_eq!(entry["enabled"], true);
    let data = std::fs::read(dir.join("data/t.jobs.json")).unwrap();
    drop(m);

    // the app was downgraded: it no longer knows `jobs`
    let mut m = Manager::open_with_features(&dir, &[]).unwrap();
    assert!(m.get("t.jobs").is_none(), "not loaded");
    let f = m.failed.iter().find(|f| f.file.ends_with("t.jobs.wasm")).expect("listed as failed");
    assert_eq!(f.missing, vec!["jobs".to_string()]);
    assert!(f.error.contains("`jobs`"), "{}", f.error);
    // every change rewrites plugins.json: the entry of the plug-in that didn't load survives
    m.set_enabled("t.other", true).unwrap();
    m.install_bytes(&plugin("t.third", "1.0.0", &[], false), None, false).unwrap();
    m.uninstall("t.third").unwrap();
    assert_eq!(plugins_json(&dir)["plugins"]["t.jobs"], entry, "enabled state and grant unchanged");
    assert_eq!(std::fs::read(dir.join("data/t.jobs.json")).unwrap(), data, "its data unchanged");
    drop(m);

    // installing an older version over it is still a downgrade
    let m = Manager::open_with_features(&dir, &[]).unwrap();
    assert_eq!(m.installed_version("t.jobs").as_deref(), Some("1.0.0"));
    assert_eq!(m.unloaded("t.jobs"), Some(("1.0.0".to_string(), catalog_and_write())));
    let mut m = m;
    let e = m.install_bytes(&plugin("t.jobs", "0.9.0", &[], true), None, false).unwrap_err();
    assert_eq!(e, Error::Downgrade { installed: "1.0.0".into(), new: "0.9.0".into() });
    assert_eq!(plugins_json(&dir)["plugins"]["t.jobs"], entry, "nothing changed");
    drop(m);

    // updated again: it loads with the same grant and data
    let mut m = Manager::open_with_features(&dir, &["jobs"]).unwrap();
    let p = m.get("t.jobs").expect("loaded again");
    assert!(p.enabled);
    assert_eq!(p.effective(), catalog_and_write());
    let fields = m.metadata_fields("7");
    assert_eq!(fields[0]["fields"][0]["value"], "J-1", "{fields:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn installing_needs_a_semver_version() {
    for bad in [None, Some(""), Some("1.0"), Some("v1.0.0"), Some("1"), Some("1.0.0.0"), Some("latest")] {
        let mut man = json!({"id": "t.ver", "name": "V"});
        if let Some(v) = bad {
            man["version"] = json!(v);
        }
        let mut m = Manager::in_memory();
        let e = m.install_bytes(&module(&man), None, false).unwrap_err();
        assert!(matches!(e, Error::Manifest(_)) && e.to_string().contains("MAJOR.MINOR.PATCH"), "{bad:?}: {e}");
        assert!(m.get("t.ver").is_none());
    }
    for good in ["0.1.0", "1.4.0", "2.0.0-beta.1", "1.0.0+build.7"] {
        let mut m = Manager::in_memory();
        assert!(m.install_bytes(&module(&json!({"id": "t.ver", "name": "V", "version": good})), None, false).is_ok(), "{good}");
    }
}

#[test]
fn a_module_already_installed_without_a_valid_version_still_loads() {
    let dir = folder("legacy");
    std::fs::write(dir.join("t.old.wasm"), module(&json!({"id": "t.old", "name": "Old", "version": "1.0"}))).unwrap();
    std::fs::write(dir.join("t.none.wasm"), module(&json!({"id": "t.none", "name": "None"}))).unwrap();
    std::fs::write(dir.join("plugins.json"), r#"{"plugins":{"t.old":{"enabled":true,"grant":{}},"t.none":{"enabled":true,"grant":{}}}}"#).unwrap();
    let mut m = Manager::open(&dir).unwrap();
    for id in ["t.old", "t.none"] {
        let p = m.get(id).unwrap_or_else(|| panic!("{id} loads: {:?}", m.failed));
        assert!(p.enabled);
        assert!(p.log.iter().any(|l| l.contains("without a valid version")), "{id}: {:?}", p.log);
    }
    // a new version replaces it without a downgrade check (there's nothing to compare with)
    m.install_bytes(&module(&json!({"id": "t.old", "name": "Old", "version": "0.1.0"})), None, false).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_downgrade_needs_confirming() {
    let dir = folder("downgrade");
    let mut m = Manager::open(&dir).unwrap();
    m.install_bytes(&plugin("t.dg", "1.4.0", &[], true), Some(catalog_and_write()), false).unwrap();
    let installed = std::fs::read(dir.join("t.dg.wasm")).unwrap();
    let state = std::fs::read(dir.join("plugins.json")).unwrap();

    // 1.3.0 asks for less than 1.4.0 did
    let older = plugin("t.dg", "1.3.0", &[], false);
    let e = m.install_bytes(&older, None, false).unwrap_err();
    assert_eq!(e, Error::Downgrade { installed: "1.4.0".into(), new: "1.3.0".into() });
    assert!(e.to_string().contains("1.4.0") && e.to_string().contains("1.3.0"), "{e}");
    assert_eq!(m.get("t.dg").unwrap().plugin.manifest().version, "1.4.0", "nothing changed");
    assert_eq!(std::fs::read(dir.join("t.dg.wasm")).unwrap(), installed);
    assert_eq!(std::fs::read(dir.join("plugins.json")).unwrap(), state);

    let p = m.install_bytes(&older, None, true).unwrap();
    assert_eq!(p.plugin.manifest().version, "1.3.0");
    assert_eq!(p.effective(), Permissions { catalog: true, ..Default::default() }, "grant kept, cut back to the request");
    assert!(p.log.iter().any(|l| l == "downgraded from 1.4.0 to 1.3.0"), "{:?}", p.log);

    // the same version again is a repair; a higher one an update
    m.install_bytes(&older, None, false).unwrap();
    m.install_bytes(&plugin("t.dg", "2.0.0", &[], false), None, false).unwrap();
    // semver precedence: a pre-release comes before its release; build metadata is ignored
    assert!(matches!(m.install_bytes(&plugin("t.dg", "2.0.0-rc.1", &[], false), None, false), Err(Error::Downgrade { .. })));
    m.install_bytes(&plugin("t.dg", "2.0.0+build.1", &[], false), None, false).unwrap();
    m.install_bytes(&plugin("t.dg", "2.0.0+build.0", &[], false), None, false).unwrap();
    m.install_bytes(&plugin("t.dg", "2.0.1-alpha", &[], false), None, false).unwrap();
    assert!(matches!(m.install_bytes(&plugin("t.dg", "1.99.99", &[], false), None, false), Err(Error::Downgrade { .. })));
    assert!(matches!(m.install_bytes(&plugin("t.dg", "2.0.0", &[], false), None, false), Err(Error::Downgrade { .. })), "2.0.0 < 2.0.1-alpha");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_install_check_reports_what_would_stop_an_install() {
    let mut m = Manager::in_memory();
    m.install_bytes(&plugin("t.c", "1.4.0", &[], false), None, false).unwrap();
    let man =
        |v: &str, req: &[&str]| Manifest::parse(json!({"id": "t.c", "name": "C", "version": v, "requires": req}).to_string().as_bytes()).unwrap();
    assert_eq!(m.install_check(&man("1.5.0", &[])), (vec![], None, None));
    assert_eq!(m.install_check(&man("1.3.0", &["notYet"])), (vec!["notYet".to_string()], None, Some("1.4.0".to_string())));
    let (_, version, downgrade) = m.install_check(&man("1.3", &[]));
    assert!(version.is_some_and(|v| v.contains("MAJOR.MINOR.PATCH")) && downgrade.is_none());
}

#[test]
fn a_plugin_that_did_not_load_can_be_removed() {
    let dir = folder("remove");
    let mut m = Manager::open_with_features(&dir, &["jobs"]).unwrap();
    m.install_bytes(&plugin("t.jobs", "1.0.0", &["jobs"], true), Some(catalog_and_write()), false).unwrap();
    m.set_metadata_field("t.jobs", "7", "job", &json!("J-1")).unwrap();
    m.install_bytes(&plugin("t.other", "1.0.0", &[], false), None, false).unwrap();
    drop(m);
    // a module that can't be read at all, with an entry of its own
    std::fs::write(dir.join("t.broken.wasm"), b"\0asm garbage").unwrap();
    let mut state = plugins_json(&dir);
    state["plugins"]["t.broken"] = json!({"enabled": true, "grant": {"catalog": true}});
    std::fs::write(dir.join("plugins.json"), state.to_string()).unwrap();

    let m = Manager::open_with_features(&dir, &[]).unwrap();
    let ids: Vec<&str> = m.failed.iter().map(|f| f.id.as_str()).collect();
    assert_eq!(ids, ["t.broken", "t.jobs"], "each failed module gives its id");
    drop(m);
    // the module is found by its own path, not only as `<id>.wasm`: one copied in under another name
    std::fs::rename(dir.join("t.jobs.wasm"), dir.join("copied-in.wasm")).unwrap();
    let mut m = Manager::open_with_features(&dir, &[]).unwrap();
    m.uninstall("t.jobs").unwrap();
    m.uninstall("t.broken").unwrap();
    assert!(m.failed.is_empty(), "{:?}", m.failed);
    assert_eq!(files(&dir), ["plugins.json", "t.other.wasm"], "modules and data gone; the other plug-in untouched");
    let plugins = plugins_json(&dir)["plugins"].clone();
    assert!(plugins.get("t.jobs").is_none() && plugins.get("t.broken").is_none() && plugins.get("t.other").is_some(), "{plugins}");
    assert_eq!(m.unloaded("t.jobs"), None);
    // gone: removing again, or something never there, is an error
    assert_eq!(m.uninstall("t.jobs"), Err(Error::NotFound("t.jobs".into())));
    assert!(matches!(m.uninstall("nope"), Err(Error::NotFound(_))));
    drop(m);
    let m = Manager::open_with_features(&dir, &["jobs"]).unwrap();
    assert!(m.get("t.jobs").is_none() && m.failed.is_empty(), "removed for good");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A module that can't be deleted (a read-only folder): the plug-in stays listed and its entry saved.
#[cfg(unix)]
#[test]
fn a_failed_removal_forgets_nothing() {
    use std::os::unix::fs::PermissionsExt;
    let dir = folder("readonly");
    let mut m = Manager::open_with_features(&dir, &["jobs"]).unwrap();
    m.install_bytes(&plugin("t.jobs", "1.0.0", &["jobs"], true), Some(catalog_and_write()), false).unwrap();
    m.set_metadata_field("t.jobs", "7", "job", &json!("J-1")).unwrap();
    drop(m);
    let entry = plugins_json(&dir)["plugins"]["t.jobs"].clone();
    // copied in under another name: `<id>.wasm` isn't there, the module is found by its own path
    std::fs::rename(dir.join("t.jobs.wasm"), dir.join("copied-in.wasm")).unwrap();
    let mut m = Manager::open_with_features(&dir, &[]).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o500)).unwrap();
    let r = m.uninstall("t.jobs");
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(matches!(r, Err(Error::Io(_))), "{r:?}");
    assert!(m.failed.iter().any(|f| f.id == "t.jobs"), "still listed");
    assert!(dir.join("copied-in.wasm").exists());
    assert!(dir.join("data/t.jobs.json").exists(), "its data kept with it");
    m.install_bytes(&plugin("t.other", "1.0.0", &[], false), None, false).unwrap();
    assert_eq!(plugins_json(&dir)["plugins"]["t.jobs"], entry, "the next save keeps its entry");
    let _ = std::fs::remove_dir_all(&dir);
}
