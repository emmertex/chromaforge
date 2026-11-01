//! Guest-side SDK for the app's WebAssembly plug-ins (ABI v1, see `docs/plugins.md`).
//!
//! A plug-in is a `cdylib` built for `wasm32-unknown-unknown`:
//!
//! ```ignore
//! use lcc_plugin_sdk::{export_plugin, host, Request};
//! use serde_json::{json, Value};
//!
//! const MANIFEST: &str = r#"{"id": "org.example.hello", "name": "Hello",
//!   "permissions": {"catalog": true},
//!   "commands": [{"id": "count", "label": "Count Photos"}]}"#;
//!
//! fn handle(req: &Request) -> Result<Value, String> {
//!     match req.hook() {
//!         "command" => Ok(json!({"message": format!("{}", host::call("catalog.stats", json!({}))?)})),
//!         other => Err(format!("unsupported hook {other}")),
//!     }
//! }
//! export_plugin!(MANIFEST, handle);
//! ```
//!
//! The SDK itself has no `unsafe` blocks: buffers the host fills are ordinary `Vec`s kept in a
//! table by address; the host imports are declared `safe`. The only `unsafe` token is the
//! `#[unsafe(no_mangle)]` the [`export_plugin!`] macro puts on the four ABI exports.

use std::cell::RefCell;
use std::collections::BTreeMap;

pub use serde_json;
use serde_json::{Value, json};

/// The ABI version this SDK implements.
pub const ABI_VERSION: i32 = 1;

#[cfg(target_arch = "wasm32")]
#[link(wasm_import_module = "host")]
unsafe extern "C" {
    #[link_name = "call"]
    safe fn host_call(ptr: i32, len: i32) -> i32;
    #[link_name = "response"]
    safe fn host_response(ptr: i32);
    #[link_name = "log"]
    safe fn host_log(ptr: i32, len: i32);
}

// Off wasm (unit tests of a plug-in's logic): no host.
#[cfg(not(target_arch = "wasm32"))]
fn host_call(_: i32, _: i32) -> i32 {
    -1
}
#[cfg(not(target_arch = "wasm32"))]
fn host_response(_: i32) {}
#[cfg(not(target_arch = "wasm32"))]
fn host_log(_: i32, _: i32) {}

