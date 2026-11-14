//! The encrypted, mutually authenticated channel between two devices, over TCP.
//!
//! Handshake (own design, after the Noise / SIGMA pattern; Sources: RFC 7748 X25519, RFC 8032
//! Ed25519, RFC 5869 HKDF, RFC 8439 ChaCha20-Poly1305):
//! 1. Each side sends `Hello {device, key, eph}`: its device id, its Ed25519 public key and a
//!    fresh X25519 public key.
//! 2. `transcript = SHA-256(label ‖ hello_initiator ‖ hello_responder)`. Each side signs
//!    `transcript ‖ role` with its Ed25519 key and checks the other's signature against the key it
//!    sent. Nobody in the middle can swap the ephemeral keys without a device's private key.
//! 3. HKDF-SHA-256 over the X25519 shared secret (salt = transcript) gives one key per direction
//!    and the pairing code. The code covers both devices' keys, so a device in the middle (which
//!    must use its own key) makes the two screens show different codes.
//!
//! Whether the other device may talk is the caller's decision: a paired device's key must match
//! the stored one ([`crate::identity::Store`]); a pairing needs the people to compare the codes.
//!
//! Frames: a 4-byte big-endian length, then ChaCha20-Poly1305 ciphertext; the nonce is a per-
//! direction counter, so a replayed, reordered or altered frame fails to decrypt.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::identity::{Identity, random, verify};
use crate::{Error, Result};

const LABEL: &[u8] = b"lcc-peer-v1";
/// The largest handshake message.
const MAX_HELLO: usize = 4096;
/// The largest frame (a batch of catalog records).
pub const MAX_FRAME: usize = 64 << 20;
/// The largest frame before the other device is known to be paired (openings, replies, decisions):
/// an unpaired device can't make this side allocate more than this.
pub const MAX_SMALL_FRAME: usize = 64 << 10;
/// How long the whole handshake may take, however slowly the other side trickles bytes.
pub const HANDSHAKE_WITHIN: Duration = Duration::from_secs(10);

#[derive(Serialize, Deserialize)]
struct Hello {
    device: String,
    key: String,
    eph: String,
}

#[derive(Serialize, Deserialize)]
struct Auth {
    sig: String,
}

fn write_raw(s: &mut TcpStream, bytes: &[u8]) -> Result<()> {
    let len = u32::try_from(bytes.len()).map_err(|_| Error::Protocol("message too large".into()))?;
    s.write_all(&len.to_be_bytes())?;
    s.write_all(bytes)?;
    s.flush()?;
    Ok(())
}

/// Fills `buf`; with a deadline, every read waits only for the time left, so a peer sending one
/// byte at a time can't stretch the read past it.
fn read_full(s: &mut TcpStream, buf: &mut [u8], deadline: Option<Instant>) -> Result<()> {
    let Some(deadline) = deadline else {
        s.read_exact(buf)?;
        return Ok(());
    };
    let mut got = 0;
    while got < buf.len() {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(Error::Io("the other device took too long".into()));
        }
        s.set_read_timeout(Some(left))?;
        match s.read(&mut buf[got..]) {
            Ok(0) => return Err(Error::Io("the connection closed".into())),
            Ok(n) => got += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

fn read_raw(s: &mut TcpStream, max: usize, deadline: Option<Instant>) -> Result<Vec<u8>> {
    let mut len = [0u8; 4];
    read_full(s, &mut len, deadline)?;
    let len = u32::from_be_bytes(len) as usize;
    if len > max {
        return Err(Error::Protocol(format!("message of {len} bytes refused")));
    }
    let mut buf = vec![0u8; len];
    read_full(s, &mut buf, deadline)?;
    Ok(buf)
}

/// One side of an established channel.
pub struct Channel {
    stream: TcpStream,
    send: ChaCha20Poly1305,
    recv: ChaCha20Poly1305,
    sent: u64,
    received: u64,
    /// The largest frame accepted now ([`MAX_SMALL_FRAME`] until [`Channel::allow_large_frames`]).
    max_frame: usize,
    /// When set, reads fail once this instant passes (see [`Channel::set_deadline`]).
    deadline: Option<Instant>,
    /// The other device's id and public key (proved by its signature).
    pub peer_device: String,
    pub peer_key: String,
    /// The six-digit code both screens show while pairing.
    pub code: String,
}

impl std::fmt::Debug for Channel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Channel").field("peer_device", &self.peer_device).finish_non_exhaustive()
    }
}

