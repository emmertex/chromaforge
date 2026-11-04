//! Self-describing `manifest.json` shipped alongside an ML model directory (`<config>/ml-models/<id>/`).
//!
//! No model bytes are bundled in the repository or binary: the person installs a directory (or asks the app to
//! download a known open-weights checkpoint from its upstream host) containing `manifest.json` and the files listed in
//! [`ModelManifest::files`]. Every file's size and SHA-256 are verified before any weights or vocabulary files are
//! opened.

use std::io::Read;
use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

/// File name of the manifest inside each model directory.
pub const MANIFEST_FILE: &str = "manifest.json";

/// Current manifest schema version.
pub const MANIFEST_VERSION: u32 = 1;

/// Maximum accepted `manifest.json` size (64 KiB).
pub const MAX_MANIFEST_BYTES: usize = 64 * 1024;

/// Maximum accepted single model file size (2 GiB).
pub const MAX_MODEL_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Maximum number of files a single model manifest may declare.
pub const MAX_MANIFEST_FILES: usize = 16;

/// Which task in the AI stack this model serves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Role {
    /// Vision + text encoder producing shared unit-norm embeddings (semantic search, culling similarity, aesthetic features).
    Embedder,
    /// Linear or MLP head mapping an image embedding to an aesthetic score.
    AestheticHead,
    /// Classifier mapping an image or embedding to a 0°/90°/180°/270° orientation correction.
    OrientationHead,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Embedder => "embedder",
            Self::AestheticHead => "aestheticHead",
            Self::OrientationHead => "orientationHead",
        }
    }
}

/// Whether the model's upstream licence permits commercial use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Commercial {
    Allowed,
    NonCommercialOnly,
}

impl Commercial {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Allowed => "allowed",
            Self::NonCommercialOnly => "nonCommercialOnly",
        }
    }
}

/// Upstream licence metadata shown in the model picker before download or activation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Licence {
    /// SPDX identifier (for example `"MIT"` or `"Apache-2.0"`).
    pub spdx: String,
    /// Whether commercial use is permitted by the model's weights licence.
    pub commercial: Commercial,
    /// Upstream licence or repository URL.
    #[serde(default)]
    pub url: String,
    /// Attribution line required by the upstream weights.
    #[serde(default)]
    pub attribution: String,
}

/// Image preprocessing expected by the vision encoder (`RGB`, `[0, 1]` scaled then `(x - mean) / std`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InputSpec {
    pub width: u32,
    pub height: u32,
    pub mean: [f32; 3],
    pub std: [f32; 3],
}

/// One file belonging to the model directory (`model.safetensors`, `vocab.json`, `merges.txt`, …).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelFile {
    /// Relative file name inside the model directory (no path separators).
    pub name: String,
    /// Lowercase 64-character hex SHA-256 digest of the file.
    pub sha256: String,
    /// Exact size of the file in bytes.
    pub size_bytes: u64,
}

