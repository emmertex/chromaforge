//! On-disk and in-memory embedding cache (`ml-embeddings-<model-id>.bin` beside the library).
//!
//! Each record stores `(photo_id: u64, fingerprint: u64, unit_embedding: [f32; dim])` under a versioned header tagged
//! with `<model-id>@<sha256-prefix>`. Switching or updating a model mismatches the tag and starts a clean store;
//! replacing a photo's file on disk changes its `fingerprint` and re-queues that photo without touching the rest.

use std::collections::{BTreeMap, HashSet};
use std::io::Write;
use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

/// File magic for the binary embedding store.
pub const STORE_MAGIC: &[u8; 4] = b"LCME";

/// Binary format version.
pub const STORE_VERSION: u32 = 1;

/// Maximum model tag length in bytes.
pub const MAX_TAG_BYTES: usize = 256;

/// Maximum embedding dimension accepted from disk.
pub const MAX_EMBED_DIM: u32 = 4096;

/// Maximum number of photo embeddings accepted from a single store file.
pub const MAX_STORE_RECORDS: usize = 1_000_000;

/// Maximum store file size read into memory (1 GiB).
pub const MAX_STORE_FILE_BYTES: u64 = 1024 * 1024 * 1024;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum StoreError {
    #[error("embedding store exceeds maximum size ({0} bytes)")]
    TooLarge(u64),
    #[error("embedding store header is truncated or has invalid magic")]
    BadMagic,
    #[error("unsupported embedding store version {0} (expected {STORE_VERSION})")]
    UnsupportedVersion(u32),
    #[error("invalid model tag in embedding store")]
    InvalidTag,
    #[error("embedding store tag {found:?} does not match expected {expected:?}")]
    TagMismatch { expected: String, found: String },
    #[error("embedding store dimension {0} is out of range (1..={MAX_EMBED_DIM})")]
    InvalidDim(u32),
    #[error("embedding dimension {found} does not match store dimension {expected}")]
    DimMismatch { expected: u32, found: u32 },
    #[error("embedding contains non-finite values or zero norm")]
    NonFiniteEmbedding,
    #[error("embedding store has too many records ({0} > {MAX_STORE_RECORDS})")]
    TooManyRecords(usize),
    #[error("cannot write embedding store {path}: {message}")]
    Io { path: String, message: String },
}

/// One stored photo embedding entry.
#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    pub photo_id: u64,
    pub fingerprint: u64,
    pub embedding: Vec<f32>,
}

/// One similarity match returned by [`Store::search`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hit {
    pub photo_id: u64,
    pub score: f32,
}

/// In-memory embedding index backed by an atomic binary file beside the catalog.
#[derive(Debug, Clone, PartialEq)]
pub struct Store {
    tag: String,
    dim: u32,
    records: BTreeMap<u64, Record>,
    dirty: bool,
}

/// Deterministic 64-bit fingerprint of a photo's content key (`size + mtime + path/hash`).
pub fn fingerprint(content_key: &str) -> u64 {
    let digest = Sha256::digest(content_key.as_bytes());
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(digest.get(..8).unwrap_or(&[0u8; 8]));
    u64::from_le_bytes(bytes)
}

/// Normalize `v` in place to unit L2 norm. Returns `false` if any component is non-finite or the norm is zero.
pub fn l2_normalize(v: &mut [f32]) -> bool {
    if v.is_empty() || v.iter().any(|x| !x.is_finite()) {
        return false;
    }
    let mut sum_sq = 0.0f64;
    for &x in v.iter() {
        sum_sq += (x as f64) * (x as f64);
    }
    let norm = sum_sq.sqrt();
    if !norm.is_finite() || norm <= 1e-12 {
        return false;
    }
    let inv = (1.0 / norm) as f32;
    for x in v.iter_mut() {
        *x *= inv;
    }
    true
}

