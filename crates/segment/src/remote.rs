//! Private SAM 3 transport. Bind the worker to loopback and forward it over SSH.
//!
//! Both ends know a token ([`TOKEN_ENV`]) and prove it to each other before any pixels move
//! (issue #2134): the editor sends a random nonce, the worker answers with its own nonce and an
//! HMAC-SHA256 of both under the token, the editor checks it and answers with its own HMAC. The
//! token never crosses the socket, so neither a local process that connects to the worker nor one
//! listening on the editor's port while the tunnel is down learns it, gets the worker's GPU, or
//! receives the photo.
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use crate::{Click, Encoded, INPUT, MASK_SIDE, Sam3};
use serde::{Deserialize, Serialize};

const CONTROL_LIMIT: usize = 16 * 1024;
const RESPONSE_LIMIT: usize = 4 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(9 * 60);
/// Environment variable (name after the app prefix, see `lcc_brand::env`) with the remote worker address.
pub const ENV: &str = "SAM3_REMOTE";
/// The token both ends share (the worker prints a new one when it isn't set). Not
/// `<PREFIX>_SAM3_TOKEN`: that is the bearer token for a gated download mirror.
pub const TOKEN_ENV: &str = "SAM3_WORKER_TOKEN";
/// Shortest token accepted.
pub const MIN_TOKEN: usize = 16;
/// How long a peer has to complete the handshake.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// Handshakes the worker runs at once; connections past that are closed.
const MAX_HANDSHAKES: usize = 32;

#[derive(Serialize, Deserialize)]
pub enum Operation {
    Prepare,
    Clicks(Vec<[f32; 3]>),
    Text(String),
}

#[derive(Serialize, Deserialize)]
pub struct Request {
    pub width: usize,
    pub height: usize,
    pub temporary: bool,
    pub operation: Operation,
}

impl Request {
    pub fn validate(&self) -> Result<(), String> {
        if self.width > INPUT || self.height > INPUT || (self.width == 0) != (self.height == 0) {
            return Err("invalid SAM image dimensions".into());
        }
        if self.temporary && self.width == 0 {
            return Err("detail pass requires an image".into());
        }
        match &self.operation {
            Operation::Prepare => {}
            Operation::Text(text) if text.len() <= 4096 && text.split([',', ';']).any(|p| !p.trim().is_empty()) => {}
            Operation::Clicks(clicks)
                if !clicks.is_empty()
                    && clicks.len() <= 64
                    && clicks.iter().all(|c| {
                        c.iter().all(|v| v.is_finite()) && (0.0..=1.0).contains(&c[0]) && (0.0..=1.0).contains(&c[1]) && (c[2] == 0.0 || c[2] == 1.0)
                    })
                    && clicks.iter().any(|c| c[2] == 1.0) => {}
            _ => return Err("invalid SAM prompt".into()),
        }
        Ok(())
    }
}

pub fn configured() -> Option<String> {
    lcc_brand::env(ENV).filter(|v| !v.trim().is_empty())
}

/// The shared token from [`TOKEN_ENV`], at least [`MIN_TOKEN`] characters.
pub fn token() -> Result<String, String> {
    let t = lcc_brand::env(TOKEN_ENV).unwrap_or_default().trim().to_string();
    if t.chars().count() < MIN_TOKEN {
        return Err(format!("set {} to the token the SAM 3 worker printed (at least {MIN_TOKEN} characters)", lcc_brand::env_var(TOKEN_ENV)));
    }
    Ok(t)
}

/// 128 unpredictable bits as hex. Std's `RandomState` is keyed from the operating system's random
/// source; hashing a counter and the time under fresh keys gives values nobody else can predict,
/// without a random-number crate.
pub fn random_hex() -> String {
    use std::hash::{BuildHasher, Hasher};
    static N: AtomicUsize = AtomicUsize::new(0);
    let word = || {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_usize(N.fetch_add(1, Ordering::Relaxed));
        h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
        h.finish()
    };
    format!("{:016x}{:016x}", word(), word())
}