/// Complete manifest for an installed or downloadable ML model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelManifest {
    pub version: u32,
    pub id: String,
    pub name: String,
    pub role: Role,
    /// Upstream version or commit tag (`"laion2b-s34b-b79k"`, …).
    pub version_tag: String,
    /// Output embedding dimension (512 for CLIP ViT-B/32).
    pub embed_dim: u32,
    pub input: InputSpec,
    pub files: Vec<ModelFile>,
    pub licence: Licence,
    #[serde(default)]
    pub notes: String,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ManifestError {
    #[error("manifest exceeds {MAX_MANIFEST_BYTES} bytes ({0} bytes)")]
    TooLarge(usize),
    #[error("invalid manifest JSON: {0}")]
    Json(String),
    #[error("unsupported manifest version {0} (expected {MANIFEST_VERSION})")]
    UnsupportedVersion(u32),
    #[error("invalid model id {0:?}: expected 1..=64 ASCII lowercase letters, digits, '.', '_' or '-'")]
    InvalidId(String),
    #[error("model name must not be empty")]
    EmptyName,
    #[error("embedding dimension {0} is out of range (expected 1..=4096)")]
    InvalidEmbedDim(u32),
    #[error("invalid input dimensions {width}x{height} (expected 16..=2048)")]
    InvalidInputDimensions { width: u32, height: u32 },
    #[error("invalid input normalization mean/std (must be finite with std > 0)")]
    InvalidNormalization,
    #[error("manifest must list between 1 and {MAX_MANIFEST_FILES} files (got {0})")]
    InvalidFileCount(usize),
    #[error("invalid file name {0:?} in manifest")]
    InvalidFileName(String),
    #[error("duplicate file name {0:?} in manifest")]
    DuplicateFileName(String),
    #[error("file {name:?} has invalid size {size} bytes (expected 1..={MAX_MODEL_BYTES})")]
    InvalidFileSize { name: String, size: u64 },
    #[error("file {name:?} has invalid sha256 {sha256:?} (expected 64 lowercase hex chars)")]
    InvalidSha256 { name: String, sha256: String },
    #[error("licence spdx identifier must not be empty")]
    EmptyLicence,
}

/// Errors encountered when reading and verifying a model directory on disk.
#[derive(Debug, Error)]
pub enum VerifyError {
    #[error("cannot read {path}: {message}")]
    Io { path: String, message: String },
    #[error(transparent)]
    Manifest(#[from] ManifestError),
    #[error("file {name:?} has size {actual} bytes, expected {expected}")]
    SizeMismatch { name: String, expected: u64, actual: u64 },
    #[error("file {name:?} exceeds maximum allowed size ({max} bytes)")]
    FileTooLarge { name: String, max: u64 },
    #[error("file {name:?} sha256 mismatch: expected {expected}, got {actual}")]
    HashMismatch { name: String, expected: String, actual: String },
}

/// Whether `id` is a safe model directory identifier (`[a-z0-9][a-z0-9._-]{0,63}`, no `..`).
pub fn valid_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    if bytes.is_empty() || bytes.len() > 64 {
        return false;
    }
    if !bytes[0].is_ascii_lowercase() && !bytes[0].is_ascii_digit() {
        return false;
    }
    if id.contains("..") {
        return false;
    }
    bytes.iter().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(*b, b'-' | b'_' | b'.'))
}

/// Whether `name` is a safe flat file name inside a model directory (`model.safetensors`, `vocab.json`, …).
pub fn valid_file_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    if bytes.is_empty() || bytes.len() > 128 {
        return false;
    }
    if bytes[0] == b'.' || name.contains("..") {
        return false;
    }
    bytes.iter().all(|b| b.is_ascii_alphanumeric() || matches!(*b, b'-' | b'_' | b'.'))
}

fn valid_sha256(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Compute `(size_bytes, lowercase_hex_sha256)` of `path` using a bounded 64 KiB streaming buffer.
pub fn sha256_file(path: &Path, max_bytes: u64) -> Result<(u64, String), VerifyError> {
    let file = std::fs::File::open(path).map_err(|e| VerifyError::Io { path: path.display().to_string(), message: e.to_string() })?;
    let meta_len = file.metadata().map_err(|e| VerifyError::Io { path: path.display().to_string(), message: e.to_string() })?.len();
    if meta_len > max_bytes {
        return Err(VerifyError::FileTooLarge { name: path.file_name().and_then(|s| s.to_str()).unwrap_or_default().to_string(), max: max_bytes });
    }
    let mut reader = file.take(max_bytes.saturating_add(1));
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let n = reader.read(&mut buf).map_err(|e| VerifyError::Io { path: path.display().to_string(), message: e.to_string() })?;
        if n == 0 {
            break;
        }
        total = total.saturating_add(n as u64);
        if total > max_bytes {
            return Err(VerifyError::FileTooLarge {
                name: path.file_name().and_then(|s| s.to_str()).unwrap_or_default().to_string(),
                max: max_bytes,
            });
        }
        hasher.update(buf.get(..n).unwrap_or(&[]));
    }
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(64);
    for b in digest {
        use std::fmt::Write;
        let _ = write!(&mut hex, "{b:02x}");
    }
    Ok((total, hex))
}

