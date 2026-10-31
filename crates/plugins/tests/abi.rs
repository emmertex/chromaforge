//! The ABI against hand-written modules: hostile ones (infinite loops, memory bombs, forbidden
//! imports, bad manifests, lying pointers) must fail with an error, quickly; host calls are
//! checked against the grant.

use std::time::{Duration, Instant};

use lcc_plugins::{Error, Limits, Manager, NoHost, Permissions, Plugin};
use serde_json::{Value, json};

/// A module implementing the ABI. `handle` is the body of `lcc_handle(ptr, len) -> i64`;
/// `request` (a host request, JSON) is placed at offset 8192 for it to send.
fn module(manifest: &str, version: i32, handle: &str, extra: &str, request: &str) -> Vec<u8> {
    let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    let src = format!(
        r#"(module
  (import "host" "call" (func $call (param i32 i32) (result i32)))
  (import "host" "response" (func $response (param i32)))
  (import "host" "log" (func $log (param i32 i32)))
  {extra}
  (memory (export "memory") 2)
  (data (i32.const 16) "{m}")
  (data (i32.const 8192) "{r}")
  (func (export "lcc_abi_version") (result i32) (i32.const {version}))
  (func (export "lcc_manifest") (result i64)
    (i64.or (i64.shl (i64.const {mlen}) (i64.const 32)) (i64.const 16)))
  (func (export "lcc_alloc") (param $n i32) (result i32) (local $old i32)
    (local.set $old (memory.grow (i32.shr_u (i32.add (local.get $n) (i32.const 65535)) (i32.const 16))))
    (if (result i32) (i32.eq (local.get $old) (i32.const -1))
      (then (i32.const 0)) (else (i32.mul (local.get $old) (i32.const 65536)))))
  (func (export "lcc_handle") (param $ptr i32) (param $len i32) (result i64) (local $n i32)
    {handle})
)"#,
        m = esc(manifest),
        r = esc(request),
        mlen = manifest.len()
    );
    wat::parse_str(&src).unwrap_or_else(|e| panic!("bad test WAT: {e}\n{src}"))
}

const MANIFEST: &str = r#"{"id":"test.plugin","name":"Test","version":"1.0.0","permissions":{"catalog":true},"commands":[{"id":"go","label":"Go"}]}"#;

/// Sends the request at 8192, copies the reply to 16384 and returns it.
fn forward(request: &str) -> Vec<u8> {
    let handle = format!(
        "(local.set $n (call $call (i32.const 8192) (i32.const {len})))
         (call $response (i32.const 16384))
         (i64.or (i64.shl (i64.extend_i32_u (local.get $n)) (i64.const 32)) (i64.const 16384))",
        len = request.len()
    );
    module(MANIFEST, 1, &handle, "", request)
}

/// Returns a constant reply placed at 8192.
fn reply(json: &str) -> Vec<u8> {
    module(MANIFEST, 1, &format!("(i64.or (i64.shl (i64.const {}) (i64.const 32)) (i64.const 8192))", json.len()), "", json)
}

fn run(bytes: &[u8], limits: Limits) -> (lcc_plugins::Result<Value>, Duration) {
    let t = Instant::now();
    let mut m = Manager::in_memory();
    m.set_limits(limits.clone());
    let r = Plugin::load(bytes, limits).and_then(|_| {
        m.install_bytes(bytes, Some(all()), false)?;
        m.run_command("test.plugin", "go", &json!({}), &[], &mut NoHost)
    });
    (r, t.elapsed())
}