/// HMAC-SHA256 (RFC 2104) under `key` of `parts`, each prefixed by its length; as hex.
fn hmac(key: &str, parts: &[&str]) -> String {
    use sha2::{Digest, Sha256};
    let mut k = [0u8; 64];
    if key.len() > 64 {
        k[..32].copy_from_slice(&Sha256::digest(key.as_bytes()));
    } else {
        k[..key.len()].copy_from_slice(key.as_bytes());
    }
    let pad = |b: u8| k.map(|x| x ^ b);
    let mut inner = Sha256::new();
    inner.update(pad(0x36));
    for p in parts {
        inner.update((p.len() as u64).to_be_bytes());
        inner.update(p.as_bytes());
    }
    let mut outer = Sha256::new();
    outer.update(pad(0x5c));
    outer.update(inner.finalize());
    outer.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Constant-time comparison (a proof must not leak through timing).
fn same(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut d = a.len() ^ b.len();
    for i in 0..a.len().max(b.len()) {
        d |= (a.get(i).copied().unwrap_or(0) ^ b.get(i).copied().unwrap_or(0)) as usize;
    }
    d == 0
}

#[derive(Serialize, Deserialize)]
struct Hello {
    nonce: String,
}

#[derive(Serialize, Deserialize)]
struct Proof {
    #[serde(default)]
    nonce: String,
    proof: String,
}

fn send<T: Serialize>(w: &mut impl Write, v: &T) -> Result<(), String> {
    write_frame(w, &serde_json::to_vec(v).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}

fn receive<T: serde::de::DeserializeOwned>(r: &mut impl Read) -> Result<T, String> {
    let frame = read_frame(r, CONTROL_LIMIT).map_err(|e| e.to_string())?;
    serde_json::from_slice(&frame).map_err(|_| "unexpected handshake".to_string())
}

/// The editor's side of the handshake: the worker must prove it knows `token` before any pixels go.
pub fn client_handshake(stream: &mut (impl Read + Write), token: &str) -> Result<(), String> {
    let mine = random_hex();
    send(stream, &Hello { nonce: mine.clone() })?;
    let reply: Proof = receive(stream)?;
    if reply.nonce.len() < 32 || !same(&reply.proof, &hmac(token, &["worker", &mine, &reply.nonce])) {
        return Err("the SAM 3 worker didn't prove it knows the token: is something else listening on that port?".into());
    }
    send(stream, &Proof { nonce: String::new(), proof: hmac(token, &["editor", &mine, &reply.nonce]) })
}

/// The worker's side: the editor must prove it knows `token` before a request is read.
pub fn server_handshake(stream: &mut (impl Read + Write), token: &str) -> Result<(), String> {
    let hello: Hello = receive(stream)?;
    if hello.nonce.len() < 32 {
        return Err("unexpected handshake".into());
    }
    let mine = random_hex();
    send(stream, &Proof { nonce: mine.clone(), proof: hmac(token, &["worker", &hello.nonce, &mine]) })?;
    let reply: Proof = receive(stream)?;
    if !same(&reply.proof, &hmac(token, &["editor", &hello.nonce, &mine])) {
        return Err("wrong token".into());
    }
    Ok(())
}

pub fn read_frame(reader: &mut impl Read, limit: usize) -> io::Result<Vec<u8>> {
    let mut size = [0; 4];
    reader.read_exact(&mut size)?;
    let size = u32::from_be_bytes(size) as usize;
    if size > limit {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "SAM frame exceeds limit"));
    }
    let mut data = vec![0; size];
    reader.read_exact(&mut data)?;
    Ok(data)
}

fn write_frame(writer: &mut impl Write, data: &[u8]) -> io::Result<()> {
    let size = u32::try_from(data.len()).map_err(io::Error::other)?;
    writer.write_all(&size.to_be_bytes())?;
    writer.write_all(data)
}

