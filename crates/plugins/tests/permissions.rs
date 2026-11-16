//! Permissions as the user controls them: commands can take them back but never add one, each
//! request is allowed, denied (with a date) or never asked, a call that needs one the user was
//! never asked about is noted for the host to ask, and folders holding the settings or the
//! plug-ins can't be granted at all.

use std::path::{Path, PathBuf};

use lcc_plugins::{Begun, Group, Manager, Permissions, Request, RequestState};
use serde_json::{Value, json};

/// A module whose command `go` sends the host request at 8192 and returns the reply as its own;
/// `job` (requires `jobs`) runs as a job.
fn module(manifest: &str, request: &str) -> Vec<u8> {
    let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    let src = format!(
        r#"(module
  (import "host" "call" (func $call (param i32 i32) (result i32)))
  (import "host" "response" (func $response (param i32)))
  (memory (export "memory") 2)
  (data (i32.const 16) "{m}")
  (data (i32.const 8192) "{r}")
  (func (export "lcc_abi_version") (result i32) (i32.const 1))
  (func (export "lcc_manifest") (result i64)
    (i64.or (i64.shl (i64.const {mlen}) (i64.const 32)) (i64.const 16)))
  (func (export "lcc_alloc") (param $n i32) (result i32) (local $old i32)
    (local.set $old (memory.grow (i32.shr_u (i32.add (local.get $n) (i32.const 65535)) (i32.const 16))))
    (if (result i32) (i32.eq (local.get $old) (i32.const -1))
      (then (i32.const 0)) (else (i32.mul (local.get $old) (i32.const 65536)))))
  (func (export "lcc_handle") (param $ptr i32) (param $len i32) (result i64) (local $n i32)
    (local.set $n (call $call (i32.const 8192) (i32.const {rlen})))
    (call $response (i32.const 16384))
    (i64.or (i64.shl (i64.extend_i32_u (local.get $n)) (i64.const 32)) (i64.const 16384)))
)"#,
        m = esc(manifest),
        r = esc(request),
        mlen = manifest.len(),
        rlen = request.len()
    );
    wat::parse_str(&src).unwrap_or_else(|e| panic!("bad test WAT: {e}\n{src}"))
}

const ID: &str = "test.perms";

/// A manifest asking for every group: `fs` roots as given (`(path, write)`).
fn manifest(roots: &[(&Path, bool)]) -> String {
    let fs: Vec<Value> = roots.iter().map(|(p, w)| json!({"path": p.display().to_string(), "write": w})).collect();
    json!({
        "id": ID, "name": "Perms", "version": "1.0.0", "requires": ["programs", "jobs"],
        "permissions": {
            "catalog": true, "metadataWrite": true, "libraryAdd": true, "pixels": true, "crop": true, "virtualCopy": true,
            "network": ["127.0.0.1", "api.example.com"], "fs": fs,
            "programs": [{"id": "conv", "name": "Converter", "macBundleIds": ["org.example.conv"], "windowsExe": "conv.exe", "linuxExe": "conv"}]
        },
        "commands": [{"id": "go", "label": "Go"}, {"id": "job", "label": "Job", "job": true}]
    })
    .to_string()
}