fn nonce(counter: u64) -> [u8; 12] {
    let mut n = [0u8; 12];
    n[4..].copy_from_slice(&counter.to_be_bytes());
    n
}

impl Channel {
    /// Runs the handshake; `initiator` is the side that connected.
    /// The whole handshake must finish within [`HANDSHAKE_WITHIN`]; afterwards the channel keeps
    /// that deadline (reads fail past it) until the caller sets another.
    pub fn handshake(stream: TcpStream, me: &Identity, initiator: bool) -> Result<Channel> {
        Self::handshake_within(stream, me, initiator, HANDSHAKE_WITHIN)
    }

    fn handshake_within(mut stream: TcpStream, me: &Identity, initiator: bool, within: Duration) -> Result<Channel> {
        let deadline = Some(Instant::now() + within);
        stream.set_write_timeout(Some(within))?;
        let eph_secret = x25519_dalek::StaticSecret::from(random::<32>()?);
        let eph_public = x25519_dalek::PublicKey::from(&eph_secret);
        let mine = serde_json::to_vec(&Hello { device: me.device_id.clone(), key: me.public_key()?, eph: B64.encode(eph_public.as_bytes()) })
            .map_err(|e| Error::Protocol(e.to_string()))?;
        let theirs = if initiator {
            write_raw(&mut stream, &mine)?;
            read_raw(&mut stream, MAX_HELLO, deadline)?
        } else {
            let t = read_raw(&mut stream, MAX_HELLO, deadline)?;
            write_raw(&mut stream, &mine)?;
            t
        };
        let hello: Hello = serde_json::from_slice(&theirs).map_err(|_| Error::Protocol("bad hello".into()))?;
        if hello.device == me.device_id {
            return Err(Error::Protocol("connected to itself".into()));
        }
        let eph: [u8; 32] = B64.decode(&hello.eph).ok().and_then(|b| b.try_into().ok()).ok_or_else(|| Error::Protocol("bad key exchange".into()))?;
        let (first, second) = if initiator { (&mine, &theirs) } else { (&theirs, &mine) };
        let transcript: [u8; 32] = Sha256::new().chain_update(LABEL).chain_update(first).chain_update(second).finalize().into();

        let role = |init: bool| -> Vec<u8> { [&transcript[..], &[u8::from(init)]].concat() };
        let my_sig = serde_json::to_vec(&Auth { sig: B64.encode(me.sign(&role(initiator))?) }).map_err(|e| Error::Protocol(e.to_string()))?;
        write_raw(&mut stream, &my_sig)?;
        let their_sig: Auth =
            serde_json::from_slice(&read_raw(&mut stream, MAX_HELLO, deadline)?).map_err(|_| Error::Protocol("bad signature".into()))?;
        let sig = B64.decode(&their_sig.sig).map_err(|_| Error::Protocol("bad signature".into()))?;
        if !verify(&hello.key, &role(!initiator), &sig) {
            return Err(Error::Protocol("the other device's signature doesn't check out".into()));
        }

        let shared = eph_secret.diffie_hellman(&x25519_dalek::PublicKey::from(eph));
        if !shared.was_contributory() {
            return Err(Error::Protocol("bad key exchange".into()));
        }
        let hk = hkdf::Hkdf::<Sha256>::new(Some(&transcript), shared.as_bytes());
        let mut keys = [0u8; 68];
        hk.expand(b"keys+code", &mut keys).map_err(|_| Error::Protocol("key derivation".into()))?;
        let (i2r, rest) = keys.split_at(32);
        let (r2i, code) = rest.split_at(32);
        let code = u32::from_be_bytes([code[0], code[1], code[2], code[3]]) % 1_000_000;
        let (send, recv) = if initiator { (i2r, r2i) } else { (r2i, i2r) };
        Ok(Channel {
            stream,
            send: ChaCha20Poly1305::new(Key::from_slice(send)),
            recv: ChaCha20Poly1305::new(Key::from_slice(recv)),
            sent: 0,
            received: 0,
            max_frame: MAX_SMALL_FRAME,
            deadline,
            peer_device: hello.device,
            peer_key: hello.key,
            code: format!("{code:06}"),
        })
    }

