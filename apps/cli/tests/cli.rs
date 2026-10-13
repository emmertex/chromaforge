//! End-to-end tests of the command-line binary: `mcp` over real stdio pipes, `render`, `commands`.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

const BIN: &str = env!("CARGO_BIN_EXE_app-cli");

#[test]
fn contact_sheet_exports_pdf_and_protects_originals() {
    let input = tmp("contact-sheet-input.png");
    gradient_png(&input);
    let out = tmp("contact-sheet.pdf");
    let (ok, lines, err) = run_cli(
        &[
            "--import",
            input.to_str().unwrap(),
            "export.contactSheet",
            &format!("path={}", out.display()),
            "paper=letter",
            "landscape=true",
            "columns=2",
            "rows=3",
        ],
        None,
    );
    assert!(ok, "{err}: {lines:?}");
    assert_eq!(lines[0]["result"]["pages"], 1);
    assert_eq!(lines[0]["result"]["photos"], 1);
    let bytes = std::fs::read(&out).unwrap();
    assert!(bytes.starts_with(b"%PDF-1.4"));
    assert!(String::from_utf8_lossy(&bytes).contains("/MediaBox [0 0 792.00 612.00]"));
    let before = std::fs::read(&input).unwrap();
    for target in [input.clone(), input.with_extension("xmp")] {
        if target != input {
            std::fs::write(&target, b"existing sidecar").unwrap();
        }
        let (ok, _, _) = run_cli(&["--import", input.to_str().unwrap(), "export.contactSheet", &format!("path={}", target.display())], None);
        assert!(!ok, "overwrote protected file");
    }
    assert_eq!(std::fs::read(&input).unwrap(), before);
    assert_eq!(std::fs::read(input.with_extension("xmp")).unwrap(), b"existing sidecar");
    let prior = std::fs::read(&out).unwrap();
    let (ok, _, _) = run_cli(&["--import", input.to_str().unwrap(), "export.contactSheet", &format!("path={}", out.display()), "columns=0"], None);
    assert!(!ok);
    assert_eq!(std::fs::read(&out).unwrap(), prior, "failed export changed prior output");
}

fn tmp(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("app-cli-test-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d.join(name)
}

fn gradient_png(path: &std::path::Path) {
    let img = lcc_raster::Rgba8::from_fn(120, 80, |x, y| [(x * 2) as u8, (y * 3) as u8, 100, 255]);
    let png = lcc_codecs::encode_png(&lcc_codecs::EncodeImage::rgba8(&img), &Default::default()).unwrap();
    std::fs::write(path, png).unwrap();
}

#[test]
fn calibrate_reads_raf_inputs_and_reports_bad_files() {
    let input = tmp("invalid.RAF");
    std::fs::write(&input, b"FUJIFILMCCD-RAW").unwrap();
    let out = tmp("raf-profiles");
    let result = Command::new(BIN).args(["calibrate", "--out"]).arg(&out).arg(&input).output().unwrap();
    assert!(!result.status.success());
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains("1 of 1 raw files"), "RAF was silently ignored: {stderr}");
    assert!(stderr.contains("invalid.RAF") && stderr.contains("no profile written"), "{stderr}");
    assert!(!out.join("X-T4.json").exists(), "bad input must not produce a profile");
}

#[test]
fn camdb_shows_entries_and_refuses_converted_dngs() {
    let result = Command::new(BIN).args(["camdb", "show", "Canon", "Canon EOS 600D"]).output().unwrap();
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    let out = String::from_utf8_lossy(&result.stdout);
    assert!(out.contains("source = \"spectral-sensitivities\"") && out.contains("color_matrix_1"), "{out}");

    let bad = tmp("not-a.dng");
    std::fs::write(&bad, b"II*\0garbage").unwrap();
    let result = Command::new(BIN).args(["camdb", "extract"]).arg(&bad).output().unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("not-a.dng"));
    assert!(result.stdout.is_empty(), "nothing is printed for a refused file");
    assert!(!Command::new(BIN).args(["camdb", "show", "Nobody", "Nothing"]).output().unwrap().status.success());
}

fn mean(path: &std::path::Path) -> f64 {
    let d = lcc_codecs::decode(&std::fs::read(path).unwrap(), Default::default()).unwrap();
    let img = d.to_srgb8();
    img.data.iter().map(|p| (p[0] as u32 + p[1] as u32 + p[2] as u32) as f64).sum::<f64>() / (3 * img.data.len()) as f64
}