impl ModelManifest {
    /// Parse and validate a `manifest.json` payload.
    pub fn parse(bytes: &[u8]) -> Result<Self, ManifestError> {
        if bytes.len() > MAX_MANIFEST_BYTES {
            return Err(ManifestError::TooLarge(bytes.len()));
        }
        let manifest: Self = serde_json::from_slice(bytes).map_err(|e| ManifestError::Json(e.to_string()))?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Read `<dir>/manifest.json` without hashing the model files yet.
    pub fn read_dir(dir: &Path) -> Result<Self, VerifyError> {
        let manifest_path = dir.join(MANIFEST_FILE);
        let file =
            std::fs::File::open(&manifest_path).map_err(|e| VerifyError::Io { path: manifest_path.display().to_string(), message: e.to_string() })?;
        let mut bytes = Vec::new();
        file.take((MAX_MANIFEST_BYTES as u64).saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|e| VerifyError::Io { path: manifest_path.display().to_string(), message: e.to_string() })?;
        Ok(Self::parse(&bytes)?)
    }

    /// Read `<dir>/manifest.json` and verify the exact byte size and SHA-256 of every declared file.
    pub fn verify_dir(dir: &Path) -> Result<Self, VerifyError> {
        let manifest = Self::read_dir(dir)?;
        for f in &manifest.files {
            let file_path = dir.join(&f.name);
            let (actual_size, actual_hash) = sha256_file(&file_path, f.size_bytes)?;
            if actual_size != f.size_bytes {
                return Err(VerifyError::SizeMismatch { name: f.name.clone(), expected: f.size_bytes, actual: actual_size });
            }
            if actual_hash != f.sha256 {
                return Err(VerifyError::HashMismatch { name: f.name.clone(), expected: f.sha256.clone(), actual: actual_hash });
            }
        }
        Ok(manifest)
    }

    /// Validate all fields, bounds, file names and hashes.
    pub fn validate(&self) -> Result<(), ManifestError> {
        if self.version != MANIFEST_VERSION {
            return Err(ManifestError::UnsupportedVersion(self.version));
        }
        if !valid_id(&self.id) {
            return Err(ManifestError::InvalidId(self.id.clone()));
        }
        if self.name.trim().is_empty() {
            return Err(ManifestError::EmptyName);
        }
        if self.embed_dim == 0 || self.embed_dim > 4096 {
            return Err(ManifestError::InvalidEmbedDim(self.embed_dim));
        }
        if !(16..=2048).contains(&self.input.width) || !(16..=2048).contains(&self.input.height) {
            return Err(ManifestError::InvalidInputDimensions { width: self.input.width, height: self.input.height });
        }
        if self.input.mean.iter().any(|v| !v.is_finite()) || self.input.std.iter().any(|v| !v.is_finite() || *v <= 0.0) {
            return Err(ManifestError::InvalidNormalization);
        }
        if self.files.is_empty() || self.files.len() > MAX_MANIFEST_FILES {
            return Err(ManifestError::InvalidFileCount(self.files.len()));
        }
        for (i, file) in self.files.iter().enumerate() {
            if !valid_file_name(&file.name) {
                return Err(ManifestError::InvalidFileName(file.name.clone()));
            }
            if self.files[..i].iter().any(|prev| prev.name == file.name) {
                return Err(ManifestError::DuplicateFileName(file.name.clone()));
            }
            if file.size_bytes == 0 || file.size_bytes > MAX_MODEL_BYTES {
                return Err(ManifestError::InvalidFileSize { name: file.name.clone(), size: file.size_bytes });
            }
            if !valid_sha256(&file.sha256) {
                return Err(ManifestError::InvalidSha256 { name: file.name.clone(), sha256: file.sha256.clone() });
            }
        }
        if self.licence.spdx.trim().is_empty() {
            return Err(ManifestError::EmptyLicence);
        }
        Ok(())
    }

    /// Sum of all declared file sizes in bytes (saturating).
    pub fn total_bytes(&self) -> u64 {
        self.files.iter().fold(0u64, |acc, f| acc.saturating_add(f.size_bytes))
    }

    /// SHA-256 of the primary weights file (first file in `files`).
    pub fn primary_sha256(&self) -> &str {
        self.files.first().map(|f| f.sha256.as_str()).unwrap_or("")
    }

    /// Versioned cache tag (`<id>@<first 16 hex chars of primary sha256>`) stored in the embedding cache header so
    /// upgrading or switching models invalidates stale embeddings cleanly.
    pub fn tag(&self) -> String {
        let sha = self.primary_sha256();
        let short = sha.get(..16).unwrap_or(sha);
        format!("{}@{short}", self.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_manifest() -> ModelManifest {
        ModelManifest {
            version: MANIFEST_VERSION,
            id: "clip-vit-b32-laion2b".into(),
            name: "CLIP ViT-B/32 (LAION-2B)".into(),
            role: Role::Embedder,
            version_tag: "s34b-b79k".into(),
            embed_dim: 512,
            input: InputSpec { width: 224, height: 224, mean: [0.48145466, 0.4578275, 0.40821073], std: [0.26862954, 0.26130258, 0.27577711] },
            files: vec![ModelFile {
                name: "model.safetensors".into(),
                sha256: "74813fbcdc750f235c9784c367ca1394d2a5c25eb0aac92761752ac239db7cff".into(),
                size_bytes: 605_157_884,
            }],
            licence: Licence {
                spdx: "MIT".into(),
                commercial: Commercial::Allowed,
                url: "https://huggingface.co/laion/CLIP-ViT-B-32-laion2B-s34B-b79K".into(),
                attribution: "LAION OpenCLIP ViT-B/32 (MIT)".into(),
            },
            notes: String::new(),
        }
    }

    #[test]
    fn roundtrips_and_validates_manifest() {
        let m = sample_manifest();
        let json = serde_json::to_vec_pretty(&m).unwrap();
        let parsed = ModelManifest::parse(&json).unwrap();
        assert_eq!(parsed, m);
        assert_eq!(parsed.tag(), "clip-vit-b32-laion2b@74813fbcdc750f23");
        assert_eq!(parsed.total_bytes(), 605_157_884);
    }

    #[test]
    fn rejects_hostile_manifest_inputs() {
        // Oversized manifest
        let huge = vec![b' '; MAX_MANIFEST_BYTES + 1];
        assert_eq!(ModelManifest::parse(&huge), Err(ManifestError::TooLarge(MAX_MANIFEST_BYTES + 1)));

        // Path traversal id
        let mut m = sample_manifest();
        m.id = "../escape".into();
        assert_eq!(m.validate(), Err(ManifestError::InvalidId("../escape".into())));

        // Path traversal filename
        let mut m = sample_manifest();
        m.files[0].name = "../weights.bin".into();
        assert_eq!(m.validate(), Err(ManifestError::InvalidFileName("../weights.bin".into())));

        // Duplicate filename
        let mut m = sample_manifest();
        let dup = m.files[0].clone();
        m.files.push(dup);
        assert_eq!(m.validate(), Err(ManifestError::DuplicateFileName("model.safetensors".into())));

        // Zero std or NaN normalization
        let mut m = sample_manifest();
        m.input.std[1] = 0.0;
        assert_eq!(m.validate(), Err(ManifestError::InvalidNormalization));

        let mut m = sample_manifest();
        m.input.mean[0] = f32::NAN;
        assert_eq!(m.validate(), Err(ManifestError::InvalidNormalization));

        // Bad SHA-256 (uppercase or short)
        let mut m = sample_manifest();
        m.files[0].sha256 = "ABCD".into();
        assert!(matches!(m.validate(), Err(ManifestError::InvalidSha256 { .. })));
    }
}