/// Cosine similarity between two equal-length vectors (returns `0.0` if lengths differ or either has non-finite values).
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0f64;
    let mut na = 0.0f64;
    let mut nb = 0.0f64;
    for (&x, &y) in a.iter().zip(b.iter()) {
        if !x.is_finite() || !y.is_finite() {
            return 0.0;
        }
        let xf = x as f64;
        let yf = y as f64;
        dot += xf * yf;
        na += xf * xf;
        nb += yf * yf;
    }
    let denom = (na * nb).sqrt();
    if !denom.is_finite() || denom <= 1e-12 {
        return 0.0;
    }
    (dot / denom).clamp(-1.0, 1.0) as f32
}

/// File name for a model's embedding store beside the catalog (`ml-embeddings-<id>.bin`).
pub fn store_file_name(model_id: &str) -> String {
    let safe: String = model_id.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '_' }).collect();
    format!("ml-embeddings-{safe}.bin")
}

impl Store {
    /// Create an empty store for `tag` and `dim`.
    pub fn new(tag: impl Into<String>, dim: u32) -> Result<Self, StoreError> {
        let tag = tag.into();
        if tag.is_empty() || tag.len() > MAX_TAG_BYTES {
            return Err(StoreError::InvalidTag);
        }
        if dim == 0 || dim > MAX_EMBED_DIM {
            return Err(StoreError::InvalidDim(dim));
        }
        Ok(Self { tag, dim, records: BTreeMap::new(), dirty: false })
    }

    pub fn tag(&self) -> &str {
        &self.tag
    }