#[test]
fn mcp_over_stdio_sets_exposure_and_renders() {
    let input = tmp("in.png");
    gradient_png(&input);
    let mut child =
        Command::new(BIN).args(["mcp", input.to_str().unwrap()]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut rpc = |id: u64, method: &str, params: Value| -> Value {
        writeln!(stdin, "{}", json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})).unwrap();
        stdin.flush().unwrap();
        let mut line = String::new();
        stdout.read_line(&mut line).unwrap();
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["id"], id);
        v
    };
    let init = rpc(1, "initialize", json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "cli-test", "version": "0"}}));
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
    let tools = rpc(2, "tools/list", json!({}));
    assert!(tools["result"]["tools"].as_array().unwrap().len() > 50);
    let base = tmp("base.png");
    let bright = tmp("bright.png");
    let r = rpc(3, "tools/call", json!({"name": "render_photo", "arguments": {"path": base.to_str().unwrap()}}));
    assert_eq!(r["result"]["isError"], false, "{r}");
    let r = rpc(4, "tools/call", json!({"name": "set_develop", "arguments": {"values": {"light.exposure": 1.0}}}));
    assert_eq!(r["result"]["isError"], false, "{r}");
    let r = rpc(5, "tools/call", json!({"name": "render_photo", "arguments": {"path": bright.to_str().unwrap()}}));
    assert_eq!(r["result"]["isError"], false, "{r}");
    assert_eq!(r["result"]["content"][0]["type"], "image");
    drop(stdin);
    assert!(child.wait().unwrap().success());
    let (a, b) = (mean(&base), mean(&bright));
    println!("cli mcp: mean sRGB exposure 0 → {a:.1}, +1 → {b:.1}");
    assert!(b > a + 10.0, "{a} → {b}");
}

#[test]
fn render_subcommand() {
    let input = tmp("r-in.png");
    gradient_png(&input);
    let plain = tmp("r-plain.png");
    let out = tmp("r-out.jpg");
    let st = Command::new(BIN).args(["render", input.to_str().unwrap(), "-o", plain.to_str().unwrap()]).status().unwrap();
    assert!(st.success());
    let st = Command::new(BIN)
        .args(["render", input.to_str().unwrap(), "-o", out.to_str().unwrap(), "--set", "light.exposure=1", "--size", "60"])
        .status()
        .unwrap();
    assert!(st.success());
    let d = lcc_codecs::decode(&std::fs::read(&out).unwrap(), Default::default()).unwrap();
    assert_eq!((d.width, d.height), (60, 40));
    let d = lcc_codecs::decode(&std::fs::read(&plain).unwrap(), Default::default()).unwrap();
    assert_eq!((d.width, d.height), (120, 80));
    assert!(mean(&out) > mean(&plain) + 10.0);
    // Bad control id fails cleanly.
    let o = Command::new(BIN).args(["render", input.to_str().unwrap(), "-o", out.to_str().unwrap(), "--set", "nope=1"]).output().unwrap();
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("unknown control"));
}

/// Issue #93: `render IMG -o IMG` replaced its own input with the render.
#[test]
fn render_refuses_to_overwrite_its_input() {
    let input = tmp("self-in.png");
    gradient_png(&input);
    let before = std::fs::read(&input).unwrap();
    let dir = input.parent().unwrap();
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    let other_spelling = dir.join("sub/../self-in.png");
    for out in [&input, &other_spelling] {
        let o =
            Command::new(BIN).args(["render", input.to_str().unwrap(), "-o", out.to_str().unwrap(), "--set", "light.exposure=1"]).output().unwrap();
        assert!(!o.status.success());
        assert!(String::from_utf8_lossy(&o.stderr).contains("never writes over an original"), "{}", String::from_utf8_lossy(&o.stderr));
    }
    assert_eq!(std::fs::read(&input).unwrap(), before, "the input is untouched");
}

#[test]
fn render_export_options() {
    let input = tmp("o-in.png");
    gradient_png(&input);
    let out = tmp("o-out.tif");
    let st = Command::new(BIN)
        .args(["render", input.to_str().unwrap(), "-o", out.to_str().unwrap()])
        .args(["--opt", "shortEdge=40", "--opt", "colorSpace=displayP3", "--opt", "bitDepth=16"])
        .status()
        .unwrap();
    assert!(st.success());
    let d = lcc_codecs::decode(&std::fs::read(&out).unwrap(), Default::default()).unwrap();
    assert_eq!((d.width, d.height), (60, 40));
    assert_eq!(d.space.named, Some(lcc_codecs::NamedSpace::DisplayP3));
    // original + sidecar named after the output
    let out = tmp("o-copy.png");
    let st = Command::new(BIN)
        .args(["render", input.to_str().unwrap(), "-o", out.to_str().unwrap(), "--set", "light.exposure=1", "--opt", "format=original"])
        .status()
        .unwrap();
    assert!(st.success());
    assert_eq!(std::fs::read(&out).unwrap(), std::fs::read(&input).unwrap());
    let xmp = std::fs::read_to_string(out.with_extension("xmp")).unwrap();
    assert!(xmp.contains("exposure"), "{xmp}");
    // unknown extension fails with a hint
    let o = Command::new(BIN).args(["render", input.to_str().unwrap(), "-o", tmp("x.bmp").to_str().unwrap()]).output().unwrap();
    assert!(!o.status.success() && String::from_utf8_lossy(&o.stderr).contains("unknown extension"));
}