pub struct Client {
    stream: TcpStream,
    pub key: Option<u64>,
}

impl Client {
    pub fn connect(endpoint: &str) -> Result<Self, String> {
        let address: SocketAddr = endpoint.parse().map_err(|e| format!("SAM endpoint must be an IP:port: {e}"))?;
        if !address.ip().is_loopback() {
            return Err("SAM endpoint must be loopback; use an SSH tunnel for remote access".into());
        }
        let token = token()?;
        let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(5)).map_err(|e| e.to_string())?;
        stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT)).map_err(|e| e.to_string())?;
        stream.set_write_timeout(Some(Duration::from_secs(30))).map_err(|e| e.to_string())?;
        client_handshake(&mut stream, &token)?;
        stream.set_read_timeout(Some(TIMEOUT)).map_err(|e| e.to_string())?;
        Ok(Self { stream, key: None })
    }

    pub fn request(&mut self, request: &Request, rgb: &[u8]) -> Result<Option<Vec<f32>>, String> {
        request.validate()?;
        if rgb.len() != request.width * request.height * 3 {
            return Err("SAM RGB buffer size does not match dimensions".into());
        }
        let bytes = serde_json::to_vec(request).map_err(|e| e.to_string())?;
        write_frame(&mut self.stream, &bytes).map_err(|e| e.to_string())?;
        write_frame(&mut self.stream, rgb).map_err(|e| e.to_string())?;
        let response = read_frame(&mut self.stream, RESPONSE_LIMIT).map_err(|e| e.to_string())?;
        let result: Result<Option<Vec<f32>>, String> = serde_json::from_slice(&response).map_err(|e| e.to_string())?;
        let logits = result?;
        if logits.as_ref().is_some_and(|v| v.len() != MASK_SIDE * MASK_SIDE || v.iter().any(|n| !n.is_finite())) {
            return Err("invalid SAM mask response".into());
        }
        Ok(logits)
    }
}

pub fn text_logits(model: &mut Sam3, enc: &mut Encoded, text: &str) -> crate::Result<Option<Vec<f32>>> {
    let mut merged: Option<Vec<f32>> = None;
    for phrase in text.split([',', ';']).map(str::trim).filter(|p| !p.is_empty()) {
        if let Some(p) = model.segment_text(enc, phrase, 0.5)? {
            if let Some(m) = &mut merged {
                for (a, b) in m.iter_mut().zip(p.logits) {
                    *a = a.max(b);
                }
            } else {
                merged = Some(p.logits);
            }
        }
    }
    Ok(merged)
}

fn predict(model: &mut Sam3, enc: &mut Encoded, operation: Operation) -> Result<Option<Vec<f32>>, String> {
    match operation {
        Operation::Prepare => {
            model.prepare_clicks(enc).map_err(|e| e.to_string())?;
            Ok(None)
        }
        Operation::Text(text) => text_logits(model, enc, &text).map_err(|e| e.to_string()),
        Operation::Clicks(points) => {
            let clicks: Vec<_> = points.into_iter().map(|p| Click { x: p[0], y: p[1], positive: p[2] == 1.0 }).collect();
            Ok(Some(model.segment_clicks(enc, &clicks).map_err(|e| e.to_string())?.logits))
        }
    }
}

