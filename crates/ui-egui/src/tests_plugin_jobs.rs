//! Plug-in jobs in the window: a job started from its menu item returns at once, shows a row in the
//! activity stack with its progress, the row's ✕ stops it, and how it ended comes as a toast.

use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::headless::Headless;
use crate::{LccApp, Services};

const T: Duration = Duration::from_secs(60);

/// A job `go` ("Go Far") of plug-in `id` that sends each of `requests`, then replies `reply`.
fn job_module(id: &str, requests: &[&str], reply: &str) -> Vec<u8> {
    let manifest =
        format!(r#"{{"id":"{id}","name":"Job Test","version":"1.0.0","requires":["jobs"],"commands":[{{"id":"go","label":"Go Far","job":true}}]}}"#);
    let esc = |s: &str| s.replace('"', "\\\"");
    let data: String = requests.iter().enumerate().map(|(k, r)| format!("(data (i32.const {}) \"{}\")", 8192 + 1024 * k, esc(r))).collect();
    let calls: String =
        requests.iter().enumerate().map(|(k, r)| format!("(drop (call $call (i32.const {}) (i32.const {})))", 8192 + 1024 * k, r.len())).collect();
    let src = format!(
        r#"(module (import "host" "call" (func $call (param i32 i32) (result i32))) (memory (export "memory") 2)
  (data (i32.const 16) "{m}") (data (i32.const 4096) "{r}") {data}
  (func (export "lcc_abi_version") (result i32) (i32.const 1))
  (func (export "lcc_manifest") (result i64) (i64.or (i64.shl (i64.const {ml}) (i64.const 32)) (i64.const 16)))
  (func (export "lcc_alloc") (param i32) (result i32) (i32.const 65536))
  (func (export "lcc_handle") (param i32 i32) (result i64) {calls} (i64.or (i64.shl (i64.const {rl}) (i64.const 32)) (i64.const 4096))))"#,
        m = esc(&manifest),
        r = esc(reply),
        ml = manifest.len(),
        rl = reply.len()
    );
    wat::parse_str(&src).unwrap()
}

fn demo_with(module: &[u8]) -> Headless {
    // install into memory, never into the real user's settings folder
    lcc_engine::cmd::plugins::use_in_memory_manager();
    lcc_engine::cmd::plugins::manager().as_mut().unwrap().install_bytes(module, None, false).unwrap();
    let app = LccApp::new(lcc_engine::Session::with_demo(), Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [1200.0, 800.0], 1.0);
    h.settle(T);
    h
}

fn has(h: &Headless, id: &str) -> bool {
    h.app.widgets.iter().any(|(w, _)| w == id)
}

/// Runs the plug-in's job from its menu item: its activity id.
fn start(h: &mut Headless, plugin: &str) -> u64 {
    let r = h.request("engine.execute", json!({"command": "plugins.runItem", "params": {"plugin": plugin, "command": "go"}}), T);
    assert_eq!(r["ok"], true, "{r}");
    r["result"]["job"].as_u64().unwrap_or_else(|| panic!("the menu item returns at once with the job's id: {r}"))
}

/// Steps frames until `f` holds (at most 20 s).
fn until(h: &mut Headless, what: &str, mut f: impl FnMut(&mut Headless) -> bool) {
    let t = Instant::now();
    while !f(h) {
        assert!(t.elapsed() < Duration::from_secs(20), "{what}: not within 20 s");
        std::thread::sleep(Duration::from_millis(20));
        h.step();
    }
}

fn row(h: &mut Headless, id: u64) -> Option<Value> {
    let inspect = h.request("ui.inspect", json!({}), T);
    inspect["result"]["activity"].as_array()?.iter().find(|t| t["id"] == id).cloned()
}