#[test]
fn commands_subcommand_lists_registry() {
    let o = Command::new(BIN).args(["commands", "--json"]).output().unwrap();
    assert!(o.status.success());
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    let ids: Vec<&str> = v.as_array().unwrap().iter().filter_map(|c| c["id"].as_str()).collect();
    for id in ["develop.set", "photo.rate", "library.import", "edit.undo", "app.export"] {
        assert!(ids.contains(&id), "{id}");
    }
    let o = Command::new(BIN).arg("controls").output().unwrap();
    assert!(String::from_utf8_lossy(&o.stdout).contains("light.exposure"));
}

#[test]
fn snapshot_subcommand_renders_the_ui_headlessly() {
    let script = tmp("snap.jsonl");
    let a = tmp("snap-grid.png");
    let b = tmp("snap-export.png");
    std::fs::write(
        &script,
        format!(
            "# comment\n{}\n{}\n{}\n{}\n{}\n{}\n",
            json!({"method": "ui.set", "params": {"view": "photoGrid"}}),
            // (settled: under load a grid shot taken before its thumbnails is darker than the
            // dimmed export dialog over them, and the comparison below fails)
            json!({"method": "ui.settle", "params": {"timeoutMs": 20000}}),
            json!({"method": "ui.screenshot"}),
            json!({"method": "engine.execute", "params": {"command": "dialog.export"}}),
            json!({"method": "ui.settle", "params": {"timeoutMs": 20000}}),
            json!({"method": "ui.screenshot", "params": {"path": b.to_str().unwrap()}}),
        ),
    )
    .unwrap();
    let o = Command::new(BIN)
        .args(["snapshot", "--demo", "--script", script.to_str().unwrap(), "-o", a.to_str().unwrap(), "--size", "640x400"])
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let replies: Vec<Value> = String::from_utf8_lossy(&o.stdout).lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(replies.len(), 6);
    assert!(replies.iter().all(|r| r["ok"] == true), "{replies:?}");
    for (p, dimmed) in [(&a, false), (&b, true)] {
        let d = lcc_codecs::decode(&std::fs::read(p).unwrap(), Default::default()).unwrap();
        assert_eq!((d.width, d.height), (640, 400));
        // the export dialog dims everything around it
        assert_eq!(mean(p) < mean(&a) - 2.0, dimmed, "{}", p.display());
    }
    // no script: one settled screenshot, at 2× scale
    let o = Command::new(BIN).args(["snapshot", "-o", a.to_str().unwrap(), "--size", "320x240", "--scale", "2"]).output().unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let d = lcc_codecs::decode(&std::fs::read(&a).unwrap(), Default::default()).unwrap();
    assert_eq!((d.width, d.height), (640, 480));
}

/// `snapshot` sessions look for the SAM 3 model where the app does (`<PREFIX>_SAM3_DIR`), so a
/// scripted Object / Describe mask works headlessly instead of failing with "no folder is set".
#[test]
fn snapshot_sessions_get_the_sam3_model_folder() {
    let script = tmp("snap-sam3.jsonl");
    let out = tmp("snap-sam3.png");
    let dir = tmp("snap-sam3-model");
    std::fs::write(&script, format!("{}\n", json!({"method": "engine.execute", "params": {"command": "segment.model.status"}}))).unwrap();
    let o = Command::new(BIN)
        .env(lcc_brand::env_var("SAM3_DIR"), &dir)
        .args(["snapshot", "--demo", "--script", script.to_str().unwrap(), "-o", out.to_str().unwrap(), "--size", "320x200"])
        .output()
        .unwrap();
    let reply: Value = serde_json::from_str(String::from_utf8_lossy(&o.stdout).lines().next().unwrap()).unwrap();
    assert_eq!(reply["result"]["dir"].as_str(), dir.to_str(), "{reply}");
}