    pub fn dim(&self) -> u32 {
        self.dim
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Whether `photo_id` has a cached embedding matching `fingerprint`.
    pub fn has(&self, photo_id: u64, fingerprint: u64) -> bool {
        self.records.get(&photo_id).is_some_and(|r| r.fingerprint == fingerprint)
    }

    /// Borrow the cached unit-norm embedding for `photo_id` if its `fingerprint` matches.
    pub fn get(&self, photo_id: u64, fingerprint: u64) -> Option<&[f32]> {
        let rec = self.records.get(&photo_id)?;
        (rec.fingerprint == fingerprint).then_some(rec.embedding.as_slice())
    }

    /// Borrow the cached record for `photo_id` regardless of fingerprint.
    pub fn get_record(&self, photo_id: u64) -> Option<&Record> {
        self.records.get(&photo_id)
    }

    /// Insert or replace the embedding for `photo_id` (normalizing it to unit L2 norm).
    pub fn insert(&mut self, photo_id: u64, fingerprint: u64, mut embedding: Vec<f32>) -> Result<(), StoreError> {
        if embedding.len() != self.dim as usize {
            return Err(StoreError::DimMismatch { expected: self.dim, found: embedding.len() as u32 });
        }
        if !l2_normalize(&mut embedding) {
            return Err(StoreError::NonFiniteEmbedding);
        }
        if !self.records.contains_key(&photo_id) && self.records.len() >= MAX_STORE_RECORDS {
            return Err(StoreError::TooManyRecords(self.records.len() + 1));
        }
        self.records.insert(photo_id, Record { photo_id, fingerprint, embedding });
        self.dirty = true;
        Ok(())
    }

    /// Remove a photo's cached embedding.
    pub fn remove(&mut self, photo_id: u64) -> bool {
        let removed = self.records.remove(&photo_id).is_some();
        if removed {
            self.dirty = true;
        }
        removed
    }

    /// Retain only photos matching `keep`.
    pub fn retain<F: FnMut(u64) -> bool>(&mut self, mut keep: F) {
        let before = self.records.len();
        self.records.retain(|&id, _| keep(id));
        if self.records.len() != before {
            self.dirty = true;
        }
    }

    /// Rank cached embeddings by cosine similarity to `query`.
    pub fn search(&self, query: &[f32], top_k: usize, threshold: f32, allowed: Option<&HashSet<u64>>) -> Result<Vec<Hit>, StoreError> {
        if query.len() != self.dim as usize {
            return Err(StoreError::DimMismatch { expected: self.dim, found: query.len() as u32 });
        }
        let mut q = query.to_vec();
        if !l2_normalize(&mut q) {
            return Err(StoreError::NonFiniteEmbedding);
        }
        if top_k == 0 {
            return Ok(Vec::new());
        }
        let min_score = if threshold.is_finite() { threshold } else { -1.0 };
        let mut hits: Vec<Hit> = self
            .records
            .values()
            .filter(|rec| allowed.is_none_or(|set| set.contains(&rec.photo_id)))
            .filter_map(|rec| {
                // Both `q` and `rec.embedding` are unit-norm, so dot product equals cosine similarity.
                let mut dot = 0.0f32;
                for (&a, &b) in q.iter().zip(rec.embedding.iter()) {
                    dot += a * b;
                }
                let score = dot.clamp(-1.0, 1.0);
                (score >= min_score).then_some(Hit { photo_id: rec.photo_id, score })
            })
            .collect();
        hits.sort_by(|a, b| b.score.total_cmp(&a.score).then_with(|| a.photo_id.cmp(&b.photo_id)));
        hits.truncate(top_k);
        Ok(hits)
    }

    /// Serialize the store to its binary representation.
    pub fn encode(&self) -> Vec<u8> {
        let tag_bytes = self.tag.as_bytes();
        let tag_len = tag_bytes.len().min(MAX_TAG_BYTES);
        let record_bytes = 16usize.saturating_add((self.dim as usize).saturating_mul(4));
        let header_bytes = 4 + 4 + 2 + tag_len + 4 + 4;
        let total = header_bytes.saturating_add(self.records.len().saturating_mul(record_bytes));
        let mut out = Vec::with_capacity(total);
        out.extend_from_slice(STORE_MAGIC);
        out.extend_from_slice(&STORE_VERSION.to_le_bytes());
        out.extend_from_slice(&(tag_len as u16).to_le_bytes());
        out.extend_from_slice(tag_bytes.get(..tag_len).unwrap_or(&[]));
        out.extend_from_slice(&self.dim.to_le_bytes());
        out.extend_from_slice(&(self.records.len() as u32).to_le_bytes());
        for rec in self.records.values() {
            out.extend_from_slice(&rec.photo_id.to_le_bytes());
            out.extend_from_slice(&rec.fingerprint.to_le_bytes());
            for &v in &rec.embedding {
                out.extend_from_slice(&v.to_le_bytes());
            }
        }
        out
    }

    /// Decode a binary store buffer, optionally enforcing `expected_tag` and `expected_dim`.
    ///
    /// A truncated trailing record (for example from an interrupted append before a power loss) is ignored cleanly so
    /// all complete records before it survive.
    pub fn decode(bytes: &[u8], expected_tag: Option<&str>, expected_dim: Option<u32>) -> Result<Self, StoreError> {
        if bytes.len() as u64 > MAX_STORE_FILE_BYTES {
            return Err(StoreError::TooLarge(bytes.len() as u64));
        }
        if bytes.get(..4) != Some(STORE_MAGIC.as_slice()) {
            return Err(StoreError::BadMagic);
        }
        let version = read_u32_le(bytes, 4).ok_or(StoreError::BadMagic)?;
        if version != STORE_VERSION {
            return Err(StoreError::UnsupportedVersion(version));
        }
        let tag_len = read_u16_le(bytes, 8).ok_or(StoreError::BadMagic)? as usize;
        if tag_len == 0 || tag_len > MAX_TAG_BYTES {
            return Err(StoreError::InvalidTag);
        }
        let tag_end = 10usize.checked_add(tag_len).ok_or(StoreError::InvalidTag)?;
        let tag_slice = bytes.get(10..tag_end).ok_or(StoreError::BadMagic)?;
        let tag = std::str::from_utf8(tag_slice).map_err(|_| StoreError::InvalidTag)?.to_string();
        if let Some(exp) = expected_tag
            && tag != exp
        {
            return Err(StoreError::TagMismatch { expected: exp.to_string(), found: tag });
        }
        let dim = read_u32_le(bytes, tag_end).ok_or(StoreError::BadMagic)?;
        if dim == 0 || dim > MAX_EMBED_DIM {
            return Err(StoreError::InvalidDim(dim));
        }
        if let Some(exp_dim) = expected_dim
            && dim != exp_dim
        {
            return Err(StoreError::DimMismatch { expected: exp_dim, found: dim });
        }
        let count_offset = tag_end.checked_add(4).ok_or(StoreError::BadMagic)?;
        let declared_count = read_u32_le(bytes, count_offset).ok_or(StoreError::BadMagic)? as usize;
        if declared_count > MAX_STORE_RECORDS {
            return Err(StoreError::TooManyRecords(declared_count));
        }
        let mut cursor = count_offset.checked_add(4).ok_or(StoreError::BadMagic)?;
        let vec_bytes = (dim as usize).checked_mul(4).ok_or(StoreError::InvalidDim(dim))?;
        let record_stride = 16usize.checked_add(vec_bytes).ok_or(StoreError::InvalidDim(dim))?;

        let mut store = Self::new(tag, dim)?;
        let mut loaded = 0usize;
        while let Some(rec_end) = cursor.checked_add(record_stride) {
            let Some(rec_slice) = bytes.get(cursor..rec_end) else {
                // Ignore a torn tail record.
                break;
            };
            if loaded >= MAX_STORE_RECORDS {
                return Err(StoreError::TooManyRecords(loaded + 1));
            }
            let photo_id = read_u64_le(rec_slice, 0).ok_or(StoreError::BadMagic)?;
            let fp = read_u64_le(rec_slice, 8).ok_or(StoreError::BadMagic)?;
            let mut emb = Vec::with_capacity(dim as usize);
            let floats_slice = rec_slice.get(16..).unwrap_or(&[]);
            for &chunk in floats_slice.as_chunks::<4>().0 {
                emb.push(f32::from_le_bytes(chunk));
            }
            if !l2_normalize(&mut emb) {
                return Err(StoreError::NonFiniteEmbedding);
            }
            store.records.insert(photo_id, Record { photo_id, fingerprint: fp, embedding: emb });
            loaded += 1;
            cursor = rec_end;
        }
        store.dirty = false;
        Ok(store)
    }

    /// Load an existing store from `path` if it matches `(expected_tag, expected_dim)`, or return a fresh empty store
    /// when the file does not exist, belongs to an older model version, or was corrupted on disk.
    pub fn load_or_reset(path: &Path, expected_tag: &str, expected_dim: u32) -> Self {
        match std::fs::metadata(path) {
            Ok(meta) if meta.len() <= MAX_STORE_FILE_BYTES => match std::fs::read(path) {
                Ok(bytes) => match Self::decode(&bytes, Some(expected_tag), Some(expected_dim)) {
                    Ok(store) => return store,
                    Err(err) => {
                        log::warn!("resetting ML embedding store {} ({err})", path.display());
                    }
                },
                Err(err) => {
                    log::warn!("cannot read ML embedding store {}: {err}", path.display());
                }
            },
            Ok(meta) => {
                log::warn!("ignoring oversized ML embedding store {} ({} bytes)", path.display(), meta.len());
            }
            Err(_) => {}
        }
        Self { tag: expected_tag.to_string(), dim: expected_dim.clamp(1, MAX_EMBED_DIM), records: BTreeMap::new(), dirty: false }
    }

    /// Atomically write the store to `path` via a sibling `.tmp` file.
    pub fn save_atomic(&mut self, path: &Path) -> Result<(), StoreError> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent).map_err(|e| StoreError::Io { path: parent.display().to_string(), message: e.to_string() })?;
        }
        let tmp_path = path.with_extension("bin.tmp");
        let bytes = self.encode();
        {
            let mut f =
                std::fs::File::create(&tmp_path).map_err(|e| StoreError::Io { path: tmp_path.display().to_string(), message: e.to_string() })?;
            f.write_all(&bytes).map_err(|e| StoreError::Io { path: tmp_path.display().to_string(), message: e.to_string() })?;
            f.sync_all().map_err(|e| StoreError::Io { path: tmp_path.display().to_string(), message: e.to_string() })?;
        }
        std::fs::rename(&tmp_path, path).map_err(|e| StoreError::Io { path: path.display().to_string(), message: e.to_string() })?;
        self.dirty = false;
        Ok(())
    }
}