thread_local! {
    /// Buffers handed to the host by `lcc_alloc`, by address.
    static BUFFERS: RefCell<BTreeMap<usize, Vec<u8>>> = const { RefCell::new(BTreeMap::new()) };
    /// The last reply, kept alive until the next call.
    static REPLY: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    static MANIFEST: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

fn addr(v: &[u8]) -> i32 {
    v.as_ptr() as usize as i32
}

fn pack(v: &[u8]) -> i64 {
    ((v.len() as i64) << 32) | (addr(v) as u32 as i64)
}

#[doc(hidden)]
pub fn __alloc(len: i32) -> i32 {
    let v = vec![0u8; len.max(1) as usize];
    let p = addr(&v);
    BUFFERS.with(|b| b.borrow_mut().insert(p as u32 as usize, v));
    p
}

#[doc(hidden)]
pub fn __manifest(text: &str) -> i64 {
    MANIFEST.with(|m| {
        let mut m = m.borrow_mut();
        *m = text.as_bytes().to_vec();
        pack(&m)
    })
}

#[doc(hidden)]
pub fn __handle(ptr: i32, len: i32, handler: fn(&Request) -> Result<Value, String>) -> i64 {
    let bytes = BUFFERS.with(|b| b.borrow_mut().remove(&(ptr as u32 as usize))).unwrap_or_default();
    let bytes = bytes.get(..len.max(0) as usize).unwrap_or(&bytes);
    let reply = match serde_json::from_slice::<Value>(bytes) {
        Ok(v) => match handler(&Request(v)) {
            Ok(v) => json!({"ok": v}),
            Err(e) => json!({"error": e}),
        },
        Err(e) => json!({"error": format!("bad request: {e}")}),
    };
    REPLY.with(|r| {
        let mut r = r.borrow_mut();
        *r = serde_json::to_vec(&reply).unwrap_or_else(|_| br#"{"error":"reply not serializable"}"#.to_vec());
        pack(&r)
    })
}

/// A request from the host.
#[derive(Clone, Debug)]
pub struct Request(pub Value);

impl Request {
    /// `command`, `command.prepare`, `settings.prepare`, `export`, `export.destination`, `metadata`,
    /// `publish.upload`, `publish.delete`, `program.end`.
    pub fn hook(&self) -> &str {
        self.0.get("hook").and_then(Value::as_str).unwrap_or("")
    }
    /// The command id (`hook == "command"` or `"command.prepare"`).
    pub fn command(&self) -> &str {
        self.0.get("command").and_then(Value::as_str).unwrap_or("")
    }
    /// A dialog value or argument.
    pub fn arg(&self, key: &str) -> &Value {
        self.0.get("args").and_then(|a| a.get(key)).unwrap_or(&Value::Null)
    }
    /// Selected photo ids (`hook == "command"` or `"command.prepare"`).
    pub fn selection(&self) -> Vec<String> {
        self.0
            .get("selection")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
            .unwrap_or_default()
    }
    /// The Library source the command was started from (`hook == "command"` or
    /// `"command.prepare"`): `{kind: "album" | "smartAlbum", id, name, quick?}`, `{kind: "folder",
    /// path}`, `{kind: "allPhotos"}` or `{kind: "other", name}`; null from an older app.
    pub fn source(&self) -> &Value {
        self.get("source")
    }
    /// The album the command was started from, when it can hold photos (`source` is an `album`):
    /// its id, for `host::library_add`.
    pub fn source_album(&self) -> Option<&str> {
        let s = self.source();
        (s.get("kind").and_then(Value::as_str) == Some("album")).then(|| s.get("id").and_then(Value::as_str)).flatten()
    }
    pub fn get(&self, key: &str) -> &Value {
        self.0.get(key).unwrap_or(&Value::Null)
    }
    /// The run a `program.start` returned (`hook == "program.end"`).
    pub fn run(&self) -> &str {
        self.0.get("run").and_then(Value::as_str).unwrap_or("")
    }
    /// The program's id in the manifest (`hook == "program.end"`).
    pub fn program(&self) -> &str {
        self.0.get("program").and_then(Value::as_str).unwrap_or("")
    }
    /// How the run ended: `exited`, `cancelled` (the user stopped waiting; the program may still
    /// run) or `failed` (`hook == "program.end"`).
    pub fn status(&self) -> &str {
        self.0.get("status").and_then(Value::as_str).unwrap_or("")
    }
    /// The program's exit code, when the app knows it (`hook == "program.end"`, status `exited`):
    /// `None` on macOS, where the app starts programs through `open`, and for a program ended by a
    /// signal.
    pub fn code(&self) -> Option<i64> {
        self.0.get("code").and_then(Value::as_i64)
    }
    /// The run's folder, lent for reading (`hook == "program.end"`).
    pub fn tmp_dir(&self) -> &str {
        self.0.get("tmpDir").and_then(Value::as_str).unwrap_or("")
    }
    /// The export destination id (`hook == "export.destination"`).
    pub fn destination(&self) -> &str {
        self.0.get("destination").and_then(Value::as_str).unwrap_or("")
    }
    /// Exported files `(path, photo id)` (`hook == "export"` or `"export.destination"`).
    pub fn files(&self) -> Vec<(String, String)> {
        self.0
            .get("files")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|f| {
                        let path = f.get("path")?.as_str()?.to_string();
                        let photo = match f.get("photo") {
                            Some(Value::String(s)) => s.clone(),
                            Some(Value::Number(n)) => n.to_string(),
                            _ => String::new(),
                        };
                        Some((path, photo))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// Standard base64 (with padding), as `fs.readBytes` / `fs.writeBytes` / `bodyBase64` use.
pub mod base64 {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    pub fn encode(bytes: &[u8]) -> String {
        let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
        for chunk in bytes.chunks(3) {
            let b = [chunk[0], chunk.get(1).copied().unwrap_or(0), chunk.get(2).copied().unwrap_or(0)];
            let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
            for i in 0..4 {
                out.push(if i <= chunk.len() { char::from(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize]) } else { '=' });
            }
        }
        out
    }

    pub fn decode(text: &str) -> Result<Vec<u8>, String> {
        let s = text.as_bytes();
        if !s.len().is_multiple_of(4) {
            return Err("base64 length must be a multiple of 4".into());
        }
        let mut out = Vec::with_capacity(s.len() / 4 * 3);
        for (i, c) in s.chunks(4).enumerate() {
            let pad = c.iter().rev().take_while(|&&b| b == b'=').count();
            if pad > 2 || (pad > 0 && i + 1 != s.len() / 4) {
                return Err("bad base64 padding".into());
            }
            let mut n = 0u32;
            for &b in &c[..4 - pad] {
                let v = ALPHABET.iter().position(|&a| a == b).ok_or("bad base64 character")? as u32;
                n = (n << 6) | v;
            }
            n <<= 6 * pad as u32;
            out.extend_from_slice(&[(n >> 16) as u8, (n >> 8) as u8, n as u8][..3 - pad]);
        }
        Ok(out)
    }
}

/// Calls into the host.
pub mod host {
    use super::*;

    /// Sends `{method, params}`; the host's reply or its error message.
    pub fn call(method: &str, params: Value) -> Result<Value, String> {
        let req = serde_json::to_vec(&json!({"method": method, "params": params})).map_err(|e| e.to_string())?;
        let n = host_call(addr(&req), req.len() as i32);
        if n < 0 {
            return Err("no host".into());
        }
        let mut buf = vec![0u8; n as usize];
        host_response(buf.as_mut_ptr() as usize as i32);
        // the host wrote the reply into `buf`; read it through a black box so it isn't assumed zero
        let buf = std::hint::black_box(buf);
        let mut reply: Value = serde_json::from_slice(&buf).map_err(|e| format!("bad host reply: {e}"))?;
        if let Some(e) = reply.get("error") {
            return Err(e.as_str().map_or_else(|| e.to_string(), str::to_string));
        }
        Ok(reply.get_mut("ok").map(Value::take).unwrap_or(Value::Null))
    }

    /// A line in the plug-in's log (Plug-in Manager → Log).
    pub fn log(line: &str) {
        host_log(addr(line.as_bytes()), line.len() as i32);
    }

    /// `catalog.query` (needs the `catalog` permission).
    pub fn query(params: Value) -> Result<Value, String> {
        call("catalog.query", params)
    }

    /// This plug-in's own metadata for `photo`.
    pub fn ns_get(photo: &str) -> Result<Value, String> {
        call("ns.get", json!({"photo": photo}))
    }

    /// Sets (or with `Value::Null`, removes) a key in this plug-in's own metadata for `photo`.
    pub fn ns_set(photo: &str, key: &str, value: Value) -> Result<(), String> {
        call("ns.set", json!({"photo": photo, "key": key, "value": value})).map(|_| ())
    }

    /// This plug-in's settings (`settings` in the manifest): every key, defaults for those never set.
    pub fn settings() -> Result<Value, String> {
        call("settings.get", json!({}))
    }

    /// One setting (its default when never set, `Value::Null` when there is no host or no such key).
    pub fn setting(key: &str) -> Value {
        settings().ok().and_then(|mut s| s.get_mut(key).map(Value::take)).unwrap_or(Value::Null)
    }

    /// Changes a setting (`Value::Null`: back to its default); the host checks it against its
    /// field and returns the value kept (a number clamped to its range).
    pub fn set_setting(key: &str, value: Value) -> Result<Value, String> {
        call("settings.set", json!({"key": key, "value": value}))
    }

    pub fn read_text(path: &str) -> Result<String, String> {
        call("fs.readText", json!({"path": path})).map(|v| v.as_str().unwrap_or_default().to_string())
    }

    pub fn write_text(path: &str, text: &str) -> Result<(), String> {
        call("fs.writeText", json!({"path": path, "text": text})).map(|_| ())
    }

    /// A binary file (needs a readable root, or a file lent to this call such as an exported file).
    pub fn read_bytes(path: &str) -> Result<Vec<u8>, String> {
        call("fs.readBytes", json!({"path": path})).and_then(|v| crate::base64::decode(v.as_str().unwrap_or_default()))
    }

    /// Writes a binary file (needs a writable root, or a file lent to this call).
    pub fn write_bytes(path: &str, bytes: &[u8]) -> Result<(), String> {
        call("fs.writeBytes", json!({"path": path, "base64": crate::base64::encode(bytes)})).map(|_| ())
    }

    /// Removes a file below a writable root.
    pub fn remove(path: &str) -> Result<(), String> {
        call("fs.remove", json!({"path": path})).map(|_| ())
    }

    /// Does `path` exist (needs a readable root)?
    pub fn exists(path: &str) -> Result<bool, String> {
        call("fs.exists", json!({"path": path})).map(|v| v.as_bool().unwrap_or(false))
    }

    /// An HTTP request with a binary body (e.g. a WebDAV `PUT` of an exported file).
    pub fn http_bytes(method: &str, url: &str, headers: Value, content_type: &str, body: &[u8]) -> Result<Value, String> {
        call(
            "http.request",
            json!({"method": method, "url": url, "headers": headers, "contentType": content_type, "bodyBase64": crate::base64::encode(body)}),
        )
    }

    /// The programs this plug-in declares: `[{program, name, granted, installed: [{bundleId?, path?,
    /// version?}]}]`. Needs nothing; tells the user what to install or allow. `path` only once the
    /// program is granted.
    pub fn program_list() -> Result<Value, String> {
        call("program.list", json!({}))
    }

    /// Starts a granted program (`programs` permission): `files` (`{name: text}`) are written into
    /// a fresh folder for the run first, and `{tmpDir}` in `args` becomes its path. → `{run,
    /// tmpDir}`; the plug-in's `program.end` hook is called when it exits or the user stops
    /// waiting. `bundle_id` picks one of the entry's versions (macOS).
    pub fn program_start(program: &str, args: &[&str], files: Value, bundle_id: Option<&str>) -> Result<Value, String> {
        call("program.start", json!({"program": program, "args": args, "files": files, "bundleId": bundle_id}))
    }

    /// Adds the file at `path`, made from photo `from`, to the library (`libraryAdd` permission).
    /// `path` must be in the folder of `from`'s original or below it. With `stack`, it is stacked
    /// on `from`; with `album`, it goes in that album, which must be a regular album holding
    /// `from`. A file the library already has is read again if it changed. → `{photo, added,
    /// reloaded}`
    pub fn library_add(path: &str, from: &str, stack: bool, album: Option<&str>) -> Result<Value, String> {
        call("library.add", json!({"path": path, "from": from, "stack": stack, "album": album}))
    }

    /// An HTTP request (needs the URL's host in the `network` permission) →
    /// `{status, contentType, location, body}`.
    pub fn http(method: &str, url: &str, headers: Value, body: Option<&str>) -> Result<Value, String> {
        call("http.request", json!({"method": method, "url": url, "headers": headers, "body": body}))
    }
}

/// Exports the ABI: `export_plugin!(MANIFEST_JSON, handler)` with
/// `fn handler(&Request) -> Result<Value, String>`.
#[macro_export]
macro_rules! export_plugin {
    ($manifest:expr, $handler:path) => {
        #[unsafe(no_mangle)]
        pub extern "C" fn lcc_abi_version() -> i32 {
            $crate::ABI_VERSION
        }
        #[unsafe(no_mangle)]
        pub extern "C" fn lcc_manifest() -> i64 {
            $crate::__manifest($manifest)
        }
        #[unsafe(no_mangle)]
        pub extern "C" fn lcc_alloc(len: i32) -> i32 {
            $crate::__alloc(len)
        }
        #[unsafe(no_mangle)]
        pub extern "C" fn lcc_handle(ptr: i32, len: i32) -> i64 {
            $crate::__handle(ptr, len, $handler)
        }
    };
}