    pub fn send(&mut self, plaintext: &[u8]) -> Result<()> {
        if plaintext.len() > MAX_FRAME {
            return Err(Error::Protocol("frame too large".into()));
        }
        let n = nonce(self.sent);
        self.sent = self.sent.checked_add(1).ok_or_else(|| Error::Protocol("channel exhausted".into()))?;
        let ct = self.send.encrypt(Nonce::from_slice(&n), plaintext).map_err(|_| Error::Protocol("encryption failed".into()))?;
        write_raw(&mut self.stream, &ct)
    }

    pub fn recv(&mut self) -> Result<Vec<u8>> {
        let ct = read_raw(&mut self.stream, self.max_frame + 16, self.deadline)?;
        let n = nonce(self.received);
        self.received = self.received.checked_add(1).ok_or_else(|| Error::Protocol("channel exhausted".into()))?;
        self.recv.decrypt(Nonce::from_slice(&n), ct.as_ref()).map_err(|_| Error::Protocol("a message failed to decrypt".into()))
    }

    pub fn send_json<T: Serialize>(&mut self, value: &T) -> Result<()> {
        let bytes = serde_json::to_vec(value).map_err(|e| Error::Protocol(e.to_string()))?;
        self.send(&bytes)
    }

    pub fn recv_json<T: for<'de> Deserialize<'de>>(&mut self) -> Result<T> {
        let bytes = self.recv()?;
        serde_json::from_slice(&bytes).map_err(|e| Error::Protocol(format!("unexpected message: {e}")))
    }

    /// Limits how long a read may wait (each read on its own; clears any overall deadline).
    pub fn set_timeout(&mut self, t: Option<Duration>) -> Result<()> {
        self.deadline = None;
        self.stream.set_read_timeout(t)?;
        Ok(())
    }

    /// Every read from now on must finish within `within` of now, in total.
    pub fn set_deadline(&mut self, within: Duration) {
        self.deadline = Some(Instant::now() + within);
    }

