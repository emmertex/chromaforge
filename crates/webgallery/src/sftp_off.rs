//! The SFTP API in a build without the `sftp` feature: every connection is refused with a reason.

pub use crate::settings::Server;

const OFF: &str = "SFTP upload is not part of this build";

/// How to authenticate.
#[derive(Clone)]
pub enum Auth {
    Password(String),
    /// A private key's text (OpenSSH / PEM) and its passphrase.
    Key {
        pem: String,
        passphrase: Option<String>,
    },
}

/// What an upload did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Uploaded {
    pub files: usize,
    pub bytes: u64,
    pub fingerprint: String,
}

pub fn host_key(_server: &Server) -> Result<String, String> {
    Err(OFF.into())
}

pub fn upload(
    _server: &Server,
    _auth: Auth,
    _files: &[(String, Vec<u8>)],
    _progress: &mut dyn FnMut(usize, usize) -> bool,
) -> Result<Uploaded, String> {
    Err(OFF.into())
}

pub fn check(_server: &Server, _auth: Auth) -> Result<(), String> {
    Err(OFF.into())
}

pub fn remove(_server: &Server, _auth: Auth, _rels: &[String]) -> Result<String, String> {
    Err(OFF.into())
}