/// Accept connections on `listener` and hand over those that complete the handshake. Handshakes
/// run on their own threads (at most [`MAX_HANDSHAKES`]) with a short deadline, so a peer without
/// the token can't hold the worker; the sessions themselves, which share the one model and device,
/// are served one at a time by the receiver. A failed accept (out of file descriptors, a
/// connection reset while queued) is logged and the loop goes on (issue #2134).
pub fn accept_authenticated(listener: TcpListener, token: String) -> std::sync::mpsc::Receiver<TcpStream> {
    let (authed, sessions) = std::sync::mpsc::channel::<TcpStream>();
    let pending = Arc::new(AtomicUsize::new(0));
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = match stream {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("SAM3 accept failed: {e}");
                    std::thread::sleep(Duration::from_millis(100));
                    continue;
                }
            };
            if pending.fetch_add(1, Ordering::SeqCst) >= MAX_HANDSHAKES {
                pending.fetch_sub(1, Ordering::SeqCst);
                continue;
            }
            let (token, authed, pending) = (token.clone(), authed.clone(), pending.clone());
            std::thread::spawn(move || {
                let peer = stream.peer_addr().map(|a| a.to_string()).unwrap_or_default();
                let ok = stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT)).is_ok()
                    && stream.set_write_timeout(Some(HANDSHAKE_TIMEOUT)).is_ok()
                    && server_handshake(&mut stream, &token).inspect_err(|e| eprintln!("SAM3 {peer}: {e}")).is_ok();
                pending.fetch_sub(1, Ordering::SeqCst);
                if ok {
                    let _ = authed.send(stream);
                }
            });
        }
    });
    sessions
}

fn serve_client(stream: &mut TcpStream, dir: &Path, device: candle_core::Device) -> Result<(), String> {
    stream.set_read_timeout(Some(Duration::from_secs(10 * 60))).map_err(|e| e.to_string())?;
    stream.set_write_timeout(Some(Duration::from_secs(30))).map_err(|e| e.to_string())?;
    let mut model = None;
    let mut cache = None;
    loop {
        let frame = read_frame(stream, CONTROL_LIMIT).map_err(|e| e.to_string())?;
        let request: Request = serde_json::from_slice(&frame).map_err(|e| e.to_string())?;
        request.validate()?;
        let rgb = read_frame(stream, request.width * request.height * 3).map_err(|e| e.to_string())?;
        if rgb.len() != request.width * request.height * 3 {
            return Err("invalid RGB payload".into());
        }
        let started = std::time::Instant::now();
        let result = (|| {
            if model.is_none() {
                model = Some(Sam3::load_on(dir, device.clone()).map_err(|e| e.to_string())?);
            }
            let m = model.as_mut().ok_or("SAM model unavailable")?;
            let mut temporary = None;
            if request.width > 0 {
                let encoded = m.encode(&rgb, request.width, request.height).map_err(|e| e.to_string())?;
                if request.temporary {
                    temporary = Some(encoded);
                } else {
                    cache = Some(encoded);
                }
            }
            let enc = if request.temporary { temporary.as_mut() } else { cache.as_mut() }.ok_or("SAM image cache empty")?;
            predict(m, enc, request.operation)
        })();
        eprintln!(
            "SAM3 Metal request elapsed={:.3}s image={}x{} detail={} success={}",
            started.elapsed().as_secs_f64(),
            request.width,
            request.height,
            request.temporary,
            result.is_ok()
        );
        let response = serde_json::to_vec(&result).map_err(|e| e.to_string())?;
        write_frame(stream, &response).map_err(|e| e.to_string())?;
        if result.is_err() {
            return Err("SAM inference failed; session released".into());
        }
    }
}