    /// Accepts frames up to [`MAX_FRAME`]: only once the other device is known to be paired.
    pub fn allow_large_frames(&mut self) {
        self.max_frame = MAX_FRAME;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    fn pair() -> (Result<Channel>, Result<Channel>, Identity, Identity) {
        let a = Identity::generate("A").unwrap();
        let b = Identity::generate("B").unwrap();
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let b2 = b.clone();
        let t = std::thread::spawn(move || Channel::handshake(l.accept().unwrap().0, &b2, false));
        let ca = Channel::handshake(TcpStream::connect(addr).unwrap(), &a, true);
        (ca, t.join().unwrap(), a, b)
    }

    #[test]
    fn both_sides_agree_and_messages_flow_both_ways() {
        let (ca, cb, a, b) = pair();
        let (mut ca, mut cb) = (ca.unwrap(), cb.unwrap());
        assert_eq!(ca.code, cb.code);
        assert_eq!(ca.code.len(), 6);
        assert_eq!(ca.peer_device, b.device_id);
        assert_eq!(cb.peer_key, a.public_key().unwrap());
        ca.send(b"one").unwrap();
        ca.send(b"two").unwrap();
        assert_eq!(cb.recv().unwrap(), b"one");
        assert_eq!(cb.recv().unwrap(), b"two");
        cb.send(b"back").unwrap();
        assert_eq!(ca.recv().unwrap(), b"back");
    }

    #[test]
    fn a_tampered_frame_fails_to_decrypt() {
        let (ca, cb, _, _) = pair();
        let (ca, mut cb) = (ca.unwrap(), cb.unwrap());
        // bypass the cipher: a raw frame of garbage where a sealed one belongs
        let mut raw = ca.stream.try_clone().unwrap();
        write_raw(&mut raw, &[7u8; 40]).unwrap();
        assert!(cb.recv().is_err());
    }

    #[test]
    fn garbage_instead_of_a_handshake_is_an_error() {
        let me = Identity::generate("A").unwrap();
        for junk in [&b"\x00\x00\x00\x05hello"[..], b"\xff\xff\xff\xff", b"\x00\x00\x00\x02{}"] {
            let l = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = l.local_addr().unwrap();
            let junk = junk.to_vec();
            let t = std::thread::spawn(move || {
                let mut s = TcpStream::connect(addr).unwrap();
                let _ = s.write_all(&junk);
            });
            let r = Channel::handshake(l.accept().unwrap().0, &me, false);
            t.join().unwrap();
            assert!(r.is_err());
        }
    }

    #[test]
    fn an_unpaired_peer_cant_send_a_large_frame_until_allowed() {
        let (ca, cb, _, _) = pair();
        let (mut ca, mut cb) = (ca.unwrap(), cb.unwrap());
        // only the 4-byte length: refused before any 64 MiB buffer is made or read
        let mut raw = ca.stream.try_clone().unwrap();
        raw.write_all(&((MAX_FRAME as u32).to_be_bytes())).unwrap();
        let err = cb.recv().unwrap_err();
        assert!(err.to_string().contains("refused"), "{err}");

        let (ca2, cb2, _, _) = pair();
        let (mut ca2, mut cb2) = (ca2.unwrap(), cb2.unwrap());
        cb2.allow_large_frames();
        let big = vec![3u8; MAX_SMALL_FRAME * 4];
        let t = std::thread::spawn(move || {
            ca2.send(&big).unwrap();
            ca2
        });
        assert_eq!(cb2.recv().unwrap().len(), MAX_SMALL_FRAME * 4);
        t.join().unwrap();
        let _ = ca.send(b"x");
    }

    /// A slow-loris peer trickling one byte at a time can't hold a handshake past its deadline.
    #[test]
    fn a_trickling_peer_is_cut_off_at_the_handshake_deadline() {
        let me = Identity::generate("A").unwrap();
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let t = std::thread::spawn(move || {
            let mut s = TcpStream::connect(addr).unwrap();
            // a length that never completes, one byte every 30 ms for 1.5 s
            for b in [0u8, 0, 0x01, 0].into_iter().chain(std::iter::repeat(b'{')).take(50) {
                if s.write_all(&[b]).is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(30));
            }
        });
        let started = Instant::now();
        let r = Channel::handshake_within(l.accept().unwrap().0, &me, false, Duration::from_millis(250));
        assert!(r.is_err());
        assert!(started.elapsed() < Duration::from_secs(3), "took {:?}", started.elapsed());
        t.join().unwrap();
    }

    /// Connecting to a host that accepts but never answers doesn't hang the initiator.
    #[test]
    fn a_silent_listener_times_out_the_outgoing_handshake() {
        let me = Identity::generate("A").unwrap();
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let s = TcpStream::connect(l.local_addr().unwrap()).unwrap();
        let _held = l.accept().unwrap();
        let started = Instant::now();
        assert!(Channel::handshake_within(s, &me, true, Duration::from_millis(150)).is_err());
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn a_deadline_bounds_reads_after_the_handshake() {
        let (ca, cb, _, _) = pair();
        let (ca, mut cb) = (ca.unwrap(), cb.unwrap());
        cb.set_deadline(Duration::from_millis(200));
        let mut raw = ca.stream.try_clone().unwrap();
        let t = std::thread::spawn(move || {
            raw.write_all(&[0, 0, 0, 40]).unwrap();
            for _ in 0..30 {
                if raw.write_all(&[1]).is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(30));
            }
        });
        let started = Instant::now();
        assert!(cb.recv().is_err());
        assert!(started.elapsed() < Duration::from_secs(2));
        drop(cb);
        t.join().unwrap();
    }
}
