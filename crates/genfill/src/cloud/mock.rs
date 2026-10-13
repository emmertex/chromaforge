//! A local stand-in for the providers, for tests and for trying generative fill without an
//! account (`lcc-cli genfill-mock`). It speaks each provider's request and response shapes
//! (as documented by the providers, see the PR notes) on `127.0.0.1` only:
//!
//! | Provider | Requests |
//! |---|---|
//! | OpenAI | `POST /v1/images/edits` (multipart) → `{data: [{b64_json}]}` |
//! | Stability | `POST /v2beta/stable-image/edit/{inpaint,erase}` (multipart, `Accept: application/json`) → `{image, finish_reason, seed}` |
//! | BFL | `POST /v1/flux-pro-1.0-fill`, `/v1/flux-tools/erase-v1` (JSON) → `{id, polling_url}`; `GET /v1/get_result?id=` → `{status: "Ready", result: {sample}}`; `GET /mock/sample/<id>.png` |
//! | fal | `POST /fal-ai/flux-pro/v1/fill`, `/fal-ai/object-removal/mask` (JSON, data URIs) → `{request_id, status_url, response_url}`; `GET …/requests/<id>/status`, `GET …/requests/<id>` → `{images: [{url}]}` |
//!
//! Each request must carry the provider's auth header with the expected key (when one is set),
//! else the reply is the provider's 401. What fills the hole is up to the caller ([`Filler`]):
//! the CLI uses the app's own content-aware fill, so the whole raw-level round trip can be
//! checked with real photos without sending them anywhere.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use serde_json::{Value, json};

use super::util::{b64_decode, b64_encode, data_uri_bytes, parse_multipart, png_decode_mask, png_decode_rgb, png_encode};

/// Fills a hole: `(w, h, rgb, mask (255 = fill), variation) → rgb` (all interleaved 8-bit).
pub type Filler = Arc<dyn Fn(usize, usize, &[u8], &[u8], u32) -> Vec<u8> + Send + Sync>;

/// A request the mock saw (for tests): path, whether the key matched, the form or JSON fields.
#[derive(Clone, Debug, Default)]
pub struct Seen {
    pub method: String,
    pub path: String,
    pub authorized: bool,
    pub fields: Vec<String>,
    /// Size of the image it received.
    pub image: Option<(usize, usize)>,
    pub store_io: Option<String>,
}

struct State {
    key: Option<String>,
    filler: Filler,
    base: String,
    results: Mutex<HashMap<String, Vec<Vec<u8>>>>,
    seen: Mutex<Vec<Seen>>,
    next: AtomicU64,
    stop: AtomicBool,
}

/// A running mock server; it stops when dropped.
pub struct MockServer {
    state: Arc<State>,
    pub base: String,
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.state.stop.store(true, Ordering::Relaxed);
        // wake the accept loop
        let _ = TcpStream::connect(self.base.trim_start_matches("http://"));
    }
}

/// The simplest fill: every hole pixel becomes the mean of the kept pixels, shifted a little per
/// variation (so variations differ).
pub fn mean_filler() -> Filler {
    Arc::new(|w, h, rgb, mask, var| {
        let (mut s, mut n) = ([0u64; 3], 0u64);
        for (i, m) in mask.iter().enumerate().take(w * h) {
            if *m < 128 {
                for c in 0..3 {
                    s[c] += u64::from(rgb.get(i * 3 + c).copied().unwrap_or(0));
                }
                n += 1;
            }
        }
        let mean = s.map(|v| (v / n.max(1)) as u8);
        let mut out = rgb.to_vec();
        for (i, m) in mask.iter().enumerate().take(w * h) {
            if *m >= 128 {
                for c in 0..3 {
                    if let Some(o) = out.get_mut(i * 3 + c) {
                        *o = mean[c].saturating_add((var % 3) as u8);
                    }
                }
            }
        }
        out
    })
}

