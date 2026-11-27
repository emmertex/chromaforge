//! The diagnostics recorder (issue #1363) through the real CLI binary: off unless `<PREFIX>_DIAG` is set, and when it
//! is, a Lightroom import leaves its steps in the ring, nothing in flight, and per-photo timings in the stats.
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

/// `library.importLightroom` of the one-photo synthetic catalog, with `diag` as `<PREFIX>_DIAG` (`None`: unset).
fn import(dir: &Path, diag: Option<&str>) -> PathBuf {
    let catalog = lightroom_fixture::write_small_catalog(dir);
    let library = dir.join("library");
    let diag_dir = dir.join("diag");
    let mut cmd = Command::new(BIN);
    cmd.args(["run", "--library", library.to_str().unwrap(), "library.importLightroom", &format!("path={}", catalog.display())])
        .env(lcc_brand::env_var("NO_PREFS"), "1")
        .env(lcc_brand::env_var("GPU"), "0")
        .env(lcc_brand::env_var("DIAG_DIR"), &diag_dir)
        .env_remove(lcc_brand::env_var("DIAG"));
    if let Some(d) = diag {
        cmd.env(lcc_brand::env_var("DIAG"), d);
    }
    let out = cmd.output().unwrap();
    assert!(out.status.success(), "stdout={} stderr={}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    diag_dir
}

/// The one file named `<stem>-<pid>.json` in `dir`.
fn only(dir: &Path, stem: &str) -> Value {
    let files: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with(&format!("{stem}-"))))
        .collect();
    assert_eq!(files.len(), 1, "{stem}: {files:?}");
    serde_json::from_slice(&std::fs::read(&files[0]).unwrap()).unwrap()
}

#[test]
fn off_by_default_writes_nothing() {
    let s = scratch("off");
    let diag_dir = import(&s.0, None);
    assert!(!diag_dir.exists(), "no diagnostics folder without <PREFIX>_DIAG");
    let s = scratch("zero");
    assert!(!import(&s.0, Some("0")).exists(), "`0` is off");
}

#[test]
fn a_lightroom_import_leaves_its_steps_in_the_ring_and_nothing_in_flight() {
    let s = scratch("on");
    let diag_dir = import(&s.0, Some("normal,commit=trace,catalog=detail"));
    let inflight = only(&diag_dir, "inflight");
    assert_eq!(inflight["open"], serde_json::json!([]), "the process ended with nothing open: {inflight}");
    let ring = only(&diag_dir, "ring");
    let entries = ring["entries"].as_array().unwrap();
    let outcome = |op: &str| entries.iter().find(|e| e["op"] == op).map(|e| e["outcome"].clone());
    assert_eq!(outcome("start"), Some(Value::Null), "the recorder's start event: {ring}");
    for op in ["lightroom_commit", "import_commit", "apply_prepared", "apply_photos", "persist"] {
        assert_eq!(outcome(op), Some(Value::from("ok")), "{op}: {ring}");
    }
    let photos = entries.iter().find(|e| e["op"] == "apply_photos").unwrap();
    assert!(photos["fields"].to_string().contains("\"photos\""), "the step names how many photos: {photos}");
    let stats = only(&diag_dir, "stats");
    let row = stats["stats"].as_array().unwrap().iter().find(|r| r["op"] == "apply_photo").cloned();
    assert_eq!(row.map(|r| r["count"].clone()), Some(Value::from(1)), "one photo applied, timed as a leaf span: {stats}");
}