#[test]
fn a_plain_reply_round_trips() {
    let (r, _) = run(&reply(r#"{"ok":{"x":1}}"#), Limits::default());
    assert_eq!(r.unwrap(), json!({"x": 1}));
    let (r, _) = run(&reply(r#"{"error":"nope"}"#), Limits::default());
    assert_eq!(r.unwrap_err(), Error::Failed("nope".into()));
}

#[test]
fn host_calls_follow_the_grant() {
    struct Cat;
    impl lcc_plugins::HostApi for Cat {
        fn catalog(&mut self, _: &str, _: &Value) -> Result<Value, String> {
            Ok(json!({"photos": 3}))
        }
    }
    let bytes = forward(r#"{"method":"catalog.stats","params":{}}"#);
    let mut m = Manager::in_memory();
    m.install_bytes(&bytes, Some(all()), false).unwrap();
    // the plug-in returns the host's reply envelope as its own
    assert_eq!(m.run_command("test.plugin", "go", &json!({}), &[], &mut Cat).unwrap(), json!({"photos": 3}));
    m.set_grant("test.plugin", Permissions::default()).unwrap();
    let e = m.run_command("test.plugin", "go", &json!({}), &[], &mut Cat).unwrap_err();
    assert!(e.to_string().contains("permission `catalog`"), "{e}");
    // a grant beyond the request is cut back to it
    let g = m
        .set_grant(
            "test.plugin",
            Permissions { catalog: true, metadata_write: true, network: vec!["x.com".into()], fs: vec![], programs: vec![], ..Default::default() },
        )
        .unwrap();
    assert_eq!(g, Permissions { catalog: true, ..Default::default() });
}

#[test]
fn unrequested_capabilities_are_refused() {
    // absolute on this platform, so they are refused for being outside every granted folder
    let (secret, scratch) = if cfg!(windows) { (r"C:\Windows\win.ini", r"C:\Windows\Temp\x") } else { ("/etc/passwd", "/tmp/x") };
    let read = serde_json::json!({"method": "fs.readText", "params": {"path": secret}}).to_string();
    let write = serde_json::json!({"method": "fs.writeText", "params": {"path": scratch, "text": "y"}}).to_string();
    for (req, needle) in [
        (r#"{"method":"metadata.setStandard","params":{}}"#, "metadataWrite"),
        (r#"{"method":"library.add","params":{"path":"/x/a.tif","from":"1"}}"#, "libraryAdd"),
        (r#"{"method":"http.request","params":{"url":"https://example.com/"}}"#, "network"),
        (read.as_str(), "not inside a folder"),
        (write.as_str(), "not inside a folder"),
        (r#"{"method":"no.such","params":{}}"#, "unknown host method"),
        ("not json", "not JSON"),
    ] {
        let (r, _) = run(&forward(req), Limits::default());
        let e = r.unwrap_err().to_string();
        assert!(e.contains(needle), "{req}: {e}");
    }
}

#[test]
fn hostile_modules_fail_fast() {
    let limits = Limits { fuel: 50_000_000, wall_time_ms: 5_000, max_memory_bytes: 8 << 20, ..Limits::default() };
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("infinite loop", module(MANIFEST, 1, "(loop $l (br $l)) (i64.const 0)", "", "")),
        ("memory bomb", module(MANIFEST, 1, "(drop (memory.grow (i32.const 60000))) (drop (memory.grow (i32.const 60000))) (i64.const 0)", "", "")),
        ("reply out of bounds", module(MANIFEST, 1, "(i64.or (i64.shl (i64.const 100) (i64.const 32)) (i64.const 0x7ffffff0))", "", "")),
        ("reply not json", reply("garbage")),
        ("unreachable", module(MANIFEST, 1, "unreachable", "", "")),
        ("deep recursion", module(MANIFEST, 1, "(call $rec) (i64.const 0)", "(func $rec (call $rec))", "")),
        ("host.call with a lying pointer", module(MANIFEST, 1, "(drop (call $call (i32.const 0x7ffffff0) (i32.const 100))) (i64.const 0)", "", "")),
        ("host.call flood", module(MANIFEST, 1, "(loop $l (drop (call $call (i32.const 8192) (i32.const 2))) (br $l)) (i64.const 0)", "", "{}")),
    ];
    for (name, bytes) in cases {
        let (r, t) = run(&bytes, limits.clone());
        assert!(r.is_err(), "{name} must fail");
        assert!(t < Duration::from_secs(10), "{name} took {t:?}");
    }
}

#[test]
fn bad_modules_are_rejected_on_load() {
    let wasi =
        module(MANIFEST, 1, "(i64.const 0)", r#"(import "wasi_snapshot_preview1" "fd_write" (func (param i32 i32 i32 i32) (result i32)))"#, "");
    assert!(matches!(Plugin::load(&wasi, Limits::default()), Err(Error::Abi(_))));
    assert!(matches!(Plugin::load(&module(MANIFEST, 2, "(i64.const 0)", "", ""), Limits::default()), Err(Error::Abi(_))));
    assert!(matches!(Plugin::load(&module(r#"{"id":"bad id","name":"x"}"#, 1, "(i64.const 0)", "", ""), Limits::default()), Err(Error::Manifest(_))));
    assert!(matches!(Plugin::load(b"\0asm garbage", Limits::default()), Err(Error::Module(_))));
    assert!(matches!(Plugin::load(b"MZ", Limits::default()), Err(Error::Module(_))));
    let big = Limits { max_module_bytes: 10, ..Limits::default() };
    assert!(matches!(Plugin::load(&reply("{}"), big), Err(Error::Module(_))));
}

#[test]
fn dialog_arguments_are_checked() {
    let manifest = r#"{"id":"test.plugin","name":"Test","version":"1.0.0","commands":[{"id":"go","label":"Go","dialog":[{"key":"n","label":"N","type":"number","min":0,"max":1,"default":0.5}]}]}"#;
    let bytes = module(manifest, 1, "(i64.or (i64.shl (i64.const 2) (i64.const 32)) (i64.const 8192))", "", "{}");
    let mut m = Manager::in_memory();
    m.install_bytes(&bytes, Some(all()), false).unwrap();
    assert!(m.run_command("test.plugin", "go", &json!({"n": "x"}), &[], &mut NoHost).is_err());
    assert!(m.run_command("test.plugin", "nope", &json!({}), &[], &mut NoHost).is_err());
    assert!(m.run_command("test.plugin", "go", &json!({"n": 7}), &[], &mut NoHost).is_ok());
}

fn all() -> Permissions {
    Permissions { catalog: true, metadata_write: true, ..Default::default() }
}

#[test]
fn install_grants_only_what_was_approved_and_updates_gain_nothing() {
    let bytes = forward(r#"{"method":"catalog.stats","params":{}}"#);
    let mut m = Manager::in_memory();
    assert!(m.install_bytes(&bytes, None, false).unwrap().effective().is_empty(), "nothing approved, nothing granted");
    m.set_grant("test.plugin", all()).unwrap();
    // an update without a new approval keeps the old grant, never more
    let v2 = module(&MANIFEST.replace(r#""catalog":true"#, r#""catalog":true,"metadataWrite":true"#), 1, "(i64.const 0)", "", "");
    let g = m.install_bytes(&v2, None, false).unwrap().effective();
    assert_eq!(g, Permissions { catalog: true, ..Default::default() });
}

const PROGRAMS: &str = concat!(
    r#"{"id":"test.plugin","name":"Test","version":"1.0.0","requires":["programs"],"permissions":{"catalog":true,"programs":["#,
    r#"{"id":"conv","name":"Converter","macBundleIds":["org.example.conv","org.example.conv2"],"windowsExe":"conv.exe","linuxExe":"conv"},"#,
    r#"{"id":"other","name":"Other","linuxExe":"other"}]},"commands":[{"id":"go","label":"Go"}]}"#
);

/// [`forward`] for a plug-in with [`PROGRAMS`].
fn forward_programs(request: &Value) -> Vec<u8> {
    let request = request.to_string();
    let handle = format!(
        "(local.set $n (call $call (i32.const 8192) (i32.const {len})))
         (call $response (i32.const 16384))
         (i64.or (i64.shl (i64.extend_i32_u (local.get $n)) (i64.const 32)) (i64.const 16384))",
        len = request.len()
    );
    module(PROGRAMS, 1, &handle, "", &request)
}

/// The programs a plug-in asks for, as a grant.
fn programs_grant(ids: &[&str]) -> Permissions {
    let requested = lcc_plugins::Manifest::parse(PROGRAMS.as_bytes()).unwrap().permissions;
    Permissions { catalog: true, programs: requested.programs.into_iter().filter(|p| ids.contains(&p.id.as_str())).collect(), ..Default::default() }
}

/// The app's side of programs: installs at `/opt/<id>`, starts recorded (and kept, so their runs
/// stay active).
#[derive(Default)]
struct Programs {
    installed: bool,
    refuse_start: bool,
    started: Vec<lcc_plugins::StartRequest>,
    /// The folders of the starts refused.
    refused: Vec<std::path::PathBuf>,
}

impl lcc_plugins::HostApi for Programs {
    fn catalog(&mut self, _: &str, _: &Value) -> Result<Value, String> {
        Ok(json!({}))
    }

    fn find_program(&mut self, decl: &lcc_plugins::ProgramDecl) -> Vec<lcc_plugins::Found> {
        if !self.installed {
            return vec![];
        }
        let found = |bundle_id: Option<&String>| lcc_plugins::Found {
            bundle_id: bundle_id.cloned(),
            path: format!("/opt/{}", decl.id),
            version: Some("2.0".into()),
        };
        if cfg!(target_os = "macos") { decl.mac_bundle_ids.iter().rev().map(|b| found(Some(b))).collect() } else { vec![found(None)] }
    }

    fn start_program(&mut self, run: lcc_plugins::StartRequest) -> Result<(), String> {
        if self.refuse_start {
            self.refused.push(run.dir.path().to_path_buf());
            return Err("it could not be started".into());
        }
        self.started.push(run);
        Ok(())
    }
}

#[test]
fn programs_are_granted_only_by_the_user() {
    let bytes = forward_programs(&json!({"method": "program.list", "params": {}}));
    let mut m = Manager::in_memory();
    let e = m.install_bytes(&bytes, Some(programs_grant(&["conv"])), false).unwrap_err().to_string();
    assert!(e.contains("Plug-in Manager"), "an install that grants a program points to the Plug-in Manager: {e}");
    assert!(m.get("test.plugin").is_none(), "nothing is installed");
    m.install_bytes(&bytes, Some(all()), false).unwrap();
    let e = m.set_grant("test.plugin", programs_grant(&["conv"])).unwrap_err().to_string();
    assert!(e.contains("Plug-in Manager"), "{e}");
    assert_eq!(m.get("test.plugin").unwrap().effective(), Permissions { catalog: true, ..Default::default() }, "the other permissions are unchanged");
    // the Plug-in Manager's own paths can
    let g = m.set_grant_by_user("test.plugin", programs_grant(&["conv"])).unwrap();
    assert_eq!(g.programs.len(), 1);
    // a command may repeat what the user granted, and leave it out to revoke it, never add or widen
    let granted = m.get("test.plugin").unwrap().effective();
    assert_eq!(m.set_grant("test.plugin", granted.clone()).unwrap(), granted, "repeating the grant keeps the program");
    assert!(m.install_bytes(&bytes, Some(granted.clone()), false).is_ok(), "an update may repeat it too");
    assert_eq!(m.get("test.plugin").unwrap().effective(), granted);
    let mut wider = granted.clone();
    wider.programs[0].mac_bundle_ids.push("com.example.other".into());
    assert!(m.set_grant("test.plugin", wider.clone()).unwrap_err().to_string().contains("Plug-in Manager"), "a wider entry is refused");
    assert!(m.install_bytes(&bytes, Some(wider), false).is_err());
    assert!(m.set_grant("test.plugin", programs_grant(&["conv", "other"])).is_err(), "another program is refused");
    assert_eq!(m.get("test.plugin").unwrap().effective(), granted, "a refused grant changes nothing");
    assert!(m.set_grant("test.plugin", all()).unwrap().programs.is_empty(), "leaving it out revokes it");
    assert!(m.set_grant("test.plugin", granted).is_err(), "and once revoked, a command can't give it back");
    let mut m = Manager::in_memory();
    assert_eq!(m.install_bytes_by_user(&bytes, Some(programs_grant(&["conv", "other"])), false).unwrap().effective().programs.len(), 2);
    // a grant can't change what a declared program is
    let mut changed = programs_grant(&["conv"]);
    changed.programs[0].linux_exe = Some("sh".into());
    assert!(m.set_grant_by_user("test.plugin", changed).unwrap().programs.is_empty(), "an entry that differs from the manifest's is not granted");
}

#[test]
fn program_list_shows_every_declared_program_before_and_after_a_grant() {
    let bytes = forward_programs(&json!({"method": "program.list", "params": {}}));
    let mut m = Manager::in_memory();
    m.install_bytes(&bytes, None, false).unwrap();
    let mut host = Programs { installed: true, ..Default::default() };
    let before = m.run_command("test.plugin", "go", &json!({}), &[], &mut host).unwrap();
    let ids: Vec<&str> = before.as_array().unwrap().iter().map(|p| p["program"].as_str().unwrap()).collect();
    assert_eq!(ids, ["conv", "other"], "in manifest order");
    assert_eq!(before[0]["granted"], false);
    assert_eq!(before[0]["name"], "Converter");
    assert_eq!(before[0]["installed"][0]["version"], "2.0");
    assert!(before[0]["installed"][0].get("path").is_none(), "where it is installed is told only once it's granted");
    m.set_grant_by_user("test.plugin", programs_grant(&["conv"])).unwrap();
    let after = m.run_command("test.plugin", "go", &json!({}), &[], &mut host).unwrap();
    assert_eq!(after[0]["granted"], true);
    assert_eq!(after[1]["granted"], false);
    assert_eq!(after[0]["installed"][0]["path"], "/opt/conv");
    assert!(after[1]["installed"][0].get("path").is_none());
    let without_path = |l: &Value| -> Vec<Value> {
        l["installed"].as_array().unwrap().iter().map(|f| json!({"bundleId": f.get("bundleId"), "version": f["version"]})).collect()
    };
    assert_eq!(without_path(&after[0]), without_path(&before[0]), "the same installs before and after the grant");
    let none = m.run_command("test.plugin", "go", &json!({}), &[], &mut Programs::default()).unwrap();
    assert_eq!(none[0]["installed"], json!([]));
    assert!(host.started.is_empty());
}

/// Held by tests that start `conv` runs: runs are one per program per plug-in, process-wide.
static CONV_RUNS: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// `program.start` with `params` for a plug-in granted `conv` only.
fn start(m: &mut Manager, host: &mut Programs, params: Value) -> lcc_plugins::Result<Value> {
    let bytes = forward_programs(&json!({"method": "program.start", "params": params}));
    m.install_bytes_by_user(&bytes, Some(programs_grant(&["conv"])), false).unwrap();
    m.run_command("test.plugin", "go", &json!({}), &[], host)
}

#[test]
fn program_start_writes_the_files_then_starts_one_run_at_a_time() {
    let _one = CONV_RUNS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut m = Manager::in_memory();
    let mut host = Programs { installed: true, ..Default::default() };
    let params = json!({"program": "conv", "args": ["-i", "{tmpDir}/job.txt"], "files": {"job.txt": "a\nb\n"}});
    let r = start(&mut m, &mut host, params.clone()).unwrap();
    let run = host.started.pop().unwrap();
    assert_eq!(r["run"], run.run());
    assert_eq!(r["tmpDir"], json!(run.dir.path()));
    assert_eq!(run.found.path, "/opt/conv");
    if cfg!(target_os = "macos") {
        assert_eq!(run.found.bundle_id.as_deref(), Some("org.example.conv"), "the first installed bundle id in the entry's order");
    }
    let job = run.dir.path().join("job.txt");
    // `{tmpDir}` is replaced as written: on Windows that is `C:…un/job.txt`, a valid path
    assert_eq!(run.args, vec!["-i".to_string(), format!("{}/job.txt", run.dir.path().display())]);
    assert_eq!(std::fs::read_to_string(&job).unwrap(), "a\nb\n");

    // the run is active: a second start fails and starts nothing
    let e = start(&mut m, &mut host, params.clone()).unwrap_err().to_string();
    assert!(e.contains("already running"), "{e}");
    assert!(host.started.is_empty());

    // once it has ended, a new start works
    let lcc_plugins::StartRequest { dir, reservation, .. } = run;
    dir.remove();
    drop(reservation);
    start(&mut m, &mut host, params.clone()).unwrap();
    let again = host.started.pop().unwrap();
    again.dir.remove();
    drop(again.reservation);

    // a start the app refuses leaves no run and no folder behind
    host.refuse_start = true;
    let e = start(&mut m, &mut host, params.clone()).unwrap_err().to_string();
    assert!(e.contains("could not be started"), "{e}");
    assert!(host.refused.iter().all(|d| !d.exists()) && !host.refused.is_empty(), "the refused run's folder is gone");
    host.refuse_start = false;
    start(&mut m, &mut host, params).unwrap();
    let last = host.started.pop().unwrap();
    last.dir.remove();
}

#[test]
fn program_start_refuses_bad_requests_and_starts_nothing() {
    let mut m = Manager::in_memory();
    let other_os_bundle = if cfg!(target_os = "macos") { "org.example.unknown" } else { "org.example.conv" };
    let many: serde_json::Map<String, Value> = (0..65).map(|i| (format!("f{i}.txt"), json!(""))).collect();
    for (params, needle) in [
        (json!({"program": "nope"}), "not one of this plug-in's programs"),
        (json!({"program": "other"}), "permission `programs`"),
        (json!({"program": "conv", "files": {"../escape.txt": "x"}}), "plain file name"),
        (json!({"program": "conv", "files": {"a/b.txt": "x"}}), "plain file name"),
        (json!({"program": "conv", "files": {"..": "x"}}), "plain file name"),
        (json!({"program": "conv", "files": {"CON.txt": "x"}}), "plain file name"),
        (json!({"program": "conv", "files": {"job?.txt": "x"}}), "plain file name"),
        (json!({"program": "conv", "files": many}), "at most 64 files"),
        (json!({"program": "conv", "files": {"n.txt": 3}}), "must be text"),
        (json!({"program": "conv", "files": "job.txt"}), "must be an object"),
        (json!({"program": "conv", "args": "-i"}), "list of strings"),
        (json!({"program": "conv", "args": ["a\u{0}b"]}), "NUL"),
        (json!({"program": "conv", "bundleId": other_os_bundle}), "bundleId"),
        (json!({"program": "conv", "bundleId": 7}), "must be a string"),
    ] {
        let mut host = Programs { installed: true, ..Default::default() };
        let e = start(&mut m, &mut host, params.clone()).unwrap_err().to_string();
        assert!(e.contains(needle), "{params}: {e}");
        assert!(host.started.is_empty(), "{params} started something");
        let logged = m.get("test.plugin").unwrap().log.iter().any(|l| l.contains("program.start failed") && l.contains(needle));
        assert!(logged, "{params}: the error is in the log");
    }
    // not installed on this system
    let mut host = Programs::default();
    let e = start(&mut m, &mut host, json!({"program": "conv"})).unwrap_err().to_string();
    assert!(e.contains("Converter is not installed"), "{e}");
    // revoked
    m.set_grant_by_user("test.plugin", Permissions::default()).unwrap();
    let bytes = forward_programs(&json!({"method": "program.start", "params": {"program": "conv"}}));
    m.install_bytes(&bytes, None, false).unwrap();
    let mut host = Programs { installed: true, ..Default::default() };
    let e = m.run_command("test.plugin", "go", &json!({}), &[], &mut host).unwrap_err().to_string();
    assert!(e.contains("permission `programs`"), "{e}");
    assert!(host.started.is_empty());
}

#[test]
fn program_end_reads_the_run_folder_and_a_failing_hook_is_logged() {
    let dir = std::env::temp_dir().join(format!("lcc-abi-program-end-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let made = dir.join("result.txt");
    std::fs::write(&made, "done").unwrap();
    let read = forward_programs(&json!({"method": "fs.readText", "params": {"path": made}}));
    let mut m = Manager::in_memory();
    m.install_bytes_by_user(&read, Some(programs_grant(&["conv"])), false).unwrap();
    let mut call = m.program_end_call("test.plugin", "run-1", "conv", lcc_plugins::EndStatus::Exited, Some(0), &dir).unwrap();
    let outcome = call.run(&mut NoHost);
    assert_eq!(m.end(call, Some(outcome)).unwrap(), json!("done"), "the run's folder is readable in program.end");
    // outside program.end the folder is not lent
    let e = m.run_command("test.plugin", "go", &json!({}), &[], &mut NoHost).unwrap_err().to_string();
    assert!(e.contains("not inside a folder"), "{e}");
    // a hook that traps: an error in the log, and the plug-in takes calls again
    let trap = module(PROGRAMS, 1, "unreachable", "", "");
    m.install_bytes(&trap, None, false).unwrap();
    let mut call = m.program_end_call("test.plugin", "run-2", "conv", lcc_plugins::EndStatus::Cancelled, None, &dir).unwrap();
    let outcome = call.run(&mut NoHost);
    assert!(m.end(call, Some(outcome)).is_err());
    assert!(m.get("test.plugin").unwrap().log.iter().any(|l| l.contains("program.end failed")));
    assert!(m.program_end_call("test.plugin", "run-3", "conv", lcc_plugins::EndStatus::Exited, None, &dir).is_ok(), "not left busy");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_program_end_hook_out_of_budget_is_stopped_and_logged() {
    let dir = std::env::temp_dir().join(format!("lcc-abi-program-end-budget-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut m = Manager::in_memory();
    m.set_limits(Limits { fuel: 5_000_000, wall_time_ms: 2_000, ..Limits::default() });
    let spin = module(PROGRAMS, 1, "(loop $l (br $l)) (i64.const 0)", "", "");
    m.install_bytes_by_user(&spin, Some(programs_grant(&["conv"])), false).unwrap();
    let t = Instant::now();
    let mut call = m.program_end_call("test.plugin", "run-1", "conv", lcc_plugins::EndStatus::Exited, Some(0), &dir).unwrap();
    let outcome = call.run(&mut NoHost);
    assert!(m.end(call, Some(outcome)).is_err(), "a hook that never returns is stopped");
    assert!(t.elapsed() < Duration::from_secs(10), "within its budget: {:?}", t.elapsed());
    assert!(m.get("test.plugin").unwrap().log.iter().any(|l| l.contains("program.end failed")), "and the log says so");
    assert!(m.program_end_call("test.plugin", "run-2", "conv", lcc_plugins::EndStatus::Exited, None, &dir).is_ok(), "not left busy");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_run_folder_that_cant_be_made_starts_nothing() {
    let _one = CONV_RUNS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let base = std::env::temp_dir().join(format!("lcc-abi-runs-root-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    std::fs::write(base.join("a-file"), "x").unwrap();
    let mut m = Manager::in_memory();
    m.set_runs_root(Some(base.join("a-file")));
    let mut host = Programs { installed: true, ..Default::default() };
    let e = start(&mut m, &mut host, json!({"program": "conv"})).unwrap_err().to_string();
    assert!(e.contains("run folder"), "{e}");
    assert!(host.started.is_empty(), "nothing started");
    assert!(m.get("test.plugin").unwrap().log.iter().any(|l| l.contains("program.start failed")));
    // the failed start held nothing: with a usable folder it starts
    m.set_runs_root(Some(base.join("runs")));
    start(&mut m, &mut host, json!({"program": "conv"})).unwrap();
    let run = host.started.pop().unwrap();
    assert!(run.dir.path().starts_with(std::fs::canonicalize(base.join("runs")).unwrap()));
    run.dir.remove();
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn a_plugin_failing_in_a_loop_cant_flood_its_log() {
    let req = json!({"method": "program.start", "params": {"program": "nope"}}).to_string();
    let handle = format!(
        "(local.set $n (i32.const 200))
         (loop $l (drop (call $call (i32.const 8192) (i32.const {len})))
           (local.set $n (i32.sub (local.get $n) (i32.const 1))) (br_if $l (local.get $n)))
         (i64.const 0)",
        len = req.len()
    );
    let mut m = Manager::in_memory();
    m.install_bytes(&module(PROGRAMS, 1, &handle, "", &req), None, false).unwrap();
    let _ = m.run_command("test.plugin", "go", &json!({}), &[], &mut NoHost);
    let log = &m.get("test.plugin").unwrap().log;
    let failures = log.iter().filter(|l| l.contains("program.start failed")).count();
    assert!((1..50).contains(&failures), "a few failures are logged, not 200: {failures}");
    assert!(log.iter().any(|l| l.contains("left out")), "and the log says the rest were left out");
}

#[test]
fn an_update_from_a_command_keeps_programs_only_for_the_same_code() {
    let version = |handle: &str| module(PROGRAMS, 1, handle, "", "");
    let granted = |m: &Manager| m.get("test.plugin").unwrap().effective().programs.len();
    let mut m = Manager::in_memory();
    m.install_bytes_by_user(&version("(i64.const 0)"), Some(programs_grant(&["conv", "other"])), false).unwrap();
    // the same module again (a repair) keeps them, with or without a grant
    m.install_bytes(&version("(i64.const 0)"), None, false).unwrap();
    assert_eq!(granted(&m), 2, "the same code keeps its programs");
    let kept = m.get("test.plugin").unwrap().effective();
    m.install_bytes(&version("(i64.const 0)"), Some(kept.clone()), false).unwrap();
    assert_eq!(granted(&m), 2);
    // other code, the same manifest: none kept, and the log says why
    m.install_bytes(&version("(nop) (i64.const 0)"), None, false).unwrap();
    assert_eq!(granted(&m), 0, "different code keeps no programs");
    let log = &m.get("test.plugin").unwrap().log;
    assert_eq!(log.iter().filter(|l| l.contains("code is different")).count(), 2, "one line per program: {log:?}");
    assert_eq!(m.get("test.plugin").unwrap().effective().catalog, kept.catalog, "the other permissions stay");
    // and a command can't name them again for other code
    let mut m = Manager::in_memory();
    m.install_bytes_by_user(&version("(i64.const 0)"), Some(programs_grant(&["conv"])), false).unwrap();
    let kept = m.get("test.plugin").unwrap().effective();
    let e = m.install_bytes(&version("(nop) (i64.const 0)"), Some(kept), false).unwrap_err().to_string();
    assert!(e.contains("Plug-in Manager"), "{e}");
    assert_eq!(granted(&m), 1, "a refused install changes nothing");
    // the user may install other code and keep them
    m.install_bytes_by_user(&version("(nop) (i64.const 0)"), None, false).unwrap();
    assert_eq!(granted(&m), 1);
}

#[test]
fn an_update_keeps_a_narrowed_program_grant_and_unticks_a_widened_one() {
    let version = |manifest: &str| module(manifest, 1, "(i64.const 0)", "", "");
    let granted = |m: &Manager| m.get("test.plugin").unwrap().effective().programs.iter().map(|p| p.id.clone()).collect::<Vec<_>>();
    let widened_logged = |m: &Manager| m.get("test.plugin").unwrap().log.iter().filter(|l| l.contains("more places than you allowed")).count();
    let mut m = Manager::in_memory();
    m.install_bytes_by_user(&version(PROGRAMS), Some(programs_grant(&["conv", "other"])), false).unwrap();
    assert_eq!(granted(&m), ["conv", "other"]);

    // renamed, bundle ids reordered and one removed, the Windows executable dropped: still granted
    let narrowed = PROGRAMS
        .replace(r#""name":"Converter""#, r#""name":"Converter 2""#)
        .replace(r#"["org.example.conv","org.example.conv2"]"#, r#"["org.example.conv2"]"#)
        .replace(r#""windowsExe":"conv.exe","#, "");
    m.install_bytes_by_user(&version(&narrowed), None, false).unwrap();
    assert_eq!(granted(&m), ["conv", "other"]);
    assert_eq!(m.get("test.plugin").unwrap().effective().programs[0].name, "Converter 2", "the grant follows the new entry");
    assert_eq!(widened_logged(&m), 0);

    // the removed bundle id comes back: that widens the narrowed grant, so it's unticked and logged
    m.install_bytes_by_user(&version(PROGRAMS), None, false).unwrap();
    assert_eq!(granted(&m), ["other"]);
    assert_eq!(widened_logged(&m), 1);

    // another executable widens too
    let mut m = Manager::in_memory();
    m.install_bytes_by_user(&version(PROGRAMS), Some(programs_grant(&["conv", "other"])), false).unwrap();
    m.install_bytes_by_user(&version(&PROGRAMS.replace(r#""linuxExe":"other""#, r#""linuxExe":"other2""#)), None, false).unwrap();
    assert_eq!(granted(&m), ["conv"]);
    assert_eq!(widened_logged(&m), 1);
    // and a program the old grant never had is not granted by an update
    m.install_bytes_by_user(&version(&PROGRAMS.replace(r#""linuxExe":"other2""#, r#""linuxExe":"other""#)), None, false).unwrap();
    assert_eq!(granted(&m), ["conv"]);
}

/// Sends its whole request to the host as `catalog.query`'s params (`{"method":"catalog.query",
/// "params":<request>}`, built in front of and behind the request) and answers `{"message":"seen"}`:
/// the host sees what each call was given.
fn request_echo(manifest: &str) -> Vec<u8> {
    let prefix = r#"{"method":"catalog.query","params":"#;
    let reply = r#"{"ok":{"message":"seen"}}"#;
    let handle = format!(
        r#"(local.set $n (i32.const 0))
    (block $done (loop $copy
      (br_if $done (i32.ge_u (local.get $n) (i32.const {pl})))
      (i32.store8 (i32.add (i32.sub (local.get $ptr) (i32.const {pl})) (local.get $n)) (i32.load8_u (i32.add (i32.const 4096) (local.get $n))))
      (local.set $n (i32.add (local.get $n) (i32.const 1)))
      (br $copy)))
    (i32.store8 (i32.add (local.get $ptr) (local.get $len)) (i32.const 125))
    (drop (call $call (i32.sub (local.get $ptr) (i32.const {pl})) (i32.add (local.get $len) (i32.const {pl1}))))
    (i64.or (i64.shl (i64.const {rl}) (i64.const 32)) (i64.const 8192))"#,
        pl = prefix.len(),
        pl1 = prefix.len() + 1,
        rl = reply.len()
    );
    let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    module(manifest, 1, &handle, &format!(r#"(data (i32.const 4096) "{}")"#, esc(prefix)), reply)
}

/// Records the params of every catalog read.
#[derive(Default)]
struct Seen(Vec<Value>);

impl lcc_plugins::HostApi for Seen {
    fn catalog(&mut self, _: &str, params: &Value) -> Result<Value, String> {
        self.0.push(params.clone());
        Ok(json!([]))
    }
}

/// A command and its prepare step get the Library source the command was started from; without
/// one (an older caller), the request has no `source`.
#[test]
fn commands_and_prepare_steps_get_their_source() {
    let manifest = r#"{"id":"test.plugin","name":"Test","version":"1.0.0","requires":["libraryAdd"],"permissions":{"catalog":true},"commands":[{"id":"go","label":"Go"},{"id":"ask","label":"Ask","prepare":true}]}"#;
    let mut m = Manager::in_memory();
    m.install_bytes(&request_echo(manifest), Some(all()), false).unwrap();
    let album = json!({"kind": "album", "id": "7", "name": "Trip"});
    let mut host = Seen::default();
    m.run_command_from("test.plugin", "go", &json!({}), &["1".into()], &album, &mut host).unwrap();
    assert_eq!(host.0.len(), 1);
    assert_eq!((host.0[0]["hook"].as_str(), &host.0[0]["source"]), (Some("command"), &album), "the command gets its source");
    m.prepare_command_from("test.plugin", "ask", &["1".into()], &album, &mut host).unwrap();
    assert_eq!((host.0[1]["hook"].as_str(), &host.0[1]["source"]), (Some("command.prepare"), &album), "the prepare step gets it too");
    m.run_command("test.plugin", "go", &json!({}), &[], &mut host).unwrap();
    assert!(host.0[2].get("source").is_none(), "no source given, none sent: {}", host.0[2]);
}