impl MockServer {
    /// Serve on `127.0.0.1:port` (0 = any free port), requiring `key` (if given).
    pub fn start(port: u16, key: Option<&str>, filler: Filler) -> std::io::Result<MockServer> {
        let l = TcpListener::bind(("127.0.0.1", port))?;
        let base = format!("http://{}", l.local_addr()?);
        let state = Arc::new(State {
            key: key.map(str::to_string),
            filler,
            base: base.clone(),
            results: Mutex::new(HashMap::new()),
            seen: Mutex::new(Vec::new()),
            next: AtomicU64::new(1),
            stop: AtomicBool::new(false),
        });
        let st = state.clone();
        std::thread::Builder::new().name("genfill-mock".into()).spawn(move || {
            for s in l.incoming() {
                if st.stop.load(Ordering::Relaxed) {
                    break;
                }
                let Ok(s) = s else { continue };
                let st = st.clone();
                let _ = std::thread::Builder::new().name("genfill-mock-conn".into()).spawn(move || {
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| handle(s, &st)));
                });
            }
        })?;
        Ok(MockServer { state, base })
    }

    /// Requests seen so far.
    pub fn seen(&self) -> Vec<Seen> {
        self.state.seen.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

fn reply(s: &mut TcpStream, status: u16, ctype: &str, body: &[u8]) {
    let reason = match status {
        200 => "OK",
        401 => "Unauthorized",
        404 => "Not Found",
        _ => "Error",
    };
    let _ = write!(s, "HTTP/1.1 {status} {reason}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
    let _ = s.write_all(body);
}

fn reply_json(s: &mut TcpStream, status: u16, v: &Value) {
    reply(s, status, "application/json", v.to_string().as_bytes());
}

/// Decode the image/mask pair a request carries.
fn pair(img: &[u8], mask: &[u8], alpha_mask: bool) -> Option<(usize, usize, Vec<u8>, Vec<u8>)> {
    let (w, h, rgb) = png_decode_rgb(img).ok()?;
    let (mw, mh, v, a) = png_decode_mask(mask).ok()?;
    if (mw, mh) != (w, h) {
        return None;
    }
    // OpenAI: transparent = edit; others: white = edit
    let m: Vec<u8> = if alpha_mask {
        a?.iter().map(|a| if *a < 128 { 255 } else { 0 }).collect()
    } else {
        v.iter().map(|v| if *v >= 128 { 255 } else { 0 }).collect()
    };
    Some((w, h, rgb, m))
}

fn handle(mut s: TcpStream, st: &State) {
    let Ok(clone) = s.try_clone() else { return };
    let mut r = BufReader::new(clone);
    let mut first = String::new();
    if r.read_line(&mut first).is_err() {
        return;
    }
    let mut parts = first.split_whitespace();
    let (method, target) = (parts.next().unwrap_or_default().to_string(), parts.next().unwrap_or("/").to_string());
    let mut headers: Vec<(String, String)> = Vec::new();
    loop {
        let mut l = String::new();
        if r.read_line(&mut l).unwrap_or(0) == 0 || l.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = l.split_once(':') {
            headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
        }
    }
    let header = |k: &str| headers.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
    let len: usize = header("content-length").and_then(|v| v.parse().ok()).unwrap_or(0).min(80 * 1024 * 1024);
    let mut body = vec![0u8; len];
    if r.read_exact(&mut body).is_err() {
        return;
    }
    let path = target.split('?').next().unwrap_or("/").to_string();
    let query = target.split_once('?').map(|(_, q)| q.to_string()).unwrap_or_default();
    let presented = header("authorization").or_else(|| header("x-key"));
    let authorized = match &st.key {
        None => presented.is_some(),
        Some(k) => presented.is_some_and(|p| p == format!("Bearer {k}") || p == format!("Key {k}") || p == *k),
    };
    let mut seen = Seen { method: method.clone(), path: path.clone(), authorized, store_io: header("x-fal-store-io"), ..Default::default() };
    let record = |seen: Seen| st.seen.lock().unwrap_or_else(PoisonError::into_inner).push(seen);
    // pre-signed result links carry no key
    if method == "GET" && path.starts_with("/mock/sample/") {
        record(seen);
        let id = path.trim_start_matches("/mock/sample/").trim_end_matches(".png");
        let got = st.results.lock().unwrap_or_else(PoisonError::into_inner).get(id).and_then(|v| v.first().cloned());
        return match got {
            Some(png) => reply(&mut s, 200, "image/png", &png),
            None => reply_json(&mut s, 404, &json!({"detail": "not found"})),
        };
    }
    if !authorized {
        record(seen);
        return reply_json(&mut s, 401, &json!({"error": {"message": "Incorrect API key provided.", "code": "invalid_api_key"}}));
    }
    let id = format!("mock{:08}", st.next.fetch_add(1, Ordering::Relaxed));
    let fill = |w: usize, h: usize, rgb: &[u8], m: &[u8], var: u32| -> Vec<u8> {
        let out = (st.filler)(w, h, rgb, m, var);
        if out.len() == w * h * 3 { png_encode(w, h, 3, &out).unwrap_or_default() } else { Vec::new() }
    };
    match (method.as_str(), path.as_str()) {
        ("POST", "/v1/images/edits") | ("POST", "/v2beta/stable-image/edit/inpaint") | ("POST", "/v2beta/stable-image/edit/erase") => {
            let boundary = header("content-type").and_then(|c| c.split("boundary=").nth(1).map(str::to_string)).unwrap_or_default();
            let form = parse_multipart(&body, &boundary);
            seen.fields = form.iter().map(|p| p.name.clone()).collect();
            let get = |n: &str| form.iter().find(|p| p.name == n || p.name == format!("{n}[]")).map(|p| p.data.clone());
            let openai = path == "/v1/images/edits";
            let Some((w, h, rgb, m)) = get("image").zip(get("mask")).and_then(|(i, m)| pair(&i, &m, openai)) else {
                record(seen);
                return reply_json(&mut s, 400, &json!({"error": {"message": "image and mask must be PNGs of the same size"}}));
            };
            seen.image = Some((w, h));
            record(seen);
            if openai {
                let n: u32 = get("n").and_then(|v| String::from_utf8(v).ok()).and_then(|v| v.trim().parse().ok()).unwrap_or(1).clamp(1, 10);
                // like the real endpoint, the result has the requested size
                let size = get("size").and_then(|v| String::from_utf8(v).ok()).unwrap_or_else(|| "1024x1024".into());
                let (sw, sh) = size.split_once('x').and_then(|(a, b)| Some((a.parse::<usize>().ok()?, b.parse::<usize>().ok()?))).unwrap_or((w, h));
                let (rgb, m) = (resize_nearest(&rgb, w, h, sw, sh, 3), resize_nearest(&m, w, h, sw, sh, 1));
                let data: Vec<Value> = (0..n).map(|v| json!({"b64_json": b64_encode(&fill(sw, sh, &rgb, &m, v))})).collect();
                reply_json(&mut s, 200, &json!({"created": 1, "data": data}))
            } else {
                reply_json(&mut s, 200, &json!({"image": b64_encode(&fill(w, h, &rgb, &m, 0)), "finish_reason": "SUCCESS", "seed": 1}))
            }
        }
        ("POST", "/v1/flux-pro-1.0-fill") | ("POST", "/v1/flux-tools/erase-v1") => {
            let v: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
            seen.fields = v.as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default();
            let img = v.get("image").and_then(Value::as_str).and_then(b64_decode);
            let mask = v.get("mask").and_then(Value::as_str).and_then(b64_decode);
            let Some((w, h, rgb, m)) = img.zip(mask).and_then(|(i, m)| pair(&i, &m, false)) else {
                record(seen);
                return reply_json(
                    &mut s,
                    422,
                    &json!({"detail": [{"loc": ["body", "image"], "msg": "invalid image or mask", "type": "value_error"}]}),
                );
            };
            seen.image = Some((w, h));
            record(seen);
            st.results.lock().unwrap_or_else(PoisonError::into_inner).insert(id.clone(), vec![fill(w, h, &rgb, &m, 0)]);
            reply_json(&mut s, 200, &json!({"id": id, "polling_url": format!("{}/v1/get_result?id={id}", st.base)}))
        }
        ("GET", "/v1/get_result") => {
            record(seen);
            let id = query.split('&').find_map(|kv| kv.strip_prefix("id=")).unwrap_or_default().to_string();
            if st.results.lock().unwrap_or_else(PoisonError::into_inner).contains_key(&id) {
                reply_json(&mut s, 200, &json!({"id": id, "status": "Ready", "result": {"sample": format!("{}/mock/sample/{id}.png", st.base)}}))
            } else {
                reply_json(&mut s, 200, &json!({"id": id, "status": "Task not found"}))
            }
        }
        ("POST", p) if p.starts_with("/fal-ai/") => {
            let v: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
            seen.fields = v.as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default();
            let img = v.get("image_url").and_then(Value::as_str).and_then(data_uri_bytes);
            let mask = v.get("mask_url").and_then(Value::as_str).and_then(data_uri_bytes);
            let Some((w, h, rgb, m)) = img.zip(mask).and_then(|(i, m)| pair(&i, &m, false)) else {
                record(seen);
                return reply_json(&mut s, 422, &json!({"detail": [{"msg": "invalid image_url or mask_url"}]}));
            };
            seen.image = Some((w, h));
            record(seen);
            let n = v.get("num_images").and_then(Value::as_u64).unwrap_or(1).clamp(1, 4) as u32;
            let pngs: Vec<Vec<u8>> = (0..n).map(|k| fill(w, h, &rgb, &m, k)).collect();
            st.results.lock().unwrap_or_else(PoisonError::into_inner).insert(id.clone(), pngs);
            let model = p.trim_start_matches('/');
            reply_json(
                &mut s,
                200,
                &json!({"request_id": id, "status_url": format!("{}/{model}/requests/{id}/status", st.base), "response_url": format!("{}/{model}/requests/{id}", st.base), "queue_position": 0}),
            )
        }
        ("GET", p) if p.starts_with("/fal-ai/") && p.contains("/requests/") => {
            record(seen);
            let rest = p.split("/requests/").nth(1).unwrap_or_default();
            let (id, status) = match rest.strip_suffix("/status") {
                Some(id) => (id.to_string(), true),
                None => (rest.to_string(), false),
            };
            let got = st.results.lock().unwrap_or_else(PoisonError::into_inner).get(&id).cloned();
            match (got, status) {
                (None, _) => reply_json(&mut s, 404, &json!({"detail": "Request not found"})),
                (Some(_), true) => reply_json(&mut s, 200, &json!({"status": "COMPLETED", "request_id": id})),
                (Some(pngs), false) => {
                    let images: Vec<Value> = pngs
                        .iter()
                        .map(|p| json!({"url": format!("data:image/png;base64,{}", b64_encode(p)), "content_type": "image/png"}))
                        .collect();
                    reply_json(&mut s, 200, &json!({"images": images, "has_nsfw_concepts": vec![false; images.len()], "seed": 1}))
                }
            }
        }
        _ => {
            record(seen);
            reply_json(&mut s, 404, &json!({"detail": "Not Found"}))
        }
    }
}

/// Nearest-neighbour resize of interleaved 8-bit pixels.
fn resize_nearest(src: &[u8], w: usize, h: usize, nw: usize, nh: usize, ch: usize) -> Vec<u8> {
    let mut out = vec![0u8; nw * nh * ch];
    for y in 0..nh {
        let sy = (y * h / nh.max(1)).min(h.saturating_sub(1));
        for x in 0..nw {
            let sx = (x * w / nw.max(1)).min(w.saturating_sub(1));
            for c in 0..ch {
                if let (Some(d), Some(s)) = (out.get_mut((y * nw + x) * ch + c), src.get((sy * w + sx) * ch + c)) {
                    *d = *s;
                }
            }
        }
    }
    out
}