/// Issue #166: a scripted request that fails makes `snapshot` exit non-zero. The failed reply is
/// still printed, the later lines still run, and the final `-o` screenshot is still written.
#[test]
fn snapshot_script_failure_exits_non_zero() {
    let script = tmp("snap-fail.jsonl");
    let out = tmp("snap-fail.png");
    std::fs::write(
        &script,
        format!(
            "{}\n{}\n",
            json!({"method": "ui.set", "params": {"view": "grid"}}), // not a view: `photoGrid`, `detail`, …
            json!({"method": "ui.set", "params": {"view": "photoGrid"}}),
        ),
    )
    .unwrap();
    let _ = std::fs::remove_file(&out);
    let o = Command::new(BIN)
        .args(["snapshot", "--demo", "--script", script.to_str().unwrap(), "-o", out.to_str().unwrap(), "--size", "320x200"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&o.stderr);
    assert!(!o.status.success(), "a failed scripted request must fail the run\n{stderr}");
    assert!(stderr.contains("1 scripted request(s) failed"), "{stderr}");
    let replies: Vec<Value> = String::from_utf8_lossy(&o.stdout).lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(replies.len(), 2, "{replies:?}");
    assert_eq!(replies[0]["ok"], false, "{replies:?}");
    assert!(replies[0]["error"].as_str().unwrap_or("").contains("grid"), "{replies:?}");
    assert_eq!(replies[1]["ok"], true, "{replies:?}");
    // continue-and-report: the final screenshot is still written
    let d = lcc_codecs::decode(&std::fs::read(&out).unwrap(), Default::default()).unwrap();
    assert_eq!((d.width, d.height), (320, 200));
}

#[test]
fn snapshot_ui_zoom_keeps_requested_pixel_dimensions() {
    let script = tmp("snap-ui-zoom.jsonl");
    std::fs::write(&script, format!("{}\n{}\n", json!({"method": "ui.zoomFactor", "params": {"factor": 1.1}}), json!({"method": "ui.inspect"})))
        .unwrap();
    for (size, scale, expected) in [("400x240", "1", (400, 240)), ("345x200", "0.9", (311, 180))] {
        let out = tmp(&format!("snap-ui-zoom-{scale}.png"));
        let o = Command::new(BIN)
            .args(["snapshot", "--script", script.to_str().unwrap(), "-o", out.to_str().unwrap(), "--size", size, "--scale", scale])
            .output()
            .unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        let replies: Vec<Value> = String::from_utf8_lossy(&o.stdout).lines().map(|line| serde_json::from_str(line).unwrap()).collect();
        assert!(replies.iter().all(|reply| reply["ok"] == true), "{replies:?}");
        assert!(replies[1]["result"]["pixelsPerPoint"].as_f64().unwrap() > scale.parse::<f64>().unwrap());
        let decoded = lcc_codecs::decode(&std::fs::read(out).unwrap(), Default::default()).unwrap();
        assert_eq!((decoded.width, decoded.height), expected);
    }
}

/// Headless snapshots default to CPU photo rendering as well as CPU UI rasterization.
#[test]
fn snapshot_defaults_to_cpu_without_gpu_environment_overrides() {
    let script = tmp("default-cpu.jsonl");
    std::fs::write(
        &script,
        format!("{}\n{}\n", json!({"method": "engine.execute", "params": {"command": "app.gpu"}}), json!({"method": "ui.screenshot"})),
    )
    .unwrap();
    let image = tmp("default-cpu.png");
    let output = Command::new(BIN)
        .env_remove(lcc_brand::env_var("GPU"))
        .env_remove(lcc_brand::env_var("GPU_BACKEND"))
        .env_remove("WGPU_BACKEND")
        .args(["snapshot", "--demo", "--script", script.to_str().unwrap(), "-o", image.to_str().unwrap(), "--size", "480x320"])
        .output()
        .unwrap();
    assert!(output.status.success(), "status {}: {}", output.status, String::from_utf8_lossy(&output.stderr));
    let replies: Vec<Value> = String::from_utf8_lossy(&output.stdout).lines().map(|line| serde_json::from_str(line).unwrap()).collect();
    assert!(replies.iter().all(|reply| reply["ok"] == true), "{replies:?}");
    let gpu = &replies[0]["result"];
    assert_eq!(gpu["enabled"], false, "{gpu}");
    assert_eq!(gpu["available"], false, "{gpu}");
    assert_eq!(gpu["adapter"], Value::Null, "no device is created: {gpu}");
    assert!(gpu["reason"].as_str().is_some_and(|reason| reason.contains("preference")), "{gpu}");
    let decoded = lcc_codecs::decode(&std::fs::read(image).unwrap(), Default::default()).unwrap();
    assert_eq!((decoded.width, decoded.height), (480, 320));
}

/// Issue #136: with the GPU switched off from the environment the UI starts and renders on the CPU,
/// without creating a GPU device (no driver is loaded), and says why.
#[test]
fn snapshot_starts_without_a_gpu() {
    let script = tmp("nogpu.jsonl");
    std::fs::write(
        &script,
        format!(
            "{}\n{}\n{}\n",
            json!({"method": "ui.set", "params": {"view": "photoGrid"}}),
            json!({"method": "engine.execute", "params": {"command": "app.gpu"}}),
            json!({"method": "ui.screenshot"}),
        ),
    )
    .unwrap();
    for (var, value) in [(lcc_brand::env_var("GPU"), "0"), (lcc_brand::env_var("GPU_BACKEND"), "off")] {
        let reason = format!("{var}={value}");
        let out = tmp(&format!("nogpu-{var}.png"));
        let o = Command::new(BIN)
            .env_remove(lcc_brand::env_var("GPU"))
            .env_remove(lcc_brand::env_var("GPU_BACKEND"))
            .env(&var, value)
            .args(["snapshot", "--demo", "--script", script.to_str().unwrap(), "-o", out.to_str().unwrap(), "--size", "480x320"])
            .output()
            .unwrap();
        assert!(o.status.success(), "{var}: {}", String::from_utf8_lossy(&o.stderr));
        let replies: Vec<Value> = String::from_utf8_lossy(&o.stdout).lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert!(replies.iter().all(|r| r["ok"] == true), "{replies:?}");
        let gpu = &replies[1]["result"];
        assert_eq!(gpu["available"], false, "{var}: {gpu}");
        assert_eq!(gpu["adapter"], Value::Null, "{var}: no device was created: {gpu}");
        assert!(gpu["reason"].as_str().is_some_and(|r| r.contains(&reason)), "{var}: {gpu}");
        let d = lcc_codecs::decode(&std::fs::read(&out).unwrap(), Default::default()).unwrap();
        assert_eq!((d.width, d.height), (480, 320));
    }
}

fn run_cli(args: &[&str], stdin: Option<&str>) -> (bool, Vec<Value>, String) {
    let mut child = Command::new(BIN).arg("run").args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    if let Some(text) = stdin {
        child.stdin.take().unwrap().write_all(text.as_bytes()).unwrap();
    }
    drop(child.stdin.take());
    let o = child.wait_with_output().unwrap();
    let lines = String::from_utf8_lossy(&o.stdout).lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    (o.status.success(), lines, String::from_utf8_lossy(&o.stderr).to_string())
}

#[test]
fn run_subcommand_chains_commands_and_persists_a_library() {
    let input = tmp("run-in.png");
    gradient_png(&input);
    let lib = tmp("run-lib");
    let _ = std::fs::remove_dir_all(&lib);
    let out = tmp("run-out.jpg");
    let (lib_s, in_s, out_s) = (lib.to_str().unwrap(), input.to_str().unwrap(), out.to_str().unwrap());
    // import into a library, edit, export, in one invocation
    let (ok, lines, err) = run_cli(
        &[
            "--library",
            lib_s,
            "--import",
            in_s,
            "develop.set",
            "control=light.exposure",
            "value=0.75",
            "app.export",
            &format!("path={out_s}"),
            "width=60",
        ],
        None,
    );
    assert!(ok, "{err}");
    assert_eq!(lines.len(), 2);
    assert!(lines.iter().all(|l| l["ok"] == true), "{lines:?}");
    assert_eq!(lines[1]["result"]["width"], 60);
    assert!(out.exists());
    // a second invocation sees the saved edit (same file: selected again, not re-imported)
    let (ok, lines, err) = run_cli(&["--library", lib_s, "--import", in_s, "develop.get"], None);
    assert!(ok, "{err}");
    assert_eq!(lines[0]["result"]["light"]["exposure"], 0.75);
    // JSON-lines script from stdin, a failing command stops the run with a non-zero status
    let script = "{\"command\": \"library.info\"}\n# comment\n{\"command\": \"no.such\"}\n{\"command\": \"library.info\"}\n";
    let (ok, lines, _) = run_cli(&["--demo", "--script", "-"], Some(script));
    assert!(!ok);
    assert_eq!(lines.len(), 2, "stopped after the failure: {lines:?}");
    assert_eq!(lines[1]["ok"], false);
    let (ok, lines, _) = run_cli(&["--demo", "--keep-going", "--script", "-"], Some(script));
    assert!(!ok);
    assert_eq!(lines.len(), 3);
    // control-protocol methods work headlessly too; bad usage is reported
    let (ok, lines, _) = run_cli(&["--demo", "engine.commands"], None);
    assert!(ok && lines[0]["result"].as_array().is_some_and(|a| a.len() > 50));
    let (ok, _, err) = run_cli(&["--demo", "x=1"], None);
    assert!(!ok && err.contains("before any command"), "{err}");
}

#[test]
fn devices_are_listed_and_imported_from() {
    let base = tmp("devices");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("CARD/DCIM/100TEST")).unwrap();
    gradient_png(&base.join("CARD/DCIM/100TEST/IMG_0001.png"));
    let o = Command::new(BIN).env(lcc_brand::env_var("DEVICE_ROOTS"), &base).args(["run", "--demo", "library.devices"]).output().unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let line: Value = serde_json::from_slice(o.stdout.split(|b| *b == b'\n').next().unwrap()).unwrap();
    let dev = &line["result"][0];
    assert_eq!(dev["name"], "CARD", "{line}");
    // the import review of its DCIM folder finds the photo
    let dcim = dev["path"].as_str().unwrap().to_string();
    // JSON-escaped: a Windows path has backslashes
    let paths = serde_json::to_string(&[&dcim]).unwrap();
    let o = Command::new(BIN).args(["run", "library.importPreview", &format!("paths={paths}")]).output().unwrap();
    let line: Value = serde_json::from_slice(o.stdout.split(|b| *b == b'\n').next().unwrap()).unwrap();
    assert_eq!(line["result"]["candidates"].as_array().map(Vec::len), Some(1), "{line}");
}