fn read_u16_le(bytes: &[u8], offset: usize) -> Option<u16> {
    let slice = bytes.get(offset..offset.checked_add(2)?)?;
    let mut b = [0u8; 2];
    b.copy_from_slice(slice);
    Some(u16::from_le_bytes(b))
}

fn read_u32_le(bytes: &[u8], offset: usize) -> Option<u32> {
    let slice = bytes.get(offset..offset.checked_add(4)?)?;
    let mut b = [0u8; 4];
    b.copy_from_slice(slice);
    Some(u32::from_le_bytes(b))
}

fn read_u64_le(bytes: &[u8], offset: usize) -> Option<u64> {
    let slice = bytes.get(offset..offset.checked_add(8)?)?;
    let mut b = [0u8; 8];
    b.copy_from_slice(slice);
    Some(u64::from_le_bytes(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrips_searches_and_survives_torn_tail() {
        let tag = "clip-vit-b32-laion2b@74813fbcdc750f23";
        let mut store = Store::new(tag, 4).unwrap();
        let fp1 = fingerprint("photo-1:100:1700000000");
        let fp2 = fingerprint("photo-2:200:1700000001");
        store.insert(10, fp1, vec![1.0, 0.0, 0.0, 0.0]).unwrap();
        store.insert(20, fp2, vec![0.0, 2.0, 0.0, 0.0]).unwrap();
        assert_eq!(store.len(), 2);
        assert!(store.has(10, fp1));
        assert!(!store.has(10, fp2));

        let hits = store.search(&[0.9, 0.1, 0.0, 0.0], 5, 0.0, None).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].photo_id, 10);
        assert!(hits[0].score > 0.99);

        // Encode and simulate a torn 7-byte trailing append
        let mut bytes = store.encode();
        bytes.extend_from_slice(&[1, 2, 3, 4, 5, 6, 7]);
        let decoded = Store::decode(&bytes, Some(tag), Some(4)).unwrap();
        assert_eq!(decoded.len(), 2);
        assert!(decoded.has(10, fp1));
        assert!(decoded.has(20, fp2));
    }

    #[test]
    fn rejects_corrupt_and_hostile_store_bytes() {
        assert_eq!(Store::decode(b"NOPE", None, None), Err(StoreError::BadMagic));
        assert_eq!(Store::decode(b"LCME\x02\x00\x00\x00", None, None), Err(StoreError::UnsupportedVersion(2)));

        let tag = "clip@1234";
        let mut store = Store::new(tag, 3).unwrap();
        store.insert(1, 42, vec![1.0, 0.0, 0.0]).unwrap();
        let bytes = store.encode();

        assert!(matches!(Store::decode(&bytes, Some("other@9999"), Some(3)), Err(StoreError::TagMismatch { .. })));
        assert!(matches!(Store::decode(&bytes, Some(tag), Some(512)), Err(StoreError::DimMismatch { expected: 512, found: 3 })));

        // Corrupt a float into NaN
        let mut nan_bytes = bytes.clone();
        let len = nan_bytes.len();
        nan_bytes[len - 4..len].copy_from_slice(&f32::NAN.to_le_bytes());
        assert_eq!(Store::decode(&nan_bytes, Some(tag), Some(3)), Err(StoreError::NonFiniteEmbedding));
    }
}