pub fn serve(address: SocketAddr, dir: &Path, device: candle_core::Device) -> Result<(), String> {
    if !address.ip().is_loopback() || !device.is_metal() {
        return Err("SAM worker requires a loopback address and a Metal device".into());
    }
    if !crate::is_model_dir(dir) {
        return Err("SAM model files are missing".into());
    }
    let token = match token() {
        Ok(t) => t,
        Err(_) => {
            let t = random_hex();
            eprintln!("SAM3 worker: no {} set; this run's token is {t}", lcc_brand::env_var(TOKEN_ENV));
            eprintln!("  start the app with {}={t} (or set it here too, to keep one token)", lcc_brand::env_var(TOKEN_ENV));
            t
        }
    };
    let listener = TcpListener::bind(address).map_err(|e| e.to_string())?;
    eprintln!("SAM3 worker listening on {address}; device=Metal; idle unload=600s");
    let sessions = accept_authenticated(listener, token);
    for mut stream in sessions {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| serve_client(&mut stream, dir, device.clone())));
        match result {
            Ok(Err(error)) => eprintln!("SAM3 session closed: {error}"),
            Err(_) => eprintln!("SAM3 session panicked; model released"),
            Ok(Ok(())) => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "a-token-of-thirty-two-characters";

    fn pair() -> (TcpStream, TcpStream) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let c = TcpStream::connect(l.local_addr().unwrap()).unwrap();
        let (s, _) = l.accept().unwrap();
        for x in [&c, &s] {
            x.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        }
        (c, s)
    }

    /// Both ends prove the token; a wrong one fails on both sides, and the token itself never
    /// crosses the socket (issue #2134).
    #[test]
    fn the_handshake_proves_the_token_both_ways() {
        for (client_token, ok) in [(TOKEN, true), ("another-token-of-32-characters!!", false)] {
            let (mut c, mut s) = pair();
            let server = std::thread::spawn(move || server_handshake(&mut s, TOKEN));
            let client = client_handshake(&mut c, client_token);
            drop(c);
            assert_eq!((client.is_ok(), server.join().unwrap().is_ok()), (ok, ok), "{client:?}");
        }
        // something else listening on the editor's port: it learns nothing, and the editor stops
        let (mut c, mut s) = pair();
        let impostor = std::thread::spawn(move || {
            let hello = read_frame(&mut s, CONTROL_LIMIT).unwrap();
            send(&mut s, &Proof { nonce: random_hex(), proof: "00".repeat(32) }).unwrap();
            let mut rest = Vec::new();
            let _ = s.read_to_end(&mut rest);
            (hello, rest)
        });
        assert!(client_handshake(&mut c, TOKEN).unwrap_err().contains("didn't prove"));
        drop(c);
        let (hello, rest) = impostor.join().unwrap();
        assert!(!String::from_utf8_lossy(&hello).contains(TOKEN));
        assert!(rest.is_empty(), "nothing more is sent to a peer that failed the handshake");
    }

    /// Peers that connect and say nothing don't keep an editor with the token waiting: handshakes
    /// run side by side, and only completed ones reach the session loop.
    #[test]
    fn silent_peers_dont_block_the_editor() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let sessions = accept_authenticated(listener, TOKEN.into());
        let _silent: Vec<TcpStream> = (0..3).map(|_| TcpStream::connect(addr).unwrap()).collect();
        let started = std::time::Instant::now();
        let mut editor = TcpStream::connect(addr).unwrap();
        editor.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        client_handshake(&mut editor, TOKEN).unwrap();
        let served = sessions.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(started.elapsed() < HANDSHAKE_TIMEOUT, "{:?}", started.elapsed());
        assert_eq!(served.peer_addr().unwrap(), editor.local_addr().unwrap());
        // a wrong token never reaches the sessions
        let mut wrong = TcpStream::connect(addr).unwrap();
        wrong.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        assert!(client_handshake(&mut wrong, "another-token-of-32-characters!!").is_err());
        assert!(sessions.recv_timeout(Duration::from_millis(500)).is_err());
    }

    #[test]
    fn hmac_depends_on_key_and_every_part() {
        let base = hmac(TOKEN, &["worker", "a", "b"]);
        assert_eq!(base.len(), 64);
        assert_eq!(base, hmac(TOKEN, &["worker", "a", "b"]));
        for other in [hmac("x", &["worker", "a", "b"]), hmac(TOKEN, &["editor", "a", "b"]), hmac(TOKEN, &["worker", "ab", ""])] {
            assert_ne!(base, other);
        }
        assert_ne!(random_hex(), random_hex());
    }
}