/// A library still in the v3 catalog format is upgraded before the command runs, with progress on
/// stderr; the next run opens it directly.
#[test]
fn an_old_library_is_upgraded_with_progress() {
    let lib = tmp("v3-lib");
    let _ = std::fs::remove_dir_all(&lib);
    std::fs::create_dir_all(&lib).unwrap();
    let catalog = json!({"photos": {}, "albums": {}, "next_photo": 1, "next_album": 1});
    std::fs::write(lib.join("catalog.snap"), json!({"format": "lcc-catalog", "version": 3, "seq": 0, "catalog": catalog}).to_string()).unwrap();
    let lib_s = lib.to_str().unwrap();
    let (ok, lines, stderr) = run_cli(&["--library", lib_s, "library.info"], None);
    assert!(ok, "{stderr}");
    assert_eq!(lines[0]["ok"], true);
    assert!(stderr.contains("upgrading the catalog") && stderr.contains("catalog upgraded"), "{stderr}");
    assert!(lib.join("catalog.redb").exists());
    let (ok, _, stderr) = run_cli(&["--library", lib_s, "library.info"], None);
    assert!(ok, "{stderr}");
    assert!(!stderr.contains("upgrading"), "{stderr}");
    let _ = std::fs::remove_dir_all(&lib);
}