/// A fresh folder for one test.
fn folder(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lcc-plugins-perms-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn installed(m: &Manager) -> &lcc_plugins::Installed {
    m.get(ID).expect("installed")
}

/// Answers every host call and records what reached it.
#[derive(Default)]
struct Host {
    seen: Vec<String>,
}

impl lcc_plugins::HostApi for Host {
    fn catalog(&mut self, method: &str, _: &Value) -> Result<Value, String> {
        self.seen.push(method.into());
        Ok(json!({"photos": 1}))
    }
    fn set_metadata(&mut self, _: &Value) -> Result<Value, String> {
        self.seen.push("metadata".into());
        Ok(Value::Null)
    }
    fn library_add(&mut self, _: &Value) -> Result<Value, String> {
        self.seen.push("libraryAdd".into());
        Ok(json!({"id": "2"}))
    }
    fn photo_pixels(&mut self, _: &str, _: u32) -> Result<Value, String> {
        self.seen.push("pixels".into());
        Ok(json!({}))
    }
    fn set_crop(&mut self, _: &str, _: [f64; 4], _: Option<f64>) -> Result<Value, String> {
        self.seen.push("crop".into());
        Ok(Value::Null)
    }
    fn virtual_copy(&mut self, _: &str, _: Option<&str>) -> Result<Value, String> {
        self.seen.push("virtualCopy".into());
        Ok(json!({"id": "3"}))
    }
    fn find_program(&mut self, decl: &lcc_plugins::ProgramDecl) -> Vec<lcc_plugins::Found> {
        vec![lcc_plugins::Found {
            bundle_id: decl.mac_bundle_ids.first().cloned().filter(|_| cfg!(target_os = "macos")),
            path: "/opt/conv".into(),
            version: None,
        }]
    }
    fn start_program(&mut self, _: lcc_plugins::StartRequest) -> Result<(), String> {
        self.seen.push("program".into());
        Ok(())
    }
}

/// Runs `go` as a command the user started: its result, and what it needed that the user was
/// never asked about.
fn run_asking(m: &mut Manager, command: &str, host: &mut Host) -> (lcc_plugins::Result<Value>, Vec<Request>) {
    let Begun::Call(mut call) = m.begin_command(ID, command, &json!({}), &[], &Value::Null, host).unwrap() else { panic!("no call") };
    call.ask_user();
    let outcome = call.run(host);
    let needs = call.take_needs();
    (m.end(*call, Some(outcome)), needs)
}

/// The files in `dir` with their contents.
fn listing(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out: Vec<(String, Vec<u8>)> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .map(|p| (p.display().to_string(), std::fs::read(&p).unwrap_or_default()))
        .collect();
    out.sort();
    out
}

fn every_group(roots: &[(&Path, bool)]) -> Permissions {
    serde_json::from_value::<lcc_plugins::Manifest>(serde_json::from_str(&manifest(roots)).unwrap()).unwrap().permissions
}

#[test]
fn commands_can_take_permissions_back_but_never_add_any() {
    let root = folder("never-add");
    let roots = [(root.as_path(), true)];
    let bytes = module(&manifest(&roots), r#"{"method":"catalog.stats","params":{}}"#);
    let all = every_group(&roots);
    let mut m = Manager::in_memory();
    // a fresh install from a command can't grant anything, of any group
    for r in all.requests() {
        let one = Permissions::default().with(&r, &all);
        let e = m.install_bytes(&bytes, Some(one), false).unwrap_err().to_string();
        assert!(e.contains("only be allowed by the user"), "installing with {r} granted is refused: {e}");
        assert!(m.get(ID).is_none(), "a refused install installs nothing ({r})");
    }
    assert!(m.install_bytes(&bytes, None, false).unwrap().effective().is_empty(), "a fresh install has nothing granted");
    // nor can plugin.grant
    for r in all.requests() {
        let e = m.set_grant(ID, Permissions::default().with(&r, &all)).unwrap_err().to_string();
        assert!(e.contains(&format!("adds `{r}`")), "a grant adding {r} is refused: {e}");
    }
    assert!(installed(&m).effective().is_empty(), "a refused grant changes nothing");
    // a folder granted read-only can't be widened to write
    let read_only = Permissions { fs: vec![lcc_plugins::FsRoot { path: root.display().to_string(), write: false }], ..Default::default() };
    m.set_grant_by_user(ID, read_only.clone()).unwrap();
    assert!(m.set_grant(ID, Permissions::default().with(&Request::value(Group::Fs, root.display().to_string()), &all)).is_err(), "write is more");
    // what the user allowed may be repeated, and left out to revoke it, by grant and by update
    m.set_grant_by_user(ID, Permissions { catalog: true, ..Default::default() }).unwrap();
    let granted = installed(&m).effective();
    assert_eq!(m.set_grant(ID, granted.clone()).unwrap(), granted, "repeating keeps it");
    assert!(m.install_bytes(&bytes, Some(Permissions { catalog: true, pixels: true, ..Default::default() }), false).is_err(), "an update can't add");
    assert_eq!(m.install_bytes(&bytes, None, false).unwrap().effective(), granted, "an update keeps what was allowed");
    assert!(m.set_grant(ID, Permissions::default()).unwrap().is_empty(), "a command revokes");
    assert!(m.set_grant(ID, granted).is_err(), "and can't give it back");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn revoke_takes_back_one_permission_or_one_value() {
    let (a, b) = (folder("revoke-a"), folder("revoke-b"));
    let roots = [(a.as_path(), false), (b.as_path(), true)];
    let mut m = Manager::in_memory();
    m.install_bytes(&module(&manifest(&roots), "{}"), None, false).unwrap();
    m.set_grant_by_user(ID, every_group(&roots)).unwrap();
    let g = m.revoke(ID, &Request::group(Group::Catalog)).unwrap();
    assert!(!g.catalog && g.metadata_write, "only catalog goes");
    let g = m.revoke(ID, &Request::value(Group::Fs, a.display().to_string())).unwrap();
    assert_eq!(g.fs.len(), 1, "one folder goes");
    assert_eq!(g.fs[0].path, b.display().to_string(), "the other stays");
    let g = m.revoke(ID, &Request::value(Group::Network, "api.example.com")).unwrap();
    assert_eq!(g.network, vec!["127.0.0.1".to_string()]);
    let g = m.revoke(ID, &Request::group(Group::Network)).unwrap();
    assert!(g.network.is_empty(), "no value: the whole group goes");
    assert_eq!(installed(&m).state(&Request::group(Group::Catalog)), RequestState::Never, "revoked counts as never asked");
    let _ = (std::fs::remove_dir_all(&a), std::fs::remove_dir_all(&b));
}

#[test]
fn requests_are_stored_as_allowed_denied_or_never_asked() {
    let dir = folder("states");
    let bytes = module(&manifest(&[]), "{}");
    // a plugins.json from before this change: its grant loads as allowed, the rest never asked
    std::fs::write(dir.join(format!("{ID}.wasm")), &bytes).unwrap();
    std::fs::write(dir.join("plugins.json"), json!({"plugins": {ID: {"enabled": true, "grant": {"catalog": true}}}}).to_string()).unwrap();
    let mut m = Manager::open(&dir).unwrap();
    assert!(installed(&m).effective().catalog, "the old grant carries over");
    assert_eq!(installed(&m).state(&Request::group(Group::Catalog)), RequestState::Allowed);
    assert_eq!(installed(&m).state(&Request::group(Group::Pixels)), RequestState::Never);
    m.deny_by_user(ID, &Request::group(Group::Pixels), "2026-11-08").unwrap();
    m.allow_by_user(ID, &Request::value(Group::Network, "127.0.0.1")).unwrap();
    drop(m);
    let m = Manager::open(&dir).unwrap();
    let p = installed(&m);
    assert_eq!(p.state(&Request::group(Group::Pixels)), RequestState::Denied("2026-11-08".into()), "a denial and its date are kept");
    assert_eq!(p.state(&Request::value(Group::Network, "127.0.0.1")), RequestState::Allowed);
    assert_eq!(p.state(&Request::value(Group::Network, "api.example.com")), RequestState::Never);
    // plugin.list reports each request's state
    let requests = p.info()["requests"].clone();
    let find = |group: &str, value: Option<&str>| {
        requests.as_array().unwrap().iter().find(|r| r["group"] == group && r["value"].as_str() == value).cloned().unwrap_or(Value::Null)
    };
    assert_eq!(find("catalog", None)["state"], "allowed");
    assert_eq!(find("pixels", None)["state"], "denied");
    assert_eq!(find("pixels", None)["deniedOn"], "2026-11-08");
    assert_eq!(find("crop", None)["state"], "never");
    assert_eq!(find("programs", Some("conv"))["state"], "never");
    // allowing a denied request forgets the denial
    let mut m = m;
    m.allow_by_user(ID, &Request::group(Group::Pixels)).unwrap();
    assert_eq!(installed(&m).state(&Request::group(Group::Pixels)), RequestState::Allowed);
    assert!(m.allow_by_user(ID, &Request::value(Group::Network, "other.example.com")).is_err(), "only what the manifest asks for");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_call_the_user_was_never_asked_about_is_noted_and_a_denied_one_fails_quietly() {
    let mut m = Manager::in_memory();
    m.install_bytes(&module(&manifest(&[]), r#"{"method":"catalog.stats","params":{}}"#), None, false).unwrap();
    let mut host = Host::default();
    let (r, needs) = run_asking(&mut m, "go", &mut host);
    let e = r.unwrap_err().to_string();
    assert!(e.contains("permission `catalog` not granted"), "the usual error: {e}");
    assert_eq!(needs, vec![Request::group(Group::Catalog)], "noted for the host to ask");
    assert!(host.seen.is_empty(), "the host saw nothing");
    // allowed: the same command runs
    m.allow_by_user(ID, &Request::group(Group::Catalog)).unwrap();
    let (r, needs) = run_asking(&mut m, "go", &mut host);
    assert_eq!(r.unwrap(), json!({"photos": 1}));
    assert!(needs.is_empty());
    // denied: later calls fail with nothing to ask
    m.deny_by_user(ID, &Request::group(Group::Catalog), "2026-11-08").unwrap();
    let (r, needs) = run_asking(&mut m, "go", &mut host);
    assert!(r.unwrap_err().to_string().contains("permission `catalog` not granted"));
    assert!(needs.is_empty(), "a denied request isn't asked again");
}

#[test]
fn with_nobody_to_ask_the_error_says_where_to_allow_it() {
    let mut m = Manager::in_memory();
    m.install_bytes(&module(&manifest(&[]), r#"{"method":"catalog.stats","params":{}}"#), None, false).unwrap();
    let mut host = Host::default();
    // a headless run (or an agent's request): not set to ask
    let e = m.run_command(ID, "go", &json!({}), &[], &mut host).unwrap_err().to_string();
    assert!(e.contains("needs `catalog`") && e.contains("allow it in the Plug-in Manager"), "{e}");
    assert_eq!(installed(&m).state(&Request::group(Group::Catalog)), RequestState::Never, "nothing is recorded as asked");
    // a job never asks, even when its command was the user's
    let (r, needs) = run_asking(&mut m, "job", &mut host);
    assert!(r.unwrap_err().to_string().contains("allow it in the Plug-in Manager"));
    assert!(needs.is_empty(), "a job runs with nobody to ask");
}

#[test]
fn folders_holding_the_settings_or_plugins_can_never_be_granted() {
    let settings = folder("protected");
    let plugins = settings.join("plugins");
    let elsewhere = folder("protected-ok");
    let inside = plugins.join("data");
    std::fs::create_dir_all(&inside).unwrap();
    let roots = [(settings.as_path(), true), (plugins.as_path(), false), (inside.as_path(), true), (elsewhere.as_path(), true)];
    let mut m = Manager::open(&plugins).unwrap();
    m.set_protected(vec![settings.clone()]);
    m.install_bytes(&module(&manifest(&roots), "{}"), None, false).unwrap();
    for refused in [&settings, &plugins, &inside] {
        let r = Request::value(Group::Fs, refused.display().to_string());
        let e = m.allow_by_user(ID, &r).unwrap_err().to_string();
        assert!(e.contains("can't be granted"), "{}: {e}", refused.display());
        assert!(matches!(installed(&m).state(&r), RequestState::Refused(_)), "{} shows as refused", refused.display());
    }
    // not through a whole grant either, nor a plugins.json written before
    let g = m.set_grant_by_user(ID, every_group(&roots)).unwrap();
    assert_eq!(g.fs.iter().map(|f| f.path.clone()).collect::<Vec<_>>(), vec![elsewhere.display().to_string()], "only the other folder");
    drop(m);
    let mut m = Manager::open(&plugins).unwrap();
    m.set_protected(vec![settings.clone()]);
    assert_eq!(installed(&m).effective().fs.len(), 1, "a refused folder in plugins.json isn't granted");
    let requests = installed(&m).info()["requests"].clone();
    let reason = requests.as_array().unwrap().iter().find(|r| r["value"] == settings.display().to_string()).map(|r| r["reason"].clone());
    assert!(reason.and_then(|r| r.as_str().map(|s| s.contains("settings"))).unwrap_or(false), "the reason is given: {requests}");
    // and a call into it is refused without asking
    let target = settings.join("plugins.json");
    let req = json!({"method": "fs.writeText", "params": {"path": target.display().to_string(), "text": "{}"}}).to_string();
    m.install_bytes(&module(&manifest(&roots), &req), None, false).unwrap();
    let before = std::fs::read(plugins.join("plugins.json")).unwrap();
    let mut host = Host::default();
    let (r, needs) = run_asking(&mut m, "go", &mut host);
    assert!(r.is_err() && needs.is_empty(), "nothing to ask: {r:?} {needs:?}");
    assert_eq!(std::fs::read(plugins.join("plugins.json")).unwrap(), before, "nothing written");
    let _ = (std::fs::remove_dir_all(&settings), std::fs::remove_dir_all(&elsewhere));
}

#[test]
fn every_gated_call_fails_ungranted_and_works_once_allowed() {
    let root = folder("calls");
    let runs = folder("calls-runs");
    let roots = [(root.as_path(), true)];
    let file = |name: &str| root.join(name).display().to_string();
    std::fs::write(root.join("in.txt"), "x").unwrap();
    std::fs::write(root.join("gone.txt"), "x").unwrap();
    let fs = Request::value(Group::Fs, root.display().to_string());
    let calls: Vec<(Value, Request, &str)> = vec![
        (json!({"method": "catalog.query", "params": {}}), Request::group(Group::Catalog), "catalog.query"),
        (json!({"method": "photo.inspect", "params": {"id": "1"}}), Request::group(Group::Catalog), "photo.inspect"),
        (json!({"method": "catalog.stats", "params": {}}), Request::group(Group::Catalog), "catalog.stats"),
        (json!({"method": "keyword.list", "params": {}}), Request::group(Group::Catalog), "keyword.list"),
        (json!({"method": "metadata.setStandard", "params": {"ids": ["1"], "title": "t"}}), Request::group(Group::MetadataWrite), "metadata"),
        (json!({"method": "library.add", "params": {"path": "/x/a.tif", "from": "1"}}), Request::group(Group::LibraryAdd), "libraryAdd"),
        (json!({"method": "photo.pixels", "params": {"photo": "1"}}), Request::group(Group::Pixels), "pixels"),
        (json!({"method": "develop.setCrop", "params": {"photo": "1", "rect": [0.1, 0.1, 0.9, 0.9]}}), Request::group(Group::Crop), "crop"),
        (json!({"method": "photo.virtualCopy", "params": {"photo": "1"}}), Request::group(Group::VirtualCopy), "virtualCopy"),
        (json!({"method": "fs.readText", "params": {"path": file("in.txt")}}), fs.clone(), ""),
        (json!({"method": "fs.readBytes", "params": {"path": file("in.txt")}}), fs.clone(), ""),
        (json!({"method": "fs.exists", "params": {"path": file("in.txt")}}), fs.clone(), ""),
        (json!({"method": "fs.list", "params": {"path": root.display().to_string()}}), fs.clone(), ""),
        (json!({"method": "fs.writeText", "params": {"path": file("out.txt"), "text": "y"}}), fs.clone(), ""),
        (json!({"method": "fs.writeBytes", "params": {"path": file("out.bin"), "base64": "eQ=="}}), fs.clone(), ""),
        (json!({"method": "fs.remove", "params": {"path": file("gone.txt")}}), fs.clone(), ""),
        (json!({"method": "program.start", "params": {"program": "conv"}}), Request::value(Group::Programs, "conv"), "program"),
        (json!({"method": "http.request", "params": {"url": "http://127.0.0.1:9/"}}), Request::value(Group::Network, "127.0.0.1"), ""),
    ];
    for (req, need, reaches) in calls {
        let mut m = Manager::in_memory();
        m.set_runs_root(Some(runs.clone()));
        m.install_bytes(&module(&manifest(&roots), &req.to_string()), None, false).unwrap();
        let mut host = Host::default();
        let before = listing(&root);
        let (r, needs) = run_asking(&mut m, "go", &mut host);
        let e = r.unwrap_err().to_string();
        assert!(e.contains(&format!("permission `{need}` not granted")), "{req}: ungranted, it fails: {e}");
        assert_eq!(needs, vec![need.clone()], "{req}: what it needs is noted");
        assert!(host.seen.is_empty(), "{req}: checked before the host sees it");
        assert_eq!(listing(&root), before, "{req}: nothing written or removed");
        m.allow_by_user(ID, &need).unwrap();
        let (r, needs) = run_asking(&mut m, "go", &mut host);
        assert!(needs.is_empty(), "{req}: nothing more to ask");
        match r {
            Ok(_) => {}
            // nothing listens on port 9: the request was made, past the permission
            Err(e) => assert!(req["method"] == "http.request" && !e.to_string().contains("not granted"), "{req}: allowed, it works: {e}"),
        }
        if !reaches.is_empty() {
            assert_eq!(host.seen, vec![reaches.to_string()], "{req}: allowed, it reaches the host");
        }
    }
    assert!(root.join("out.txt").exists() && root.join("out.bin").exists() && !root.join("gone.txt").exists(), "the allowed writes happened");
    let _ = (std::fs::remove_dir_all(&root), std::fs::remove_dir_all(&runs));
}
