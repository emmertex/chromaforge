//! Example plug-in. Build: `cargo build --release --target wasm32-unknown-unknown`, then install
//! `target/wasm32-unknown-unknown/release/program_tools.wasm` from the Plug-in Manager; the first
//! run asks to "Start the program Example Converter".
//!
//! - **Send to Converter** (menu command): writes the selected photos' ids and original files into
//!   the run's folder, one `<photo id>\t<original>` line each, and the album the command was
//!   started from (if any) into `album.txt`, then starts the converter with the list's path. The
//!   converter (a stand-in in the host's tests) writes its result next to each original and lists
//!   them in `result.txt` next to the job: `<photo id>\t<path of what it made>` lines.
//! - **`program.end` hook**: reads `result.txt`, adds each result to the library, stacked on its
//!   photo and in the album the command was started from, and records it in the plug-in's own
//!   metadata namespace (`converted`).

use lcc_plugin_sdk::serde_json::{Value, json};
use lcc_plugin_sdk::{Request, export_plugin, host};

const MANIFEST: &str = r#"{
  "id": "org.example.program-tools",
  "name": "Program Tools (example)",
  "version": "0.1.0",
  "description": "Example plug-in for the SDK: starts a program",
  "author": "the app's authors",
  "requires": ["programs", "libraryAdd"],
  "permissions": {
    "catalog": true,
    "libraryAdd": true,
    "programs": [
      {"id": "converter", "name": "Example Converter",
       "macBundleIds": ["org.example.converter"], "windowsExe": "converter.exe", "linuxExe": "converter"}
    ]
  },
  "commands": [
    {"id": "send", "label": "Send to Converter", "menu": "Examples"}
  ]
}"#;

const PROGRAM: &str = "converter";

fn handle(req: &Request) -> Result<Value, String> {
    match req.hook() {
        "command" => match req.command() {
            "send" => send(&req.selection(), req.source_album()),
            other => Err(format!("unknown command {other}")),
        },
        "program.end" => end(req),
        other => Err(format!("unsupported hook {other}")),
    }
}

/// Starts the converter on `photos`, or says why it can't. `album`: the album the command was
/// started from, where the results go too.
fn send(photos: &[String], album: Option<&str>) -> Result<Value, String> {
    if photos.is_empty() {
        return Ok(json!({"message": "Select the photos to convert first."}));
    }
    let list = host::program_list()?;
    let entry = list.as_array().and_then(|l| l.iter().find(|p| p.get("program").and_then(Value::as_str) == Some(PROGRAM)));
    let name = entry.and_then(|p| p.get("name")).and_then(Value::as_str).unwrap_or(PROGRAM);
    if entry.and_then(|p| p.get("granted")).and_then(Value::as_bool) != Some(true) {
        return Ok(json!({"message": format!("Allow {name} in the Plug-in Manager first.")}));
    }
    if entry.and_then(|p| p.get("installed")).and_then(Value::as_array).is_none_or(|i| i.is_empty()) {
        return Ok(json!({"message": format!("{name} is not installed.")}));
    }
    let mut job = String::new();
    for photo in photos {
        let inspected = host::call("photo.inspect", json!({"id": photo}))?;
        let Some(original) = inspected.get("source").and_then(|s| s.get("path")).and_then(Value::as_str) else { continue };
        job.push_str(&format!("{photo}\t{original}\n"));
    }
    if job.is_empty() {
        return Ok(json!({"message": "None of the selected photos has a file to convert."}));
    }
    let files = json!({"job.txt": job, "album.txt": album.unwrap_or("")});
    let started = host::program_start(PROGRAM, &["{tmpDir}/job.txt"], files, None)?;
    let run = started.get("run").and_then(Value::as_str).unwrap_or_default();
    host::log(&format!("{run}: sent {} photo(s) to {name}", photos.len()));
    Ok(json!({"message": format!("Sent {} photo(s) to {name}.", photos.len()), "run": run}))
}

/// Adds what the converter made to the library, from `result.txt` in the run's folder: stacked on
/// its photo, and in the album the command was started from.
fn end(req: &Request) -> Result<Value, String> {
    if req.status() != "exited" {
        host::log(&format!("{}: {}", req.run(), req.status()));
        return Ok(json!({"converted": [], "photos": []}));
    }
    if let Some(code) = req.code().filter(|c| *c != 0) {
        host::log(&format!("{}: the converter failed (exit code {code})", req.run()));
        return Ok(json!({"converted": [], "photos": []}));
    }
    let result = host::read_text(&format!("{}/result.txt", req.tmp_dir()))?;
    let album = host::read_text(&format!("{}/album.txt", req.tmp_dir())).unwrap_or_default();
    let album = Some(album.trim()).filter(|a| !a.is_empty());
    let (mut converted, mut photos) = (Vec::new(), Vec::new());
    for line in result.lines() {
        let Some((photo, path)) = line.split_once('\t') else { continue };
        // a photo that left the album meanwhile still gets its result, stacked
        let added = host::library_add(path, photo, true, album).or_else(|e| match album {
            Some(_) => host::library_add(path, photo, true, None),
            None => Err(e),
        });
        match added {
            Ok(added) => {
                host::ns_set(photo, "converted", json!(path))?;
                converted.push(photo.to_string());
                photos.push(added.get("photo").cloned().unwrap_or(Value::Null));
            }
            Err(e) => host::log(&format!("{}: {path} was not added: {e}", req.run())),
        }
    }
    host::log(&format!("{}: {} photo(s) converted", req.run(), converted.len()));
    Ok(json!({"converted": converted, "photos": photos}))
}

export_plugin!(MANIFEST, handle);