/// Issue #99: a library open in one process (here `mcp --library`) is refused by a second one,
/// with who has it and how to drive the running app instead; free again once the first exits.
#[test]
fn a_library_open_in_another_process_is_refused() {
    let lib = tmp("locked-lib");
    let _ = std::fs::remove_dir_all(&lib);
    let lib_s = lib.to_str().unwrap();
    let mut holder =
        Command::new(BIN).args(["mcp", "--library", lib_s]).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().unwrap();
    let mut err = BufReader::new(holder.stderr.take().unwrap());
    let mut line = String::new();
    while !line.contains("opened library") {
        line.clear();
        assert!(err.read_line(&mut line).unwrap() > 0, "mcp exited before opening the library");
    }
    let (ok, _, stderr) = run_cli(&["--library", lib_s, "library.info"], None);
    assert!(!ok);
    assert!(stderr.contains("already open in ") && stderr.contains(&format!("process {}", holder.id())), "{stderr}");
    assert!(stderr.contains("mcp --connect"), "{stderr}");

    drop(holder.stdin.take()); // EOF: the server exits and lets go of the library
    assert!(holder.wait().unwrap().success());
    let (ok, lines, stderr) = run_cli(&["--library", lib_s, "library.info"], None);
    assert!(ok, "{stderr}");
    assert_eq!(lines[0]["ok"], true);
    let _ = std::fs::remove_dir_all(&lib);
}

/// Issue #167: when `--connect ADDR` cannot reach the app, the recovery hint must name the port
/// ADDR actually names — following the old fixed "7980" left the retry connecting elsewhere.
#[test]
fn connect_failure_hint_names_the_attempted_port() {
    let (ok, _lines, err) = run_cli(&["--connect", "127.0.0.1:18437", "ui.inspect"], None);
    assert!(!ok);
    assert!(err.contains("--control 18437"), "the hint must name the port it tried: {err}");
    assert!(!err.contains("--control 7980"), "{err}");
}

/// The CLI logs warnings on stderr (#168): with `RUST_LOG=warn` an unknown GPU backend name is
/// reported, and `RUST_LOG=off` silences it. Before, no logger was installed and nothing appeared.
#[test]
fn warnings_are_logged_on_stderr() {
    let input = tmp("log-in.png");
    gradient_png(&input);
    let out = tmp("log-out.jpg");
    let run = |level: &str| {
        let o = Command::new(BIN)
            .args(["render", input.to_str().unwrap(), "-o", out.to_str().unwrap()])
            .env(lcc_brand::env_var("GPU_BACKEND"), "bogus")
            .env("RUST_LOG", level)
            .env_remove(lcc_brand::env_var("LOG"))
            .env_remove(lcc_brand::env_var("GPU"))
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&o.stderr).into_owned();
        assert!(o.status.success(), "{stderr}");
        stderr
    };
    let warn = run("warn");
    assert!(warn.contains(&format!("{}=bogus names no known backend", lcc_brand::env_var("GPU_BACKEND"))), "{warn}");
    let off = run("off");
    assert!(!off.contains("names no known backend"), "{off}");
}

