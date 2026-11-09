//! Example plug-in. Build: `cargo build --release --target wasm32-unknown-unknown`, then install
//! `target/wasm32-unknown-unknown/release/job_tools.wasm` from the Plug-in Manager.
//!
//! - **Check Photos** (menu command, a job): works through the selected photos on a worker, a few
//!   per call, with a row in the activity stack. For each photo it reads the catalog
//!   (`photo.inspect`), counts it done (or failed, with the reason), and marks it `checked` in the
//!   plug-in's own metadata namespace. Between calls it goes on with `{continue: next}`; the row's ✕
//!   stops it, and it says how far it got.

use lcc_plugin_sdk::serde_json::{Value, json};
use lcc_plugin_sdk::{Request, export_plugin, host, job};

const MANIFEST: &str = r#"{
  "id": "org.example.job-tools",
  "name": "Job Tools (example)",
  "description": "Example plug-in for the SDK: a job with progress",
  "author": "the app's authors",
  "requires": ["jobs"],
  "permissions": {"catalog": true},
  "commands": [
    {"id": "check", "label": "Check Photos", "menu": "Examples", "job": true}
  ]
}"#;

/// Photos one call works through: each call stays well inside the per-call budget.
const PER_CALL: usize = 10;

fn handle(req: &Request) -> Result<Value, String> {
    match (req.hook(), req.command()) {
        ("command", "check") => check(req),
        (hook, command) => Err(format!("unsupported request {hook} {command}")),
    }
}

/// One call of the job: the next [`PER_CALL`] photos from `state` (the index of the next photo).
fn check(req: &Request) -> Result<Value, String> {
    let photos = req.selection();
    if photos.is_empty() {
        return Ok(json!({"message": "Select some photos to check"}));
    }
    let total = photos.len();
    let mut next = req.state().as_u64().map_or(0, |n| n as usize).min(total);
    let end = next.saturating_add(PER_CALL).min(total);
    for photo in photos.get(next..end).unwrap_or_default() {
        if job::cancelled() {
            return Ok(json!({"message": format!("Stopped after {next} of {total} photos")}));
        }
        job::detail(photo);
        match host::call("photo.inspect", json!({"id": photo})) {
            Ok(_) => {
                host::ns_set(photo, "checked", json!(true))?;
                job::item_done();
            }
            Err(e) => job::item_failed(&format!("{photo}: {e}")),
        }
        next += 1;
        job::progress(next as u64, total as u64);
    }
    if next < total { Ok(job::continue_with(json!(next))) } else { Ok(json!({"message": format!("Checked {total} photos")})) }
}

export_plugin!(package: MANIFEST, handle);
