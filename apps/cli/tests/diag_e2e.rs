//! The diagnostics recorder (issue #1363) through the real CLI binary: off unless `<PREFIX>_DIAG` is set (and off for
//! the off words), and when it is, an import and a Lightroom import leave their steps in the ring with their real
//! progress and outcome, nothing in flight, a clean end, and per-item timings in the stats.
#![cfg(not(target_arch = "wasm32"))]

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

const BIN: &str = env!("CARGO_BIN_EXE_app-cli");

#[path = "support/lightroom_fixture.rs"]
#[allow(dead_code)]
mod lightroom_fixture;

struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn scratch(tag: &str) -> Scratch {
    let dir = std::env::temp_dir().join(format!("app-diag-e2e-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    Scratch(dir)
}

/// The CLI running `command`, with `diag` as `<PREFIX>_DIAG` (`None`: unset, legacy spellings included) and the
/// recorder's folder in `dir/diag`. Returns that folder.
fn cli(dir: &Path, diag: Option<&str>, command: &[String]) -> PathBuf {
    let library = dir.join("library");
    let diag_dir = dir.join("diag");
    let mut cmd = Command::new(BIN);
    cmd.args(["run", "--library", library.to_str().unwrap()])
        .args(command)
        .env(lcc_brand::env_var("NO_PREFS"), "1")
        .env(lcc_brand::env_var("GPU"), "0")
        .env(lcc_brand::env_var("DIAG_DIR"), &diag_dir);
    for prefix in std::iter::once(lcc_brand::ENV_PREFIX).chain(lcc_brand::LEGACY_ENV_PREFIXES.iter().copied()) {
        cmd.env_remove(format!("{prefix}_DIAG"));
    }
    if let Some(d) = diag {
        cmd.env(lcc_brand::env_var("DIAG"), d);
    }
    let out = cmd.output().unwrap();
    assert!(out.status.success(), "stdout={} stderr={}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    diag_dir
}

fn lightroom_import(dir: &Path, diag: Option<&str>) -> PathBuf {
    let catalog = lightroom_fixture::write_small_catalog(dir);
    cli(dir, diag, &["library.importLightroom".into(), format!("path={}", catalog.display())])
}

/// The one finished `<stem>-…json` file in `dir`.
fn only(dir: &Path, stem: &str) -> Value {
    let files: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with(&format!("{stem}-")) && n.to_string_lossy().ends_with(".json")))
        .collect();
    assert_eq!(files.len(), 1, "{stem}: {files:?}");
    serde_json::from_slice(&std::fs::read(&files[0]).unwrap()).unwrap()
}

fn entries(diag_dir: &Path) -> Vec<Value> {
    only(diag_dir, "ring")["entries"].as_array().unwrap().clone()
}

fn field(e: &Value, key: &str) -> Value {
    e["fields"].as_array().unwrap().iter().find(|f| f[0] == key).map(|f| f[1]["v"].clone()).unwrap_or(Value::Null)
}

#[test]
fn off_by_default_and_for_every_off_word() {
    for (i, diag) in [None, Some("0"), Some("false"), Some("OFF"), Some("off,off")].into_iter().enumerate() {
        let s = scratch(&format!("off{i}"));
        assert!(!lightroom_import(&s.0, diag).exists(), "no diagnostics folder for {diag:?}");
    }
}

#[test]
fn a_lightroom_import_leaves_its_steps_a_clean_end_and_nothing_in_flight() {
    let s = scratch("on");
    let diag_dir = lightroom_import(&s.0, Some("normal,commit=trace,catalog=detail"));
    let inflight = only(&diag_dir, "inflight");
    assert_eq!(inflight["open"], serde_json::json!([]), "the process ended with nothing open: {inflight}");
    assert_eq!(inflight["ended"], "clean", "{inflight}");
    let entries = entries(&diag_dir);
    let outcome = |op: &str| entries.iter().find(|e| e["op"] == op).map(|e| e["outcome"].clone());
    assert_eq!(outcome("start"), Some(Value::Null), "the recorder's start event: {entries:?}");
    for op in ["lightroom_commit", "import_commit", "apply_photos", "apply_collections", "index_json", "persist"] {
        assert_eq!(outcome(op), Some(Value::from("ok")), "{op}: {entries:?}");
    }
    let photos = entries.iter().find(|e| e["op"] == "apply_photos").unwrap();
    assert_eq!(field(photos, "done"), field(photos, "photos"), "the finished stage says every photo was done: {photos}");
    let stats = only(&diag_dir, "stats");
    let row = stats["stats"].as_array().unwrap().iter().find(|r| r["op"] == "apply_photo").cloned().unwrap();
    assert_eq!((row["count"].clone(), row["failed"].clone()), (Value::from(1), Value::from(0)), "one photo, timed as a leaf: {stats}");
}

#[test]
fn a_folder_import_records_the_probe_with_this_batchs_count() {
    let s = scratch("probe");
    let shoot = s.0.join("shoot");
    std::fs::create_dir_all(&shoot).unwrap();
    for (i, scene) in lcc_scenes::demo_library().iter().take(3).enumerate() {
        let img = scene.render_fit(64).to_srgb8();
        let jpg = lcc_codecs::encode_jpeg(&lcc_codecs::EncodeImage::rgba8(&img), 80, Default::default(), &Default::default()).unwrap();
        std::fs::write(shoot.join(format!("IMG_{i}.jpg")), jpg).unwrap();
    }
    // (the path goes through serde_json: a Windows path's backslashes are not valid JSON as written)
    let diag_dir = cli(&s.0, Some("normal,probe=trace"), &["library.import".into(), format!("paths={}", serde_json::json!([shoot.display().to_string()]))]);
    let entries = entries(&diag_dir);
    let probes: Vec<&Value> = entries.iter().filter(|e| e["op"] == "probe_all").collect();
    assert!(!probes.is_empty(), "{entries:?}");
    for p in &probes {
        assert_eq!(p["outcome"], "ok", "{p}");
        assert_eq!(field(p, "done"), field(p, "files"), "done counts this call's files, all of them: {p}");
    }
    let probed: u64 = probes.iter().map(|p| field(p, "files").as_u64().unwrap()).sum();
    assert_eq!(probed, 3, "{probes:?}");
    assert!(entries.iter().any(|e| e["op"] == "import_commit" && e["outcome"] == "ok"), "{entries:?}");
    let stats = only(&diag_dir, "stats");
    let row = stats["stats"].as_array().unwrap().iter().find(|r| r["op"] == "probe_file").cloned().unwrap();
    assert_eq!(row["count"], 3, "{stats}");
    assert_eq!(only(&diag_dir, "inflight")["ended"], "clean");
}