#[test]
fn a_job_shows_its_progress_and_its_cross_stops_it() {
    // reports progress, then asks to go on: it runs until it is stopped
    let mut h = demo_with(&job_module(
        "test.uijob",
        &[r#"{"method":"job.progress","params":{"done":1,"total":3}}"#, r#"{"method":"job.detail","params":{"text":"a.jpg"}}"#],
        r#"{"ok":{"continue":1}}"#,
    ));
    let id = start(&mut h, "test.uijob");
    until(&mut h, "the row with its progress", |h| row(h, id).is_some_and(|r| r["detail"] == "a.jpg"));
    let r = row(&mut h, id).unwrap();
    assert_eq!(r["label"], "Go Far (Job Test)", "{r}");
    assert_eq!((r["done"].as_u64(), r["total"].as_u64()), (Some(1), Some(3)), "{r}");
    // the window keeps drawing frames while the job runs, and shows the row with its ✕
    let cross = format!("activity:cancel:{id}");
    until(&mut h, "the row's ✕", |h| has(h, &cross));
    assert_eq!(h.request("ui.clickWidget", json!({"id": cross}), T)["ok"], true);
    until(&mut h, "the row gone", |h| !h.app.session.activity.list().iter().any(|t| t.id == id));
    let log = h.app.session.execute("plugin.log", &json!({"id": "test.uijob"})).unwrap();
    assert!(log.as_array().unwrap().iter().any(|l| l == "command cancelled"), "{log}");
    assert!(h.app.ui.toast().is_none_or(|t| !t.0.contains("failed")), "a stop isn't a failure: {:?}", h.app.ui.toast().map(|t| &t.0));
    lcc_engine::cmd::plugins::manager().as_mut().unwrap().uninstall("test.uijob").unwrap();
}

#[test]
fn a_job_ends_with_its_message_or_its_error() {
    let mut h = demo_with(&job_module("test.uidone", &[], r#"{"ok":{"message":"Sent 3 photos"}}"#));
    let id = start(&mut h, "test.uidone");
    until(&mut h, "the message", |h| h.app.ui.toasts.iter().any(|t| t.0 == "Sent 3 photos"));
    assert!(!h.app.session.activity.list().iter().any(|t| t.id == id), "its row is gone");
    lcc_engine::cmd::plugins::manager().as_mut().unwrap().uninstall("test.uidone").unwrap();

    let mut h = demo_with(&job_module("test.uifail", &[], r#"{"error":"the server said no"}"#));
    start(&mut h, "test.uifail");
    until(&mut h, "the error", |h| h.app.ui.toasts.iter().any(|t| t.0.contains("Go Far") && t.0.contains("the server said no")));
    lcc_engine::cmd::plugins::manager().as_mut().unwrap().uninstall("test.uifail").unwrap();
}

#[test]
fn quit_asks_while_a_job_runs_and_quit_anyway_stops_it() {
    let mut h = demo_with(&job_module("test.uiquit", &[r#"{"method":"job.progress","params":{"done":1,"total":0}}"#], r#"{"ok":{"continue":1}}"#));
    let id = start(&mut h, "test.uiquit");
    until(&mut h, "the job's row", |h| row(h, id).is_some());
    assert!(!crate::panels::notices::may_close(&mut h.app), "Quit asks while a job runs");
    match &h.app.quit_prompt {
        Some(crate::QuitPrompt::Tasks(text)) => assert!(format!("{text:?}").contains("Go Far"), "the prompt names the job: {text:?}"),
        other => panic!("Quit asks about the running job: {other:?}"),
    }
    h.step();
    let anyway = "button:quitAnyway";
    until(&mut h, "Quit Anyway", |h| has(h, anyway));
    assert_eq!(h.request("ui.clickWidget", json!({"id": anyway}), T)["ok"], true);
    until(&mut h, "the job stopped", |h| !h.app.session.activity.list().iter().any(|t| t.id == id));
    let log = h.app.session.execute("plugin.log", &json!({"id": "test.uiquit"})).unwrap();
    assert!(log.as_array().unwrap().iter().any(|l| l == "command cancelled"), "{log}");
    lcc_engine::cmd::plugins::manager().as_mut().unwrap().uninstall("test.uiquit").unwrap();
}