#[test]
fn snapshot_rejects_invalid_dimensions_before_creating_an_image() {
    for (index, (size, scale)) in
        [("NaNx100", "1"), ("100x100", "inf"), ("9000x9000", "1"), ("9000x100", "2"), ("100x100", "0.001")].into_iter().enumerate()
    {
        let image = tmp(&format!("invalid-viewport-{index}.png"));
        let output = Command::new(BIN).args(["snapshot", "--size", size, "--scale", scale, "-o"]).arg(&image).output().unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("snapshot: bad --size/--scale"), "{}", String::from_utf8_lossy(&output.stderr));
        assert!(!image.exists());
    }
}

#[test]
fn snapshot_preserves_fractional_scale_rounding() {
    let image = tmp("fractional-scale-rounding.png");
    let output = Command::new(BIN)
        .env(lcc_brand::env_var("GPU"), "0")
        .args(["snapshot", "--size", "345x200", "--scale", "0.9", "-o"])
        .arg(&image)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let decoded = lcc_codecs::decode(&std::fs::read(image).unwrap(), Default::default()).unwrap();
    assert_eq!((decoded.width, decoded.height), (311, 180));
}

/// Snapshot's CPU setting must never replace the desktop preference, including with --library.
#[cfg(target_os = "linux")]
#[test]
fn snapshot_leaves_desktop_gpu_preferences_untouched() {
    let dir = tmp("snapshot-desktop-preferences");
    let library = dir.join("library");
    let mut session = lcc_engine::Session::new().with_fs();
    session.open_library(&library, true).unwrap();
    drop(session);
    let config = dir.join("config");
    let app_config = config.join(lcc_brand::settings_dir_name(lcc_brand::SETTINGS_DIR));
    std::fs::create_dir_all(&app_config).unwrap();
    let ui_path = app_config.join("ui.json");
    let mut ui = lcc_ui_egui::UiState::default();
    ui.settings.gpu = true;
    let saved = serde_json::to_vec_pretty(&ui).unwrap();
    std::fs::write(&ui_path, &saved).unwrap();
    let script = dir.join("preferences.jsonl");
    std::fs::write(
        &script,
        format!("{}\n{}\n", json!({"method": "engine.execute", "params": {"command": "app.gpu"}}), json!({"method": "ui.inspect"})),
    )
    .unwrap();
    for gpu_environment in [None, Some("1")] {
        let image = dir.join(format!("{}.png", gpu_environment.unwrap_or("default")));
        let mut cmd = Command::new(BIN);
        cmd.env("XDG_CONFIG_HOME", &config)
            .env_remove(lcc_brand::env_var("GPU"))
            .env_remove(lcc_brand::env_var("GPU_BACKEND"))
            .env_remove("WGPU_BACKEND");
        if let Some(value) = gpu_environment {
            cmd.env(lcc_brand::env_var("GPU"), value);
        }
        let output = cmd
            .args(["snapshot", "--library"])
            .arg(&library)
            .args(["--script"])
            .arg(&script)
            .args(["-o"])
            .arg(&image)
            .args(["--size", "480x320"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        let replies: Vec<Value> = String::from_utf8_lossy(&output.stdout).lines().map(|line| serde_json::from_str(line).unwrap()).collect();
        assert!(replies.iter().all(|reply| reply["ok"] == true), "{replies:?}");
        assert_eq!(replies[0]["result"]["enabled"], false);
        assert_eq!(replies[0]["result"]["available"], false);
        assert_eq!(replies[0]["result"]["adapter"], Value::Null);
        assert_eq!(replies[1]["result"]["ui"]["settings"]["gpu"], false);
        assert_eq!(std::fs::read(&ui_path).unwrap(), saved, "snapshot changed the desktop preferences");
        assert!(!app_config.join("ui.json.tmp").exists());
        assert!(image.exists());
    }
}

/// RT.B5: Edit In end to end with a stand-in editor (a script that takes `--wait`, overwrites the
/// file it was given and exits 0): `photo.editIn {launch, wait}` returns once the editor exits and
/// the photo is read back with the editor's pixels.
#[cfg(unix)]
#[test]
fn edit_in_waits_for_a_fake_editor_and_reloads() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tmp("edit-in-wait");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let input = dir.join("photo.png");
    gradient_png(&input);
    let replacement = dir.join("saved-by-editor.bin");
    let img = lcc_raster::Rgba8::from_fn(64, 48, |x, _| [x as u8 * 3, 10, 200, 255]);
    std::fs::write(&replacement, lcc_codecs::encode_png(&lcc_codecs::EncodeImage::rgba8(&img), &Default::default()).unwrap()).unwrap();
    let script = dir.join("fake-editor.sh");
    let log = dir.join("editor-args.txt");
    std::fs::write(
        &script,
        format!("#!/bin/sh\necho \"$@\" > '{}'\nfor f; do :; done\ncp '{}' \"$f\"\nexit 0\n", log.display(), replacement.display()),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let (ok, lines, err) = run_cli(
        &[
            "--import",
            input.to_str().unwrap(),
            "photo.editIn",
            "mode=copy",
            &format!("app={}", script.display()),
            r#"args=["--wait"]"#,
            "launch=true",
            "wait=true",
        ],
        None,
    );
    assert!(ok, "{err}: {lines:?}");
    let r = &lines[0]["result"];
    assert_eq!(r["opened"]["waited"], true, "{r}");
    assert_eq!(r["opened"]["saved"], true, "{r}");
    assert_eq!(r["opened"]["reloaded"], json!([r["id"]]), "{r}");
    let copy = r["path"].as_str().unwrap();
    assert!(copy.ends_with("photo-Edit.png"), "{copy}");
    let args = std::fs::read_to_string(&log).unwrap();
    assert!(args.starts_with("--wait ") && args.trim_end().ends_with("photo-Edit.png"), "{args}");
    assert_eq!(std::fs::read(copy).unwrap(), std::fs::read(&replacement).unwrap());
    let _ = std::fs::remove_dir_all(&dir);
}

/// `denoise-serve` serves the selected installed model; a client greets it and gets the CPU's answer for a tile
/// (within half-float rounding on the wire), and a wrong token or a missing token variable is refused.
#[cfg(feature = "denoise")]
#[test]
fn denoise_serve_runs_tiles_for_another_computer() {
    use lcc_denoise::manifest::{DenoiserManifest, Domain, Gain};
    use lcc_denoise::remote::Remote;
    use lcc_denoise::run::TileRunner;
    let models = tmp("denoise-models");
    let _ = std::fs::remove_dir_all(&models);
    let dir = models.join("synthetic");
    std::fs::create_dir_all(&dir).unwrap();
    let manifest = DenoiserManifest {
        id: "synthetic".into(),
        name: "Synthetic".into(),
        version: "1".into(),
        licence: Default::default(),
        source: None,
        sha256: None,
        size_bytes: None,
        provenance: "made for a test".into(),
        domain: Domain::BayerToRgb,
        tile: 64,
        overlap: 16,
        gain: Gain::None,
    };
    let onnx = dir.join("model.onnx");
    std::fs::write(&onnx, lcc_denoise::synthetic::unet_onnx(64, 8, 2, 7)).unwrap();
    std::fs::write(dir.join("denoise-model.json"), serde_json::to_vec(&manifest).unwrap()).unwrap();
    std::fs::write(models.join("settings.json"), r#"{"model": "synthetic"}"#).unwrap();
    let sha = lcc_denoise::hash::sha256_file(&onnx).unwrap();

    // without its token the server doesn't start
    let o = Command::new(BIN).args(["denoise-serve", "--models", models.to_str().unwrap(), "--token-env", "LC_TEST_NO_SUCH_VAR"]).output().unwrap();
    assert!(!o.status.success() && String::from_utf8_lossy(&o.stderr).contains("LC_TEST_NO_SUCH_VAR"));

    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let addr = format!("127.0.0.1:{port}");
    let token = "cli-test-token-0123456789";
    let mut child = Command::new(BIN)
        .args(["denoise-serve", "--models", models.to_str().unwrap(), "--listen", &addr, "--run-on", "cpu"])
        .env(lcc_brand::env_var("DENOISE_TOKEN"), token)
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // wait for the banner
    let mut err = BufReader::new(child.stderr.take().unwrap());
    let mut banner = String::new();
    err.read_line(&mut banner).unwrap();
    assert!(banner.contains("synthetic 1") && banner.contains(&addr), "{banner}");

    let remote = Remote::new(&addr, token, &sha, 64);
    let info = remote.hello().unwrap();
    assert!(info.device.starts_with("CPU"), "{info:?}");
    let (input, _) = lcc_denoise::runtime::test_tile(64);
    let got = remote.try_run(&input).unwrap().unwrap();
    let want = lcc_denoise::runtime::CpuRunner::load(&onnx, &manifest).unwrap().run(&input).unwrap();
    let scale = want.iter().fold(1e-9f32, |m, v| m.max(v.abs()));
    let worst = want.iter().zip(&got).fold(0f32, |m, (a, b)| m.max((a - b).abs())) / scale;
    assert!(worst < 5e-3, "remote vs local: {worst}");
    assert!(Remote::new(&addr, "a-wrong-token-000000", &sha, 64).hello().is_err());
    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&models);
}
